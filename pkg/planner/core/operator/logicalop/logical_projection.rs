// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 逻辑算子：投影（LogicalProjection / Projection）。
//
// 对应 SELECT 列表：Exprs 为输出表达式。负责谓词经投影替换后下推、
// 列裁剪、键信息映射、TopN 排序项替换、统计/GroupNDV 推导、
// 函数依赖（FD）抽取，以及物理属性中排序项向子节点的变换。

use crate::{
    AttachSelectionToPlan, BaseLogicalPlan, Column, CorrelatedColumn, Expression,
    LogicalAggregation, LogicalPlan, LogicalPlanRef, LogicalSchemaProducer, LogicalSelection,
    LogicalTableDual, LogicalTopN, NewBaseLogicalPlan, PredicatePushDownPlan, Result, Schema,
    SortProperties, StatsInfo,
};
use expression::ScalarFunction;
use std::any::Any;
use std::collections::HashMap;

/// 投影逻辑算子：计算并输出 Exprs 中的表达式列。
#[derive(Default)]
pub struct LogicalProjection {
    /// Schema 与基类逻辑计划。
    pub LogicalSchemaProducer: LogicalSchemaProducer,
    /// 投影表达式列表（与输出列一一对应）。
    pub Exprs: Vec<Expression>,
    /// 是否立即计算（不延迟）。
    pub CalculateNoDelay: bool,
    /// 是否为 Expand 生成的投影（不可宽松消除）。
    pub Proj4Expand: bool,
}

impl LogicalProjection {
    /// 初始化为 Projection（TypeProj）节点。
    pub fn Init(mut self, ctx: base::ContextRef, qb_offset: i32) -> Self {
        self.LogicalSchemaProducer.BaseLogicalPlan =
            NewBaseLogicalPlan(ctx, plancodec::TypeProj, qb_offset);
        self
    }

    /// 生成 EXPLAIN 中的投影表达式列表。
    pub fn ExplainInfo(&self) -> String {
        let parameters = self.SCtx().map(|ctx| ctx.GetExprCtx().GetEvalCtx());
        self.Exprs
            .iter()
            .map(|expr| expr.StringWithCtx(parameters.map(|ctx| ctx as _), ""))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// 按哈希映射替换投影表达式中的列。
    pub fn ReplaceExprColumns(&mut self, replace: &HashMap<Vec<u8>, Column>) {
        for expression in &mut self.Exprs {
            *expression = replaceProjectionColumns(expression.clone(), replace);
        }
    }

    /// 计算计划哈希：物理类型、查询块偏移与各表达式哈希。
    pub fn HashCode(&self) -> Vec<u8> {
        let mut result = Vec::with_capacity(12 + self.Exprs.len() * 10);
        result.extend_from_slice(
            &(plancodec::TypeStringToPhysicalID(self.TP()) as u32).to_be_bytes(),
        );
        result.extend_from_slice(&(self.QueryBlockOffset() as u32).to_be_bytes());
        result.extend_from_slice(&(self.Exprs.len() as u32).to_be_bytes());
        for expression in &self.Exprs {
            let hash = expression.HashCode();
            result.extend_from_slice(&(hash.len() as u32).to_be_bytes());
            result.extend(hash);
        }
        result
    }

    /// 谓词下推：可替换投影列的谓词下推子节点，含 set_var 赋值则阻塞。
    pub fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        // 含赋值型 set_var 时副作用阻止下推，仅清空子侧残留。
        if self
            .Exprs
            .iter()
            .any(|expression| hasAssignSetVarFunc(expression.as_ref()))
        {
            if let Some(child) = self.Children_mut().first_mut() {
                let residual = PredicatePushDownPlan(child, Vec::new())?;
                AttachSelectionToPlan(child, residual)?;
            }
            return Ok(predicates);
        }
        let (pushable, retained) = breakDownPredicates(self, predicates);
        let remaining = if let Some(child) = self.Children_mut().first_mut() {
            PredicatePushDownPlan(child, pushable)?
        } else {
            pushable
        };
        // 无相关列的残留谓词挂回本投影下的新 Selection；相关列上抛。
        let (local_remaining, mut outer_remaining): (Vec<_>, Vec<_>) = remaining
            .into_iter()
            .partition(|condition| expression::ExtractCorColumns(condition.as_ref()).is_empty());
        if !local_remaining.is_empty()
            && let Some(context) = self.SCtx().cloned()
            && self.Children().len() == 1
        {
            let child = self.TakeChildren().remove(0);
            let schema = child.Schema().Clone();
            let names = child.OutputNames().Shallow();
            let mut selection = LogicalSelection {
                Conditions: local_remaining,
                ..LogicalSelection::default()
            }
            .Init(context, child.QueryBlockOffset());
            selection.SetSchema(schema);
            selection.SetOutputNames(names);
            selection.SetChildren(vec![child]);
            self.SetChildren(vec![Box::new(selection)]);
        }
        outer_remaining.extend(retained);
        Ok(outer_remaining)
    }

