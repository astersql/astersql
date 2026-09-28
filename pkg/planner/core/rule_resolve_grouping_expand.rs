// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// Copyright 2026 AsterSQL.

// 解析 Grouping Sets 的 Expand 算子（Resolve Expand）。
//
// ROLLUP / CUBE / GROUPING SETS 在逻辑计划中常表现为 Expand：对每个 grouping set
// 生成一层投影，缺席的分组列置 NULL，并附加 grouping_id / grouping_position，
// 供后续聚合与 GROUPING() 函数使用。

use crate::rule_aggregation_elimination::{LogicalPlan, Result};
use crate::task::Expression;
use std::collections::{BTreeSet, HashSet};

/// 遍历计划树并为 Expand 节点生成各层投影的规则。
#[derive(Default)]
pub struct ResolveExpand;
impl ResolveExpand {
    /// 入口：调用 genExpand 生成/补全 Expand 的 level_projections。
    pub fn Optimize(&self, plan: LogicalPlan) -> Result<(LogicalPlan, bool)> {
        let (plan, _) = genExpand(plan)?;
        // 与 Go ResolveExpand 一致：生成层级投影不会被视为计划结构变化。
        Ok((plan, false))
    }
    /// 返回规则注册名。
    pub fn Name(&self) -> &'static str {
        "resolve_expand"
    }
}

/// 递归处理 Expand：按 grouping_sets 构造每层投影、grouping_id 与位置标记。
pub fn genExpand(plan: LogicalPlan) -> Result<(LogicalPlan, bool)> {
    match plan {
        LogicalPlan::Expand {
            child,
            grouping_sets,
            mut level_projections,
            schema,
        } => {
            let (child, _) = genExpand(*child)?;
            let column_count = child.schema().len();
            // 所有 grouping set 中出现过的列；不在当前 set 中的需置 NULL。
            let all_group_columns: BTreeSet<_> = grouping_sets.iter().flatten().copied().collect();
            // Go DistinctSize 按集合语义去重；集合内的顺序和重复列不影响结果。
            let distinct_sets: HashSet<Vec<_>> = grouping_sets
                .iter()
                .map(|set| {
                    set.iter()
                        .copied()
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect()
                })
                .collect();
            let has_duplicate_grouping_set = distinct_sets.len() != grouping_sets.len();
            level_projections.clear();
            for (level, grouping_set) in grouping_sets.iter().enumerate() {
                let present: HashSet<_> = grouping_set.iter().copied().collect();
                let generated_count = if has_duplicate_grouping_set { 2 } else { 1 };
                let mut projection = Vec::with_capacity(column_count + generated_count);
                for column in 0..column_count {
                    let absent_group_column =
                        all_group_columns.contains(&column) && !present.contains(&column);
                    projection.push(Expression {
                        name: if absent_group_column {
                            "null".into()
                        } else {
                            format!("col_{column}")
                        },
                        column: (!absent_group_column).then_some(column),
                        return_type: child.schema().get(column).cloned(),
                        ..Expression::default()
                    });
                }
                // grouping_id：按稳定的分组列顺序，用 1 标记本层存在的列。
                let gid = all_group_columns
                    .iter()
                    .enumerate()
                    .filter(|(_, column)| present.contains(column))
                    .fold(0_u64, |mask, (bit, _)| {
                        mask | 1_u64.checked_shl(bit as u32).unwrap_or(0)
                    });
                projection.push(Expression {
                    name: format!("grouping_id:{gid}"),
                    return_type: schema.get(column_count).cloned(),
                    ..Expression::default()
                });
                if has_duplicate_grouping_set {
                    projection.push(Expression {
                        name: format!("grouping_position:{level}"),
                        return_type: schema.get(column_count + 1).cloned(),
                        ..Expression::default()
                    });
                }
                level_projections.push(projection);
            }
            Ok((
                LogicalPlan::Expand {
                    child: Box::new(child),
                    grouping_sets,
                    level_projections,
                    schema,
                },
                false,
            ))
        }
        LogicalPlan::Aggregation(mut agg) => {
            let (child, changed) = genExpand(*agg.child)?;
            agg.child = Box::new(child);
            Ok((LogicalPlan::Aggregation(agg), changed))
        }
        LogicalPlan::Projection {
            expressions,
            child,
            schema,
        } => {
            let (child, changed) = genExpand(*child)?;
            Ok((
                LogicalPlan::Projection {
                    expressions,
                    child: Box::new(child),
                    schema,
                },
                changed,
            ))
        }
        LogicalPlan::Join {
            join_type,
            left,
            right,
            equal_conditions,
            other_conditions,
            schema,
        } => {
            let (left, l) = genExpand(*left)?;
            let (right, r) = genExpand(*right)?;
            Ok((
                LogicalPlan::Join {
                    join_type,
                    left: Box::new(left),
                    right: Box::new(right),
                    equal_conditions,
                    other_conditions,
                    schema,
                },
                l || r,
            ))
        }
        LogicalPlan::UnionAll { children, schema } => {
            let mut out = Vec::new();
            let mut changed = false;
            for child in children {
                let (child, c) = genExpand(child)?;
                changed |= c;
                out.push(child);
            }
            Ok((
                LogicalPlan::UnionAll {
                    children: out,
                    schema,
                },
                changed,
            ))
        }
        node => Ok((node, false)),
    }
}
