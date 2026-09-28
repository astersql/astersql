// Copyright 2022 PingCAP, Inc. Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 结果集重排（Result Reorder）规则。
//
// 当计划树尚无完整排序（Sort/TopN）且当前节点也不保序时，注入以 handle 列
// （行句柄，通常对应主键/隐式 row id）为键的 Sort，保证结果行序稳定可复现。

use crate::task::{Expression, PlanKind, PlanNode};
/// 必要时注入 Sort 以保证输出顺序确定的优化规则。
#[derive(Default)]
pub struct ResultReorder;
impl ResultReorder {
    /// 已有完整排序或保序父节点下已有排序则跳过，否则注入 Sort。
    pub fn Optimize(&self, mut p: PlanNode) -> (PlanNode, bool) {
        if !self.completeSort(&mut p) {
            p = self.injectSort(p);
        }
        // Keep parity with Go: this customer-specific rule mutates/replaces the
        // plan but deliberately never sets planChanged.
        (p, false)
    }
    /// 穿过保序节点；遇到 Sort 时补齐所有输出列（能提取句柄时只补句柄）。
    pub fn completeSort(&self, p: &mut PlanNode) -> bool {
        if self.isInputOrderKeeper(p) {
            return p
                .children
                .first_mut()
                .is_none_or(|child| self.completeSort(child));
        }
        if p.kind != PlanKind::Sort {
            return false;
        }

        let columns = p
            .children
            .first()
            .and_then(|child| self.extractHandleCol(child))
            .map(|handle| vec![handle])
            .unwrap_or_else(|| Self::schemaColumns(p.schema.len()));
        for column in columns {
            if !p.by_items.iter().any(|item| item.column == column.column) {
                p.by_items.push(column);
            }
        }
        true
    }
    /// 在计划外层包一层按 handle 列排序的 Sort。
    pub fn injectSort(&self, p: PlanNode) -> PlanNode {
        if self.isInputOrderKeeper(&p) {
            let mut keeper = p;
            if let Some(child_slot) = keeper.children.first_mut() {
                let child = std::mem::take(child_slot);
                *child_slot = self.injectSort(child);
            }
            return keeper;
        }
        let Some(handle) = self.extractHandleCol(&p) else {
            let mut sort = PlanNode::new(PlanKind::Sort);
            sort.by_items = Self::schemaColumns(p.schema.len());
            sort.schema = p.schema.clone();
            sort.children.push(p);
            return sort;
        };
        let mut sort = PlanNode::new(PlanKind::Sort);
        sort.by_items.push(handle);
        sort.schema = p.schema.clone();
        sort.children.push(p);
        sort
    }
    /// Projection/Selection/Limit 会保留孩子输入顺序（order keeper）。
    pub fn isInputOrderKeeper(&self, p: &PlanNode) -> bool {
        matches!(
            p.kind,
            PlanKind::Projection | PlanKind::Selection | PlanKind::Limit
        )
    }
    /// 从 DataSource 的 `handle:` 标签提取句柄；Selection/Limit 递归到孩子。
    pub fn extractHandleCol(&self, p: &PlanNode) -> Option<Expression> {
        if matches!(p.kind, PlanKind::Selection | PlanKind::Limit) {
            let handle = self.extractHandleCol(p.children.first()?)?;
            return handle
                .column
                .filter(|index| *index < p.schema.len())
                .map(|_| handle);
        }
        if !p.flags.from_data_source || p.labels.contains_key("common_handle") {
            return None;
        }
        p.labels.keys().find_map(|key| {
            key.strip_prefix("handle:")
                .and_then(|v| v.parse().ok())
                .map(|c| Expression {
                    name: key.clone(),
                    column: Some(c),
                    ..Default::default()
                })
        })
    }
    fn schemaColumns(count: usize) -> Vec<Expression> {
        (0..count)
            .map(|column| Expression {
                name: format!("column:{column}"),
                column: Some(column),
                ..Default::default()
            })
            .collect()
    }
    /// 返回规则注册名。
    pub fn Name(&self) -> &'static str {
        "result_reorder"
    }
}
