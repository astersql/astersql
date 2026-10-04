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

// 逻辑 Apply 算子（LogicalApply）。
//
// Apply 用于关联子查询：对外层每一行绑定关联列后执行内层计划，再按 Join 类型合并。
// 去关联（Decorrelate）规则可能把 Apply 改写为普通 Join；本文件还包含关联谓词上提、
// 列裁剪与基数估计等逻辑。

use crate::*;
use base::PlanContext as BasePlanContext;
use std::any::Any;
use std::collections::{HashMap, HashSet};

struct ApplyCardinalityContext<'a>(&'a dyn BasePlanContext);

impl cardinality::CardinalityContext for ApplyCardinalityContext<'_> {
    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        self.0.GetSessionVars()
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        self.0.GetExprCtx()
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        self.0.GetRangerCtx()
    }
}

// 子树是否包含聚合节点（影响关联谓词能否安全上提）。
fn contains_aggregation(plan: &dyn LogicalPlan) -> bool {
    plan.as_any().is::<LogicalAggregation>()
        || plan
            .Children()
            .iter()
            .any(|child| contains_aggregation(child.as_ref()))
}

// 按具体算子类型提取关联列，否则回落到基类实现。
fn extract_correlated_columns(plan: &dyn LogicalPlan) -> Vec<CorrelatedColumn> {
    macro_rules! extract {
        ($type:ty) => {
            if let Some(operator) = plan.as_any().downcast_ref::<$type>() {
                return operator.ExtractCorrelatedCols();
            }
        };
    }
    extract!(LogicalAggregation);
    extract!(LogicalApply);
    extract!(LogicalCTE);
    extract!(DataSource);
    extract!(LogicalExpand);
    extract!(LogicalJoin);
    extract!(LogicalProjection);
    extract!(LogicalSelection);
    extract!(LogicalSort);
    extract!(LogicalTopN);
    extract!(LogicalWindow);
    plan.base().ExtractCorrelatedCols()
}

// 收集计划树关联列，并按外层 schema 列对齐 UniqueID。
fn extract_correlated_columns_by_schema(
    plan: &dyn LogicalPlan,
    schema: &Schema,
) -> Vec<CorrelatedColumn> {
    let mut correlated = extract_correlated_columns(plan);
    for child in plan.Children() {
        correlated.extend(extract_correlated_columns_by_schema(child.as_ref(), schema));
    }
    let mut result = Vec::new();
    for column in &schema.Columns {
        if let Some(found) = correlated
            .iter()
            .find(|candidate| candidate.column.UniqueID == column.UniqueID)
        {
            let mut found = found.Clone();
            found.column = column.Clone();
            result.push(found);
        }
    }
    result
}

// 自底向上把引用外层列的 Selection/DataSource 谓词上提并 Decorrelate。
fn lift_correlated_selections(
    plan: &mut LogicalPlanRef,
    outer_schema: &Schema,
    lifted: &mut Vec<Expression>,
) {
    let mut children = plan.TakeChildren();
    for child in &mut children {
        lift_correlated_selections(child, outer_schema, lifted);
    }
    plan.SetChildren(children);
    // 条件含关联列或引用外层 schema 列则视为外层相关。
    let references_outer = |condition: &Expression| {
        !expression::ExtractCorColumns(condition.as_ref()).is_empty()
            || expression::ExtractColumns(condition.as_ref())
                .iter()
                .any(|column| outer_schema.Contains(column))
    };
    if let Some(source) = plan.as_any_mut().downcast_mut::<DataSource>() {
        let mut local = Vec::new();
        for condition in std::mem::take(&mut source.PushedDownConds) {
            if references_outer(&condition) {
                lifted.push(condition.Decorrelate(outer_schema));
            } else {
                local.push(condition);
            }
        }
        source.PushedDownConds = local;
        source
            .AllConds
            .retain(|condition| !references_outer(condition));
        return;
    }
    let Some(selection) = plan.as_any_mut().downcast_mut::<LogicalSelection>() else {
        return;
    };
    let mut local = Vec::new();
    for condition in std::mem::take(&mut selection.Conditions) {
        if references_outer(&condition) {
            lifted.push(condition.Decorrelate(outer_schema));
        } else {
            local.push(condition);
        }
    }
    selection.Conditions = local;
    if selection.Conditions.is_empty() && selection.Children().len() == 1 {
        *plan = selection.TakeChildren().remove(0);
    }
}

