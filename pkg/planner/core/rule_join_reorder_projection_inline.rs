// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");

// Join 组上的投影内联（Projection Inline）辅助逻辑。
//
// 当 Join 组外层包着可内联的 Projection（投影算子，仅做列映射/简单表达式）时，
// 把投影中的列映射回写到等值边（eqEdges），使后续 Join Reorder 能直接按子树列编号工作，
// 并保留原始输出 schema 供重排后还原。

use crate::rule_join_reorder::{JoinNode, JoinPlan, extractJoinGroup, joinGroupResult};
use crate::task::Expression;
use std::collections::HashMap;

/// 若根节点为可内联 Projection，则提取子树 Join 组并把列映射应用到等值边。
pub fn tryInlineProjectionForJoinGroup(plan: &JoinPlan) -> (Option<joinGroupResult>, bool) {
    let JoinNode::Projection { expressions, child } = &plan.node else {
        return (None, false);
    };
    // Go 侧只允许穿透直接包在 Join 上的 Projection；其它 Projection 由常规
    // join-group 抽取逻辑作为原子叶处理。
    if !matches!(child.node, JoinNode::Join { .. }) || !canInlineProjectionBasic(plan) {
        return (None, false);
    }
    let child_result = extractJoinGroup(child);
    if !canInlineProjection(plan, &child_result) {
        return (
            Some(joinGroupResult {
                group: crate::rule_join_reorder::basicJoinGroupInfo {
                    joinNodePlans: vec![plan.clone()],
                    ..Default::default()
                },
                originalSchema: plan.schema.clone(),
                ..Default::default()
            }),
            true,
        );
    }
    // 投影输出列 → 子表达式，用于改写等值边两侧列号。
    let mapping = buildColExprMapForProjection(&plan.schema, expressions);
    let mut result = child_result;
    for edge in &mut result.group.eqEdges {
        if let Some(expr) = mapping.get(&edge.left_column) {
            if let Some(column) = expr.column {
                edge.left_column = column;
            }
        }
        if let Some(expr) = mapping.get(&edge.right_column) {
            if let Some(column) = expr.column {
                edge.right_column = column;
            }
        }
    }
    // 保留投影后的 schema，重排完成后再按此还原输出列。
    result.originalSchema = plan.schema.clone();
    (Some(result), true)
}
/// 建立投影输出 schema 列号到对应表达式的映射。
pub fn buildColExprMapForProjection(
    schema: &[usize],
    expressions: &[Expression],
) -> HashMap<usize, Expression> {
    schema
        .iter()
        .copied()
        .zip(expressions.iter().cloned())
        // 与 Go 的 pass-through 规则一致，避免建立 output -> 同一 output 的自引用。
        .filter(|(output, expression)| expression.column != Some(*output))
        .collect()
}
/// 判断单个投影表达式是否可安全内联（列引用或确定性简单表达式）。
pub fn isInlineableProjectionExpr(expression: &Expression) -> bool {
    // 当前 Rust Expression 用 column 表示表达式树中唯一可追踪的列引用。Go
    // 明确拒绝 constant-only 表达式以及非确定性、子查询和相关表达式。
    expression.column.is_some()
        && !expression.name.starts_with("nondeterministic:")
        && !expression.name.starts_with("mutable:")
        && !expression.name.starts_with("subquery:")
        && !expression.name.starts_with("correlated:")
}
/// 基础可内联检查：全部表达式可内联且与 schema 长度一致。
pub fn canInlineProjectionBasic(plan: &JoinPlan) -> bool {
    matches!(&plan.node, JoinNode::Projection { expressions, .. } if expressions.iter().all(isInlineableProjectionExpr) && expressions.len() == plan.schema.len())
}
/// 完整可内联检查：基础条件 + Join 组多于一表，且投影列引用不重复。
pub fn canInlineProjection(plan: &JoinPlan, childResult: &joinGroupResult) -> bool {
    canInlineProjectionBasic(plan) && childResult.group.joinNodePlans.len() > 1 && {
        let JoinNode::Projection { expressions, .. } = &plan.node else {
            return false;
        };
        // 建立列到叶子的唯一归属。列若出现在多个叶 schema 中，或表达式引用
        // join group 之外的列，均与 Go 一样保守拒绝。多个输出引用同一叶列是安全的。
        let mut leaf_by_column = HashMap::new();
        for (leaf_index, leaf) in childResult.group.joinNodePlans.iter().enumerate() {
            for column in &leaf.schema {
                if leaf_by_column.insert(*column, leaf_index).is_some() {
                    return false;
                }
            }
        }
        expressions.iter().all(|expression| {
            expression
                .column
                .is_some_and(|column| leaf_by_column.contains_key(&column))
        })
    }
}
