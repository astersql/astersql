// Copyright 2026 AsterSQL.

use super::rule_join_reorder::{JoinNode, JoinPlan, baseSingleGroupJoinOrderSolver};

fn leaf(id: usize, row_count: f64) -> JoinPlan {
    JoinPlan {
        id,
        node: JoinNode::Leaf {
            name: format!("t{id}"),
            predicates: Vec::new(),
            unique_keys: Vec::new(),
            correlated_columns: Vec::new(),
        },
        schema: vec![id],
        row_count,
    }
}

#[test]
fn base_node_cumulative_cost_includes_descendants() {
    let mut solver = baseSingleGroupJoinOrderSolver::default();
    let join = solver.newCartesianJoin(leaf(1, 2.0), leaf(2, 3.0));

    assert_eq!(solver.baseNodeCumCost(&join), join.row_count + 2.0 + 3.0);
}

#[test]
fn cartesian_group_is_combined_pairwise_into_a_bushy_tree() {
    let mut solver = baseSingleGroupJoinOrderSolver::default();
    let plan = solver
        .makeBushyJoin(vec![leaf(1, 1.0), leaf(2, 1.0), leaf(3, 1.0), leaf(4, 1.0)])
        .expect("non-empty group must produce a plan");

    let JoinNode::Join { left, right, .. } = plan.node else {
        panic!("four leaves must produce a join root");
    };
    assert!(matches!(left.node, JoinNode::Join { .. }));
    assert!(matches!(right.node, JoinNode::Join { .. }));
    assert_eq!(left.schema, vec![1, 2]);
    assert_eq!(right.schema, vec![3, 4]);
}
