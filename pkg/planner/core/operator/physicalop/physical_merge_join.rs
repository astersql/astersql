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

// MergeJoin（归并连接）物理算子：要求两侧已按连接键有序，线性扫描合并匹配行。
//
// 相比 HashJoin，无需构建哈希表，适合两侧均已排序或可廉价保序的场景；
// `Desc` 表示按降序归并。

use crate::{BasePhysicalJoin, PhysicalSchemaProducer};
use base::{ContextRef, JoinType, PhysicalPlan, Plan, Task};
use costusage::{CostVer2, PlanCostOption};
use expression::{Column, Expression};
use std::collections::HashSet;

/// 对应 Go `PhysicalMergeJoin`：在 BasePhysicalJoin 上增加升降序标志。
pub struct PhysicalMergeJoin {
    pub BasePhysicalJoin: BasePhysicalJoin,
    /// 为 true 时按连接键降序归并。
    pub Desc: bool,
}

/// 从 LogicalJoin 抽取等值/单侧/其他条件，组装 MergeJoin（默认升序）。
pub fn GetMergeJoin(
    logical: &logicalop::LogicalJoin,
    producer: PhysicalSchemaProducer,
) -> PhysicalMergeJoin {
    let mut base = BasePhysicalJoin::New(producer, logical.JoinType);
    base.LeftConditions = logical
        .LeftConditions
        .iter()
        .map(|e| e.CloneExpr())
        .collect();
    base.RightConditions = logical
        .RightConditions
        .iter()
        .map(|e| e.CloneExpr())
        .collect();
    base.OtherConditions = logical
        .OtherConditions
        .iter()
        .map(|e| e.CloneExpr())
        .collect();
    // 等值条件拆成左右连接键列对。
    for equality in &logical.EqualConditions {
        if let Some(function) = equality
            .as_any()
            .downcast_ref::<expression::ScalarFunction>()
        {
            let (left, right) = expression::ExtractColumnsFromColOpCol(function);
            if let (Some(left), Some(right)) = (left, right) {
                base.LeftJoinKeys.push(left.clone());
                base.RightJoinKeys.push(right.clone());
            }
        }
    }
    PhysicalMergeJoin {
        BasePhysicalJoin: base,
        Desc: false,
    }
}

/// 会话偏好禁用 HashJoin 或功能关闭时，应跳过 HashJoin 枚举。
pub fn ShouldSkipHashJoin(prefer_no_hash: bool, disabled: bool) -> bool {
    prefer_no_hash || disabled
}

/// 按给定左右连接键构造最小 MergeJoin 计划骨架。
pub fn BuildMergeJoinPlan(
    ctx: ContextRef,
    join_type: JoinType,
    left: Vec<Column>,
    right: Vec<Column>,
) -> PhysicalMergeJoin {
    let producer = PhysicalSchemaProducer::New(crate::NewBasePhysicalPlan(ctx, "MergeJoin", 0));
    let mut base = BasePhysicalJoin::New(producer, join_type);
    base.DefaultValues = vec![types::datum::NewIntDatum(1), types::datum::NewIntDatum(1)];
    base.LeftJoinKeys = left;
    base.RightJoinKeys = right;
    PhysicalMergeJoin {
        BasePhysicalJoin: base,
        Desc: false,
    }
}

