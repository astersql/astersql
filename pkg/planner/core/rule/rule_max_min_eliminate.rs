// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// MAX/MIN 聚合消除（Max/Min Eliminate）逻辑优化规则。
//
// 把无 GROUP BY 的标量 `MAX(col)` / `MIN(col)` 改写为「按列排序 + LIMIT 1」；
// 多聚合且列上有可用索引时，拆成多路子计划再用笛卡尔 Inner Join 合并结果。

// 优化规则的数据结构、递归控制流和错误传播形状。
// 外部 TiDB 类型与函数调用保留为后续模块接线点；本任务不创建跨文件模块连线。
//
// Go imports（仅保留来源依赖，暂不接入 Rust crate）：
// 	"context"
// 	"github.com/pingcap/tidb/pkg/expression"
// 	"github.com/pingcap/tidb/pkg/expression/aggregation"
// 	"github.com/pingcap/tidb/pkg/meta/model"
// 	"github.com/pingcap/tidb/pkg/parser/ast"
// 	"github.com/pingcap/tidb/pkg/parser/mysql"
// 	"github.com/pingcap/tidb/pkg/planner/core/base"
// 	"github.com/pingcap/tidb/pkg/planner/core/operator/logicalop"
// 	"github.com/pingcap/tidb/pkg/planner/util"
// 	"github.com/pingcap/tidb/pkg/types"
// 	"github.com/pingcap/tidb/pkg/util/ranger"
//
// MaxMinEliminator tries to eliminate max/min aggregate function.
// For SQL like `select max(id) from t;`, we could optimize it to `select max(id) from (select id from t order by id desc limit 1 where id is not null) t;`.
// For SQL like `select min(id) from t;`, we could optimize it to `select min(id) from (select id from t order by id limit 1 where id is not null) t;`.
// For SQL like `select max(id), min(id) from t;`, we could optimize it to the cartesianJoin result of the two queries above if `id` has an index.
// MaxMinEliminator 对应 Go 的同名 struct；字段、方法与声明顺序按来源保留。
// pub struct MaxMinEliminator {
// }
// Optimize implements base.LogicalOptRule.<0th> interface.
// Optimize 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn (a *MaxMinEliminator) Optimize(_ context.Context, p base.LogicalPlan) (base.LogicalPlan, bool, error) {
//     planChanged := false
//     return a.eliminateMaxMin(p), planChanged, nil
// }
//
// composeAggsByInnerJoin composes the scalar aggregations by cartesianJoin.
// composeAggsByInnerJoin 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn (*MaxMinEliminator) composeAggsByInnerJoin(aggs []*logicalop.LogicalAggregation) (plan base.LogicalPlan) {
//     plan = aggs[0]
//     sctx := plan.SCtx()
//     joins := make([]*logicalop.LogicalJoin, 0)
//     for i := 1; i < len(aggs); i++ {
//         join := logicalop.LogicalJoin{JoinType: base.InnerJoin}.Init(sctx, plan.QueryBlockOffset())
//         join.SetChildren(plan, aggs[i])
//         join.SetSchema(logicalop.BuildLogicalJoinSchema(base.InnerJoin, join))
//         plan = join
//         joins = append(joins, join)
//     }
//     return
// }
//
// checkColCanUseIndex checks whether there is an AccessPath satisfy the conditions:
// 1. all of the selection's condition can be pushed down as AccessConds of the path.
// 2. the path can keep order for `col` after pushing down the conditions.
// checkColCanUseIndex 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn (a *MaxMinEliminator) checkColCanUseIndex(plan base.LogicalPlan, col *expression.Column, conditions []expression.Expression) bool {
//     switch p := plan.(type) {
//     case *logicalop.LogicalSelection:
//         conditions = append(conditions, p.Conditions...)
//         return a.checkColCanUseIndex(p.Children()[0], col, conditions)
//     case *logicalop.DataSource:
// Check whether there is an AccessPath can use index for col.
//         for _, path := range p.AllPossibleAccessPaths {
//             if path.IsIntHandlePath {
// Since table path can contain accessConds of at most one column,
// we only need to check if all of the conditions can be pushed down as accessConds
// and `col` is the handle column.
//                 if p.HandleCols != nil && col.EqualColumn(p.HandleCols.GetCol(0)) {
//                     if _, filterConds := ranger.DetachCondsForColumn(p.SCtx().GetRangerCtx(), conditions, col); len(filterConds) != 0 {
//                         return false
//                     }
//                     return true
//                 }
//             } else {
//                 indexCols, indexColLen := path.FullIdxCols, path.FullIdxColLens
//                 if path.IsCommonHandlePath {
//                     indexCols, indexColLen = p.CommonHandleCols, p.CommonHandleLens
//                 }
// 1. whether all of the conditions can be pushed down as accessConds.
// 2. whether the AccessPath can satisfy the order property of `col` with these accessConds.
//                 result, err := ranger.DetachCondAndBuildRangeForIndex(p.SCtx().GetRangerCtx(), conditions, indexCols, indexColLen, p.SCtx().GetSessionVars().RangeMaxSize)
//                 if err != nil || len(result.RemainedConds) != 0 {
//                     continue
//                 }
//                 for i := 0; i <= result.EqCondCount; i++ {
//                     if i < len(indexCols) && col.EqualColumn(indexCols[i]) {
//                         return true
//                     }
//                 }
//             }
//         }
//         return false
//     default:
//         return false
//     }
// }
//
// cloneSubPlans shallow clones the subPlan. We only consider `Selection` and `DataSource` here,
// because we have restricted the subPlan in `checkColCanUseIndex`.
// cloneSubPlans 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn (a *MaxMinEliminator) cloneSubPlans(plan base.LogicalPlan) base.LogicalPlan {
//     switch p := plan.(type) {
//     case *logicalop.LogicalSelection:
//         newConditions := make([]expression.Expression, len(p.Conditions))
//         copy(newConditions, p.Conditions)
//         sel := logicalop.LogicalSelection{Conditions: newConditions}.Init(p.SCtx(), p.QueryBlockOffset())
//         sel.SetChildren(a.cloneSubPlans(p.Children()[0]))
//         return sel
//     case *logicalop.DataSource:
// Quick clone a DataSource.
// ReadOnly fields uses a shallow copy, while the fields which will be overwritten must use a deep copy.
//         newDs := *p
//         newDs.BaseLogicalPlan = logicalop.NewBaseLogicalPlan(p.SCtx(), p.TP(), &newDs, p.QueryBlockOffset())
//         newDs.SetSchema(p.Schema().Clone())
//         newDs.Columns = make([]*model.ColumnInfo, len(p.Columns))
//         copy(newDs.Columns, p.Columns)
//         allAccessPaths := make([]*util.AccessPath, 0, len(p.AllPossibleAccessPaths))
// alloc len for copy func.
//         newDs.PossibleAccessPaths = make([]*util.AccessPath, len(p.AllPossibleAccessPaths))
//         for _, path := range p.AllPossibleAccessPaths {
//             newPath := *path
//             allAccessPaths = append(allAccessPaths, &newPath)
//         }
//         newDs.AllPossibleAccessPaths = allAccessPaths
//         copy(newDs.PossibleAccessPaths, allAccessPaths)
//         return &newDs
//     }
// This won't happen, because we have checked the subtree.
//     return nil
// }
//
// splitAggFuncAndCheckIndices splits the agg to multiple aggs and check whether each agg needs a sort
// after the transformation. For example, we firstly split the sql: `select max(a), min(a), max(b) from t` ->
// `select max(a) from t` + `select min(a) from t` + `select max(b) from t`.
// Then we check whether `a` and `b` have indices. If any of the used column has no index, we cannot eliminate
// this aggregation.
// splitAggFuncAndCheckIndices 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn (a *MaxMinEliminator) splitAggFuncAndCheckIndices(agg *logicalop.LogicalAggregation) (aggs []*logicalop.LogicalAggregation, canEliminate bool) {
//     for _, f := range agg.AggFuncs {
// We must make sure the args of max/min is a simple single column.
//         col, ok := f.Args[0].(*expression.Column)
//         if !ok {
//             return nil, false
//         }
//         if !a.checkColCanUseIndex(agg.Children()[0], col, make([]expression.Expression, 0)) {
//             return nil, false
//         }
//     }
//     aggs = make([]*logicalop.LogicalAggregation, 0, len(agg.AggFuncs))
// we can split the aggregation only if all of the aggFuncs pass the check.
//     for i, f := range agg.AggFuncs {
//         newAgg := logicalop.LogicalAggregation{AggFuncs: []*aggregation.AggFuncDesc{f}}.Init(agg.SCtx(), agg.QueryBlockOffset())
//         newAgg.SetChildren(a.cloneSubPlans(agg.Children()[0]))
//         newAgg.SetSchema(expression.NewSchema(agg.Schema().Columns[i]))
// Since LogicalAggregation doesn't use the parent base.LogicalPlan, passing an incorrect parameter here won't affect subsequent optimizations.
//         var (
//             p   base.LogicalPlan
//             err error
//         )
//         if p, err = newAgg.PruneColumns([]*expression.Column{newAgg.Schema().Columns[0]}); err != nil {
//             return nil, false
//         }
//         newAgg = p.(*logicalop.LogicalAggregation)
//         aggs = append(aggs, newAgg)
//     }
//     return aggs, true
// }
//
// eliminateSingleMaxMin tries to convert a single max/min to Limit+Sort operators.
// eliminateSingleMaxMin 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn (*MaxMinEliminator) eliminateSingleMaxMin(agg *logicalop.LogicalAggregation) *logicalop.LogicalAggregation {
//     f := agg.AggFuncs[0]
//     child := agg.Children()[0]
//     ctx := agg.SCtx()
//
//     var sel *logicalop.LogicalSelection
//     var sort *logicalop.LogicalSort
// If there's no column in f.GetArgs()[0], we still need limit and read data from real table because the result should be NULL if the input is empty.
//     if len(expression.ExtractColumns(f.Args[0])) > 0 {
// If it can be NULL, we need to filter NULL out first.
//         if !mysql.HasNotNullFlag(f.Args[0].GetType(ctx.GetExprCtx().GetEvalCtx()).GetFlag()) {
//             sel = logicalop.LogicalSelection{}.Init(ctx, agg.QueryBlockOffset())
//             isNullFunc := expression.NewFunctionInternal(ctx.GetExprCtx(), ast.IsNull, types.NewFieldType(mysql.TypeTiny), f.Args[0])
//             notNullFunc := expression.NewFunctionInternal(ctx.GetExprCtx(), ast.UnaryNot, types.NewFieldType(mysql.TypeTiny), isNullFunc)
//             sel.Conditions = []expression.Expression{notNullFunc}
//             sel.SetChildren(agg.Children()[0])
//             child = sel
//         }
//
// Add Sort and Limit operators.
// For max function, the sort order should be desc.
//         desc := f.Name == ast.AggFuncMax
// Compose Sort operator.
//         sort = logicalop.LogicalSort{}.Init(ctx, agg.QueryBlockOffset())
//         sort.ByItems = append(sort.ByItems, &util.ByItems{Expr: f.Args[0], Desc: desc})
//         sort.SetChildren(child)
//         child = sort
//     }
//
// Compose Limit operator.
//     li := logicalop.LogicalLimit{Count: 1}.Init(ctx, agg.QueryBlockOffset())
//     li.SetChildren(child)
//
// If no data in the child, we need to return NULL instead of empty. This cannot be done by sort and limit themselves.
// Since now there would be at most one row returned, the remained agg operator is not expensive anymore.
//     agg.SetChildren(li)
//     return agg
// }
//
// eliminateMaxMin tries to convert max/min to Limit+Sort operators.
// eliminateMaxMin 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn (a *MaxMinEliminator) eliminateMaxMin(p base.LogicalPlan) base.LogicalPlan {
// CTE's logical optimization is indenpent.
//     if _, ok := p.(*logicalop.LogicalCTE); ok {
//         return p
//     }
//     newChildren := make([]base.LogicalPlan, 0, len(p.Children()))
//     for _, child := range p.Children() {
//         newChildren = append(newChildren, a.eliminateMaxMin(child))
//     }
//     p.SetChildren(newChildren...)
//     if agg, ok := p.(*logicalop.LogicalAggregation); ok {
//         if len(agg.GroupByItems) != 0 {
//             return agg
//         }
//         if len(agg.AggFuncs) == 0 {
//             return agg
//         }
// Make sure that all of the aggFuncs are Max or Min.
//         for _, aggFunc := range agg.AggFuncs {
//             if aggFunc.Name != ast.AggFuncMax && aggFunc.Name != ast.AggFuncMin {
//                 return agg
//             }
//         }
// Limit+Sort operators are sorted by value, but ENUM/SET field types are sorted by name.
//         cols := agg.GetUsedCols()
//         for _, col := range cols {
//             if col.RetType.GetType() == mysql.TypeEnum || col.RetType.GetType() == mysql.TypeSet {
//                 return agg
//             }
//         }
//         if len(agg.AggFuncs) == 1 {
// If there is only one aggFunc, we don't need to guarantee that the child of it is a data
// source, or whether the sort can be eliminated. This transformation won't be worse than previous.
//             return a.eliminateSingleMaxMin(agg)
//         }
// If we have more than one aggFunc, we can eliminate this agg only if all of the aggFuncs can benefit from
// their column's index.
//         aggs, canEliminate := a.splitAggFuncAndCheckIndices(agg)
//         if !canEliminate {
//             return agg
//         }
//         for i := range aggs {
//             aggs[i] = a.eliminateSingleMaxMin(aggs[i])
//         }
//         return a.composeAggsByInnerJoin(aggs)
//     }
//     return p
// }
//
// Name implements base.LogicalOptRule.<1st> interface.
// Name 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn (*MaxMinEliminator) Name() string {
//     return "max_min_eliminate"
// }
// */
use crate::rule_init::{AggKind, Expr, FieldType, LogicalRule, Plan, PlanKind};

