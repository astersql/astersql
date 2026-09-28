// Copyright 2024 PingCAP, Inc. Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// Sequence 算子下推规则。
//
// Sequence（序列算子）表示按顺序执行多个子计划；本规则把 Sequence 尽量下推到
// Projection/Selection/Sort/Limit 等可穿透算子之下，使序列边界更靠近真正需要
// 顺序语义的子树，便于后续局部优化。

use crate::task::{PlanKind, PlanNode};
/// 将 Sequence 节点下推穿过可穿透算子的优化规则。
#[derive(Default)]
pub struct PushDownSequenceSolver;
impl PushDownSequenceSolver {
    /// 返回规则注册名。
    pub fn Name(&self) -> &'static str {
        "push_down_sequence"
    }
    /// 入口：无待下推 Sequence 时开始递归。
    pub fn Optimize(&self, p: PlanNode) -> (PlanNode, bool) {
        (self.recursiveOptimize(None, p).0, false)
    }
    /// 遇到 Sequence 时记下并继续处理其最后一个孩子；可穿透算子则透传；否则在此落地。
    pub fn recursiveOptimize(
        &self,
        pushedSequence: Option<PlanNode>,
        mut p: PlanNode,
    ) -> (PlanNode, bool) {
        // Sequence：合并嵌套 Sequence 的 CTE，并沿最后一个主查询继续下推。
        if p.kind == PlanKind::Sequence {
            let Some(main_query) = p.children.pop() else {
                return (p, false);
            };
            if let Some(mut outer) = pushedSequence {
                // 外层最后一个孩子是当前主查询，只保留其之前的 CTE；新 Sequence
                // 使用内层节点的其余元数据，对应 Go 以当前 lp 初始化新节点。
                outer.children.pop();
                let mut ctes = outer.children;
                ctes.append(&mut p.children);
                p.children = ctes;
            }
            p.children.push(main_query.clone());
            return self.recursiveOptimize(Some(p), main_query);
        }

        // 没有待下推 Sequence 时，Go 会递归优化当前节点的每棵子树。
        let Some(pushedSequence) = pushedSequence else {
            p.children = p
                .children
                .into_iter()
                .map(|child| self.recursiveOptimize(None, child).0)
                .collect();
            return (p, false);
        };

        // Go 的 default 分支允许穿透任意恰有一个孩子的逻辑算子。
        if p.children.len() == 1 {
            let (c, x) = self.recursiveOptimize(Some(pushedSequence), p.children.remove(0));
            p.children.push(c);
            return (p, x);
        }
        // 不可再穿透：把当前子树挂回 Sequence 末尾并返回。
        let mut seq = pushedSequence;
        seq.children.pop();
        seq.children.push(p);
        (seq, false)
    }
}
