// Copyright 2026 AsterSQL.

use crate::rule_correlate::CorrelateSolver;
use crate::rule_join_reorder::{JoinEdge, JoinNode, JoinPlan};
use crate::task::{Expression, JoinType};

fn leaf(id: usize, schema: Vec<usize>) -> JoinPlan {
    JoinPlan {
        id,
        node: JoinNode::Leaf {
            name: format!("t{id}"),
            predicates: Vec::new(),
            unique_keys: Vec::new(),
            correlated_columns: Vec::new(),
        },
        schema,
        row_count: 1.0,
    }
}

fn preferred_semi_join(edge: JoinEdge) -> JoinPlan {
    JoinPlan {
        id: 3,
        node: JoinNode::Join {
            join_type: JoinType::Semi,
            left: Box::new(leaf(1, vec![1])),
            right: Box::new(leaf(2, vec![2])),
            equal_conditions: vec![edge],
            other_conditions: Vec::new(),
            preferred_method: Some("correlate".into()),
        },
        schema: vec![1],
        row_count: 1.0,
    }
}

#[test]
fn preferred_semi_join_is_rebuilt_as_apply() {
    let plan = preferred_semi_join(JoinEdge {
        left_column: 2,
        right_column: 1,
        null_equal: false,
    });

    let (optimized, changed) = CorrelateSolver.Optimize(plan).expect("correlate");
    assert!(changed);
    let JoinNode::Apply {
        join_type,
        right,
        correlated_columns,
        no_decorrelate,
        ..
    } = optimized.node
    else {
        panic!("preferred semi join must become apply");
    };
    assert_eq!(join_type, JoinType::Semi);
    assert_eq!(correlated_columns, vec![1]);
    assert!(!no_decorrelate);
    let JoinNode::Leaf { predicates, .. } = right.node else {
        panic!("inner side remains a leaf");
    };
    assert_eq!(predicates.len(), 1);
    assert_eq!(predicates[0].name, "correlated_eq");
    assert_eq!(predicates[0].column, Some(2));
}

#[test]
fn correlate_aborts_when_go_safety_gates_are_not_met() {
    let cases = [
        preferred_semi_join(JoinEdge {
            left_column: 1,
            right_column: 2,
            null_equal: true,
        }),
        {
            let mut plan = preferred_semi_join(JoinEdge {
                left_column: 1,
                right_column: 2,
                null_equal: false,
            });
            if let JoinNode::Join {
                other_conditions, ..
            } = &mut plan.node
            {
                other_conditions.push(Expression::default());
            }
            plan
        },
    ];

    for plan in cases {
        let (optimized, changed) = CorrelateSolver.Optimize(plan).expect("safe skip");
        assert!(!changed);
        assert!(matches!(optimized.node, JoinNode::Join { .. }));
    }
}

#[test]
fn existing_apply_is_not_recorrelated() {
    let plan = JoinPlan {
        id: 3,
        node: JoinNode::Apply {
            join_type: JoinType::Semi,
            left: Box::new(leaf(1, vec![1])),
            right: Box::new(leaf(2, vec![2])),
            correlated_columns: vec![1],
            no_decorrelate: false,
        },
        schema: vec![1],
        row_count: 1.0,
    };

    let (optimized, changed) = CorrelateSolver.Optimize(plan).expect("skip apply");
    assert!(!changed);
    let JoinNode::Apply { right, .. } = optimized.node else {
        panic!("apply is preserved");
    };
    let JoinNode::Leaf { predicates, .. } = right.node else {
        panic!("inner leaf is preserved");
    };
    assert!(predicates.is_empty());
}
