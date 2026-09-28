// Copyright 2024 PingCAP, Inc. Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 消除 UnionAll 中无用 Dual 分支的逻辑优化规则。
//
// UnionAll 合并多个分支的结果集；TableDual（空表/对偶表，行数为 0）分支不贡献
// 任何行，可安全剔除。若剔除后仅剩一个分支，则整个 UnionAll 可退化为该分支。

use crate::task::{PlanKind, PlanNode};
/// 消除 UnionAll 中零行 Dual 子项的优化规则。
#[derive(Default)]
pub struct EliminateUnionAllDualItem;
impl EliminateUnionAllDualItem {
    /// 返回规则注册名。
    pub fn Name(&self) -> &'static str {
        "union_all_eliminate_dual_item"
    }
    /// 优化入口：委托给递归消除函数。
    pub fn Optimize(&self, p: PlanNode) -> (PlanNode, bool) {
        unionAllEliminateDualItem(p)
    }
}
fn is_zero_row_dual(p: &PlanNode) -> bool {
    matches!(&p.kind, PlanKind::Other(name) if name == "TableDual") && p.stats.row_count == 0.0
}

/// 按 Go 顺序先清理当前 UnionAll，再递归处理剩余子树。
pub fn unionAllEliminateDualItem(mut p: PlanNode) -> (PlanNode, bool) {
    if p.kind == PlanKind::UnionAll {
        p.children.retain(|child| {
            if is_zero_row_dual(child) {
                return false;
            }
            if child.kind == PlanKind::Projection
                && child.children.first().is_some_and(is_zero_row_dual)
            {
                return false;
            }
            true
        });
        if p.children.is_empty() {
            let mut dual = PlanNode::new(PlanKind::Other("TableDual".to_owned()));
            dual.schema = p.schema;
            return (dual, true);
        }
    }

    let mut changed = false;
    p.children = p
        .children
        .into_iter()
        .map(|child| {
            let (child, child_changed) = unionAllEliminateDualItem(child);
            changed |= child_changed;
            child
        })
        .collect();
    (p, changed)
}
