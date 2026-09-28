// Copyright 2026 AsterSQL.

use crate::rule_eliminate_empty_selection::EmptySelectionEliminator;
use crate::task::{PlanKind, PlanNode};

#[test]
fn optimize_matches_go_root_and_change_contract() {
    let root =
        PlanNode::new(PlanKind::Selection).with_children(vec![PlanNode::new(PlanKind::TableScan)]);

    let (optimized, changed) = EmptySelectionEliminator.Optimize(root);

    assert_eq!(optimized.kind, PlanKind::Selection);
    assert!(!changed);
}

#[test]
fn recursive_plan_eliminates_only_empty_selection_children() {
    let scan = PlanNode::new(PlanKind::TableScan);
    let inner = PlanNode::new(PlanKind::Selection).with_children(vec![scan]);
    let outer = PlanNode::new(PlanKind::Selection).with_children(vec![inner]);
    let root = PlanNode::new(PlanKind::Projection).with_children(vec![outer]);

    let (optimized, changed) = EmptySelectionEliminator.Optimize(root);

    assert!(!changed);
    assert_eq!(optimized.children[0].kind, PlanKind::Selection);
    assert_eq!(optimized.children[0].children[0].kind, PlanKind::TableScan);
}

#[test]
fn keeps_conditional_selection_and_matches_go_rule_name() {
    let mut selection =
        PlanNode::new(PlanKind::Selection).with_children(vec![PlanNode::new(PlanKind::TableScan)]);
    selection.conditions.push(Default::default());
    let root = PlanNode::new(PlanKind::Projection).with_children(vec![selection]);

    let (optimized, changed) = EmptySelectionEliminator.Optimize(root);

    assert!(!changed);
    assert_eq!(optimized.children[0].kind, PlanKind::Selection);
    assert_eq!(EmptySelectionEliminator.Name(), "eliminate_empty_selection");
}
