// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");

// 相关子查询（Correlate）求解规则。
//
// 在 Join 计划树中递归处理标记为 PreferCorrelate 的 Semi Join，将其恢复为
// Apply（相关应用）并把等值条件下推到内层 DataSource。

use crate::rule_join_reorder::{JoinEdge, JoinNode, JoinPlan, Result};
use crate::task::Expression;

/// 相关子查询求解器：遍历计划树并改写 Apply 内层条件。
#[derive(Default)]
pub struct CorrelateSolver;
impl CorrelateSolver {
    /// 入口：对整棵 Join 计划执行相关化处理，返回是否发生变化。
    pub fn Optimize(&self, plan: JoinPlan) -> Result<(JoinPlan, bool)> {
        self.correlate(plan)
    }
    /// 递归相关化：先处理子树，再把安全的首选 Semi Join 恢复为 Apply。
    pub fn correlate(&self, mut plan: JoinPlan) -> Result<(JoinPlan, bool)> {
        let mut changed = false;
        plan.node = match plan.node {
            JoinNode::Apply {
                join_type,
                left,
                right,
                correlated_columns,
                no_decorrelate,
            } => {
                let (left, l) = self.correlate(*left)?;
                let (right, r) = self.correlate(*right)?;
                changed |= l || r;
                JoinNode::Apply {
                    join_type,
                    left: Box::new(left),
                    right: Box::new(right),
                    correlated_columns,
                    no_decorrelate,
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
                let (left, l) = self.correlate(*left)?;
                let (right, r) = self.correlate(*right)?;
                changed |= l || r;
                let can_correlate = matches!(
                    join_type,
                    crate::task::JoinType::Semi | crate::task::JoinType::AntiSemi
                ) && preferred_method.as_deref() == Some("correlate")
                    && !equal_conditions.is_empty()
                    && other_conditions.is_empty()
                    && equal_conditions.iter().all(|edge| !edge.null_equal);
                let oriented = can_correlate
                    .then(|| {
                        equal_conditions
                            .iter()
                            .map(|edge| {
                                if left.schema.contains(&edge.left_column)
                                    && right.schema.contains(&edge.right_column)
                                {
                                    Some((edge.left_column, edge.right_column))
                                } else if left.schema.contains(&edge.right_column)
                                    && right.schema.contains(&edge.left_column)
                                {
                                    Some((edge.right_column, edge.left_column))
                                } else {
                                    None
                                }
                            })
                            .collect::<Option<Vec<_>>>()
                    })
                    .flatten();
                if let Some(columns) = oriented {
                    let mut candidate = liftDataSourceConds(right.clone());
                    if columns
                        .iter()
                        .all(|(_, inner)| push_correlated_predicate(&mut candidate, *inner))
                    {
                        resetStatsForCorrelatedDS(&mut candidate);
                        changed = true;
                        JoinNode::Apply {
                            join_type,
                            left: Box::new(left),
                            right: Box::new(candidate),
                            correlated_columns: columns
                                .into_iter()
                                .map(|(outer, _)| outer)
                                .collect(),
                            no_decorrelate: false,
                        }
                    } else {
                        JoinNode::Join {
                            join_type,
                            left: Box::new(left),
                            right: Box::new(right),
                            equal_conditions,
                            other_conditions,
                            preferred_method,
                        }
                    }
                } else {
                    JoinNode::Join {
                        join_type,
                        left: Box::new(left),
                        right: Box::new(right),
                        equal_conditions,
                        other_conditions,
                        preferred_method,
                    }
                }
            }
            JoinNode::Projection { expressions, child } => {
                let (child, c) = self.correlate(*child)?;
                changed |= c;
                JoinNode::Projection {
                    expressions,
                    child: Box::new(child),
                }
            }
            JoinNode::Selection { conditions, child } => {
                let (child, c) = self.correlate(*child)?;
                changed |= c;
                JoinNode::Selection {
                    conditions,
                    child: Box::new(child),
                }
            }
            JoinNode::Aggregation {
                group_by,
                child,
                default_values,
            } => {
                let (child, c) = self.correlate(*child)?;
                changed |= c;
                JoinNode::Aggregation {
                    group_by,
                    child: Box::new(child),
                    default_values,
                }
            }
            JoinNode::Window {
                partition_by,
                row_number_column,
                upper_bound,
                child,
            } => {
                let (child, c) = self.correlate(*child)?;
                changed |= c;
                JoinNode::Window {
                    partition_by,
                    row_number_column,
                    upper_bound,
                    child: Box::new(child),
                }
            }
            JoinNode::UnionAll(children) => {
                let mut rewritten = Vec::with_capacity(children.len());
                for child in children {
                    let (child, c) = self.correlate(child)?;
                    changed |= c;
                    rewritten.push(child);
                }
                JoinNode::UnionAll(rewritten)
            }
            node => node,
        };
        Ok((plan, changed))
    }
    /// 构造外层列与内层列之间的相关等值边（JoinEdge）。
    pub fn buildCorrelatedCond(&self, outerColumn: usize, innerColumn: usize) -> JoinEdge {
        JoinEdge {
            left_column: outerColumn,
            right_column: innerColumn,
            null_equal: false,
        }
    }
    /// 规则注册名。
    pub fn Name(&self) -> &'static str {
        "correlate"
    }
}