impl PhysicalMergeJoin {
    /// 绑定基类计划节点与统计信息。
    pub fn Init(mut self, ctx: ContextRef, stats: property::StatsInfo, offset: i32) -> Self {
        let mut plan = crate::NewBasePhysicalPlan(ctx, "MergeJoin", offset);
        plan.set_stats(stats);
        self.BasePhysicalJoin
            .PhysicalSchemaProducer
            .BasePhysicalPlan = plan;
        self
    }
    /// 克隆 Join 基类状态与 Desc 标志。
    pub fn Clone(&self, new_ctx: ContextRef) -> Result<Self, expression::Error> {
        Ok(Self {
            BasePhysicalJoin: self.BasePhysicalJoin.CloneWithSelf(new_ctx)?,
            Desc: self.Desc,
        })
    }
    /// 将算子挂到任务树（Task）上。
    pub fn Attach2Task(&self, tasks: Vec<Box<dyn Task>>) -> Box<dyn Task> {
        base::PhysicalPlan::attach_to_task(self, tasks)
    }
    /// 简化代价：左右子代价非负之和。
    pub fn GetCost(&self, left: f64, right: f64, _flag: u64) -> f64 {
        left.max(0.0) + right.max(0.0)
    }
    /// 代价模型 v1。
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
    /// 代价模型 v2。
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
    /// EXPLAIN：连接类型与左右键。
    pub fn ExplainInfo(&self) -> String {
        self.explain(false)
    }
    /// 归一化 EXPLAIN（列名脱敏）。
    pub fn ExplainNormalizedInfo(&self) -> String {
        self.explain(true)
    }
    /// 按是否归一化渲染连接类型与左右键列表。
    fn explain(&self, normalized: bool) -> String {
        let eval_ctx = self.s_ctx().GetExprCtx().GetEvalCtx();
        let render_column = |column: &Column| {
            if normalized {
                column.ExplainNormalizedInfo()
            } else {
                column.ExplainInfo(eval_ctx)
            }
        };
        let render_exprs = |exprs: &[expression::ExprBox]| {
            if normalized {
                String::from_utf8_lossy(&expression::SortedExplainNormalizedExpressionList(exprs))
                    .into_owned()
            } else {
                String::from_utf8_lossy(&expression::SortedExplainExpressionList(eval_ctx, exprs))
                    .into_owned()
            }
        };
        let mut info = self.BasePhysicalJoin.JoinType.to_string();
        if !self.BasePhysicalJoin.JoinType.is_inner_join() {
            if let Some(left) = self
                .BasePhysicalJoin
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .Children()
                .first()
            {
                info.push_str(", left side:");
                let left_info = if normalized {
                    left.tp(&[])
                } else {
                    left.explain_id(&[]).to_string()
                };
                info.push_str(&left_info);
            }
        }
        if !self.BasePhysicalJoin.LeftJoinKeys.is_empty() {
            info.push_str(", left key:");
            info.push_str(
                &self
                    .BasePhysicalJoin
                    .LeftJoinKeys
                    .iter()
                    .map(render_column)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if !self.BasePhysicalJoin.RightJoinKeys.is_empty() {
            info.push_str(", right key:");
            info.push_str(
                &self
                    .BasePhysicalJoin
                    .RightJoinKeys
                    .iter()
                    .map(render_column)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if !self.BasePhysicalJoin.LeftConditions.is_empty() {
            info.push_str(", left cond:");
            info.push_str(&render_exprs(&self.BasePhysicalJoin.LeftConditions));
        }
        if !self.BasePhysicalJoin.RightConditions.is_empty() {
            info.push_str(", right cond:");
            info.push_str(&render_exprs(&self.BasePhysicalJoin.RightConditions));
        }
        if !self.BasePhysicalJoin.OtherConditions.is_empty() {
            info.push_str(", other cond:");
            info.push_str(&render_exprs(&self.BasePhysicalJoin.OtherConditions));
        }
        info
    }
    /// 内存占用：基类 + Desc 布尔标志。
    pub fn MemoryUsage(&self) -> i64 {
        self.BasePhysicalJoin.MemoryUsage() + 1
    }
    /// 按左右孩子 Schema 分别解析左右连接键列下标。
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
            return Err(expression::errors::New(
                "merge join requires exactly two children",
            ));
        }
        let left = children[0].schema().Clone();
        let right = children[1].schema().Clone();
        for column in &mut self.BasePhysicalJoin.LeftJoinKeys {
            *column = column.ResolveIndices(&left)?;
        }
        for column in &mut self.BasePhysicalJoin.RightJoinKeys {
            *column = column.ResolveIndices(&right)?;
        }
        for expr in &mut self.BasePhysicalJoin.LeftConditions {
            *expr = expr.ResolveIndices(&left)?;
        }
        for expr in &mut self.BasePhysicalJoin.RightConditions {
            *expr = expr.ResolveIndices(&right)?;
        }
        let merged = expression::MergeSchema(Some(&left), Some(&right))
            .ok_or_else(|| expression::errors::New("merge join schemas are unavailable"))?;
        for expr in &mut self.BasePhysicalJoin.OtherConditions {
            *expr = expr.ResolveIndices(&merged)?;
        }

        let mut output = self
            .BasePhysicalJoin
            .PhysicalSchemaProducer
            .SchemaRef()
            .map(expression::Schema::Clone)
            .unwrap_or_else(|| expression::NewSchema(Vec::new()));
        let mut columns_to_resolve = output.Len();
        if matches!(
            self.BasePhysicalJoin.JoinType,
            JoinType::LeftOuterSemiJoin | JoinType::AntiLeftOuterSemiJoin
        ) {
            columns_to_resolve = columns_to_resolve.saturating_sub(1);
        }
        let mut found = 0;
        let mut merged_index = 0;
        for output_index in 0..columns_to_resolve {
            while merged_index < merged.Columns.len()
                && !output.Columns[output_index].EqualColumn(&merged.Columns[merged_index])
            {
                merged_index += 1;
            }
            if merged_index == merged.Columns.len() {
                break;
            }
            output.Columns[output_index] = output.Columns[output_index].Clone();
            output.Columns[output_index].Index = merged_index as isize;
            found += 1;
            merged_index += 1;
        }
        if found < columns_to_resolve {
            return Err(expression::errors::New(format!(
                "Some columns of {} cannot find the reference from its child(ren)",
                self.BasePhysicalJoin
                    .PhysicalSchemaProducer
                    .BasePhysicalPlan
                    .explain_id(&[])
            )));
        }
        self.BasePhysicalJoin
            .PhysicalSchemaProducer
            .SetSchema(output);
        Ok(())
    }
}

pub(crate) fn find_max_prefix_len(candidates: &[Vec<Column>], keys: &[Column]) -> usize {
    candidates
        .iter()
        .map(|candidate| {
            keys.iter()
                .zip(candidate)
                .take_while(|(key, candidate_key)| key.EqualColumn(*candidate_key))
                .count()
        })
        .max()
        .unwrap_or(0)
}

pub(crate) fn move_equal_to_other_conditions(
    other: &[expression::ExprBox],
    equal: &[expression::ExprBox],
    used_offsets: &[usize],
) -> Vec<expression::ExprBox> {
    let used = used_offsets.iter().copied().collect::<HashSet<_>>();
    other
        .iter()
        .chain(
            equal
                .iter()
                .enumerate()
                .filter_map(|(index, condition)| (!used.contains(&index)).then_some(condition)),
        )
        .map(|condition| condition.CloneExpr())
        .collect()
}

pub(crate) fn reorder_by_offsets<T: Clone>(values: &[T], offsets: &[usize]) -> Vec<T> {
    offsets
        .iter()
        .map(|offset| values[*offset].clone())
        .chain(
            values
                .iter()
                .enumerate()
                .filter(|(index, _)| !offsets.contains(index))
                .map(|(_, value)| value.clone()),
        )
        .collect()
}

pub(crate) fn is_sort_prop_compatible_with_join_keys(
    sort_items: &[property::SortItem],
    join_keys: &[Column],
    constant_columns: &HashSet<i64>,
) -> bool {
    let mut key_position = 0;
    for item in sort_items {
        let mut matched = false;
        while key_position < join_keys.len() {
            if item.Col.EqualColumn(&join_keys[key_position]) {
                key_position += 1;
                matched = true;
                break;
            }
            if constant_columns.contains(&join_keys[key_position].UniqueID) {
                key_position += 1;
                continue;
            }
            return false;
        }
        if !matched {
            return false;
        }
    }
    true
}
