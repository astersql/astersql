// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 物理算子：索引连接（PhysicalIndexJoin）。
//
// 以外表行构造索引查找条件，对内表做点查/范围扫描再连接；
// 常用于内表有可用索引的 Nested-Loop 变体。含比较过滤管理器，用于按行构建 ranger 范围。

use base::{ContextRef, JoinType, PhysicalPlan, Plan, Task};
use costusage::{CostVer2, PlanCostOption};
use expression::{Column, Constant, ExprBox, Schema};

use crate::{BasePhysicalJoin, PhysicalExchangeSender};

/// Add a bare cross-child equality to aligned IndexHashJoin hash-key vectors.
/// Go's completePhysicalIndexJoin follows the same outer/inner schema test.
pub(super) fn append_outer_hash_key_pair(
    outer_schema: &Schema,
    inner_schema: &Schema,
    left: &Column,
    right: &Column,
    outer_keys: &mut Vec<Column>,
    inner_keys: &mut Vec<Column>,
) -> bool {
    if left.InOperand || right.InOperand {
        return false;
    }
    let (outer, inner) = if outer_schema.Contains(left) && inner_schema.Contains(right) {
        (left, right)
    } else if outer_schema.Contains(right) && inner_schema.Contains(left) {
        (right, left)
    } else {
        return false;
    };
    if outer_keys
        .iter()
        .zip(inner_keys.iter())
        .any(|(existing_outer, existing_inner)| {
            existing_outer.EqualColumn(outer) && existing_inner.EqualColumn(inner)
        })
    {
        return false;
    }
    outer_keys.push(outer.Clone());
    inner_keys.push(inner.Clone());
    true
}

/// IndexJoin 外侧 MPP 子树中，极小 build 侧应保持广播，避免两侧同时产生 Hash Exchange。
pub fn NormalizeNestedIndexJoinBuildExchange(
    sender: &mut PhysicalExchangeSender,
    build_rows: f64,
    probe_rows: f64,
) -> bool {
    if build_rows > 1.0 || probe_rows <= 512.0 || build_rows >= probe_rows {
        return false;
    }
    sender.ExchangeType = tipb::ExchangeType::Broadcast;
    sender.HashCols.clear();
    true
}

/// 物理索引连接：内表计划、索引范围、键到索引偏移与哈希键等。
pub struct PhysicalIndexJoin {
    /// 连接公共基座。
    pub BasePhysicalJoin: BasePhysicalJoin,
    /// 内表侧物理计划（按索引探测）。
    pub InnerPlan: Option<Box<dyn PhysicalPlan>>,
    /// 预计算或模板化的索引扫描范围。
    pub Ranges: ranger::Ranges,
    /// 连接键下标 → 索引列下标映射。
    pub KeyOff2IdxOff: Vec<i32>,
    /// 索引列前缀长度（前缀索引场景）。
    pub IdxColLens: Vec<i32>,
    /// 非等值比较过滤，用于按 outer 行动态建范围。
    pub CompareFilters: Option<ColWithCmpFuncManager>,
    /// 外表哈希键（IndexHashJoin 路径复用）。
    pub OuterHashKeys: Vec<Column>,
    /// 内表哈希键。
    pub InnerHashKeys: Vec<Column>,
    /// Rust 规划路由保留的原始等值条件；执行时用于补齐哈希键。
    pub EqualConditions: Vec<expression::ScalarFunction>,
    /// 是否由相关 Apply 解相关后改写而来。
    pub FromDecorrelatedApply: bool,
}

impl PhysicalIndexJoin {
    /// 以连接基座构造空的 IndexJoin。
    pub fn New(base: BasePhysicalJoin) -> Self {
        Self {
            BasePhysicalJoin: base,
            InnerPlan: None,
            Ranges: ranger::Ranges::default(),
            KeyOff2IdxOff: Vec::new(),
            IdxColLens: Vec::new(),
            CompareFilters: None,
            OuterHashKeys: Vec::new(),
            InnerHashKeys: Vec::new(),
            EqualConditions: Vec::new(),
            FromDecorrelatedApply: false,
        }
    }

