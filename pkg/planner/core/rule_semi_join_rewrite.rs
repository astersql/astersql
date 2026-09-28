// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// Copyright 2026 AsterSQL.

// 半连接改写（Semi Join Rewrite）规则。
//
// Semi Join（半连接）只判断内表是否存在匹配行，不输出内表列（常见于 EXISTS/IN）。
// 本规则将 Semi Join 改写为 Inner Join + 右表按连接键聚合，再以投影恢复外表 schema，
// 以便复用内连接执行路径与后续优化。

use crate::rule_join_reorder::{JoinNode, JoinPlan, Result};
use crate::task::JoinType;

/// 将 Semi Join 改写为 Inner Join 加聚合去重的规则。
#[derive(Default)]
pub struct SemiJoinRewriter;
impl SemiJoinRewriter {
    /// 入口：递归改写整棵计划树。与 Go 一致，本规则不设置 planChanged。
    pub fn Optimize(&self, plan: JoinPlan) -> Result<(JoinPlan, bool)> {
        let (plan, _) = self.recursivePlan(plan)?;
        Ok((plan, false))
    }
    /// 返回规则注册名。
    pub fn Name(&self) -> &'static str {
        "semi_join_rewrite"
    }
    /// 后序遍历全部子计划，并将适用的 Semi Join 改写为 Projection(Inner+Agg)。
    pub fn recursivePlan(&self, mut plan: JoinPlan) -> Result<(JoinPlan, bool)> {
        let mut rewritten_schema = None;
        plan.node = match plan.node {
            JoinNode::Join {
                join_type,
                left,
                right,
                equal_conditions,
                other_conditions,
                preferred_method,
            } => {
                let (left, _) = self.recursivePlan(*left)?;
                let (right, _) = self.recursivePlan(*right)?;
                if join_type == JoinType::Semi && other_conditions.is_empty() {
                    let outer_schema = left.schema.clone();
                    rewritten_schema = Some(outer_schema.clone());
                    let group_by = equal_conditions
                        .iter()
                        .map(|condition| condition.right_column)
                        .collect::<Vec<_>>();
                    let aggregate = JoinPlan {
                        id: right.id,
                        node: JoinNode::Aggregation {
                            group_by: group_by.clone(),
                            child: Box::new(right),
                            default_values: Default::default(),
                        },
                        schema: group_by,
                        row_count: plan.row_count,
                    };
                    let mut inner_schema = outer_schema.clone();
                    for column in &aggregate.schema {
                        if !inner_schema.contains(column) {
                            inner_schema.push(*column);
                        }
                    }
                    let inner_join = JoinPlan {
                        id: plan.id,
                        node: JoinNode::Join {
                            join_type: JoinType::Inner,
                            left: Box::new(left),
                            right: Box::new(aggregate),
                            equal_conditions,
                            other_conditions,
                            preferred_method,
                        },
                        schema: inner_schema,
                        row_count: plan.row_count,
                    };
                    let expressions = outer_schema
                        .iter()
                        .map(|column| crate::task::Expression {
                            column: Some(*column),
                            ..Default::default()
                        })
                        .collect();
                    JoinNode::Projection {
                        expressions,
                        child: Box::new(inner_join),
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
            JoinNode::Projection { expressions, child } => JoinNode::Projection {
                expressions,
                child: Box::new(self.recursivePlan(*child)?.0),
            },
            JoinNode::Selection { conditions, child } => JoinNode::Selection {
                conditions,
                child: Box::new(self.recursivePlan(*child)?.0),
            },
            JoinNode::Aggregation {
                group_by,
                child,
                default_values,
            } => JoinNode::Aggregation {
                group_by,
                child: Box::new(self.recursivePlan(*child)?.0),
                default_values,
            },
            JoinNode::Apply {
                join_type,
                left,
                right,
                correlated_columns,
                no_decorrelate,
            } => JoinNode::Apply {
                join_type,
                left: Box::new(self.recursivePlan(*left)?.0),
                right: Box::new(self.recursivePlan(*right)?.0),
                correlated_columns,
                no_decorrelate,
            },
            JoinNode::Window {
                partition_by,
                row_number_column,
                upper_bound,
                child,
            } => JoinNode::Window {
                partition_by,
                row_number_column,
                upper_bound,
                child: Box::new(self.recursivePlan(*child)?.0),
            },
            JoinNode::UnionAll(children) => JoinNode::UnionAll(
                children
                    .into_iter()
                    .map(|child| self.recursivePlan(child).map(|result| result.0))
                    .collect::<Result<Vec<_>>>()?,
            ),
            node => node,
        };
        if let Some(schema) = rewritten_schema {
            plan.schema = schema;
        }
        Ok((plan, false))
    }
}
