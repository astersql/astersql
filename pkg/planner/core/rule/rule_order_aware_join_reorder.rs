// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 顺序感知的 Join 重排序（Order-Aware Join Reorder）逻辑优化规则。
//
// 当上层存在排序/Limit 需求时，尽量把能提供所需顺序的子关系放在 Inner Join
// 的最左侧，其余子节点按估计行数升序排列，以保留索引有序性收益。

// 优化规则的数据结构、递归控制流和错误传播形状。
// 外部 TiDB 类型与函数调用保留为后续模块接线点；本任务不创建跨文件模块连线。
//
// Go imports（仅保留来源依赖，暂不接入 Rust crate）：
// 	"context"
// 	"slices"
// 	"github.com/pingcap/tidb/pkg/expression"
// 	"github.com/pingcap/tidb/pkg/planner/core/base"
// 	"github.com/pingcap/tidb/pkg/planner/core/joinorder"
// 	"github.com/pingcap/tidb/pkg/planner/core/operator/logicalop"
// 	plannerutil "github.com/pingcap/tidb/pkg/planner/util"
//
// OrderAwareJoinReorder annotates a join group with an internal leading
// preference when a TopN above the group can benefit from keeping one leaf's
// index order alive.
// type OrderAwareJoinReorder struct{}
//
// Optimize implements the base.LogicalOptRule.<0th> interface.
// Optimize 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn (r *OrderAwareJoinReorder) Optimize(_ context.Context, p base.LogicalPlan) (base.LogicalPlan, bool, error) {
//     changed, _, err := r.optimizeRecursive(p, nil, nil)
//     return p, changed, err
// }
// optimizeRecursive 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn (r *OrderAwareJoinReorder) optimizeRecursive(
//     p base.LogicalPlan,
//     orderCols []*expression.Column,
//     midFilters []expression.Expression,
// ) (changed bool, ordered bool, err error) {
//     if p == nil {
//         return false, false, nil
//     }
//     if _, ok := p.(*logicalop.LogicalCTE); ok {
//         return false, false, nil
//     }
//     if len(orderCols) == 0 {
// only selection filter under order requirement can be used.
//         midFilters = nil
//     }
//
//     switch node := p.(type) {
// for TopN and Sort, we extract the ordering columns and pass down to children so they can annotate join groups with leading preferences!
//     case *logicalop.LogicalTopN:
//         extractedOrder := extractOrderingColumns(node.ByItems)
//         childChanged, childOrdered, err := r.optimizeChildren(node.Children(), extractedOrder, nil, 0)
//         if err != nil {
//             return false, false, err
//         }
//         if len(orderCols) > 0 {
//             return changed || childChanged, sameOrderingColumns(orderCols, extractedOrder), nil
//         }
//         return changed || childChanged, childOrdered, nil
//     case *logicalop.LogicalSort:
//         extractedOrder := extractOrderingColumns(node.ByItems)
//         childChanged, childOrdered, err := r.optimizeChildren(node.Children(), extractedOrder, nil, 0)
//         if err != nil {
//             return false, false, err
//         }
//         if len(orderCols) > 0 {
//             return changed || childChanged, sameOrderingColumns(orderCols, extractedOrder), nil
//         }
//         return changed || childChanged, childOrdered, nil
//     case *logicalop.LogicalProjection:
//         rewrittenOrder := rewriteOrderingForProjection(node, orderCols)
//         childChanged, childOrdered, err := r.optimizeChildren(node.Children(), rewrittenOrder, midFilters, 0)
//         if err != nil {
//             return false, false, err
//         }
//         if len(orderCols) > 0 {
//             return changed || childChanged, len(rewrittenOrder) > 0 && childOrdered, nil
//         }
//         return changed || childChanged, childOrdered, nil
//     case *logicalop.LogicalLimit:
//         childChanged, childOrdered, err := r.optimizeChildren(node.Children(), orderCols, midFilters, 0)
//         return changed || childChanged, childOrdered, err
//     case *logicalop.LogicalSelection:
//         canPushThroughSelection := node.SCtx().GetSessionVars().TiDBOptJoinReorderThroughSel &&
//             !slices.ContainsFunc(node.Conditions, expression.IsMutableEffectsExpr)
//         if canPushThroughSelection {
//             var accumulatedFilters []expression.Expression
//             if len(orderCols) > 0 {
// These filters may help child groups preserve the same ordering
// by fixing leading index columns above the current anchor join.
//                 accumulatedFilters = append(slices.Clone(midFilters), node.Conditions...)
//             }
//             var localChoice *joinorder.OrderedLeadingChoice
//             if len(orderCols) > 0 && shouldUseCDCBasedJoinReorder(node) {
//                 localChoice = joinorder.FindOrderedLeadingChoice(node, orderCols)
//             }
//             if localChoice != nil && len(localChoice.Vertices) > 0 {
// This subtree owns the required ordering columns, so recurse into it
// first and only annotate the current anchor after the chosen child
// proves it can still satisfy the order requirement.
//                 childChanged, childOrdered, err := r.optimizeChildren(localChoice.Vertices, orderCols, accumulatedFilters, localChoice.CarrierVertex.ID())
//                 if err != nil {
//                     return false, false, err
//                 }
//                 if childOrdered && joinorder.TryAnnotateOrderedLeading(node, localChoice) {
//                     changed = true
//                     return changed || childChanged, true, nil
//                 }
//                 return changed || childChanged, false, nil
//             }
//             childChanged, childOrdered, err := r.optimizeChildren(node.Children(), orderCols, accumulatedFilters, 0)
//             return changed || childChanged, childOrdered, err
//         }
//         childChanged, _, err := r.optimizeChildren(node.Children(), nil, nil, 0)
//         return changed || childChanged, false, err
//     case *logicalop.LogicalJoin:
//         var localChoice *joinorder.OrderedLeadingChoice
//         if len(orderCols) > 0 && shouldUseCDCBasedJoinReorder(node) {
//             localChoice = joinorder.FindOrderedLeadingChoice(node, orderCols)
//         }
//         if localChoice != nil && len(localChoice.Vertices) > 0 {
//             childChanged, childOrdered, err := r.optimizeChildren(localChoice.Vertices, orderCols, midFilters, localChoice.CarrierVertex.ID())
//             if err != nil {
//                 return false, false, err
//             }
//             if childOrdered && joinorder.TryAnnotateOrderedLeading(node, localChoice) {
//                 changed = true
//                 return changed || childChanged, true, nil
//             }
//             return changed || childChanged, false, nil
//         }
// The order requirement does not belong to any vertex of this join group,
// so there is no subtree worth propagating into from this point.
//         childChanged, _, err := r.optimizeChildren(node.Children(), nil, nil, 0)
//         return changed || childChanged, false, err
//     case *logicalop.DataSource:
//         if len(orderCols) == 0 {
//             return false, false, nil
//         }
//         return false, joinorder.DsSatisfiesOrdering(node, orderCols, midFilters), nil
//     default:
//         childChanged, _, err := r.optimizeChildren(p.Children(), nil, nil, 0)
//         return changed || childChanged, false, err
//     }
// }
//
// optimizeChildren 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn (r *OrderAwareJoinReorder) optimizeChildren(
//     children []base.LogicalPlan,
//     orderCols []*expression.Column,
//     parentFilters []expression.Expression,
//     vertexIDShouldFollowOrder int,
// ) (changed bool, ordered bool, err error) {
//     for _, child := range children {
//         nextOrderCols := orderCols
//         nextParentFilters := parentFilters
//         if vertexIDShouldFollowOrder == 0 || vertexIDShouldFollowOrder == child.ID() {
// A zero vertex ID means we are still walking the tree to find the
// first anchor join, so every child keeps the current requirement.
// Once a join group chooses one carrier vertex, only that vertex
// continues to inherit the ordering and accumulated filters.
//             childChanged, childOrdered, err := r.optimizeRecursive(child, nextOrderCols, nextParentFilters)
//             if err != nil {
//                 return false, false, err
//             }
//             changed = changed || childChanged
//             ordered = ordered || childOrdered
//             continue
//         }
//         childChanged, _, err := r.optimizeRecursive(child, nil, nil)
//         if err != nil {
//             return false, false, err
//         }
//         changed = changed || childChanged
//     }
//     return changed, ordered, nil
// }
//
// shouldUseCDCBasedJoinReorder 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn shouldUseCDCBasedJoinReorder(p base.LogicalPlan) bool {
//     vars := p.SCtx().GetSessionVars()
//     return vars.TiDBOptEnableAdvancedJoinReorder && vars.TiDBOptJoinReorderThreshold <= 0
// }
//
// extractOrderingColumns 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn extractOrderingColumns(items []*plannerutil.ByItems) []*expression.Column {
//     if len(items) == 0 {
//         return nil
//     }
//     cols := make([]*expression.Column, 0, len(items))
//     for _, item := range items {
// The current matcher only reasons about forward index order, so bail out
// once ORDER BY contains a descending item instead of silently treating it
// as ascending.
//         if item.Desc {
//             return nil
//         }
//         col, ok := item.Expr.(*expression.Column)
//         if !ok {
//             return nil
//         }
//         cols = append(cols, col)
//     }
//     return cols
// }
//
// sameOrderingColumns 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn sameOrderingColumns(left, right []*expression.Column) bool {
//     if len(left) == 0 || len(right) == 0 {
//         return false
//     }
//     return slices.EqualFunc(left, right, func(leftCol, rightCol *expression.Column) bool {
//         return leftCol != nil && rightCol != nil && leftCol.UniqueID == rightCol.UniqueID
//     })
// }
//
// rewriteOrderingForProjection 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn rewriteOrderingForProjection(
//     proj *logicalop.LogicalProjection,
//     orderCols []*expression.Column,
// ) []*expression.Column {
//     if proj == nil || len(orderCols) == 0 {
//         return nil
//     }
//     rewritten := make([]*expression.Column, 0, len(orderCols))
//     for _, col := range orderCols {
//         offset := proj.Schema().ColumnIndex(col)
//         if offset < 0 {
//             return nil
//         }
//         mappedCol, ok := proj.Exprs[offset].(*expression.Column)
//         if !ok {
//             return nil
//         }
//         rewritten = append(rewritten, mappedCol)
//     }
//     return rewritten
// }
//
// Name implements the base.LogicalOptRule.<1st> interface.
// Name 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn (*OrderAwareJoinReorder) Name() string {
//     return "order_aware_join_reorder"
// }
// */
use crate::rule_init::{Expr, JoinType, LogicalRule, Plan, PlanKind};
/// 顺序感知 Join 重排规则：优先保留能满足上层排序需求的叶子。
pub struct OrderAwareJoinReorder;