    /// 列裁剪：按父用列与副作用保留表达式，再向子节点裁剪输入列。
    pub fn PruneColumns(&mut self, parent_used_cols: &[Column]) -> Result<()> {
        let eval_context = self
            .SCtx()
            .map(|ctx| ctx.GetExprCtx().GetEvalCtx())
            .ok_or_else(|| crate::PlannerError("Projection has no expression context".into()))?;
        let mut used =
            expression::GetUsedList(eval_context, parent_used_cols.to_vec(), self.Schema());
        let parent_ids = parent_used_cols
            .iter()
            .map(|column| column.UniqueID)
            .collect::<std::collections::HashSet<_>>();
        if let Some(child) = self.Children().first() {
            for (needed, projected) in used.iter_mut().zip(&self.Exprs) {
                *needed |= projection_lineage_is_used(projected, child.as_ref(), &parent_ids);
            }
        }
        for (needed, expression) in used.iter_mut().zip(&self.Exprs) {
            *needed |= expression::ExprHasSetVarOrSleep(expression.as_ref());
        }
        // Dual 子节点且无用列时保留常量 0 列，避免空投影。
        if used.iter().all(|needed| !needed)
            && self
                .Children()
                .first()
                .is_some_and(|child| child.as_any().is::<LogicalTableDual>())
            && !self.Exprs.is_empty()
        {
            let zero = expression::NewZero();
            let unique_id = self
                .SCtx()
                .expect("initialized Projection must retain context")
                .GetSessionVars()
                .AllocPlanColumnID();
            let column = Column::new(
                expression::Expression::GetType(&zero, eval_context).clone(),
                0,
                unique_id,
                0,
            );
            self.Exprs = vec![Box::new(zero)];
            self.SetSchema(expression::NewSchema(vec![column]));
            return Ok(());
        }

        for index in (0..used.len()).rev() {
            if !used[index] {
                self.Exprs.remove(index);
                self.Schema_mut().Columns.remove(index);
            }
        }
        let child_columns = expression::ExtractColumnsFromExpressions(&self.Exprs, None)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        if let Some(child) = self.Children_mut().first_mut() {
            child.PruneColumns(&child_columns)?;
        }
        Ok(())
    }

    /// 将子节点主键/唯一键经纯列投影映射到输出列。
    pub fn BuildKeyInfo(&mut self) {
        self.LogicalSchemaProducer.BuildKeyInfo();
        let Some(child) = self.Children().first() else {
            return;
        };
        let projected = self.buildSchemaByExprs();
        let keys = child
            .Schema()
            .PKOrUK
            .iter()
            .filter_map(|key| projected.ColumnsIndices(key))
            .map(|indices| {
                indices
                    .into_iter()
                    .map(|index| self.Schema().Columns[index].Clone())
                    .collect()
            })
            .collect();
        self.Schema_mut().PKOrUK = keys;
    }

