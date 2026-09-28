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

// 逻辑聚合算子（LogicalAggregation）。
//
// 对应 SQL 的 GROUP BY / 聚合函数（SUM、COUNT、MAX 等）。负责谓词下推跨越聚合边界、
// 列裁剪、键信息与统计推导，以及 Partial/Complete 等聚合执行模式判定。
// AggFuncDesc 描述单个聚合函数的名称、参数与模式。

use crate::*;
use base::PlanContext as BasePlanContext;
use std::any::Any;
use std::collections::{HashMap, HashSet};

// 复用 aggregation crate 的聚合函数描述与执行模式常量。
pub use aggregation::{
    AggFuncDesc, AggFunctionMode, CompleteMode, DedupMode, FinalMode, Partial1Mode, Partial2Mode,
};

/// Bridges core's object-safe plan context into cardinality's minimal view.
struct AggregationCardinalityContext<'a>(&'a dyn BasePlanContext);

impl cardinality::CardinalityContext for AggregationCardinalityContext<'_> {
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

#[derive(Default)]
/// 逻辑聚合节点：聚合函数列表、分组项与物理偏好提示。
pub struct LogicalAggregation {
    /// schema 生产者嵌入。
    pub LogicalSchemaProducer: LogicalSchemaProducer,
    /// 聚合函数描述列表（与输出列一一对应）。
    pub AggFuncs: Vec<AggFuncDesc>,
    /// GROUP BY 表达式列表。
    pub GroupByItems: Vec<Expression>,
    /// 物理聚合类型偏好位图（Hash/Stream 等）。
    pub PreferAggType: u64,
    /// 是否偏好下推到 Coprocessor 执行聚合。
    pub PreferAggToCop: bool,
    /// 可能保留的有序属性（来自子计划排序前缀）。
    pub PossibleProperties: Vec<Vec<Column>>,
    /// 输入行数估计（来自子计划统计）。
    pub InputCount: f64,
    /// 禁止下推到 Coprocessor。
    pub NoCopPushDown: bool,
}

impl LogicalAggregation {
    /// 初始化基类，类型标记为 Aggregation。
    pub fn Init(mut self, ctx: base::ContextRef, offset: i32) -> Self {
        self.LogicalSchemaProducer.BaseLogicalPlan = NewBaseLogicalPlan(ctx, "Aggregation", offset);
        self
    }

    /// 生成 group by / funcs 的 Explain 文本。
    /// 生成 EXPLAIN：group by 与 funcs 摘要。
    pub fn ExplainInfo(&self) -> String {
        let groups = self
            .GroupByItems
            .iter()
            .map(|item| String::from_utf8_lossy(&item.HashCode()).into_owned())
            .collect::<Vec<_>>()
            .join(", ");
        let funcs = self
            .AggFuncs
            .iter()
            .map(|func| func.Name.clone())
            .collect::<Vec<_>>()
            .join(", ");
        match (groups.is_empty(), funcs.is_empty()) {
            (false, false) => format!("group by:{groups}, funcs:{funcs}"),
            (false, true) => format!("group by:{groups}"),
            (true, false) => format!("funcs:{funcs}"),
            (true, true) => String::new(),
        }
    }

    /// 按 UniqueID 映射替换聚合参数、ORDER BY 与 GROUP BY 中的列引用。
    /// 按 UniqueID 映射替换聚合参数与 GROUP BY 中的列。
    pub fn ReplaceExprColumns(&mut self, replacements: &HashMap<i64, Column>) {
        for func in &mut self.AggFuncs {
            for arg in &mut func.Args {
                *arg = replace_expr(arg, replacements);
            }
            for item in &mut func.OrderByItems {
                item.Expr = replace_expr(&item.Expr, replacements);
            }
        }
        for item in &mut self.GroupByItems {
            *item = replace_expr(item, replacements);
        }
    }