/// 若 Selection 直接盖在 Leaf（DataSource）上，把过滤条件并入 Leaf 谓词并去掉 Selection。
pub fn liftDataSourceConds(mut plan: JoinPlan) -> JoinPlan {
    if let JoinNode::Selection { conditions, child } = plan.node {
        if matches!(child.node, JoinNode::Leaf { .. }) {
            let mut child = *child;
            if let JoinNode::Leaf { predicates, .. } = &mut child.node {
                predicates.extend(conditions);
            }
            return child;
        }
        plan.node = JoinNode::Selection { conditions, child };
    }
    plan
}

fn push_correlated_predicate(plan: &mut JoinPlan, inner_column: usize) -> bool {
    match &mut plan.node {
        JoinNode::Leaf { predicates, .. } if plan.schema.contains(&inner_column) => {
            predicates.push(Expression {
                name: "correlated_eq".into(),
                column: Some(inner_column),
                ..Expression::default()
            });
            true
        }
        JoinNode::Join { left, right, .. } | JoinNode::Apply { left, right, .. } => {
            push_correlated_predicate(left, inner_column)
                || push_correlated_predicate(right, inner_column)
        }
        JoinNode::Projection { child, .. }
        | JoinNode::Selection { child, .. }
        | JoinNode::Aggregation { child, .. }
        | JoinNode::Window { child, .. } => push_correlated_predicate(child, inner_column),
        JoinNode::UnionAll(children) => children
            .iter_mut()
            .any(|child| push_correlated_predicate(child, inner_column)),
        _ => false,
    }
}

/// 重置含相关列的 DataSource 统计：行数至少为 1，避免相关扫描被估成空。
pub fn resetStatsForCorrelatedDS(plan: &mut JoinPlan) -> bool {
    let mut changed = false;
    match &mut plan.node {
        JoinNode::Leaf {
            correlated_columns, ..
        } if !correlated_columns.is_empty() => {
            plan.row_count = plan.row_count.max(1.0);
            changed = true;
        }
        JoinNode::Join { left, right, .. } | JoinNode::Apply { left, right, .. } => {
            changed |= resetStatsForCorrelatedDS(left);
            changed |= resetStatsForCorrelatedDS(right);
        }
        JoinNode::Projection { child, .. }
        | JoinNode::Selection { child, .. }
        | JoinNode::Aggregation { child, .. }
        | JoinNode::Window { child, .. } => changed |= resetStatsForCorrelatedDS(child),
        JoinNode::UnionAll(children) => {
            for child in children {
                changed |= resetStatsForCorrelatedDS(child);
            }
        }
        _ => {}
    }
    changed
}