    /// TopN 下推：替换 ByItems 为子侧表达式，去掉常量/相关列排序项。
    pub fn PushDownTopN(&mut self, top_n: Option<LogicalTopN>) -> Option<LogicalPlanRef> {
        // 含赋值 set_var 时走基类路径，避免跨副作用重排。
        if self
            .Exprs
            .iter()
            .any(|expression| hasAssignSetVarFunc(expression.as_ref()))
        {
            return self
                .LogicalSchemaProducer
                .BaseLogicalPlan
                .PushDownTopN(top_n.map(|plan| Box::new(plan) as LogicalPlanRef));
        }
        let mut plan = top_n;
        if let Some(top_n) = plan.as_mut() {
            for item in &mut top_n.ByItems {
                item.Expr = SubstituteProjectionExpr(item.Expr.clone(), self.Schema(), &self.Exprs);
            }
            top_n.ByItems.retain(|item| {
                !item.Expr.as_any().is::<expression::Constant>()
                    && !item.Expr.as_any().is::<CorrelatedColumn>()
            });
        }
        self.Children_mut()
            .first_mut()
            .and_then(|child| child.PushDownTopN(plan.map(|plan| Box::new(plan) as LogicalPlanRef)))
    }

    /// 可宽松消除的纯列投影可上拉子节点常量谓词。
    pub fn PullUpConstantPredicates(&self) -> Vec<Expression> {
        if !canProjectionBeEliminatedLoose(self) {
            return Vec::new();
        }
        let candidates = self
            .Children()
            .first()
            .map(|child| pullUpConstantPredicates(child.as_ref()))
            .unwrap_or_default();
        rewriteProjectionConstantPredicates(&self.Exprs, self.Schema(), candidates)
    }

    /// 推导统计：行数继承子节点；列 NDV 按列引用或表达式列最大 NDV 估计。
    pub fn DeriveStats(
        &mut self,
        child_stats: &StatsInfo,
        self_schema: &Schema,
        reloads: &[bool],
    ) -> Result<(StatsInfo, bool)> {
        let reload = reloads.len() == 1 && reloads[0];
        if !reload && let Some(stats) = LogicalPlan::StatsInfo(self) {
            let mut stats = stats.clone();
            stats.GroupNDVs = self.getGroupNDVs(child_stats, self_schema);
            self.SetStats(stats.clone());
            return Ok((stats, false));
        }
        let mut stats = StatsInfo {
            RowCount: child_stats.RowCount,
            ..StatsInfo::default()
        };
        for (expression, output) in self.Exprs.iter().zip(&self_schema.Columns) {
            let ndv = if let Some(column) = expression.as_any().downcast_ref::<Column>() {
                child_stats
                    .ColNDVs
                    .get(&column.UniqueID)
                    .copied()
                    .unwrap_or(child_stats.RowCount)
            } else {
                expression::ExtractColumns(expression.as_ref())
                    .into_iter()
                    .filter_map(|column| child_stats.ColNDVs.get(&column.UniqueID).copied())
                    .max_by(f64::total_cmp)
                    .unwrap_or(1.0)
                    .min(child_stats.RowCount)
            };
            stats.ColNDVs.insert(output.UniqueID, ndv);
        }
        stats.GroupNDVs = self.getGroupNDVs(child_stats, self_schema);
        self.SetStats(stats.clone());
        Ok((stats, true))
    }

    /// 将列组经纯列投影映射回子侧列并排序。
    pub fn ExtractColGroups(&self, groups: &[Vec<Column>]) -> Vec<Vec<Column>> {
        let (offset_groups, _) = self.Schema().ExtractColGroups(groups);
        offset_groups
            .into_iter()
            .filter_map(|offsets| {
                offsets
                    .into_iter()
                    .map(|offset| {
                        self.Exprs[offset]
                            .as_any()
                            .downcast_ref::<Column>()
                            .map(Column::Clone)
                    })
                    .collect::<Option<Vec<_>>>()
            })
            .map(|columns| expression::SortColumns(&columns))
            .collect()
    }