#[derive(Default)]
/// Apply：内嵌 LogicalJoin，并记录关联列与 Lateral/去关联控制标志。
pub struct LogicalApply {
    /// 连接语义与条件（Apply 复用 Join 框架）。
    pub LogicalJoin: LogicalJoin,
    /// 内层引用的外层关联列。
    pub CorCols: Vec<CorrelatedColumn>,
    /// 禁止去关联改写。
    pub NoDecorrelate: bool,
    /// 是否 LATERAL 派生表语义。
    pub IsLateral: bool,
    /// 列裁剪后若可消除右支，置位供优化器立即消费（Rust 无法像 Go 直接返回左子树）。
    /// Rust's in-place pruning API cannot return the left child as Go does.
    /// The optimizer consumes this marker immediately after pruning.
    pub PrunedToLeft: bool,
}

impl LogicalApply {
    /// 初始化基类，类型标记为 Apply。
    pub fn Init(mut self, ctx: base::ContextRef, offset: i32) -> Self {
        self.LogicalJoin.LogicalSchemaProducer.BaseLogicalPlan =
            NewBaseLogicalPlan(ctx, "Apply", offset);
        self
    }

    /// 复用 LogicalJoin 的 Explain 信息。
    pub fn ExplainInfo(&self) -> String {
        self.LogicalJoin.ExplainInfo()
    }

    /// 替换 Join 表达式与关联列中的列引用。
    pub fn ReplaceExprColumns(&mut self, replacements: &HashMap<i64, Column>) {
        self.LogicalJoin.ReplaceExprColumns(replacements);
        for correlated in &mut self.CorCols {
            if let Some(column) = replacements.get(&correlated.column.UniqueID) {
                correlated.column = column.clone();
            }
        }
    }

    /// 列裁剪；左外连接且右支无用列时可剪成仅左支并置 PrunedToLeft。
    pub fn PruneColumns(&mut self, parent_used_cols: &[Column]) -> Result<()> {
        let (mut left_columns, right_columns) = self.LogicalJoin.ExtractUsedCols(parent_used_cols);
        // 非 Lateral 左外连接且父不用右列：Apply 可退化为左子树。
        let eliminate = !self.IsLateral
            && self.LogicalJoin.JoinType == JoinType::LeftOuterJoin
            && right_columns.is_empty();
        if eliminate {
            if let Some(left) = self.Children_mut().first_mut() {
                left.PruneColumns(parent_used_cols)?;
            }
            self.PrunedToLeft = true;
            return Ok(());
        }
        if let Some(inner) = self.Children_mut().get_mut(1) {
            inner.PruneColumns(&right_columns)?;
        }
        let outer_schema = self
            .Children()
            .first()
            .and_then(findChildFullSchema)
            .unwrap_or_else(|| self.Children()[0].Schema().Clone());
        // 按外层完整 schema 重新收集内层关联列，并并入左支裁剪集合。
        self.CorCols =
            extract_correlated_columns_by_schema(self.Children()[1].as_ref(), &outer_schema);
        left_columns.extend(
            self.CorCols
                .iter()
                .map(|correlated| correlated.column.clone()),
        );
        left_columns.sort_by_key(|column| column.UniqueID);
        left_columns.dedup_by_key(|column| column.UniqueID);
        if let Some(outer) = self.Children_mut().first_mut() {
            outer.PruneColumns(&left_columns)?;
        }
        self.LogicalJoin.MergeSchema();
        Ok(())
    }

