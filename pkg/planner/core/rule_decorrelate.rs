// Copyright 2017 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// Copyright 2026 AsterSQL.

// 去相关（Decorrelate）求解规则。
//
// 将满足条件的 Apply（相关应用）改写成普通 Join：收集内层相关等值边作为
// 等值连接条件，并剪掉冗余的 Left Outer Apply。相关子查询指内层引用外层列。

use crate::rule_join_reorder::{JoinEdge, JoinNode, JoinPlan, Result};
use crate::task::{Expression, JoinType};
use std::collections::{HashMap, HashSet};

/// 提取计划树中外层 Apply 的相关列列表（去重保序）。
pub fn ExtractOuterApplyCorrelatedCols(plan: &JoinPlan) -> Vec<usize> {
    extractOuterApplyCorrelatedColsHelper(plan).0
}

/// 遍历 Apply/Join 子树，收集相关列，并记录各 Apply 左子树的列集合快照。
pub fn extractOuterApplyCorrelatedColsHelper(plan: &JoinPlan) -> (Vec<usize>, Vec<HashSet<usize>>) {
    let mut columns = Vec::new();
    let mut schemas = Vec::new();
    fn walk(plan: &JoinPlan, columns: &mut Vec<usize>, schemas: &mut Vec<HashSet<usize>>) {
        match &plan.node {
            JoinNode::Apply {
                left,
                right,
                correlated_columns,
                ..
            } => {
                for column in correlated_columns {
                    if !columns.contains(column) {
                        columns.push(*column);
                    }
                }
                schemas.push(left.columns());
                walk(left, columns, schemas);
                walk(right, columns, schemas);
            }
            JoinNode::Join { left, right, .. } => {
                walk(left, columns, schemas);
                walk(right, columns, schemas);
            }
            JoinNode::Projection { child, .. }
            | JoinNode::Selection { child, .. }
            | JoinNode::Aggregation { child, .. }
            | JoinNode::Window { child, .. } => walk(child, columns, schemas),
            JoinNode::UnionAll(children) => {
                for child in children {
                    walk(child, columns, schemas);
                }
            }
            JoinNode::Leaf {
                correlated_columns, ..
            } => {
                for column in correlated_columns {
                    if !columns.contains(column) {
                        columns.push(*column);
                    }
                }
            }
        }
    }
    walk(plan, &mut columns, &mut schemas);
    // Go only returns correlations owned by an Apply outside this subtree. A
    // correlation resolved by any Apply's outer schema belongs to that inner
    // Apply and must not escape from this helper.
    columns.retain(|column| !schemas.iter().any(|schema| schema.contains(column)));
    (columns, schemas)
}

/// 去相关求解器：递归把可去相关的 Apply 变成 Join，并剪冗余 Apply。
#[derive(Default)]
pub struct DecorrelateSolver;
impl DecorrelateSolver {
    /// 读取聚合节点上的默认值映射（用于外连接空行填充）。
    pub fn aggDefaultValueMap(&self, plan: &JoinPlan) -> HashMap<usize, String> {
        if let JoinNode::Aggregation { default_values, .. } = &plan.node {
            default_values.clone()
        } else {
            HashMap::new()
        }
    }
    /// 优化入口：自顶向下去相关，初始分组列集合为空。
    pub fn Optimize(&self, plan: JoinPlan) -> Result<(JoinPlan, bool)> {
        self.optimize(plan, &HashSet::new())
    }
    /// 递归去相关；`groupByColumn` 向下传递，供剪枝冗余 Left Outer Apply。
    pub fn optimize(
        &self,
        mut plan: JoinPlan,
        groupByColumn: &HashSet<usize>,
    ) -> Result<(JoinPlan, bool)> {
        let mut changed = false;
        plan.node = match plan.node {
            JoinNode::Apply {
                join_type,
                left,
                right,
                correlated_columns,
                no_decorrelate,
            } => {
                let (left, l) = self.optimize(*left, groupByColumn)?;
                let (right, r) = self.optimize(*right, groupByColumn)?;
                changed |= l || r;
                // 相关列均可由左子树提供，且能收集到等值边（或本无无相关列）时改为 Join。
                if !no_decorrelate && correlated_columns.iter().all(|c| left.contains_column(*c)) {
                    let edges = collect_correlated_edges(&right, &correlated_columns);
                    if !edges.is_empty() || correlated_columns.is_empty() {
                        changed = true;
                        JoinNode::Join {
                            join_type,
                            left: Box::new(left),
                            right: Box::new(strip_correlated_conditions(right)),
                            equal_conditions: edges,
                            other_conditions: Vec::new(),
                            preferred_method: None,
                        }
                    } else {
                        JoinNode::Apply {
                            join_type,
                            left: Box::new(left),
                            right: Box::new(right),
                            correlated_columns,
                            no_decorrelate,
                        }
                    }
                } else {
                    JoinNode::Apply {
                        join_type,
                        left: Box::new(left),
                        right: Box::new(right),
                        correlated_columns,
                        no_decorrelate,
                    }
                }
            }
            JoinNode::Aggregation {
                group_by,
                child,
                default_values,
            } => {
                // 聚合的分组列并入向下传递的集合，供子树剪枝使用。
                let groups: HashSet<_> = group_by
                    .iter()
                    .copied()
                    .chain(groupByColumn.iter().copied())
                    .collect();
                let (child, c) = self.optimize(*child, &groups)?;
                changed |= c;
                JoinNode::Aggregation {
                    group_by,
                    child: Box::new(child),
                    default_values,
                }
            }
            JoinNode::Projection { expressions, child } => {
                let (child, c) = self.optimize(*child, groupByColumn)?;
                changed |= c;
                JoinNode::Projection {
                    expressions,
                    child: Box::new(child),
                }
            }
            JoinNode::Join {
                join_type,
                left,
                right,
                equal_conditions,
                other_conditions,
                preferred_method,
            } => {
                let (left, l) = self.optimize(*left, groupByColumn)?;
                let (right, r) = self.optimize(*right, groupByColumn)?;
                changed |= l || r;
                JoinNode::Join {
                    join_type,
                    left: Box::new(left),
                    right: Box::new(right),
                    equal_conditions,
                    other_conditions,
                    preferred_method,
                }
            }
            node => node,
        };
        let (plan, pruned) = pruneRedundantApply(plan, groupByColumn);
        Ok((plan, changed || pruned))
    }
    /// 规则注册名。
    pub fn Name(&self) -> &'static str {
        "decorrelate"
    }
}