    /// 将子节点可能排序属性映射到输出列（遇非列表达式截断）。
    pub fn PreparePossibleProperties(&self, child: &SortProperties) -> SortProperties {
        let mut output = Vec::new();
        for order in &child.Orders {
            let mut mapped = Vec::new();
            for column in order {
                let Some(index) = self.Exprs.iter().position(|expression| {
                    expression
                        .as_any()
                        .downcast_ref::<Column>()
                        .is_some_and(|candidate| candidate.EqualColumn(column))
                }) else {
                    break;
                };
                mapped.push(self.Schema().Columns[index].Clone());
            }
            if !mapped.is_empty() {
                output.push(mapped);
            }
        }
        SortProperties {
            Orders: output,
            HasTiFlash: child.HasTiFlash,
        }
    }

    /// 抽取投影表达式中的相关列。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        self.Exprs
            .iter()
            .flat_map(|expression| expression::ExtractCorColumns(expression.as_ref()))
            .map(CorrelatedColumn::Clone)
            .collect()
    }

    /// 抽取函数依赖：列等价、常量、标量函数决定关系，再投影到输出。
    pub fn ExtractFD(&mut self) -> fd::FDSet {
        let output_columns = self.Schema().Columns.clone();
        let expressions = self.Exprs.clone();
        let context = self.SCtx().cloned();
        let mut dependencies = self
            .LogicalSchemaProducer
            .BaseLogicalPlan
            .ExtractFD()
            .clone();
        let mut output_ids = intset::NewFastIntSet(Vec::new());
        let mut not_null_ids = intset::NewFastIntSet(Vec::new());
        for output in &output_columns {
            output_ids.Insert(output.UniqueID as i32);
        }

        for (expression, output) in expressions.iter().zip(&output_columns) {
            let output_id = output.UniqueID as i32;
            // 纯列投影：建立输入列与输出列的等价关系。
            if let Some(column) = expression.as_any().downcast_ref::<Column>() {
                if column.UniqueID != output.UniqueID {
                    dependencies.AddEquivalence(
                        intset::NewFastIntSet(vec![column.UniqueID as i32]),
                        intset::NewFastIntSet(vec![output_id]),
                    );
                }
                continue;
            }
            if expression.as_any().is::<CorrelatedColumn>() {
                continue;
            }

            let hash = String::from_utf8_lossy(&expression.HashCode()).into_owned();
            if expression.as_any().is::<expression::Constant>() {
                let (registered, found) = dependencies.IsHashCodeRegistered(&hash);
                let constant_id = if found { registered } else { output_id };
                if !found {
                    dependencies.RegisterUniqueID(hash, constant_id);
                }
                dependencies.AddConstants(intset::NewFastIntSet(vec![constant_id]));
                continue;
            }

            let Some(function) = expression.as_any().downcast_ref::<ScalarFunction>() else {
                continue;
            };
            let (registered, found) = dependencies.IsHashCodeRegistered(&hash);
            if expression::CheckNonDeterministic(function) {
                if !found {
                    dependencies.RegisterUniqueID(hash, output_id);
                }
                continue;
            }
            let function_id = if found { registered } else { output_id };
            if !found {
                dependencies.RegisterUniqueID(hash, function_id);
            } else {
                dependencies.AddEquivalence(
                    intset::NewFastIntSet(vec![function_id]),
                    intset::NewFastIntSet(vec![output_id]),
                );
            }

            let mut determinants = intset::NewFastIntSet(Vec::new());
            for column in expression::ExtractColumns(function) {
                determinants.Insert(column.UniqueID as i32);
                output_ids.Insert(column.UniqueID as i32);
            }
            for correlated in expression::ExtractCorColumns(function) {
                determinants.Insert(correlated.column.UniqueID as i32);
                output_ids.Insert(correlated.column.UniqueID as i32);
            }
            let rejected = context.as_ref().is_some_and(|context| {
                planner_util::IsNullRejected(context.as_ref(), self.Schema(), expression.clone())
            });
            if rejected || determinants.SubsetOf(&dependencies.NotNullCols) {
                not_null_ids.Insert(function_id);
            }
            dependencies.AddStrictFunctionalDependency(
                determinants,
                intset::NewFastIntSet(vec![function_id]),
            );
        }
        dependencies.MakeNotNull(not_null_ids);
        let projected_ids = output_ids.Union(&dependencies.GroupByCols);
        dependencies.ProjectCols(projected_ids);
        self.LogicalSchemaProducer
            .BaseLogicalPlan
            .SetFDs(dependencies.clone());
        dependencies
    }

    /// 可下推谓词参与外连接转内连接。
    pub fn ConvertOuterToInnerJoin(&mut self, predicates: Vec<Expression>) {
        let (pushable, _) = breakDownPredicates(self, predicates);
        self.LogicalSchemaProducer
            .BaseLogicalPlan
            .ConvertOuterToInnerJoin(&pushable);
    }

    /// 收集投影表达式引用的全部列。
    pub fn GetUsedCols(&self) -> Vec<Column> {
        self.Exprs
            .iter()
            .flat_map(|expression| expression::ExtractColumns(expression.as_ref()))
            .cloned()
            .collect()
    }

    /// 按表达式构造临时 Schema（列引用复用，其它复用对应输出列）。
    fn buildSchemaByExprs(&self) -> Schema {
        expression::NewSchema(
            self.Exprs
                .iter()
                .enumerate()
                .map(|(index, expression)| {
                    expression
                        .as_any()
                        .downcast_ref::<Column>()
                        .map(Column::Clone)
                        .unwrap_or_else(|| {
                            self.Schema()
                                .Columns
                                .get(index)
                                .map(Column::Clone)
                                .unwrap_or_default()
                        })
                })
                .collect(),
        )
    }

    /// 将物理属性中的排序项变换为子节点可理解的列排序。
    pub fn TryToGetChildProp(
        &self,
        property: &property::PhysicalProperty,
    ) -> (Option<property::PhysicalProperty>, bool) {
        let mut transformed = property.CloneEssentialFields();
        let (items, ok) = self.tryTransformSortItems(&property.SortItems);
        if !ok {
            return (None, false);
        }
        transformed.SortItems = items;
        if let Some(partial) = &property.PartialOrderInfo {
            let (items, ok) = self.tryTransformSortItems(&partial.SortItems);
            if !ok {
                return (None, false);
            }
            transformed.PartialOrderInfo = Some(property::PartialOrderInfo { SortItems: items });
        }
        if property.MPPPartitionTp == property::HashType {
            let mapped = property
                .MPPPartitionCols
                .iter()
                .map(|partition_column| {
                    let index = self.Schema().ColumnIndex(&partition_column.Col)?;
                    let column = self.Exprs[index].as_any().downcast_ref::<Column>()?;
                    Some(property::MPPPartitionColumn {
                        Col: column.Clone(),
                        CollateID: partition_column.CollateID,
                    })
                })
                .collect::<Option<Vec<_>>>();
            if let Some(columns) = mapped {
                transformed.MPPPartitionCols = columns;
            } else {
                // A computed output partition key does not exist below the Projection.
                // Let the child produce arbitrary MPP partitions; the parent-required
                // exchange must be enforced after this Projection has computed the key.
                transformed.MPPPartitionTp = property::AnyType;
                transformed.MPPPartitionCols.clear();
            }
        }
        (Some(transformed), true)
    }

    /// 排序项经投影：输出列映射为输入列；遇标量函数则失败。
    fn tryTransformSortItems(
        &self,
        items: &[property::SortItem],
    ) -> (Vec<property::SortItem>, bool) {
        let mut transformed = Vec::with_capacity(items.len());
        for item in items {
            let Some(index) = self.Schema().ColumnIndex(&item.Col) else {
                continue;
            };
            if let Some(column) = self.Exprs[index].as_any().downcast_ref::<Column>() {
                transformed.push(property::SortItem {
                    Col: column.Clone(),
                    Desc: item.Desc,
                });
            } else if self.Exprs[index].as_any().is::<ScalarFunction>() {
                return (Vec::new(), false);
            }
        }
        (transformed, true)
    }

    /// tryTransformSortItems 的指针风格别名入口。
    fn tryTransformSortItemPtrs(
        &self,
        items: &[property::SortItem],
    ) -> (Vec<property::SortItem>, bool) {
        self.tryTransformSortItems(items)
    }

    /// 将子节点 GroupNDV 经纯列投影映射到输出列 ID。
    fn getGroupNDVs(&self, child: &StatsInfo, self_schema: &Schema) -> Vec<property::GroupNDV> {
        let mapping = self
            .Exprs
            .iter()
            .zip(&self_schema.Columns)
            .filter_map(|(expression, projected)| {
                expression
                    .as_any()
                    .downcast_ref::<Column>()
                    .map(|source| (source.UniqueID, projected.UniqueID))
            })
            .collect::<HashMap<_, _>>();
        child
            .GroupNDVs
            .iter()
            .filter_map(|group| {
                let mut columns = group
                    .Cols
                    .iter()
                    .map(|column| mapping.get(column).copied())
                    .collect::<Option<Vec<_>>>()?;
                columns.sort_unstable();
                Some(property::GroupNDV {
                    Cols: columns,
                    NDV: group.NDV,
                })
            })
            .collect()
    }

    /// 追加表达式到投影并扩展 Schema；已是列则直接返回。
    pub fn AppendExpr(&mut self, expression: Expression) -> Column {
        if let Some(column) = expression.as_any().downcast_ref::<Column>() {
            return column.Clone();
        }
        let expression = SubstituteProjectionExpr(expression, self.Schema(), &self.Exprs);
        let unique_id = self
            .SCtx()
            .expect("initialized Projection must retain context")
            .GetSessionVars()
            .AllocPlanColumnID();
        let ret_type = self
            .SCtx()
            .map(|ctx| expression.GetType(ctx.GetExprCtx().GetEvalCtx()).clone());
        let mut column = Column::default();
        column.RetType = ret_type;
        column.UniqueID = unique_id;
        self.Exprs.push(expression);
        self.Schema_mut().Append([column.Clone()]);
        column
    }
}

