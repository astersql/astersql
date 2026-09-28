// Copyright 2024 PingCAP, Inc. Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 消除空 Selection 的逻辑优化规则。
//
// Selection 是过滤算子；若其谓词列表为空且仅有一个孩子，则语义上等价于直接
// 返回该孩子，可安全移除该层以简化逻辑计划（Logical Plan，优化器代数算子树）。

use crate::task::{PlanKind, PlanNode};
/// 空 Selection 消除器：去掉无谓词的多余过滤节点。
#[derive(Default)]
pub struct EmptySelectionEliminator;
impl EmptySelectionEliminator {
    /// 优化入口：Go 规则不消除根节点，且保留其固定的 `planChanged=false` 契约。
    pub fn Optimize(&self, p: PlanNode) -> (PlanNode, bool) {
        (self.recursivePlan(p), false)
    }
    /// 递归检查每个父节点的孩子，并移除作为孩子出现的空 Selection。
    pub fn recursivePlan(&self, mut p: PlanNode) -> PlanNode {
        p.children = p
            .children
            .into_iter()
            .map(|mut child| {
                if child.kind == PlanKind::Selection && child.conditions.is_empty() {
                    // 与 Go 的 `sel.Children()[0]` 一致：LogicalSelection 必须有孩子；
                    // 若不满足该计划树不变量，索引访问会立即失败。
                    self.recursivePlan(child.children.remove(0))
                } else {
                    self.recursivePlan(child)
                }
            })
            .collect();
        p
    }
    /// 返回规则注册名。
    pub fn Name(&self) -> &'static str {
        "eliminate_empty_selection"
    }
}
