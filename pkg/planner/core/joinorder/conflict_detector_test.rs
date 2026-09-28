// Copyright 2026 AsterSQL.

use crate::conflict_detector::ConflictDetector;
use crate::util::{JoinType, PlanKind, PlanNode};
use std::collections::BTreeSet;

fn leaf(id: usize) -> PlanNode {
    PlanNode::leaf(
        id,
        "test",
        format!("t{id}"),
        BTreeSet::new(),
        1.0,
        Vec::new(),
    )
}

fn join(join_type: JoinType) -> PlanNode {
    PlanNode {
        id: 2,
        kind: PlanKind::Join {
            join_type,
            equal_conditions: Vec::new(),
            other_conditions: Vec::new(),
            hint: Default::default(),
        },
        children: vec![leaf(0), leaf(1)],
        columns: BTreeSet::new(),
        estimated_rows: 1.0,
        cumulative_cost: 1.0,
    }
}

#[test]
fn non_inner_connection_restores_original_side_order() {
    let (detector, leaves) = ConflictDetector::build(&join(JoinType::LeftOuter)).unwrap();
    let result = detector.check_connection(&leaves[1], &leaves[0]).unwrap();

    assert!(result.connected());
    assert_eq!(result.left.vertexes, BTreeSet::from([0]));
    assert_eq!(result.right.vertexes, BTreeSet::from([1]));
}

#[test]
fn predicate_less_edges_are_not_reported_as_remaining() {
    let (detector, _) = ConflictDetector::build(&join(JoinType::Inner)).unwrap();

    assert!(!detector.has_remaining_edges(&BTreeSet::new()));
}