    /// 收集 GROUP BY 表达式涉及的列 UniqueID。
    /// 收集 GROUP BY 表达式中的列 UniqueID。
    fn group_by_column_ids(&self) -> HashSet<i64> {
        self.GroupByItems
            .iter()
            .flat_map(|item| expression::ExtractColumns(item.as_ref()))
            .map(|column| column.UniqueID)
            .collect()
    }

    /// 谓词是否仅依赖分组键（及 firstrow 参数列），因而可下推到聚合之下。
    /// 谓词是否仅依赖分组键（及 firstrow 参数），可安全下推到聚合下方。
    fn predicate_is_group_invariant(&self, predicate: &Expression) -> bool {
        let mut group_ids = self.group_by_column_ids();
        group_ids.extend(
            self.AggFuncs
                .iter()
                .filter(|function| function.Name.eq_ignore_ascii_case("firstrow"))
                .flat_map(|function| &function.Args)
                .flat_map(|argument| expression::ExtractColumns(argument.as_ref()))
                .map(|column| column.UniqueID),
        );
        let columns = expression::ExtractColumns(predicate.as_ref());
        !columns.is_empty()
            && columns
                .iter()
                .all(|column| group_ids.contains(&column.UniqueID))
    }

    /// 将谓词拆成可下推与必须保留在聚合之上两部分；可下推时用 firstrow 输出替换回子列。
    /// 将谓词分为可下推（分组不变量）与需保留在聚合上方两类。
    pub fn splitCondForAggregation(
        &self,
        predicates: Vec<Expression>,
    ) -> (Vec<Expression>, Vec<Expression>) {
        let first_row_outputs = self
            .AggFuncs
            .iter()
            .zip(&self.Schema().Columns)
            .filter_map(|(function, output)| {
                (function.Name.eq_ignore_ascii_case("firstrow") && function.Args.len() == 1)
                    .then(|| (output.UniqueID, function.Args[0].CloneExpr()))
            })
            .collect::<HashMap<_, _>>();
        let mut pushable = Vec::new();
        let mut retained = Vec::new();
        for predicate in predicates {
            let substituted = substitute_aggregate_outputs(&predicate, &first_row_outputs);
            let (to_push, to_retain) = self.split_cnf_for_aggregation(&substituted);
            pushable.extend(to_push);
            if to_retain
                .iter()
                .any(|condition| !self.is_redundant_not_null(condition))
            {
                // The residual remains the original aggregate predicate.  It
                // is not safe to evaluate the substituted child expression at
                // the aggregate boundary.
                retained.push(predicate);
            }
        }
        (pushable, retained)
    }

