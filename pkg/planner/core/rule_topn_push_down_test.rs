// Copyright 2026 AsterSQL.

use super::rule_topn_push_down::PushDownTopNOptimizer;
use crate::task::{PlanKind, PlanNode};

fn node(kind: PlanKind, children: Vec<PlanNode>) -> PlanNode {
    PlanNode::new(kind).with_children(children)
}

#[test]
fn optimize_keeps_go_logical_rule_changed_contract() {
    let plan = node(
        PlanKind::TopN,
        vec![node(
            PlanKind::Selection,
            vec![node(PlanKind::TableScan, vec![])],
        )],
    );

    let (optimized, changed) = PushDownTopNOptimizer.Optimize(plan);

    assert!(
        !changed,
        "Go LogicalOptRule always reports planChanged=false"
    );
    assert_eq!(optimized.kind, PlanKind::Selection);
    assert_eq!(optimized.children[0].kind, PlanKind::TopN);
}

#[test]
fn union_branch_count_uses_go_unsigned_addition_semantics() {
    let mut top_n = node(
        PlanKind::TopN,
        vec![node(
            PlanKind::UnionAll,
            vec![node(PlanKind::TableScan, vec![])],
        )],
    );
    top_n.offset = u64::MAX;
    top_n.count = 2;

    let (optimized, changed) = PushDownTopNOptimizer.Optimize(top_n);

    assert!(
        !changed,
        "Go LogicalOptRule always reports planChanged=false"
    );
    let branch_top_n = &optimized.children[0].children[0];
    assert_eq!(branch_top_n.offset, 0);
    assert_eq!(branch_top_n.count, 1, "Go uint64 addition wraps");
}