/// 动态分派常量谓词上拉；Rust trait 尚未暴露该 Go 接口，需覆盖已实现该契约的算子。
fn pullUpConstantPredicates(plan: &dyn LogicalPlan) -> Vec<Expression> {
    if let Some(selection) = plan.as_any().downcast_ref::<LogicalSelection>() {
        return selection.PullUpConstantPredicates();
    }
    if let Some(projection) = plan.as_any().downcast_ref::<LogicalProjection>() {
        return projection.PullUpConstantPredicates();
    }
    plan.base().PullUpConstantPredicates()
}

/// 将子节点的单列常量谓词改写到投影输出列；不可见列和多列表达式不能上拉。
pub(crate) fn rewriteProjectionConstantPredicates(
    expressions: &[Expression],
    schema: &Schema,
    candidates: Vec<Expression>,
) -> Vec<Expression> {
    let replace = expressions
        .iter()
        .zip(&schema.Columns)
        .map(|(expression, output)| (expression.HashCode(), output.Clone()))
        .collect::<HashMap<_, _>>();
    candidates
        .into_iter()
        .filter_map(|predicate| {
            let columns = expression::ExtractColumns(predicate.as_ref());
            if columns.len() != 1
                || !replace.contains_key(&expression::Expression::HashCode(columns[0]))
            {
                return None;
            }
            Some(replaceProjectionColumns(predicate, &replace))
        })
        .collect()
}