    /// COUNT-like aggregate outputs are non-null by construction.  Go's
    /// predicate simplification removes `NOT(ISNULL(count))` instead of
    /// retaining an unnecessary Selection above the aggregation.
    fn is_redundant_not_null(&self, condition: &Expression) -> bool {
        let Some(not) = condition.as_scalar_function() else {
            return false;
        };
        if not.FuncName.L != expression::ast::UnaryNot || not.GetArgs().len() != 1 {
            return false;
        }
        let Some(is_null) = not.GetArgs()[0]
            .as_any()
            .downcast_ref::<expression::ScalarFunction>()
        else {
            return false;
        };
        if is_null.FuncName.L != expression::ast::IsNull || is_null.GetArgs().len() != 1 {
            return false;
        }
        let Some(column) = is_null.GetArgs()[0].as_column() else {
            return false;
        };
        self.Schema()
            .Columns
            .iter()
            .find(|output| output.UniqueID == column.UniqueID)
            .and_then(|output| output.RetType.as_ref())
            .is_some_and(|field_type| mysql::r#type::HasNotNullFlag(field_type.GetFlag()))
    }

    /// Split one CNF condition using Go's DNF-aware aggregation push-down.
    fn split_cnf_for_aggregation(
        &self,
        condition: &Expression,
    ) -> (Vec<Expression>, Vec<Expression>) {
        let mut pushable = Vec::new();
        let mut retained = Vec::new();
        for item in expression::SplitCNFItems(condition.as_ref()) {
            let (item_pushable, item_retained) = self.split_dnf_for_aggregation(&item);
            pushable.extend(item_pushable);
            retained.extend(item_retained);
        }
        (pushable, retained)
    }

    /// For each DNF branch, retain the original condition when an aggregate
    /// output prevents a complete branch from crossing the aggregation, but
    /// still push the relaxed common branch predicate below it.
    fn split_dnf_for_aggregation(
        &self,
        condition: &Expression,
    ) -> (Vec<Expression>, Vec<Expression>) {
        let items = expression::SplitDNFItems(condition.as_ref());
        if items.len() == 1 {
            if self.predicate_is_group_invariant(&items[0]) {
                return (vec![items[0].CloneExpr()], Vec::new());
            }
            return (Vec::new(), vec![items[0].CloneExpr()]);
        }

        let Some(context) = self.SCtx().cloned() else {
            return (Vec::new(), vec![condition.CloneExpr()]);
        };
        let mut pushed_branches = Vec::with_capacity(items.len());
        let mut retained_branches = Vec::new();
        for item in items {
            let (branch_pushable, branch_retained) = self.split_cnf_for_aggregation(&item);
            if branch_pushable.is_empty() {
                return (Vec::new(), vec![condition.CloneExpr()]);
            }
            pushed_branches.push(
                expression::ComposeCNFCondition(context.GetExprCtx(), &branch_pushable)
                    .expect("non-empty aggregation DNF push-down branch"),
            );
            if !branch_retained.is_empty() {
                retained_branches.push(
                    expression::ComposeCNFCondition(context.GetExprCtx(), &branch_retained)
                        .expect("non-empty aggregation DNF residual branch"),
                );
            }
        }
        if retained_branches.is_empty() {
            return (vec![condition.CloneExpr()], Vec::new());
        }
        (
            vec![
                expression::ComposeDNFCondition(context.GetExprCtx(), &pushed_branches)
                    .expect("non-empty aggregation DNF push-down condition"),
            ],
            vec![condition.CloneExpr()],
        )
    }

    /// 拆分谓词：可下推部分交给子节点，其余与子节点残差一并返回。
    /// 分组不变量下推到子节点，其余留在聚合上方。
    pub fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        let (to_push, retained) = self.splitCondForAggregation(predicates);
        let mut child_retained = if let Some(child) = self.Children_mut().first_mut() {
            PredicatePushDownPlan(child, to_push)?
        } else {
            to_push
        };
        child_retained.extend(retained);
        Ok(child_retained)
    }

