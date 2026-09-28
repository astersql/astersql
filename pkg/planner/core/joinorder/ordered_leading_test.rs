// Copyright 2026 AsterSQL.

use crate::ordered_leading::{
    OrderedLeadingChoice, find_ordered_leading_choice, try_annotate_ordered_leading,
};
use crate::util::{JoinMethodHint, JoinType, PlanKind, PlanNode};

fn leaf(id: usize, columns: &[i64], indexes: Vec<Vec<i64>>) -> PlanNode {
    PlanNode::leaf(
        id,
        "test",
        format!("t{id}"),
        columns.iter().copied().collect(),
        1.0,
        indexes,
    )
}

fn join(id: usize, left: PlanNode, right: PlanNode, hint: JoinMethodHint) -> PlanNode {
    let columns = left.columns.union(&right.columns).copied().collect();
    PlanNode {
        id,
        kind: PlanKind::Join {
            join_type: JoinType::Inner,
            equal_conditions: Vec::new(),
            other_conditions: Vec::new(),
            hint,
        },
        children: vec![left, right],
        columns,
        estimated_rows: 1.0,
        cumulative_cost: 1.0,
    }
}

#[test]
fn ordered_leading_rejects_invalid_duplicate_and_single_leaf_inputs() {
    let carrier = leaf(0, &[1], vec![vec![1]]);
    assert_eq!(find_ordered_leading_choice(&carrier, &[1]), None);

    let root = join(2, carrier, leaf(1, &[2], Vec::new()), Default::default());
    assert_eq!(find_ordered_leading_choice(&root, &[0]), None);
    assert_eq!(find_ordered_leading_choice(&root, &[-1]), None);
    assert_eq!(find_ordered_leading_choice(&root, &[1, 1]), None);
}

#[test]
fn annotation_refuses_existing_join_method_hints() {
    let choice = OrderedLeadingChoice {
        leaf_id: 0,
        ordering_columns: vec![1],
        reverse: false,
    };
    let existing = JoinMethodHint {
        prefer_hash: true,
        ..Default::default()
    };
    let mut root = join(
        2,
        leaf(0, &[1], vec![vec![1]]),
        leaf(1, &[2], Vec::new()),
        existing.clone(),
    );

    assert!(!try_annotate_ordered_leading(&mut root, &choice));
    let PlanKind::Join { hint, .. } = root.kind else {
        unreachable!();
    };
    assert_eq!(hint, existing);
}

#[test]
fn annotation_only_marks_the_join_group_anchor() {
    let nested = join(
        3,
        leaf(0, &[1], vec![vec![1]]),
        leaf(1, &[2], Vec::new()),
        Default::default(),
    );
    let mut root = join(4, nested, leaf(2, &[3], Vec::new()), Default::default());
    let choice = OrderedLeadingChoice {
        leaf_id: 0,
        ordering_columns: vec![1],
        reverse: false,
    };

    assert!(try_annotate_ordered_leading(&mut root, &choice));
    let PlanKind::Join { hint, .. } = &root.kind else {
        unreachable!();
    };
    assert!(hint.prefer_merge);
    let PlanKind::Join { hint, .. } = &root.children[0].kind else {
        unreachable!();
    };
    assert_eq!(hint, &JoinMethodHint::default());
}