impl LogicalPlan for LogicalProjection {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn base(&self) -> &BaseLogicalPlan {
        &self.LogicalSchemaProducer.BaseLogicalPlan
    }
    fn base_mut(&mut self) -> &mut BaseLogicalPlan {
        &mut self.LogicalSchemaProducer.BaseLogicalPlan
    }
    fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        LogicalProjection::PredicatePushDown(self, predicates)
    }
    fn ConvertOuterToInner(&mut self, predicates: Vec<Expression>) {
        let (pushable, _) = breakDownPredicates(self, predicates);
        for child in self.Children_mut() {
            child.ConvertOuterToInner(pushable.iter().cloned().collect());
        }
    }
    fn PruneColumns(&mut self, parent_used_cols: &[Column]) -> Result<()> {
        LogicalProjection::PruneColumns(self, parent_used_cols)
    }
    fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        if self.Children().len() != 1 {
            return Err(crate::PlannerError(
                "projection must have exactly one child".to_owned(),
            ));
        }
        let schema = self.Schema().Clone();
        let (child_stats, child_reloaded) = self.Children_mut()[0].DeriveStats(reload)?;
        LogicalProjection::DeriveStats(self, &child_stats, &schema, &[reload || child_reloaded])
    }
    fn BuildKeyInfo(&mut self) {
        LogicalProjection::BuildKeyInfo(self)
    }
}