    /// 按 Join 类型与关联列 NDV 估计 Apply 输出行数。
    pub fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        if !reload && self.StatsInfo().is_some() {
            let group_ndvs = self
                .Children()
                .first()
                .and_then(|child| child.StatsInfo())
                .map(|outer| self.getGroupNDVs(outer))
                .unwrap_or_default();
            let mut stats = self.StatsInfo().cloned().expect("stats checked above");
            stats.GroupNDVs = group_ndvs;
            self.SetStats(stats.clone());
            return Ok((stats, false));
        }
        let [outer, inner] = self.Children_mut() else {
            return Err(PlannerError(
                "LogicalApply requires two children".to_owned(),
            ));
        };
        let outer_stats = outer.DeriveStats(reload)?.0;
        let inner_stats = inner.DeriveStats(reload)?.0;
        // The inner statistics already include correlated predicate selectivity.
        let row_count = if self.IsLateral
            && matches!(
                self.LogicalJoin.JoinType,
                JoinType::InnerJoin | JoinType::LeftOuterJoin
            ) {
            let (left_keys, right_keys) = self.LogicalJoin.GetJoinKeys();
            let count = if !left_keys.is_empty() {
                let context = self
                    .SCtx()
                    .cloned()
                    .ok_or_else(|| PlannerError("LogicalApply has no plan context".to_owned()))?;
                let estimated = cardinality::EstimateFullJoinRowCount(
                    &ApplyCardinalityContext(context.as_ref()),
                    false,
                    &outer_stats,
                    &inner_stats,
                    &left_keys,
                    &right_keys,
                    self.Children()[0].Schema(),
                    self.Children()[1].Schema(),
                    None,
                    None,
                );
                self.LogicalJoin.EqualCondOutCnt = estimated;
                estimated
            } else {
                // Each outer row runs the inner plan once. Dividing by outer NDV
                // here would apply the correlated selectivity a second time.
                outer_stats.RowCount * inner_stats.RowCount
            };
            if self.LogicalJoin.JoinType == JoinType::LeftOuterJoin {
                count.max(outer_stats.RowCount)
            } else {
                count
            }
        // Semi/Anti 类：经验系数 0.8；其它默认取外行数。
        } else if matches!(
            self.LogicalJoin.JoinType,
            JoinType::SemiJoin | JoinType::AntiSemiJoin
        ) {
            outer_stats.RowCount * 0.8
        } else {
            outer_stats.RowCount
        };
        let mut stats = StatsInfo {
            RowCount: row_count,
            ..StatsInfo::default()
        };
        stats
            .ColNDVs
            .extend(outer_stats.ColNDVs.iter().map(|(id, ndv)| (*id, *ndv)));
        if matches!(
            self.LogicalJoin.JoinType,
            JoinType::LeftOuterSemiJoin | JoinType::AntiLeftOuterSemiJoin
        ) {
            if let Some(marker) = self.Schema().Columns.last() {
                stats.ColNDVs.insert(marker.UniqueID, 2.0);
            }
        } else {
            let outer_len = self.Children()[0].Schema().Columns.len();
            for column in self.Schema().Columns.iter().skip(outer_len) {
                stats.ColNDVs.insert(column.UniqueID, row_count);
            }
        }
        stats.GroupNDVs = self.getGroupNDVs(&outer_stats);
        self.SetStats(stats.clone());
        Ok((stats, true))
    }

    /// 仅保留完全落在外层 schema 上的列组。
    pub fn ExtractColGroups(&self, groups: &[Vec<Column>]) -> Vec<Vec<Column>> {
        if !matches!(
            self.LogicalJoin.JoinType,
            JoinType::LeftOuterJoin | JoinType::LeftOuterSemiJoin | JoinType::AntiLeftOuterSemiJoin
        ) {
            return Vec::new();
        }
        let Some(outer) = self.Children().first() else {
            return Vec::new();
        };
        groups
            .iter()
            .filter(|group| group.iter().all(|column| outer.Schema().Contains(column)))
            .cloned()
            .collect()
    }

    /// 合并 Join 与 CorCols 中的关联列，并去掉已由外层提供的列。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        let outer_ids = self
            .Children()
            .first()
            .map(|child| {
                child
                    .Schema()
                    .Columns
                    .iter()
                    .map(|column| column.UniqueID)
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();
        let mut result = self.LogicalJoin.ExtractCorrelatedCols();
        result.retain(|column| !outer_ids.contains(&column.column.UniqueID));
        result
    }

    /// 当内层为 MaxOneRow→Selection 时，上提引用外层的选择条件到 Join ON。
    fn lift_correlated_selection_below_max_one_row(&mut self) {
        let Some(outer_schema) = self.Children().first().map(|child| child.Schema().Clone()) else {
            return;
        };
        let Some(inner) = self.Children_mut().get_mut(1) else {
            return;
        };
        let Some(max_one_row) = inner.as_any_mut().downcast_mut::<LogicalMaxOneRow>() else {
            return;
        };
        if max_one_row.Children().len() != 1
            || !max_one_row.Children()[0].as_any().is::<LogicalSelection>()
        {
            return;
        }
        let selection = max_one_row.Children_mut()[0]
            .as_any_mut()
            .downcast_mut::<LogicalSelection>()
            .expect("selection type checked above");
        let mut local = Vec::new();
        let mut lifted = Vec::new();
        for condition in std::mem::take(&mut selection.Conditions) {
            let references_outer = !expression::ExtractCorColumns(condition.as_ref()).is_empty()
                || expression::ExtractColumns(condition.as_ref())
                    .iter()
                    .any(|column| outer_schema.Contains(column));
            if references_outer {
                lifted.push(condition.Decorrelate(&outer_schema));
            } else {
                local.push(condition);
            }
        }
        selection.Conditions = local;
        if selection.Conditions.is_empty() {
            let child = selection.TakeChildren().into_iter().next();
            if let Some(child) = child {
                max_one_row.SetChildren(vec![child]);
            }
        }
        self.LogicalJoin.AttachOnConds(lifted);
    }

    /// Join 重排序场景：仅当内层为可重排内连接子树时上提关联 Selection。
    pub fn LiftInnerCorrelatedSelectionsForJoinReorder(&mut self) {
        fn contains_join(plan: &dyn LogicalPlan) -> bool {
            plan.as_any().is::<LogicalJoin>()
                || plan
                    .Children()
                    .iter()
                    .any(|child| contains_join(child.as_ref()))
        }
        fn contains_barrier(plan: &dyn LogicalPlan) -> bool {
            plan.as_any().is::<LogicalAggregation>()
                || plan.as_any().is::<LogicalMaxOneRow>()
                || plan.as_any().is::<crate::LogicalLimit>()
                || plan.as_any().is::<crate::LogicalTopN>()
                || plan.as_any().is::<crate::LogicalWindow>()
                || plan
                    .as_any()
                    .downcast_ref::<LogicalJoin>()
                    .is_some_and(|join| join.JoinType != JoinType::InnerJoin)
                || plan
                    .Children()
                    .iter()
                    .any(|child| contains_barrier(child.as_ref()))
        }
        let Some(outer_schema) = self.Children().first().map(|child| child.Schema().Clone()) else {
            return;
        };
        let Some(inner) = self.Children_mut().get_mut(1) else {
            return;
        };
        // 穿越聚合/行数屏障/外连接需要去关联器的 FIRST_ROW 等改写，否则不安全。
        // Join reorder may lift a correlated Selection only through a fully
        // reorderable inner-join subtree.  Crossing aggregation, row-count
        // barriers, or an outer join would require the Go decorrelator's
        // FIRST_ROW/null-extension rewrites and otherwise leaves referenced
        // inner columns outside the Apply child's schema.
        if !contains_join(inner.as_ref()) || contains_barrier(inner.as_ref()) {
            return;
        }
        let mut lifted = Vec::new();
        lift_correlated_selections(inner, &outer_schema, &mut lifted);
        self.LogicalJoin
            .AttachOnConds(expression::RemoveDupExprs(lifted));
    }

    /// 在 Join FD 上补充关联列 UniqueID 与内层列的等价关系。
    pub fn ExtractFD(&mut self) -> fd::FDSet {
        let mut result = self.LogicalJoin.ExtractFD();
        if let Some(inner) = self.Children().get(1) {
            for column in &inner.Schema().Columns {
                if column.CorrelatedColUniqueID != 0 {
                    result.AddEquivalence(
                        fd::intset::NewFastIntSet(vec![column.CorrelatedColUniqueID as i32]),
                        fd::intset::NewFastIntSet(vec![column.UniqueID as i32]),
                    );
                }
            }
        }
        self.base_mut().SetFDs(result.clone());
        result
    }

    /// 内/左连接且无连接条件、外层有唯一键时，内层聚合可考虑上拉。
    pub fn CanPullUpAgg(&self) -> bool {
        matches!(
            self.LogicalJoin.JoinType,
            JoinType::InnerJoin | JoinType::LeftOuterJoin
        ) && self.LogicalJoin.EqualConditions.is_empty()
            && self.LogicalJoin.LeftConditions.is_empty()
            && self.LogicalJoin.RightConditions.is_empty()
            && self.LogicalJoin.OtherConditions.is_empty()
            && self
                .Children()
                .first()
                .is_some_and(|child| !child.Schema().PKOrUK.is_empty())
    }

    /// 从 eq/nulleq 中拆出（外层关联列, 内层列）对，供去关联建等值条件。
    pub fn DeCorColFromEqExpr(&self, expression: &Expression) -> Option<(Column, Column)> {
        let function = expression.as_scalar_function()?;
        if function.FuncName.L != "eq" || function.GetArgs().len() != 2 {
            return None;
        }
        let arguments = function.GetArgs();
        if let (Some(column), Some(correlated)) = (
            arguments[0].as_column(),
            arguments[1].as_correlated_column(),
        ) {
            return Some((correlated.column.clone(), column.clone()));
        }
        if let (Some(correlated), Some(column)) = (
            arguments[0].as_correlated_column(),
            arguments[1].as_column(),
        ) {
            return Some((correlated.column.clone(), column.clone()));
        }
        None
    }

    /// 左外/Semi 类连接继承外层 GroupNDV，否则清空。
    fn getGroupNDVs(&self, outer: &StatsInfo) -> Vec<property::GroupNDV> {
        if matches!(
            self.LogicalJoin.JoinType,
            JoinType::LeftOuterJoin | JoinType::LeftOuterSemiJoin | JoinType::AntiLeftOuterSemiJoin
        ) {
            outer.GroupNDVs.clone()
        } else {
            Vec::new()
        }
    }
}