    /// Build aligned execution hash keys from index lookup keys and remaining
    /// bare `EQ(column, column)` join conditions, matching Go completion.
    pub fn CompleteHashKeys(&mut self, outer_schema: &Schema, inner_schema: &Schema) {
        self.OuterHashKeys = self
            .BasePhysicalJoin
            .OuterJoinKeys
            .iter()
            .map(Column::Clone)
            .collect();
        self.InnerHashKeys = self
            .BasePhysicalJoin
            .InnerJoinKeys
            .iter()
            .map(Column::Clone)
            .collect();
        for condition in &self.EqualConditions {
            if condition.FuncName.L != expression::ast::EQ {
                continue;
            }
            let (left, right, is_column_equality) = expression::IsColOpCol(condition);
            if is_column_equality && let (Some(left), Some(right)) = (left, right) {
                append_outer_hash_key_pair(
                    outer_schema,
                    inner_schema,
                    left,
                    right,
                    &mut self.OuterHashKeys,
                    &mut self.InnerHashKeys,
                );
            }
        }
    }
    /// 初始化类型为 IndexJoin 的基座计划与统计/属性。
    pub fn Init(
        mut self,
        ctx: ContextRef,
        stats: property::StatsInfo,
        offset: i32,
        props: Vec<Box<property::PhysicalProperty>>,
    ) -> Self {
        let mut plan = crate::NewBasePhysicalPlan(ctx, "IndexJoin", offset);
        plan.set_stats(stats);
        plan.SetChildrenReqProps(props);
        self.BasePhysicalJoin
            .PhysicalSchemaProducer
            .BasePhysicalPlan = plan;
        self
    }
    /// 深拷贝；CompareFilters 走计划缓存安全克隆。
    pub fn Clone(&self, new_ctx: ContextRef) -> Result<Self, expression::Error> {
        Ok(Self {
            BasePhysicalJoin: self.BasePhysicalJoin.CloneWithSelf(new_ctx.clone())?,
            InnerPlan: self
                .InnerPlan
                .as_ref()
                .map(|plan| plan.clone_physical(new_ctx))
                .transpose()?,
            Ranges: self.Ranges.clone(),
            KeyOff2IdxOff: self.KeyOff2IdxOff.clone(),
            IdxColLens: self.IdxColLens.clone(),
            CompareFilters: self
                .CompareFilters
                .as_ref()
                .map(ColWithCmpFuncManager::cloneForPlanCache),
            OuterHashKeys: self.OuterHashKeys.iter().map(Column::Clone).collect(),
            InnerHashKeys: self.InnerHashKeys.iter().map(Column::Clone).collect(),
            EqualConditions: self
                .EqualConditions
                .iter()
                .map(expression::ScalarFunction::clone_scalar)
                .collect(),
            FromDecorrelatedApply: self.FromDecorrelatedApply,
        })
    }
    /// 估算内存：基座、容器、内表计划、比较过滤与哈希键。
    pub fn MemoryUsage(&self) -> i64 {
        self.BasePhysicalJoin.MemoryUsage()
            + (std::mem::size_of::<Option<Box<dyn PhysicalPlan>>>() * 2) as i64
            + (std::mem::size_of::<Vec<usize>>() * 4) as i64
            + ((self.KeyOff2IdxOff.capacity() + self.IdxColLens.capacity())
                * std::mem::size_of::<i32>()) as i64
            + self
                .InnerPlan
                .as_ref()
                .map_or(0, |plan| plan.memory_usage())
            + self
                .CompareFilters
                .as_ref()
                .map_or(0, ColWithCmpFuncManager::MemoryUsage)
            + self
                .OuterHashKeys
                .iter()
                .chain(&self.InnerHashKeys)
                .map(Column::MemoryUsage)
                .sum::<i64>()
    }
    /// 拼连接类型、内表与内外键的 EXPLAIN 片段。
    pub fn ExplainInfoInternal(&self, normalized: bool, is_index_merge_join: bool) -> String {
        let mut out = self.BasePhysicalJoin.JoinType.to_string();
        let children = self.children();
        if let Some(inner) = children.get(self.BasePhysicalJoin.InnerChildIdx) {
            out.push_str(&format!(
                ", inner:{}",
                if normalized {
                    inner.tp(&[])
                } else {
                    inner.explain_id(&[]).to_string()
                }
            ));
        }
        if let Some(left) = children.first() {
            explainJoinLeftSide(
                &mut out,
                self.BasePhysicalJoin.JoinType.is_inner_join(),
                normalized,
                *left,
            );
        }
        let eval_ctx = self.s_ctx().GetExprCtx().GetEvalCtx();
        if !self.BasePhysicalJoin.OuterJoinKeys.is_empty() {
            out.push_str(&format!(
                ", outer key:{}",
                self.BasePhysicalJoin
                    .OuterJoinKeys
                    .iter()
                    .map(Column::String)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !self.BasePhysicalJoin.InnerJoinKeys.is_empty() {
            out.push_str(&format!(
                ", inner key:{}",
                self.BasePhysicalJoin
                    .InnerJoinKeys
                    .iter()
                    .map(Column::String)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !self.OuterHashKeys.is_empty() && !is_index_merge_join {
            let equal_conditions = self
                .OuterHashKeys
                .iter()
                .zip(&self.InnerHashKeys)
                .enumerate()
                .filter_map(|(index, (outer, inner))| {
                    expression::NewFunctionBase(
                        self.s_ctx().GetExprCtx(),
                        if self
                            .BasePhysicalJoin
                            .IsNullEQ
                            .get(index)
                            .copied()
                            .unwrap_or(false)
                        {
                            expression::ast::NullEQ
                        } else {
                            expression::ast::EQ
                        },
                        *expression::types::NewFieldType(mysql::r#type::TypeLonglong),
                        vec![Box::new(outer.Clone()), Box::new(inner.Clone())],
                    )
                    .ok()
                })
                .collect::<Vec<_>>();
            if !equal_conditions.is_empty() {
                out.push_str(", equal cond:");
                let rendered = if normalized {
                    expression::SortedExplainNormalizedExpressionList(&equal_conditions)
                } else {
                    expression::SortedExplainExpressionList(eval_ctx, &equal_conditions)
                };
                out.push_str(&String::from_utf8_lossy(&rendered));
            }
        }
        for (label, conditions) in [
            (", left cond:", &self.BasePhysicalJoin.LeftConditions[..]),
            (", right cond:", &self.BasePhysicalJoin.RightConditions[..]),
            (", other cond:", &self.BasePhysicalJoin.OtherConditions[..]),
        ] {
            if conditions.is_empty() {
                continue;
            }
            out.push_str(label);
            let rendered = if normalized {
                expression::SortedExplainNormalizedExpressionList(conditions)
            } else {
                expression::SortedExplainExpressionList(eval_ctx, conditions)
            };
            out.push_str(&String::from_utf8_lossy(&rendered));
        }
        out
    }
    /// 归一化 EXPLAIN。
    pub fn ExplainNormalizedInfo(&self) -> String {
        self.ExplainInfoInternal(true, false)
    }
    /// 详细 EXPLAIN。
    pub fn ExplainInfo(&self) -> String {
        self.ExplainInfoInternal(false, false)
    }
    /// 旧版代价：outer_cost + outer × (inner_cost + inner)。
    pub fn GetCost(
        &self,
        outer: f64,
        inner: f64,
        outer_cost: f64,
        inner_cost: f64,
        _flag: u64,
    ) -> f64 {
        outer_cost + outer.max(0.0) * (inner_cost + inner.max(0.0))
    }
    /// 计划代价 V1。
    pub fn GetPlanCostVer1(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.BasePhysicalJoin
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer1(task, option)
    }
    /// 计划代价 V2。
    pub fn GetPlanCostVer2(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
        inl: &[bool],
    ) -> Result<CostVer2, expression::Error> {
        self.BasePhysicalJoin
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer2(task, option, inl)
    }
    /// 挂接子 Task。
    pub fn Attach2Task(&self, tasks: Vec<Box<dyn Task>>) -> Box<dyn Task> {
        base::PhysicalPlan::attach_to_task(self, tasks)
    }
    /// 按 outer/inner Schema 解析连接键、哈希键与比较过滤下标。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        self.BasePhysicalJoin
            .PhysicalSchemaProducer
            .ResolveIndices()?;
        let children = self
            .BasePhysicalJoin
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .Children();
        if children.len() < 2 {
            return Ok(());
        }
        // InnerChildIdx 指向内表；另一侧为外表。
        let left = children[0].schema().Clone();
        let right = children[1].schema().Clone();
        let outer_idx = 1 - self.BasePhysicalJoin.InnerChildIdx;
        let outer = if outer_idx == 0 { &left } else { &right };
        let inner = if self.BasePhysicalJoin.InnerChildIdx == 0 {
            &left
        } else {
            &right
        };
        for column in &mut self.BasePhysicalJoin.OuterJoinKeys {
            *column = column.ResolveIndices(outer)?;
        }
        for column in &mut self.BasePhysicalJoin.InnerJoinKeys {
            *column = column.ResolveIndices(inner)?;
        }
        for expression in &mut self.BasePhysicalJoin.LeftConditions {
            *expression = expression.ResolveIndices(&left)?;
        }
        for expression in &mut self.BasePhysicalJoin.RightConditions {
            *expression = expression.ResolveIndices(&right)?;
        }
        let merged = expression::MergeSchema(Some(&left), Some(&right))
            .ok_or_else(|| expression::errors::New("index join child schemas are unavailable"))?;
        for condition in &mut self.BasePhysicalJoin.OtherConditions {
            *condition = condition.ResolveIndices(&merged)?;
        }
        for column in &mut self.OuterHashKeys {
            *column = column.ResolveIndices(outer)?;
        }
        for column in &mut self.InnerHashKeys {
            *column = column.ResolveIndices(inner)?;
        }
        if let Some(manager) = &mut self.CompareFilters {
            manager.resolveIndices(outer)?;
            for column in &mut manager.AffectedColSchema.Columns {
                *column = column.ResolveIndices(outer)?;
            }
            manager.rebuild_compare_funcs();
        }
        let mut output = self
            .BasePhysicalJoin
            .PhysicalSchemaProducer
            .Schema()
            .Clone();
        let mut columns_to_resolve = output.Columns.len();
        if matches!(
            self.BasePhysicalJoin.JoinType,
            JoinType::LeftOuterSemiJoin | JoinType::AntiLeftOuterSemiJoin
        ) {
            columns_to_resolve = columns_to_resolve.saturating_sub(1);
        }
        // The physical child can reorder columns independently of the join output.
        // Match each occurrence once so duplicate columns retain distinct indices.
        let mut used = vec![false; merged.Columns.len()];
        for output_index in 0..columns_to_resolve {
            let Some(child_index) = merged
                .Columns
                .iter()
                .enumerate()
                .position(|(index, child)| {
                    !used[index] && output.Columns[output_index].EqualColumn(child)
                })
            else {
                return Err(expression::errors::New(format!(
                    "some index join output columns cannot find references from children: resolved {output_index} of {columns_to_resolve}"
                )));
            };
            used[child_index] = true;
            let mut column = output.Columns[output_index].Clone();
            column.Index = child_index as isize;
            output.Columns[output_index] = column;
        }
        self.BasePhysicalJoin
            .PhysicalSchemaProducer
            .SetSchema(output);
        Ok(())
    }
}

/// 当左孩子不是内表时，向缓冲追加 left side 解释文本。
pub fn explainJoinLeftSide(
    buffer: &mut String,
    inner: bool,
    normalized: bool,
    left: &dyn PhysicalPlan,
) {
    if !inner {
        buffer.push_str(&format!(
            ", left side:{}",
            if normalized {
                left.tp(&[])
            } else {
                left.explain_id(&[]).to_string()
            }
        ));
    }
}

/// 管理目标列上的比较运算，支持按行求值并构建列范围（ranger）。
pub struct ColWithCmpFuncManager {
    /// 参与比较的目标列。
    pub TargetCol: Option<Column>,
    /// 列前缀长度限制。
    pub ColLength: i32,
    /// 比较算子名列表（如 gt/lt）。
    pub OpType: Vec<String>,
    /// 各比较的另一侧表达式。
    pub OpArg: Vec<ExprBox>,
    /// 按行求值后填充的临时常量。
    pub TmpConstant: Vec<Constant>,
    /// 受影响列组成的 Schema，用于行比较。
    pub AffectedColSchema: Schema,
    /// 与 AffectedColSchema 对齐的逐列比较函数。
    compare_funcs: Vec<ranger::chunk::CompareFunc>,
}

impl ColWithCmpFuncManager {
    /// 构造空的比较过滤管理器。
    pub fn New(target: Option<Column>, length: i32) -> Self {
        Self {
            TargetCol: target,
            ColLength: length,
            OpType: Vec::new(),
            OpArg: Vec::new(),
            TmpConstant: Vec::new(),
            AffectedColSchema: expression::NewSchema(Vec::new()),
            compare_funcs: Vec::new(),
        }
    }
    /// 计划缓存安全克隆并重建比较函数。
    pub fn cloneForPlanCache(&self) -> Self {
        let mut cloned = Self::New(self.TargetCol.as_ref().map(Column::Clone), self.ColLength);
        cloned.OpType = self.OpType.clone();
        cloned.OpArg = self.OpArg.iter().map(|e| e.CloneExpr()).collect();
        cloned.TmpConstant = self.TmpConstant.iter().map(Constant::Clone).collect();
        cloned.AffectedColSchema = self.AffectedColSchema.Clone();
        cloned.rebuild_compare_funcs();
        cloned
    }
    /// 追加一条比较表达式，并合并受影响列。
    pub fn AppendNewExpr(&mut self, op: String, arg: ExprBox, affected: &[Column]) {
        self.OpType.push(op);
        self.OpArg.push(arg);
        let field_type = self
            .TargetCol
            .as_ref()
            .and_then(|c| c.RetType.clone())
            .unwrap_or_else(|| *expression::types::NewFieldType(mysql::r#type::TypeUnspecified));
        self.TmpConstant.push(Constant::with_type(
            expression::types::Datum::default(),
            field_type,
        ));
        for column in affected {
            if !self.AffectedColSchema.Contains(column) {
                self.AffectedColSchema.Append([column.Clone()]);
            }
        }
        self.rebuild_compare_funcs();
    }
    /// 按受影响列类型重建 CompareFunc 列表。
    fn rebuild_compare_funcs(&mut self) {
        self.compare_funcs = self
            .AffectedColSchema
            .Columns
            .iter()
            .filter_map(|c| c.RetType.as_ref().and_then(ranger::chunk::GetCompareFunc))
            .collect();
    }

    pub(crate) fn restore_cache_snapshot(
        target_col: Option<Column>,
        col_length: i32,
        op_type: Vec<String>,
        op_arg: Vec<ExprBox>,
        tmp_constant: Vec<Constant>,
        affected_col_schema: Schema,
    ) -> Self {
        let mut restored = Self {
            TargetCol: target_col,
            ColLength: col_length,
            OpType: op_type,
            OpArg: op_arg,
            TmpConstant: tmp_constant,
            AffectedColSchema: affected_col_schema,
            compare_funcs: Vec::new(),
        };
        restored.rebuild_compare_funcs();
        restored
    }
    /// 按受影响列字典序比较两行，返回 -1/0/1。
    pub fn CompareRow(&self, lhs: ranger::chunk::Row, rhs: ranger::chunk::Row) -> i32 {
        for (index, column) in self.AffectedColSchema.Columns.iter().enumerate() {
            let Some(compare) = self.compare_funcs.get(index) else {
                continue;
            };
            let result = compare(
                lhs.clone(),
                column.Index as usize,
                rhs.clone(),
                column.Index as usize,
            );
            if result != 0 {
                return result;
            }
        }
        0
    }
    /// 用当前 outer 行求值参数，为目标列构建索引/列范围。
    pub fn BuildRangesByRow(
        &mut self,
        ctx: &mut rangerctx::RangerContext<'_>,
        row: ranger::chunk::Row,
    ) -> Result<ranger::Ranges, expression::Error> {
        let target = self
            .TargetCol
            .as_ref()
            .ok_or_else(|| expression::errors::New("target column is required"))?;
        let target_type = target
            .RetType
            .clone()
            .ok_or_else(|| expression::errors::New("target column type is required"))?;
        let mut conditions = Vec::with_capacity(self.OpType.len());
        // 将 OpArg 在 row 上求值写入 TmpConstant，再拼成比较函数条件。
        for (index, op) in self.OpType.iter().enumerate() {
            let value = self.OpArg[index].Eval(ctx.ExprCtx.GetEvalCtx(), row.clone())?;
            self.TmpConstant[index] = Constant::with_type(value, target_type.clone());
            conditions.push(expression::NewFunction(
                ctx.ExprCtx.as_ref(),
                op,
                *expression::types::NewFieldType(mysql::r#type::TypeTiny),
                vec![
                    Box::new(target.Clone()),
                    Box::new(self.TmpConstant[index].Clone()),
                ],
            )?);
        }
        let (ranges, _, _) =
            ranger::BuildColumnRange(conditions, ctx, &target_type, self.ColLength, 0)?;
        Ok(ranges)
    }
    /// 解析 OpArg 在给定 Schema 中的列下标。
    fn resolveIndices(&mut self, schema: &Schema) -> Result<(), expression::Error> {
        for arg in &mut self.OpArg {
            *arg = arg.ResolveIndices(schema)?;
        }
        Ok(())
    }
    /// 调试用字符串：算子(目标列, 参数哈希)。
    pub fn String(&self) -> String {
        self.OpType
            .iter()
            .enumerate()
            .map(|(i, op)| {
                format!(
                    "{}({}, {:?})",
                    op,
                    self.TargetCol
                        .as_ref()
                        .map(Column::String)
                        .unwrap_or_default(),
                    self.OpArg[i].HashCode()
                )
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
    pub fn MemoryUsage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
            + (self.compare_funcs.capacity() * std::mem::size_of::<ranger::chunk::CompareFunc>())
                as i64
            + self.TargetCol.as_ref().map_or(0, Column::MemoryUsage)
            + self.AffectedColSchema.MemoryUsage()
            + self.OpType.iter().map(|op| op.len() as i64).sum::<i64>()
            + self.OpArg.iter().map(|e| e.MemoryUsage()).sum::<i64>()
            + self
                .TmpConstant
                .iter()
                .map(Constant::MemoryUsage)
                .sum::<i64>()
    }
}

/// 空 ColWithCmpFuncManager 的结构体大小常量。
pub const EMPTY_COL_WITH_CMP_FUNC_MANAGER_SIZE: i64 =
    std::mem::size_of::<ColWithCmpFuncManager>() as i64;
/// 从逻辑路径抽出的 IndexJoin 附加信息（范围与键映射）。
pub struct IndexJoinInfo {
    pub IdxColLens: Vec<i32>,
    pub KeyOff2IdxOff: Vec<i32>,
    pub Ranges: ranger::Ranges,
    pub CompareFilters: Option<ColWithCmpFuncManager>,
}