/// 将表达式中的投影输出列替换为对应投影输入表达式。
pub fn SubstituteProjectionExpr(
    expression: Expression,
    schema: &Schema,
    replacements: &[Expression],
) -> Expression {
    if let Some(column) = expression.as_any().downcast_ref::<Column>() {
        return schema
            .ColumnIndex(column)
            .and_then(|index| replacements.get(index).cloned())
            .unwrap_or(expression);
    }
    let Some(function) = expression.as_any().downcast_ref::<ScalarFunction>() else {
        return expression;
    };
    let mut function = function.clone_scalar();
    for argument in function.GetArgsMut() {
        *argument = SubstituteProjectionExpr(argument.clone(), schema, replacements);
    }
    function.CleanHashCode();
    Box::new(function)
}

/// 按哈希映射递归替换投影表达式中的列/相关列。
fn replaceProjectionColumns(
    expression: Expression,
    replace: &HashMap<Vec<u8>, Column>,
) -> Expression {
    if let Some(column) = expression.as_any().downcast_ref::<Column>() {
        return replace
            .get(&expression::Expression::HashCode(column))
            .map(|replacement| Box::new(replacement.Clone()) as Expression)
            .unwrap_or(expression);
    }
    if let Some(correlated) = expression.as_any().downcast_ref::<CorrelatedColumn>() {
        let Some(replacement) = replace.get(&expression::Expression::HashCode(&correlated.column))
        else {
            return expression;
        };
        let mut correlated = correlated.Clone();
        correlated.column = replacement.Clone();
        return Box::new(correlated);
    }
    let Some(function) = expression.as_any().downcast_ref::<ScalarFunction>() else {
        return expression;
    };
    let mut function = function.clone_scalar();
    for argument in function.GetArgsMut() {
        *argument = replaceProjectionColumns(argument.clone(), replace);
    }
    function.CleanHashCode();
    Box::new(function)
}

/// 拆分可经投影下推的谓词与必须保留的谓词。
pub fn breakDownPredicates(
    projection: &LogicalProjection,
    predicates: Vec<Expression>,
) -> (Vec<Expression>, Vec<Expression>) {
    let mut pushable = Vec::with_capacity(predicates.len());
    let mut retained = Vec::new();
    for predicate in predicates {
        let predicate_columns = expression::ExtractColumns(predicate.as_ref());
        let references_projection = predicate_columns
            .iter()
            .all(|column| projection.Schema().Contains(column));
        let references_projection_inputs = predicate_columns.iter().all(|column| {
            projection.Exprs.iter().any(|projected| {
                expression::ExtractColumns(projected.as_ref())
                    .iter()
                    .any(|input| input.UniqueID == column.UniqueID)
            }) || projection
                .Children()
                .first()
                .is_some_and(|child| child_accepts_group_column(child.as_ref(), column))
        });
        // 引用投影输出列时先做列替换；含 get/set_var 不可下推。
        let substituted = if references_projection {
            SubstituteProjectionExpr(predicate.clone(), projection.Schema(), &projection.Exprs)
        } else {
            predicate.clone()
        };
        if (references_projection || references_projection_inputs)
            && !hasGetSetVarFunc(substituted.as_ref())
        {
            pushable.push(substituted);
        } else {
            retained.push(predicate);
        }
    }
    (pushable, retained)
}