/// 沿 Join/Apply/Selection 链查找完整（未裁剪）schema。
pub fn findChildFullSchema(plan: &LogicalPlanRef) -> Option<Schema> {
    if let Some(join) = plan.as_any().downcast_ref::<LogicalJoin>() {
        return join.FullSchema.as_ref().map(Schema::Clone);
    }
    if let Some(apply) = plan.as_any().downcast_ref::<LogicalApply>() {
        return apply.LogicalJoin.FullSchema.as_ref().map(Schema::Clone);
    }
    if plan.as_any().is::<LogicalSelection>() && plan.Children().len() == 1 {
        return findChildFullSchema(&plan.Children()[0]);
    }
    None
}

/// LogicalPlan 适配：谓词下推委托 LogicalJoin，其余转发本类型。
impl LogicalPlan for LogicalApply {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn base(&self) -> &BaseLogicalPlan {
        &self.LogicalJoin.LogicalSchemaProducer.BaseLogicalPlan
    }
    fn base_mut(&mut self) -> &mut BaseLogicalPlan {
        &mut self.LogicalJoin.LogicalSchemaProducer.BaseLogicalPlan
    }
    fn ExplainInfo(&self) -> String {
        Self::ExplainInfo(self)
    }
    fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        self.LogicalJoin.PredicatePushDown(predicates)
    }
    fn PruneColumns(&mut self, columns: &[Column]) -> Result<()> {
        Self::PruneColumns(self, columns)
    }
    fn BuildKeyInfo(&mut self) {
        self.LogicalJoin.BuildKeyInfo();
    }
    fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        Self::DeriveStats(self, reload)
    }
}
