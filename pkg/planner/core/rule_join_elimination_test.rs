// Copyright 2026 AsterSQL.

use super::rule_join_elimination::OuterJoinEliminator;
use super::rule_join_reorder::{JoinEdge, JoinNode, JoinPlan};
use super::task::{Expression, JoinType};

fn leaf(id: usize, schema: Vec<usize>, unique_keys: Vec<Vec<usize>>) -> JoinPlan {
    JoinPlan {
        id,
        node: JoinNode::Leaf {
            name: format!("t{id}"),
            predicates: Vec::new(),
            unique_keys,
            correlated_columns: Vec::new(),
        },
        schema,
        row_count: 1.0,
    }
}

fn left_join(
    id: usize,
    left: JoinPlan,
    right: JoinPlan,
    left_key: usize,
    right_key: usize,
) -> JoinPlan {
    let mut schema = left.schema.clone();
    schema.extend_from_slice(&right.schema);
    JoinPlan {
        id,
        node: JoinNode::Join {
            join_type: JoinType::LeftOuter,
            left: Box::new(left),
            right: Box::new(right),
            equal_conditions: vec![JoinEdge {
                left_column: left_key,
                right_column: right_key,
                null_equal: false,
            }],
            other_conditions: Vec::new(),
            preferred_method: None,
        },
        schema,
        row_count: 1.0,
    }
}

#[test]
fn duplicate_agnostic_aggregate_cannot_drop_join_when_it_uses_inner_column() {
    let plan = left_join(
        3,
        leaf(1, vec![1], Vec::new()),
        leaf(2, vec![2], Vec::new()),
        1,
        2,
    );
    let (eliminated, changed) = OuterJoinEliminator
        .tryToEliminateOuterJoin(&plan, &[2], &[1])
        .unwrap();

    assert!(!changed);
    assert!(eliminated.is_none());
}

#[test]
fn optimization_eliminates_consecutive_outer_joins() {
    let inner_join = left_join(
        4,
        leaf(1, vec![1], Vec::new()),
        leaf(2, vec![2], vec![vec![2]]),
        1,
        2,
    );
    let plan = left_join(5, inner_join, leaf(3, vec![3], vec![vec![3]]), 1, 3);

    let (optimized, changed) = OuterJoinEliminator.doOptimize(plan, &[], &[1]).unwrap();

    assert!(changed);
    assert!(matches!(optimized.node, JoinNode::Leaf { .. }));
    assert_eq!(optimized.id, 1);
}

#[test]
fn join_conditions_remain_required_while_optimizing_children() {
    let eliminable_child = left_join(
        4,
        leaf(1, vec![1], Vec::new()),
        leaf(2, vec![2], vec![vec![2]]),
        1,
        2,
    );
    let plan = JoinPlan {
        id: 6,
        schema: vec![1, 3],
        row_count: 1.0,
        node: JoinNode::Join {
            join_type: JoinType::Inner,
            left: Box::new(eliminable_child),
            right: Box::new(leaf(3, vec![3], Vec::new())),
            equal_conditions: vec![JoinEdge {
                left_column: 2,
                right_column: 3,
                null_equal: false,
            }],
            other_conditions: vec![Expression {
                name: "predicate".into(),
                column: Some(2),
                ..Expression::default()
            }],
            preferred_method: None,
        },
    };

    let (optimized, _) = OuterJoinEliminator.doOptimize(plan, &[], &[]).unwrap();
    let JoinNode::Join { left, .. } = optimized.node else {
        panic!("root inner join must remain")
    };
    assert!(matches!(left.node, JoinNode::Join { .. }));
}

#[test]
fn rule_name_matches_go_registration_name() {
    assert_eq!(OuterJoinEliminator.Name(), "outer_join_eliminate");
}