/// 子节点 schema 或聚合分组/firstrow 参数是否接受该列。
fn child_accepts_group_column(plan: &dyn LogicalPlan, column: &Column) -> bool {
    if plan.Schema().Contains(column) {
        return true;
    }
    plan.as_any()
        .downcast_ref::<LogicalAggregation>()
        .is_some_and(|aggregation| {
            aggregation
                .GroupByItems
                .iter()
                .chain(
                    aggregation
                        .AggFuncs
                        .iter()
                        .filter(|function| function.Name.eq_ignore_ascii_case("firstrow"))
                        .flat_map(|function| &function.Args),
                )
                .flat_map(|expression| expression::ExtractColumns(expression.as_ref()))
                .any(|candidate| candidate.UniqueID == column.UniqueID)
        })
}

/// 判断投影表达式血缘是否被父用列（含 firstrow 聚合）引用。
fn projection_lineage_is_used(
    expression: &Expression,
    child: &dyn LogicalPlan,
    used_ids: &std::collections::HashSet<i64>,
) -> bool {
    expression::ExtractColumns(expression.as_ref())
        .iter()
        .any(|column| {
            used_ids.contains(&column.UniqueID)
                || child
                    .as_any()
                    .downcast_ref::<LogicalAggregation>()
                    .and_then(|aggregation| {
                        aggregation
                            .Schema()
                            .ColumnIndex(column)
                            .map(|index| (aggregation, index))
                    })
                    .and_then(|(aggregation, index)| aggregation.AggFuncs.get(index))
                    .filter(|function| function.Name.eq_ignore_ascii_case("firstrow"))
                    .is_some_and(|function| {
                        function.Args.iter().any(|argument| {
                            expression::ExtractColumns(argument.as_ref())
                                .iter()
                                .any(|source| used_ids.contains(&source.UniqueID))
                        })
                    })
        })
}

/// 表达式是否包含 get_var/set_var。
fn hasGetSetVarFunc(expression: &dyn expression::Expression) -> bool {
    let Some(function) = expression.as_any().downcast_ref::<ScalarFunction>() else {
        return false;
    };
    if function.FuncName.L == parser_ast::SetVar || function.FuncName.L == parser_ast::GetVar {
        return true;
    }
    function
        .GetArgs()
        .iter()
        .any(|argument| hasGetSetVarFunc(argument.as_ref()))
}

/// 是否为赋值型 set_var（参数含标量函数），会阻塞谓词/TopN 下推。
fn hasAssignSetVarFunc(expression: &dyn expression::Expression) -> bool {
    let Some(function) = expression.as_any().downcast_ref::<ScalarFunction>() else {
        return false;
    };
    if function.FuncName.L == parser_ast::SetVar
        && function
            .GetArgs()
            .iter()
            .any(|argument| argument.as_any().is::<ScalarFunction>())
    {
        return true;
    }
    function
        .GetArgs()
        .iter()
        .any(|argument| hasAssignSetVarFunc(argument.as_ref()))
}

/// 非 Expand 投影且表达式全为列时，可宽松消除。
pub fn canProjectionBeEliminatedLoose(projection: &LogicalProjection) -> bool {
    !projection.Proj4Expand
        && projection
            .Exprs
            .iter()
            .all(|expression| expression.as_any().is::<Column>())
}

/// 向计划注入表达式：已有投影则追加，否则外包一层恒等投影再追加。
pub fn InjectExpr(mut plan: LogicalPlanRef, expression: Expression) -> (LogicalPlanRef, Column) {
    if let Some(projection) = plan.as_any_mut().downcast_mut::<LogicalProjection>() {
        let column = projection.AppendExpr(expression);
        return (plan, column);
    }
    let ctx = plan
        .SCtx()
        .cloned()
        .expect("initialized child plan must retain context");
    let schema = plan.Schema().Clone();
    let expressions = expression::Column2Exprs(&schema.Columns);
    let mut projection = LogicalProjection {
        Exprs: expressions,
        ..LogicalProjection::default()
    }
    .Init(ctx, plan.QueryBlockOffset());
    projection.SetSchema(schema);
    projection.SetChildren(vec![plan]);
    let column = projection.AppendExpr(expression);
    (Box::new(projection), column)
}