/// 剪掉无相关列且右侧重估 ≤1 行的 Left Outer Apply，直接返回左子树。
pub fn pruneRedundantApply(plan: JoinPlan, groupByColumn: &HashSet<usize>) -> (JoinPlan, bool) {
    if let JoinNode::Apply {
        join_type: JoinType::LeftOuter,
        left,
        right,
        correlated_columns,
        no_decorrelate: false,
    } = &plan.node
    {
        if correlated_columns.is_empty()
            && right.row_count <= 1.0
            && groupByColumn.iter().all(|c| left.contains_column(*c))
        {
            return ((**left).clone(), true);
        }
    }
    (plan, false)
}

/// Left Outer Apply 上方投影含 null/ifnull 时，跳过某些去相关投影改写。
pub fn skipDecorrelateProjectionForLeftOuterApply(apply: &JoinPlan, projection: &JoinPlan) -> bool {
    let JoinNode::Apply {
        join_type: JoinType::LeftOuter,
        left,
        ..
    } = &apply.node
    else {
        return false;
    };
    let JoinNode::Projection { expressions, .. } = &projection.node else {
        return false;
    };

    let all_const = !expressions.is_empty() && expressions.iter().all(|expr| expr.column.is_none());
    all_const
        || expressions.iter().all(|expr| {
            expr.column
                .is_some_and(|column| left.contains_column(column))
        })
}

/// 从内层 Leaf/Selection 谓词中收集名为 `correlated_eq` 的等值边。
fn collect_correlated_edges(plan: &JoinPlan, outer: &[usize]) -> Vec<JoinEdge> {
    let mut edges = Vec::new();
    fn walk(plan: &JoinPlan, outer: &[usize], edges: &mut Vec<JoinEdge>) {
        match &plan.node {
            JoinNode::Leaf { predicates, .. } => {
                for predicate in predicates {
                    if predicate.name == "correlated_eq" {
                        if let (Some(inner), Some(outer)) = (predicate.column, outer.first()) {
                            edges.push(JoinEdge {
                                left_column: *outer,
                                right_column: inner,
                                null_equal: false,
                            });
                        }
                    }
                }
            }
            JoinNode::Selection { conditions, child } => {
                for predicate in conditions {
                    if predicate.name == "correlated_eq" {
                        if let (Some(inner), Some(outer)) = (predicate.column, outer.first()) {
                            edges.push(JoinEdge {
                                left_column: *outer,
                                right_column: inner,
                                null_equal: false,
                            });
                        }
                    }
                }
                walk(child, outer, edges);
            }
            _ => {}
        }
    }
    walk(plan, outer, &mut edges);
    edges
}

/// 去掉内层 Leaf/Selection 上的 `correlated_eq` 谓词（已提升为 Join 等值条件）。
fn strip_correlated_conditions(mut plan: JoinPlan) -> JoinPlan {
    match &mut plan.node {
        JoinNode::Leaf { predicates, .. } => predicates.retain(|p| p.name != "correlated_eq"),
        JoinNode::Selection { conditions, child } => {
            conditions.retain(|p| p.name != "correlated_eq");
            **child = strip_correlated_conditions((**child).clone());
        }
        _ => {}
    }
    plan
}

/// 保留 Expression 类型引用，避免未使用告警（迁移占位）。
fn _expression(_: Expression) {}
