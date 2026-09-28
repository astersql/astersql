// Copyright 2017 PingCAP, Inc. Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// TopN / Limit 下推规则。
//
// TopN（排序后取前 N 行）与 Limit（截断行数）尽量下推到 Projection/Selection 之下，
// 或复制到 UnionAll 各分支（分支上 limit 需覆盖 offset+count），以尽早减少中间结果。

use crate::task::{PlanKind, PlanNode};
/// 将 TopN/Limit 下推穿过投影、过滤，并分发到 Union 分支的优化规则。
#[derive(Default)]
pub struct PushDownTopNOptimizer;
impl PushDownTopNOptimizer {
    /// 自底向上：先优化孩子，再尝试把本层 TopN/Limit 与孩子交换或分发。
    pub fn Optimize(&self, mut p: PlanNode) -> (PlanNode, bool) {
        p.children = p
            .children
            .into_iter()
            .map(|c| {
                let (c, _) = self.Optimize(c);
                c
            })
            .collect();
        if matches!(p.kind, PlanKind::TopN | PlanKind::Limit) && p.children.len() == 1 {
            let child = p.children.remove(0);
            match child.kind {
                // Projection/Selection：交换父子，使截断更靠近数据源。
                PlanKind::Projection | PlanKind::Selection => {
                    let mut child = child;
                    let grand = child.children.remove(0);
                    let mut pushed = p;
                    pushed.children.push(grand);
                    child.children.push(pushed);
                    return (child, false);
                }
                // UnionAll：每个分支复制一份 TopN/Limit，offset 归零，count 放大为 offset+count。
                PlanKind::UnionAll => {
                    let mut union = child;
                    for branch in &mut union.children {
                        let mut pushed = p.clone();
                        pushed.offset = 0;
                        pushed.count = p.offset.wrapping_add(p.count);
                        pushed.children.push(branch.clone());
                        *branch = pushed;
                    }
                    p.children.push(union);
                    return (p, false);
                }
                _ => p.children.push(child),
            }
        }
        // Go 规则虽可能原地改写计划，但 planChanged 契约固定为 false。
        (p, false)
    }
    /// 返回规则注册名。
    pub fn Name(&self) -> &'static str {
        "topn_push_down"
    }
}