/// MAX/MIN 消除规则：用 Sort+Limit 替代可走索引顺序的标量聚合。
pub struct MaxMinEliminator;

impl LogicalRule for MaxMinEliminator {
    fn name(&self) -> &'static str {
        "max_min_eliminate"
    }

    fn optimize(&self, mut plan: Plan) -> Result<(Plan, bool), String> {
        eliminate(&mut plan);
        // Match Go's LogicalOptRule contract: this rule rewrites the plan but
        // intentionally never requests another optimizer iteration.
        Ok((plan, false))
    }
}

/// 后序改写：仅处理无分组、且全部为单列 MAX/MIN 的聚合节点。
fn eliminate(plan: &mut Plan) -> bool {
    let mut changed = plan
        .children
        .iter_mut()
        .fold(false, |seen, child| eliminate(child) || seen);
    let PlanKind::Aggregation {
        aggregates,
        group_by,
    } = &plan.kind
    else {
        return changed;
    };
    if !group_by.is_empty() || aggregates.is_empty() || plan.children.len() != 1 {
        return changed;
    }
    // 非 MAX/MIN、多参数聚合不参与本规则。MAX/MIN DISTINCT 与普通
    // MAX/MIN 等价，Go 实现也允许消除。
    if aggregates.iter().any(|aggregate| {
        aggregate.args.len() != 1 || !matches!(aggregate.kind, AggKind::Max | AggKind::Min)
    }) {
        return changed;
    }

    let source = plan.children.remove(0);
    // A single MAX/MIN can always use the limit transformation.  The Go rule
    // keeps the aggregate wrapper so an empty input still produces one NULL;
    // only the multi-aggregate form requires every argument to have index
    // order support.
    if aggregates.len() == 1 {
        let aggregate = &aggregates[0];
        let argument = aggregate.args[0].clone();
        let mut child = source;
        // Go always adds Sort for a column expression in the single-aggregate
        // case; index availability only gates splitting multiple aggregates.
        if !argument.columns().is_empty() {
            let direction = match aggregate.kind {
                AggKind::Max => "desc",
                AggKind::Min => "asc",
                _ => unreachable!(),
            };
            child = Plan {
                kind: PlanKind::Sort {
                    by: vec![Expr::Scalar {
                        function: direction.into(),
                        args: vec![argument],
                        field_type: FieldType::Bool,
                    }],
                },
                schema: child.schema.clone(),
                children: vec![child.clone()],
                predicates: Vec::new(),
                keys: child.keys.clone(),
                estimated_rows: child.estimated_rows,
                used_stats: child.used_stats.clone(),
            };
        }
        plan.children = vec![Plan {
            kind: PlanKind::Limit { count: 1 },
            schema: child.schema.clone(),
            children: vec![child],
            predicates: Vec::new(),
            keys: Vec::new(),
            estimated_rows: 1.0,
            used_stats: Default::default(),
        }];
        return true;
    }
    let aggregates = aggregates.clone();
    let output_schema = plan.schema.clone();
    let mut branches = Vec::with_capacity(aggregates.len());
    for (index, aggregate) in aggregates.into_iter().enumerate() {
        let argument = aggregate.args[0].clone();
        if !matches!(argument, Expr::Column { .. }) {
            plan.children.push(source);
            return changed;
        }
        // 任一聚合列无法由索引提供有序扫描则整体放弃改写。
        if !index_can_produce_order(&source, &argument) {
            plan.children.push(source);
            return changed;
        }
        let direction = match aggregate.kind {
            AggKind::Max => "desc",
            AggKind::Min => "asc",
            _ => unreachable!(),
        };
        let order = Expr::Scalar {
            function: direction.into(),
            args: vec![argument],
            field_type: FieldType::Bool,
        };
        let sorted = Plan {
            kind: PlanKind::Sort { by: vec![order] },
            schema: source.schema.clone(),
            children: vec![source.clone()],
            predicates: Vec::new(),
            keys: source.keys.clone(),
            estimated_rows: source.estimated_rows,
            used_stats: source.used_stats.clone(),
        };
        let limit = Plan {
            kind: PlanKind::Limit { count: 1 },
            schema: sorted.schema.clone(),
            children: vec![sorted],
            predicates: Vec::new(),
            keys: Vec::new(),
            estimated_rows: 1.0,
            used_stats: Default::default(),
        };
        branches.push(Plan {
            kind: PlanKind::Aggregation {
                aggregates: vec![aggregate],
                group_by: Vec::new(),
            },
            schema: output_schema.get(index).copied().into_iter().collect(),
            children: vec![limit],
            predicates: Vec::new(),
            keys: Vec::new(),
            estimated_rows: 1.0,
            used_stats: Default::default(),
        });
    }
    // 多个标量聚合用无等值条件的 Inner Join 做笛卡尔积。
    *plan = Plan {
        kind: PlanKind::Join {
            join_type: crate::rule_init::JoinType::Inner,
            equal_conditions: Vec::new(),
            other_conditions: Vec::new(),
        },
        schema: branches
            .iter()
            .flat_map(|branch| branch.schema.iter().copied())
            .collect(),
        children: branches,
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 1.0,
        used_stats: Default::default(),
    };
    changed = true;
    changed
}

/// 检查表达式是否为单列，且 DataSource 上存在以该列为前缀的索引。
fn index_can_produce_order(plan: &Plan, expression: &Expr) -> bool {
    let columns = expression.columns();
    if columns.len() != 1 {
        return false;
    }
    let column = *columns.iter().next().expect("checked one column");
    match &plan.kind {
        PlanKind::DataSource { indexes, .. } => {
            indexes.values().any(|index| index.first() == Some(&column))
        }
        PlanKind::Selection => plan
            .children
            .first()
            .is_some_and(|child| index_can_produce_order(child, expression)),
        _ => false,
    }
}
