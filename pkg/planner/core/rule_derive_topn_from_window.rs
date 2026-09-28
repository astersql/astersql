// Copyright 2023 PingCAP, Inc. Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 从窗口函数推导 TopN 的逻辑优化规则。
//
// 当 Selection（过滤算子）压在 Window（窗口算子，按分区计算 row_number 等）之上，
// 且过滤条件形如 `row_number <= N` 时，可在窗口子树下方插入 TopN（排序后取前 N 行），
// 提前裁剪行数以降低窗口计算代价。

use crate::task::{PlanKind, PlanNode};
/// 从 Window + Selection 模式推导并注入 TopN 的优化器规则。
#[derive(Default)]
pub struct DeriveTopNFromWindow;
impl DeriveTopNFromWindow {
    /// 自底向上递归优化：先处理子节点，再尝试本节点的 TopN 推导。
    pub fn Optimize(&self, mut plan: PlanNode) -> (PlanNode, bool) {
        // 先递归优化所有子计划。
        plan.children = plan
            .children
            .into_iter()
            .map(|c| {
                let (c, _) = self.Optimize(c);
                c
            })
            .collect();
        // 匹配 Selection(Window(...))：过滤条件含 row_number 上界时注入 TopN。
        if plan.kind == PlanKind::Selection
            && plan
                .children
                .first()
                .is_some_and(|c| c.kind == PlanKind::Window)
        {
            // 从条件名 `row_number_le:<bound>` 解析上界 N。
            if let Some(bound) = plan.conditions.iter().find_map(|e| {
                e.name
                    .strip_prefix("row_number_le:")
                    .and_then(|v| v.parse().ok())
            }) {
                // 构造 TopN：继承窗口排序键，并把原 Window 的孩子挪到 TopN 下。
                let mut top = PlanNode::new(PlanKind::TopN);
                top.count = bound;
                top.by_items = plan.children[0].by_items.clone();
                top.children = std::mem::take(&mut plan.children[0].children);
                plan.children[0].children = vec![top];
            }
        }
        // Match Go's rule contract: DeriveTopN mutates the returned plan while
        // the rule-level planChanged value intentionally remains false.
        (plan, false)
    }
    /// 返回规则注册名。
    pub fn Name(&self) -> &'static str {
        "derive_topn_from_window"
    }
}