impl LogicalRule for OrderAwareJoinReorder {
    fn name(&self) -> &'static str {
        "order_aware_join_reorder"
    }

    fn optimize(&self, mut plan: Plan) -> Result<(Plan, bool), String> {
        let changed = reorder(&mut plan, &[]);
        Ok((plan, changed))
    }
}

/// 向下传递排序列集合；对多路 Inner Join 按「有序优先、行数次之」重排子节点。
fn reorder(plan: &mut Plan, inherited_order: &[i64]) -> bool {
    // Sort 节点重置本地排序需求；Projection 按输出 schema 位置改写到输入表达式。
    let local_order: Vec<i64> = match &plan.kind {
        PlanKind::Sort { by } => extract_ordering_columns(by),
        PlanKind::Projection { expressions } => {
            rewrite_ordering_for_projection(&plan.schema, expressions, inherited_order)
        }
        PlanKind::Selection if plan.predicates.iter().any(|expr| !expr.deterministic()) => {
            Vec::new()
        }
        PlanKind::Limit { .. } => inherited_order.to_vec(),
        _ => inherited_order.to_vec(),
    };
    let mut changed = plan
        .children
        .iter_mut()
        .fold(false, |seen, child| reorder(child, &local_order) || seen);
    let PlanKind::Join { join_type, .. } = &plan.kind else {
        return changed;
    };
    if *join_type != JoinType::Inner || plan.children.len() < 2 {
        return changed;
    }
    // This rule is order-aware; without a required ordering the normal join
    // order must remain stable for plan reproducibility and hint semantics.
    if local_order.is_empty() {
        return changed;
    }

    // Preserve the ordered relation as the left-most input, then use the Go
    // greedy fallback (smaller estimated cardinality first) for the rest.
    // 含排序列的子关系优先放左；其余按估计基数（cardinality）升序。
    let previous = plan.children.clone();
    plan.children.sort_by(|left, right| {
        let left_ordered = satisfies_ordering(left, &local_order);
        let right_ordered = satisfies_ordering(right, &local_order);
        right_ordered.cmp(&left_ordered).then_with(|| {
            left.estimated_rows
                .partial_cmp(&right.estimated_rows)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    changed |= plan.children != previous;
    changed
}

fn extract_ordering_columns(items: &[Expr]) -> Vec<i64> {
    items
        .iter()
        .map_while(|item| match item {
            Expr::Column { id, .. } => Some(*id),
            _ => None,
        })
        .collect()
}

fn rewrite_ordering_for_projection(
    schema: &[i64],
    expressions: &[Expr],
    order: &[i64],
) -> Vec<i64> {
    let mut rewritten = Vec::with_capacity(order.len());
    for column_id in order {
        let Some(offset) = schema.iter().position(|id| id == column_id) else {
            return Vec::new();
        };
        let Some(Expr::Column { id, .. }) = expressions.get(offset) else {
            return Vec::new();
        };
        rewritten.push(*id);
    }
    rewritten
}

fn satisfies_ordering(plan: &Plan, order: &[i64]) -> bool {
    if order.is_empty() {
        return false;
    }
    match &plan.kind {
        PlanKind::DataSource { indexes, .. } => {
            indexes.values().any(|columns| columns.starts_with(order))
        }
        PlanKind::Projection { expressions } => {
            let rewritten = rewrite_ordering_for_projection(&plan.schema, expressions, order);
            !rewritten.is_empty()
                && plan
                    .children
                    .first()
                    .is_some_and(|child| satisfies_ordering(child, &rewritten))
        }
        PlanKind::Selection | PlanKind::Limit { .. } => {
            plan.predicates.iter().all(Expr::deterministic)
                && plan
                    .children
                    .first()
                    .is_some_and(|child| satisfies_ordering(child, order))
        }
        _ => false,
    }
}
