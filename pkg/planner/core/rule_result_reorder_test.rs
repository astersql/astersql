// Copyright 2026 AsterSQL.

use crate::rule_result_reorder::ResultReorder;
use crate::task::{FieldType, PlanKind, PlanNode, TypeCode};

fn field() -> FieldType {
    FieldType {
        code: TypeCode::Int,
        flen: 11,
        decimal: 0,
        unsigned: false,
    }
}

fn node(kind: PlanKind, columns: usize) -> PlanNode {
    let mut plan = PlanNode::new(kind);
    plan.schema = vec![field(); columns];
    plan
}

#[test]
fn optimize_injects_below_order_keepers_and_reports_go_changed_flag() {
    let source = node(PlanKind::TableScan, 2);
    let selection = node(PlanKind::Selection, 2).with_children(vec![source]);

    let (optimized, changed) = ResultReorder.Optimize(selection);

    assert!(!changed, "Go's rule intentionally leaves planChanged false");
    assert_eq!(optimized.kind, PlanKind::Selection);
    assert_eq!(optimized.children[0].kind, PlanKind::Sort);
    assert_eq!(optimized.children[0].by_items.len(), 2);
    assert_eq!(optimized.children[0].children[0].kind, PlanKind::TableScan);
}

#[test]
fn complete_sort_extends_existing_sort_like_go() {
    let child = node(PlanKind::TableScan, 2);
    let mut sort = node(PlanKind::Sort, 2).with_children(vec![child]);
    sort.by_items.push(crate::task::Expression {
        name: "first".into(),
        column: Some(0),
        ..Default::default()
    });

    let (optimized, changed) = ResultReorder.Optimize(sort);

    assert!(!changed);
    assert_eq!(optimized.kind, PlanKind::Sort);
    assert_eq!(optimized.by_items.len(), 2);
    assert_eq!(optimized.by_items[0].column, Some(0));
    assert_eq!(optimized.by_items[1].column, Some(1));
}

#[test]
fn leaf_input_order_keeper_is_already_complete() {
    let leaf = node(PlanKind::Projection, 1);
    let (optimized, changed) = ResultReorder.Optimize(leaf);
    assert!(!changed);
    assert_eq!(optimized.kind, PlanKind::Projection);
    assert!(optimized.children.is_empty());
}

#[test]
fn top_n_is_not_a_complete_sort_in_the_go_rule() {
    let mut top_n = node(PlanKind::TopN, 1);
    top_n.by_items.push(crate::task::Expression {
        name: "first".into(),
        column: Some(0),
        ..Default::default()
    });

    let (optimized, _) = ResultReorder.Optimize(top_n);
    assert_eq!(optimized.kind, PlanKind::Sort);
    assert_eq!(optimized.children[0].kind, PlanKind::TopN);
}

#[test]
fn handle_is_used_only_when_explicitly_available_from_data_source() {
    let ordinary = node(PlanKind::TableScan, 2);
    assert!(ResultReorder.extractHandleCol(&ordinary).is_none());

    let mut with_handle = node(PlanKind::TableScan, 2);
    with_handle.flags.from_data_source = true;
    with_handle.labels.insert("handle:1".into(), 1.0);
    let handle = ResultReorder.extractHandleCol(&with_handle).unwrap();
    assert_eq!(handle.column, Some(1));
}