    /// 按父使用列裁剪聚合函数/输出列，并向子节点传递仍需的输入列。
    /// 裁剪父层未用的聚合输出；全空时保留 count(1)/firstrow(1) 以保持空输入基数。
    pub fn PruneColumns(&mut self, parent_used_cols: &[Column]) -> Result<()> {
        let all_first_row = self
            .AggFuncs
            .iter()
            .all(|function| function.Name == aggregation::ast::AggFuncFirstRow);
        let used_ids = parent_used_cols
            .iter()
            .map(|column| column.UniqueID)
            .collect::<HashSet<_>>();
        let old_columns = self.Schema().Columns.clone();
        let keep = old_columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                used_ids.contains(&column.UniqueID)
                    || self
                        .AggFuncs
                        .get(index)
                        .is_some_and(|func| expression::ExprsHasSideEffects(&func.Args))
            })
            .collect::<Vec<_>>();

        let mut new_funcs = Vec::new();
        let mut new_columns = Vec::new();
        for (index, retain) in keep.into_iter().enumerate() {
            if retain {
                if let Some(func) = self.AggFuncs.get(index) {
                    new_funcs.push(func.clone());
                }
                if let Some(column) = old_columns.get(index) {
                    new_columns.push(column.clone());
                }
            }
        }
        // 聚合在空输入时也需保留一个输出列：全裁剪后用 count(1) 或 firstrow(1) 占位。
        // A logical aggregation must retain one output to preserve empty-input
        // cardinality. Go deliberately replaces a fully pruned aggregate with
        // count(1), or firstrow(1) when every original function was firstrow;
        // retaining an old argument would keep an otherwise unused input
        // column alive.
        if new_funcs.is_empty() && !self.AggFuncs.is_empty() {
            let grouped_first_row = self.AggFuncs.iter().enumerate().find(|(_, function)| {
                function.Name == aggregation::ast::AggFuncFirstRow
                    && function.Args.first().is_some_and(|argument| {
                        self.GroupByItems.iter().any(|group| {
                            argument.Equal(
                                self.SCtx()
                                    .expect("aggregation context")
                                    .GetExprCtx()
                                    .GetEvalCtx(),
                                group.as_ref(),
                            )
                        })
                    })
            });
            if let Some((index, function)) = grouped_first_row
                && let Some(column) = old_columns.get(index)
            {
                new_funcs.push(function.clone());
                new_columns.push(column.Clone());
            }
        }
        if new_funcs.is_empty() && !self.AggFuncs.is_empty() {
            let context = self
                .SCtx()
                .cloned()
                .ok_or_else(|| PlannerError("aggregation has no plan context".to_owned()))?;
            let name = if all_first_row {
                aggregation::ast::AggFuncFirstRow
            } else {
                aggregation::ast::AggFuncCount
            };
            let descriptor = aggregation::NewAggFuncDesc(
                context.GetExprCtx(),
                name,
                vec![Box::new(expression::NewOne())],
                false,
            )
            .map_err(|error| PlannerError(error.to_string()))?;
            let column = Column::new(
                descriptor.RetTp.clone().unwrap_or_default(),
                0,
                context.GetExprCtx().AllocPlanColumnID(),
                0,
            );
            new_funcs.push(descriptor);
            new_columns.push(column);
        }
        self.AggFuncs = new_funcs;
        self.Schema_mut().Columns = new_columns;

        // 去掉无列引用且无副作用的分组项；若清空则补常量 1，保持标量聚合语义。
        if !self.GroupByItems.is_empty() {
            self.GroupByItems.retain(|item| {
                !expression::ExtractColumns(item.as_ref()).is_empty()
                    || expression::ExprHasSetVarOrSleep(item.as_ref())
            });
            if self.GroupByItems.is_empty() {
                self.GroupByItems
                    .push(Box::new(expression::NewOne()) as Expression);
            }
        }

        let mut child_columns = Vec::new();
        for func in &self.AggFuncs {
            for expression in func
                .Args
                .iter()
                .chain(func.OrderByItems.iter().map(|item| &item.Expr))
            {
                child_columns.extend(
                    expression::ExtractColumns(expression.as_ref())
                        .into_iter()
                        .cloned(),
                );
            }
        }
        for expression in &self.GroupByItems {
            child_columns.extend(
                expression::ExtractColumns(expression.as_ref())
                    .into_iter()
                    .cloned(),
            );
        }
        if let Some(child) = self.Children_mut().first_mut() {
            child.PruneColumns(&child_columns)?;
        }
        Ok(())
    }

    /// 若 GROUP BY 全为列，则这些列构成唯一键；无 GROUP BY 则 MaxOneRow。
    /// 由 GROUP BY 列推导唯一键；无 GROUP BY 则 MaxOneRow。
    pub fn BuildSelfKeyInfo(&mut self) {
        let group_columns = self.GetGroupByCols();
        if group_columns.len() == self.GroupByItems.len() && !self.GroupByItems.is_empty() {
            if let Some(indices) = self.Schema().ColumnsIndices(&group_columns) {
                let key = indices
                    .into_iter()
                    .map(|index| self.Schema().Columns[index].Clone())
                    .collect();
                self.Schema_mut().PKOrUK.push(key);
            }
        }
        if self.GroupByItems.is_empty() {
            self.SetMaxOneRow(true);
        }
    }

    /// 非 Partial 模式下推导 schema 键与自身键信息。
    /// 非 Partial 模式下构建键信息。
    pub fn BuildKeyInfo(&mut self) {
        if !self.IsPartialModeAgg() {
            self.LogicalSchemaProducer.BuildKeyInfo();
            self.BuildSelfKeyInfo();
        }
    }

    /// 用分组列 NDV 乘积估计输出行数，并写入各输出列 NDV。
    /// 按分组列 NDV 乘积估计输出行数（无 GROUP BY 则为 1）。
    pub fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        if !reload && let Some(stats) = self.StatsInfo() {
            return Ok((stats.clone(), false));
        }
        let (child_stats, child_schema) = if let Some(child) = self.Children_mut().first_mut() {
            let stats = child.DeriveStats(reload)?.0;
            (stats, child.Schema().Clone())
        } else {
            (StatsInfo::default(), expression::NewSchema(Vec::new()))
        };
        self.InputCount = child_stats.RowCount;
        // Go estimates GROUP BY cardinality through the shared multi-column
        // NDV estimator.  Multiplying per-column NDVs makes every multi-key
        // pseudo group clamp to the full input cardinality and inflated the
        // TPC-H aggregate/TopN trees.
        let group_columns = self
            .GroupByItems
            .iter()
            .flat_map(|item| expression::ExtractColumns(item.as_ref()))
            .cloned()
            .collect::<Vec<_>>();
        let context = self
            .SCtx()
            .map(|context| AggregationCardinalityContext(context.as_ref()));
        let (estimated_row_count, _) = cardinality::EstimateColsNDVWithMatchedLen(
            context
                .as_ref()
                .map(|context| context as &dyn cardinality::CardinalityContext),
            &group_columns,
            &child_schema,
            &child_stats,
        );
        // Go LogicalAggregation.DeriveStats uses the shared NDV estimator
        // directly, including for pseudo child statistics. The pseudo 8,000
        // cap belongs to missing column statistics at the source, not to a
        // known group-column NDV propagated through a join.
        let row_count = estimated_row_count;
        let group_ndvs = child_stats
            .GetGroupNDV4Cols(&group_columns)
            .cloned()
            .into_iter()
            .collect();
        let mut stats = StatsInfo {
            RowCount: row_count,
            GroupNDVs: group_ndvs,
            ..StatsInfo::default()
        };
        for column in &self.Schema().Columns {
            stats.ColNDVs.insert(column.UniqueID, row_count);
        }
        self.SetStats(stats.clone());
        Ok((stats, true))
    }

    /// 多列 GROUP BY 时作为列组返回。
    /// 多列 GROUP BY 时作为列组返回。
    pub fn ExtractColGroups(&self, _parent: &[Vec<Column>]) -> Vec<Vec<Column>> {
        let mut columns = self
            .GroupByItems
            .iter()
            .flat_map(|expr| {
                expression::ExtractColumns(expr.as_ref())
                    .into_iter()
                    .cloned()
            })
            .collect::<Vec<_>>();
        columns.sort_by_key(|column| column.UniqueID);
        columns.dedup_by_key(|column| column.UniqueID);
        (columns.len() > 1).then_some(columns).into_iter().collect()
    }

    /// 子排序前缀覆盖分组列时可保留该有序属性。
    /// 从子树排序中筛选前缀覆盖 GROUP BY 的可能属性。
    pub fn PreparePossibleProperties(&mut self, child_orders: &[Vec<Column>]) -> Vec<Vec<Column>> {
        let groups = self.GetGroupByCols();
        self.PossibleProperties = child_orders
            .iter()
            .filter(|order| {
                order.len() >= groups.len()
                    && groups.iter().all(|group| {
                        order[..groups.len()]
                            .iter()
                            .any(|column| column.UniqueID == group.UniqueID)
                    })
            })
            .map(|order| order[..groups.len()].to_vec())
            .collect();
        if groups.is_empty() {
            self.PossibleProperties.push(Vec::new());
        }
        self.PossibleProperties.clone()
    }

    /// 从聚合/分组表达式中提取关联列。
    /// 从聚合参数与 GROUP BY 中提取关联列。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        self.GetUsedExprs()
            .into_iter()
            .flat_map(|expr| {
                expression::ExtractCorColumns(expr.as_ref())
                    .into_iter()
                    .map(CorrelatedColumn::Clone)
            })
            .collect()
    }

    /// 分组列函数决定非 firstrow 聚合输出列（严格 FD）。
    /// 分组键严格决定非 firstrow 聚合输出列。
    pub fn ExtractFD(&mut self) -> fd::FDSet {
        let mut result = self.base_mut().ExtractFD().clone();
        let mut from = fd::intset::NewFastIntSet(Vec::new());
        for column in self.GetGroupByCols() {
            from.Insert(column.UniqueID as i32);
        }
        if from.IsEmpty() {
            from.Insert(0);
        }
        let mut to = fd::intset::NewFastIntSet(Vec::new());
        for (index, column) in self.Schema().Columns.iter().enumerate() {
            if self
                .AggFuncs
                .get(index)
                .is_some_and(|func| func.Name != "firstrow")
            {
                to.Insert(column.UniqueID as i32);
            }
        }
        result.AddStrictFunctionalDependency(from, to);
        self.base_mut().SetFDs(result.clone());
        result
    }

    /// 提取直接作为 GROUP BY 项的列，保留 Go 侧的顺序与重复项。
    pub fn GetGroupByCols(&self) -> Vec<Column> {
        self.GroupByItems
            .iter()
            .filter_map(|expr| expr.as_column().cloned())
            .collect()
    }

    /// 聚合与分组用到的全部列。
    /// 聚合与 GROUP BY 引用的全部列。
    pub fn GetUsedCols(&self) -> Vec<Column> {
        let mut columns = self
            .GetUsedExprs()
            .into_iter()
            .flat_map(|expr| {
                expression::ExtractColumns(expr.as_ref())
                    .into_iter()
                    .cloned()
            })
            .collect::<Vec<_>>();
        columns.sort_by_key(|column| column.UniqueID);
        columns.dedup_by_key(|column| column.UniqueID);
        columns
    }

    /// 汇总 GROUP BY、聚合参数与 OrderBy 表达式。
    /// 收集 GROUP BY 与聚合参数/ORDER BY 表达式引用。
    fn GetUsedExprs(&self) -> Vec<&Expression> {
        let mut expressions = self.GroupByItems.iter().collect::<Vec<_>>();
        for func in &self.AggFuncs {
            expressions.extend(func.Args.iter());
            expressions.extend(func.OrderByItems.iter().map(|item| &item.Expr));
        }
        expressions
    }

    /// 潜在分区键即分组列。
    pub fn GetPotentialPartitionKeys(&self) -> Vec<Column> {
        self.GetGroupByCols()
    }
    /// 是否存在 DISTINCT 聚合。
    pub fn HasDistinct(&self) -> bool {
        self.AggFuncs.iter().any(|func| func.HasDistinct)
    }
    /// 是否存在带 ORDER BY 的聚合（如 GROUP_CONCAT）。
    pub fn HasOrderBy(&self) -> bool {
        self.AggFuncs
            .iter()
            .any(|func| !func.OrderByItems.is_empty())
    }
    /// 首个聚合是否为 Partial1 模式（两阶段聚合的第一段）。
    pub fn IsPartialModeAgg(&self) -> bool {
        // Go deliberately indexes AggFuncs[0]: an empty list is an invalid
        // LogicalAggregation state and must fail fast instead of returning false.
        self.AggFuncs[0].Mode == Partial1Mode
    }
    /// 首个聚合是否为 Complete 模式（单阶段完成）。
    pub fn IsCompleteModeAgg(&self) -> bool {
        self.AggFuncs[0].Mode == CompleteMode
    }
    /// 无 GROUP BY 且所有参数在子 schema 列替换为 NULL 后仍为 NULL 时可上拉。
    pub fn CanPullUp(&self) -> bool {
        if !self.GroupByItems.is_empty() {
            return false;
        }
        let Some(child_schema) = self.Children().first().map(|child| child.Schema()) else {
            return self.AggFuncs.is_empty();
        };
        let mut context = exprstatic::NewExprContext(Vec::new());
        self.AggFuncs.iter().all(|function| {
            function.Args.iter().all(|argument| {
                expression::EvaluateExprWithNull(
                    &mut context,
                    child_schema,
                    argument.CloneExpr(),
                    true,
                )
                .ok()
                .and_then(|result| result.as_constant().cloned())
                .is_some_and(|constant| constant.Value.IsNull())
            })
        })
    }
    /// 常量 max/min 在非空组上结果等于唯一参数。
    pub fn aggFuncResultMatchesArgForNonEmptyGroup(&self, index: usize) -> bool {
        self.AggFuncs.get(index).is_some_and(|func| {
            !func.HasDistinct
                && func.OrderByItems.is_empty()
                && matches!(func.Name.as_str(), "min" | "max")
                && func.Args.len() == 1
                && func.Args[0].ConstLevel() >= expression::ConstOnlyInContext
        })
    }
    /// DISTINCT 聚合参数列是否都落在给定属性列集合内。
    pub fn DistinctArgsMeetsProperty(&self, columns: &[Column]) -> bool {
        let ids = columns
            .iter()
            .map(|column| column.UniqueID)
            .collect::<HashSet<_>>();
        self.AggFuncs
            .iter()
            .filter(|func| func.HasDistinct)
            .all(|func| {
                func.Args
                    .iter()
                    .flat_map(|arg| expression::ExtractColumns(arg.as_ref()))
                    .all(|column| ids.contains(&column.UniqueID))
            })
    }
    /// GROUP BY 是否全为无列引用的常量表达式。
    /// GROUP BY 是否仅含常量表达式。
    pub fn hasOnlyConstGroupByItems(&self) -> bool {
        self.GroupByItems
            .iter()
            .all(|item| item.ConstLevel() >= expression::ConstOnlyInContext)
    }
    /// 未禁止且基类允许时才可下推 Coprocessor。
    pub fn CanPushToCop(&self, store: crate::base_logical_plan::StoreType) -> bool {
        !self.NoCopPushDown && self.base().CanPushToCop(store)
    }
    /// 物理偏好位冲突时清零 PreferAggType。
    pub fn ResetHintIfConflicted(&mut self) {
        if self.PreferAggType.count_ones() > 1 {
            self.PreferAggType = 0;
        }
    }
    /// 复制聚合物理偏好提示。
    pub fn CopyAggHints(&mut self, source: &LogicalAggregation) {
        self.PreferAggType = source.PreferAggType;
        self.PreferAggToCop = source.PreferAggToCop;
    }
    /// 读取统计中的分组 NDV 列表。
    pub fn getGroupNDVs(&self, stats: &StatsInfo) -> Vec<property::GroupNDV> {
        stats.GroupNDVs.clone()
    }
    /// 常量结果场景下用到的列。
    pub fn getAggFuncsColsForConstResult(&self) -> Vec<Column> {
        if self.GroupByItems.is_empty() {
            return Vec::new();
        }
        self.Schema()
            .Columns
            .iter()
            .enumerate()
            .filter(|(index, _)| self.aggFuncResultMatchesArgForNonEmptyGroup(*index))
            .map(|(_, column)| column.Clone())
            .collect()
    }
    /// firstrow 相关用到的列。
    pub fn getAggFuncsColsForFirstRow(&self) -> Vec<Column> {
        if self.hasOnlyConstGroupByItems() {
            return Vec::new();
        }
        self.Schema()
            .Columns
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                self.AggFuncs.get(*index).is_some_and(|function| {
                    function.Name == aggregation::ast::AggFuncFirstRow
                        && function.Args.first().is_some_and(|argument| {
                            expression::ExtractColumns(argument.as_ref()).len() == 1
                        })
                })
            })
            .map(|(_, column)| column.Clone())
            .collect()
    }
    /// 谓词下推拆分（委托 splitCondForAggregation）。
    pub fn pushDownPredicates(
        &self,
        predicates: Vec<Expression>,
    ) -> (Vec<Expression>, Vec<Expression>) {
        self.splitCondForAggregation(predicates)
    }
    /// 按聚合函数维度的谓词下推拆分（同上）。
    pub fn pushDownPredicatesByAggFuncs(
        &self,
        predicates: Vec<Expression>,
    ) -> (Vec<Expression>, Vec<Expression>) {
        self.splitCondForAggregation(predicates)
    }
    /// 按 GROUP BY 维度的谓词下推拆分（同上）。
    pub fn pushDownPredicatesByGroupby(
        &self,
        predicates: Vec<Expression>,
    ) -> (Vec<Expression>, Vec<Expression>) {
        self.splitCondForAggregation(predicates)
    }
    /// CNF 谓词下推拆分（同上）。
    pub fn pushDownCNFPredicatesForAggregation(
        &self,
        predicates: Vec<Expression>,
    ) -> (Vec<Expression>, Vec<Expression>) {
        self.splitCondForAggregation(predicates)
    }
    /// DNF 谓词下推拆分（同上）。
    pub fn pushDownDNFPredicates(
        &self,
        predicates: Vec<Expression>,
    ) -> (Vec<Expression>, Vec<Expression>) {
        self.splitCondForAggregation(predicates)
    }
}

