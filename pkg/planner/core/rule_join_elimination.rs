// Copyright 2017 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// Copyright 2026 AsterSQL.

// 外连接消除（Outer Join Elimination）逻辑优化规则。
//
// 当上层不引用内表列，且内表连接键能保证唯一性（唯一键、索引，或
// `row_number() = 1` 窗口分区）时，Left/Right Outer Join 可安全去掉内表侧，
// 仅保留外表（必要时用 NULL 扩展投影对齐 schema）。这能减少连接代价。

use crate::rule_join_reorder::{JoinEdge, JoinNode, JoinPlan, Result};
use crate::task::{Expression, JoinType};
use std::collections::{HashMap, HashSet};

/// 外连接消除器：在保证语义等价时去掉多余的 Outer Join 内表。
#[derive(Default)]
pub struct OuterJoinEliminator;
/// 将相关列（correlated columns）去重追加到目标列表。
pub fn appendUniqueCorrelatedCols(target: &mut Vec<usize>, corCols: &[usize]) {
    let mut seen = HashSet::new();
    for column in corCols {
        if seen.insert(*column) {
            target.push(*column);
        }
    }
}
/// 外表保留、内表列填 NULL 的投影，用于消除后仍需对齐原 Join 输出 schema。
pub fn buildOuterJoinNullExtendedProjection(
    outer: JoinPlan,
    outputSchema: &[usize],
    innerColumns: &HashSet<usize>,
) -> JoinPlan {
    let expressions = outputSchema
        .iter()
        .map(|column| Expression {
            name: if innerColumns.contains(column) {
                "null".into()
            } else {
                format!("col_{column}")
            },
            column: Some(*column),
            ..Expression::default()
        })
        .collect();
    JoinPlan {
        id: outer.id,
        node: JoinNode::Projection {
            expressions,
            child: Box::new(outer),
        },
        schema: outputSchema.to_vec(),
        row_count: 0.0,
    }
}
impl OuterJoinEliminator {
    /// 尝试消除当前 Outer Join：上层不引用内表列且内表侧唯一时返回外表（或 NULL 扩展投影）。
    pub fn tryToEliminateOuterJoin(
        &self,
        plan: &JoinPlan,
        aggCols: &[usize],
        parentCols: &[usize],
    ) -> Result<(Option<JoinPlan>, bool)> {
        let JoinNode::Join {
            join_type,
            left,
            right,
            equal_conditions,
            ..
        } = &plan.node
        else {
            return Ok((None, false));
        };
        // 仅处理左/右外连接；确定外表、内表及内表孩子下标。
        let (outer, inner, inner_idx) = match join_type {
            JoinType::LeftOuter => (left, right, 1),
            JoinType::RightOuter => (right, left, 0),
            _ => return Ok((None, false)),
        };
        let inner_columns = inner.columns();
        // 父层仍需要内表列则不能消除。
        if parentCols.iter().any(|c| inner_columns.contains(c)) {
            return Ok((None, false));
        }
        // Duplicate-agnostic aggregates may discard duplicates only when none of
        // their arguments comes from the inner side.
        if !aggCols.is_empty() {
            if aggCols.iter().any(|c| inner_columns.contains(c)) {
                return Ok((None, false));
            }
            return Ok((Some((**outer).clone()), true));
        }
        let (join_keys, null_equal) = self.extractInnerJoinKeys(equal_conditions, inner_idx);
        // 唯一性：唯一键 / 索引 / 分区 row_number=1 窗口；无聚合列时必须唯一。
        let unique = self.isInnerJoinKeysContainUniqueKey(inner, &join_keys, &null_equal)?
            || self.isInnerJoinKeysContainIndex(inner, &join_keys, &null_equal)?
            || isSelectionPartitionedRowNumberWindowOneUnique(inner, &join_keys);
        if !unique {
            return Ok((None, false));
        }
        // For uniqueness-based elimination Go returns the outer child directly;
        // NULL extension is reserved for a proven zero-row inner child.
        Ok((Some((**outer).clone()), true))
    }
    /// 从等值条件中抽取内表侧连接键，以及允许 NULL 相等的键集合。
    pub fn extractInnerJoinKeys(
        &self,
        conditions: &[JoinEdge],
        innerChildIdx: usize,
    ) -> (HashSet<usize>, HashSet<usize>) {
        let mut keys = HashSet::new();
        let mut nulls = HashSet::new();
        for edge in conditions {
            let column = if innerChildIdx == 0 {
                edge.left_column
            } else {
                edge.right_column
            };
            keys.insert(column);
            if edge.null_equal {
                nulls.insert(column);
            }
        }
        (keys, nulls)
    }
    /// 判断内表叶子的唯一键是否被连接键覆盖（且不含 null-equal 键）。
    pub fn isInnerJoinKeysContainUniqueKey(
        &self,
        inner: &JoinPlan,
        joinKeys: &HashSet<usize>,
        nullKeys: &HashSet<usize>,
    ) -> Result<bool> {
        let JoinNode::Leaf { unique_keys, .. } = &inner.node else {
            return Ok(false);
        };
        Ok(unique_keys.iter().any(|key| {
            key.iter()
                .all(|column| joinKeys.contains(column) && !nullKeys.contains(column))
        }))
    }
    /// 索引唯一性检查（当前实现复用唯一键判定）。
    pub fn isInnerJoinKeysContainIndex(
        &self,
        inner: &JoinPlan,
        joinKeys: &HashSet<usize>,
        nullKeys: &HashSet<usize>,
    ) -> Result<bool> {
        self.isInnerJoinKeysContainUniqueKey(inner, joinKeys, nullKeys)
    }
    /// 递归优化：在 Aggregation/Projection/Join 上传递所需列并尝试消除 Outer Join。
    pub fn doOptimize(
        &self,
        mut plan: JoinPlan,
        aggCols: &[usize],
        parentCols: &[usize],
    ) -> Result<(JoinPlan, bool)> {
        let mut changed = false;
        plan.node = match plan.node {
            JoinNode::Aggregation {
                group_by,
                child,
                default_values,
            } => {
                // 聚合的 group_by 列作为下层「聚合相关列」传入。
                let (child, c) = self.doOptimize(*child, &group_by, &plan.schema)?;
                changed |= c;
                JoinNode::Aggregation {
                    group_by,
                    child: Box::new(child),
                    default_values,
                }
            }
            JoinNode::Projection { expressions, child } => {
                let columns: Vec<_> = expressions.iter().filter_map(|e| e.column).collect();
                let (child, c) = self.doOptimize(*child, aggCols, &columns)?;
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
                // 先尝试消除本层 Outer Join，成功则直接返回。
                let candidate = JoinPlan {
                    id: plan.id,
                    schema: plan.schema.clone(),
                    row_count: plan.row_count,
                    node: JoinNode::Join {
                        join_type,
                        left: left.clone(),
                        right: right.clone(),
                        equal_conditions: equal_conditions.clone(),
                        other_conditions: other_conditions.clone(),
                        preferred_method: preferred_method.clone(),
                    },
                };
                if let (Some(eliminated), _) =
                    self.tryToEliminateOuterJoin(&candidate, aggCols, parentCols)?
                {
                    let (eliminated, _) = self.doOptimize(eliminated, aggCols, parentCols)?;
                    return Ok((eliminated, true));
                }
                // A join's own predicates remain required by its children even
                // when those columns are not part of the join output schema.
                let mut required_columns = plan.schema.clone();
                for edge in &equal_conditions {
                    required_columns.push(edge.left_column);
                    required_columns.push(edge.right_column);
                }
                required_columns.extend(other_conditions.iter().filter_map(|e| e.column));
                required_columns.sort_unstable();
                required_columns.dedup();
                let (left, l) = self.doOptimize(*left, aggCols, &required_columns)?;
                let (right, r) = self.doOptimize(*right, aggCols, &required_columns)?;
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
        Ok((plan, changed))
    }
    /// 优化入口：以整棵计划 schema 为父层所需列开始递归。
    pub fn Optimize(&self, plan: JoinPlan) -> Result<(JoinPlan, bool)> {
        let schema = plan.schema.clone();
        self.doOptimize(plan, &[], &schema)
    }
    /// 返回规则注册名。
    pub fn Name(&self) -> &'static str {
        "outer_join_eliminate"
    }
}
/// 判定内表是否为「按连接键分区且 row_number 上界为 1」的窗口模式（等价于唯一）。
pub fn isSelectionPartitionedRowNumberWindowOneUnique(
    inner: &JoinPlan,
    joinKeys: &HashSet<usize>,
) -> bool {
    match &inner.node {
        JoinNode::Selection { conditions, child } => {
            if let JoinNode::Window {
                partition_by,
                row_number_column: Some(row),
                upper_bound,
                ..
            } = &child.node
            {
                partition_by.iter().all(|c| joinKeys.contains(c))
                    && (upper_bound == &Some(1) || hasRowNumberUpperBoundOne(conditions, *row))
            } else {
                false
            }
        }
        _ => false,
    }
}
/// 过滤条件中是否存在对 row_number 列的 `=1` / `<=1` / `<2` 上界约束。
pub fn hasRowNumberUpperBoundOne(conditions: &[Expression], rowNumberCol: usize) -> bool {
    conditions.iter().any(|e| {
        e.column == Some(rowNumberCol) && (e.name == "eq:1" || e.name == "le:1" || e.name == "lt:2")
    })
}
/// 判断表达式是否为「某列等于常量 value」的等值谓词。
pub fn isColEqConst(expression: &Expression, targetCol: usize, value: i64) -> bool {
    expression.column == Some(targetCol) && expression.name == format!("eq:{value}")
}
/// 占位：返回空的聚合默认值映射（与 Go 侧辅助函数对齐）。
fn _default_values() -> HashMap<usize, String> {
    HashMap::new()
}
