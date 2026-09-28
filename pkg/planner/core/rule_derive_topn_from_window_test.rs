// Copyright 2026 AsterSQL.

use crate::rule_derive_topn_from_window::DeriveTopNFromWindow;
use crate::task::{Expression, PlanKind, PlanNode};

fn selection_over_window(bound: &str) -> PlanNode {
    let scan = PlanNode::new(PlanKind::TableScan);
    let mut window = PlanNode::new(PlanKind::Window).with_children(vec![scan]);
    window.by_items = vec![Expression {
        name: "a".to_owned(),
        ..Expression::default()
    }];
    let mut selection = PlanNode::new(PlanKind::Selection).with_children(vec![window]);
    selection.conditions = vec![Expression {
        name: bound.to_owned(),
        ..Expression::default()
    }];
    selection
}

#[test]
fn optimize_derives_topn_but_preserves_go_rule_changed_flag() {
    let input = PlanNode::new(PlanKind::Projection)
        .with_children(vec![selection_over_window("row_number_le:5")]);

    let (result, changed) = DeriveTopNFromWindow.Optimize(input);

    assert!(!changed, "Go's rule-level planChanged is always false");
    let top_n = &result.children[0].children[0].children[0];
    assert_eq!(top_n.kind, PlanKind::TopN);
    assert_eq!(top_n.count, 5);
    assert_eq!(top_n.by_items[0].name, "a");
    assert_eq!(top_n.children[0].kind, PlanKind::TableScan);
}
