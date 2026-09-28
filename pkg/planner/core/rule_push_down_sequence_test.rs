// Copyright 2026 AsterSQL.

use super::rule_push_down_sequence::PushDownSequenceSolver;
use crate::task::{PlanKind, PlanNode};

fn node(kind: PlanKind, children: Vec<PlanNode>) -> PlanNode {
    let mut node = PlanNode::new(kind);
    node.children = children;
    node
}

#[test]
fn optimizes_sequences_below_every_non_sequence_subtree() {
    let sequence = node(
        PlanKind::Sequence,
        vec![
            node(PlanKind::Cte, vec![]),
            node(PlanKind::TableScan, vec![]),
        ],
    );
    let root = node(
        PlanKind::HashJoin,
        vec![
            node(PlanKind::HashAgg, vec![sequence]),
            node(PlanKind::TableScan, vec![]),
        ],
    );

    let (optimized, changed) = PushDownSequenceSolver.Optimize(root);

    assert!(!changed, "Go LogicalOptRule reports planChanged=false");
    assert_eq!(optimized.kind, PlanKind::HashJoin);
    assert_eq!(optimized.children[0].kind, PlanKind::HashAgg);
    assert_eq!(optimized.children[0].children[0].kind, PlanKind::Sequence);
    assert_eq!(optimized.children[0].children[0].children.len(), 2);
}

#[test]
fn pushes_through_any_unary_operator_without_duplicating_main_query() {
    let sequence = node(
        PlanKind::Sequence,
        vec![
            node(PlanKind::Cte, vec![]),
            node(PlanKind::HashAgg, vec![node(PlanKind::TableScan, vec![])]),
        ],
    );

    let (optimized, changed) = PushDownSequenceSolver.Optimize(sequence);

    assert!(!changed);
    assert_eq!(optimized.kind, PlanKind::HashAgg);
    assert_eq!(optimized.children.len(), 1);
    let pushed = &optimized.children[0];
    assert_eq!(pushed.kind, PlanKind::Sequence);
    assert_eq!(pushed.children.len(), 2);
    assert_eq!(pushed.children[0].kind, PlanKind::Cte);
    assert_eq!(pushed.children[1].kind, PlanKind::TableScan);
}

#[test]
fn merges_nested_sequences_in_cte_order() {
    let inner = node(
        PlanKind::Sequence,
        vec![
            node(PlanKind::Other("inner-cte".into()), vec![]),
            node(PlanKind::TableScan, vec![]),
        ],
    );
    let outer = node(
        PlanKind::Sequence,
        vec![node(PlanKind::Other("outer-cte".into()), vec![]), inner],
    );

    let (optimized, changed) = PushDownSequenceSolver.Optimize(outer);

    assert!(!changed);
    assert_eq!(optimized.kind, PlanKind::Sequence);
    assert_eq!(optimized.children.len(), 3);
    assert_eq!(
        optimized.children[0].kind,
        PlanKind::Other("outer-cte".into())
    );
    assert_eq!(
        optimized.children[1].kind,
        PlanKind::Other("inner-cte".into())
    );
    assert_eq!(optimized.children[2].kind, PlanKind::TableScan);
}
