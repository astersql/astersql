// Copyright 2020 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// Copyright 2026 AsterSQL.

// 外连接转内连接（Outer-to-Inner Join）逻辑优化规则。
//
// 当上层谓词（WHERE/Selection）对内表侧形成 null-reject（拒绝外连接未匹配时补上的 NULL）时，
// LEFT/RIGHT OUTER JOIN 可安全改写为 INNER JOIN，从而便于谓词下推与 Join 重排。

use crate::rule_join_reorder::{JoinNode, JoinPlan, Result};
use crate::task::{Expression, JoinType};

/// 将满足 null-reject 条件的外连接改写为内连接的规则。
#[derive(Default)]
pub struct ConvertOuterToInnerJoin;
impl ConvertOuterToInnerJoin {
    /// 入口：自顶向下改写计划树，返回新计划与是否发生改写。
    pub fn Optimize(&self, plan: JoinPlan) -> Result<(JoinPlan, bool)> {
        let (plan, _) = self.rewrite(plan, &[])?;
        // 与 Go 一致：该规则会原地改写计划，但优化器的 planChanged 契约固定为 false。
        Ok((plan, false))
    }
    /// 递归改写：Selection 累积谓词；Join 处根据 null-reject 尝试降级连接类型。
    fn rewrite(&self, mut plan: JoinPlan, predicates: &[Expression]) -> Result<(JoinPlan, bool)> {
        let mut changed = false;
        plan.node = match plan.node {
            JoinNode::Selection { conditions, child } => {
                // 把本层过滤条件并入向下传递的谓词集合。
                let mut all = predicates.to_vec();
                all.extend(conditions.clone());
                let (child, c) = self.rewrite(*child, &all)?;
                changed |= c;
                JoinNode::Selection {
                    conditions,
                    child: Box::new(child),
                }
            }
            JoinNode::Join {
                mut join_type,
                left,
                right,
                equal_conditions,
                other_conditions,
                preferred_method,
            } => {
                let left_cols = left.columns();
                let right_cols = right.columns();
                // 左外连接：谓词拒绝右表 NULL → 可转内连接；右外连接对称。
                if join_type == JoinType::LeftOuter
                    && predicates.iter().any(|p| null_rejected(p, &right_cols))
                    || join_type == JoinType::RightOuter
                        && predicates.iter().any(|p| null_rejected(p, &left_cols))
                {
                    join_type = JoinType::Inner;
                    changed = true;
                }

                // Go 的 mergeOnClausePredicates 会把当前 Join 的 ON 条件加入向下传递
                // 的谓词。简化模型中的等值边没有 Expression 载体，因此这里只合并
                // 完整保留的 other_conditions；等值边本身不会形成对子 Join 的 null-reject。
                let mut combined = predicates.to_vec();
                combined.extend(other_conditions.clone());
                let (left_predicates, right_predicates): (&[Expression], &[Expression]) =
                    match join_type {
                        JoinType::LeftOuter => (predicates, &combined),
                        JoinType::RightOuter => (&combined, predicates),
                        JoinType::Inner | JoinType::Semi => (&combined, &combined),
                        JoinType::AntiSemi => (predicates, &combined),
                    };
                let (left, l) = self.rewrite(*left, left_predicates)?;
                let (right, r) = self.rewrite(*right, right_predicates)?;
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
            JoinNode::Projection { expressions, child } => {
                // Go projection 仅下推可映射到子 schema 的谓词。
                let child_columns = child.columns();
                let pushable: Vec<_> = predicates
                    .iter()
                    .filter(|predicate| {
                        predicate
                            .column
                            .is_some_and(|column| child_columns.contains(&column))
                    })
                    .cloned()
                    .collect();
                let (child, c) = self.rewrite(*child, &pushable)?;
                changed |= c;
                JoinNode::Projection {
                    expressions,
                    child: Box::new(child),
                }
            }
            JoinNode::Aggregation {
                group_by,
                child,
                default_values,
            } => {
                let (child, c) = self.rewrite(*child, predicates)?;
                changed |= c;
                JoinNode::Aggregation {
                    group_by,
                    child: Box::new(child),
                    default_values,
                }
            }
            JoinNode::Apply {
                join_type,
                left,
                right,
                correlated_columns,
                no_decorrelate,
            } => {
                let (left, l) = self.rewrite(*left, predicates)?;
                let (right, r) = self.rewrite(*right, predicates)?;
                changed |= l || r;
                JoinNode::Apply {
                    join_type,
                    left: Box::new(left),
                    right: Box::new(right),
                    correlated_columns,
                    no_decorrelate,
                }
            }
            JoinNode::Window {
                partition_by,
                row_number_column,
                upper_bound,
                child,
            } => {
                let (child, c) = self.rewrite(*child, predicates)?;
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
                    let (child, c) = self.rewrite(child, predicates)?;
                    changed |= c;
                    rewritten.push(child);
                }
                JoinNode::UnionAll(rewritten)
            }
            node => node,
        };
        Ok((plan, changed))
    }
    /// 返回规则注册名。
    pub fn Name(&self) -> &'static str {
        "convert_outer_to_inner_joins"
    }
}
/// 判断表达式是否对给定列集合形成 null-reject（引用这些列且非 isnull/恒真）。
fn null_rejected(expression: &Expression, columns: &std::collections::HashSet<usize>) -> bool {
    expression.column.is_some_and(|c| columns.contains(&c))
        && !expression.name.starts_with("isnull:")
        && expression.name != "true"
}