// 将聚合输出列（如 firstrow 结果列）替换为对应子表达式，便于下推判定。
/// 将 firstrow 输出列替换为子节点表达式，便于判定分组不变量。
fn substitute_aggregate_outputs(
    expression: &Expression,
    replacements: &HashMap<i64, Expression>,
) -> Expression {
    if let Some(column) = expression.as_column() {
        return replacements
            .get(&column.UniqueID)
            .map(|replacement| replacement.CloneExpr())
            .unwrap_or_else(|| expression.CloneExpr());
    }
    let Some(function) = expression.as_scalar_function() else {
        return expression.CloneExpr();
    };
    let mut function = function.clone_scalar();
    for argument in function.GetArgsMut() {
        *argument = substitute_aggregate_outputs(argument, replacements);
    }
    function.CleanHashCode();
    Box::new(function)
}

// 按 UniqueID 替换列/关联列，并递归进入标量函数参数。
/// 按 UniqueID 递归替换表达式中的列/关联列。
fn replace_expr(expr: &Expression, replacements: &HashMap<i64, Column>) -> Expression {
    if let Some(column) = expr.as_column() {
        return replacements
            .get(&column.UniqueID)
            .cloned()
            .map(|column| Box::new(column) as Expression)
            .unwrap_or_else(|| expr.clone());
    }
    if let Some(correlated) = expr.as_correlated_column() {
        let mut result = correlated.Clone();
        if let Some(column) = replacements.get(&result.column.UniqueID) {
            result.column = column.clone();
        }
        return Box::new(result);
    }
    if let Some(function) = expr.as_scalar_function() {
        let mut result = function.clone_scalar();
        for argument in result.GetArgsMut() {
            *argument = replace_expr(argument, replacements);
        }
        result.CleanHashCode();
        return Box::new(result);
    }
    expr.clone()
}

/// LogicalPlan 适配：转发到本类型方法。
/// 将 LogicalAggregation 接入 LogicalPlan trait。
impl LogicalPlan for LogicalAggregation {
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
    fn ExplainInfo(&self) -> String {
        Self::ExplainInfo(self)
    }
    fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        Self::PredicatePushDown(self, predicates)
    }
    fn PruneColumns(&mut self, columns: &[Column]) -> Result<()> {
        Self::PruneColumns(self, columns)
    }
    fn BuildKeyInfo(&mut self) {
        Self::BuildKeyInfo(self)
    }
    fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        Self::DeriveStats(self, reload)
    }
}
