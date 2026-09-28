// Copyright 2026 AsterSQL.

use crate::rule_join_reorder::{JoinEdge, JoinNode, JoinPlan};
use crate::rule_semi_join_rewrite::SemiJoinRewriter;
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
        row_count: 10.0,
    }
}

fn join(
    id: usize,
    join_type: JoinType,
    left: JoinPlan,
    right: JoinPlan,
    equal_conditions: Vec<JoinEdge>,
    other_conditions: Vec<Expression>,
) -> JoinPlan {
    let mut schema = left.schema.clone();
    schema.extend(right.schema.iter().copied());
    JoinPlan {
        id,
        node: JoinNode::Join {
            join_type,
            left: Box::new(left),
            right: Box::new(right),
            equal_conditions,
            other_conditions,
            preferred_method: Some("hash".to_owned()),
        },
        schema,
        row_count: 5.0,
    }
}

#[test]
fn semi_join_rewrite_matches_go_plan_shape_and_change_contract() {
    let semi = join(
        3,
        JoinType::Semi,
        leaf(1, vec![1]),
        leaf(2, vec![2, 3]),
        vec![JoinEdge {
            left_column: 1,
            right_column: 2,
            null_equal: false,
        }],
        Vec::new(),
    );

    let (result, changed) = SemiJoinRewriter.Optimize(semi).unwrap();
    assert!(!changed, "Go Optimize deliberately keeps planChanged false");
    assert_eq!(result.schema, vec![1]);
    let JoinNode::Projection { expressions, child } = result.node else {
        panic!("Go rewrite must restore the outer schema with a projection");
    };
    assert_eq!(expressions.len(), 1);
    assert_eq!(expressions[0].column, Some(1));
    let JoinNode::Join {
        join_type,
        right,
        preferred_method,
        ..
    } = child.node
    else {
        panic!("projection child must be the replacement inner join");
    };
    assert_eq!(join_type, JoinType::Inner);
    assert_eq!(preferred_method.as_deref(), Some("hash"));
    let JoinNode::Aggregation {
        group_by, child, ..
    } = right.node
    else {
        panic!("inner side must be deduplicated before the inner join");
    };
    assert_eq!(group_by, vec![2]);
    assert_eq!(child.schema, vec![2, 3]);
}

#[test]
fn inapplicable_parent_still_recurses_into_children() {
    let inner_semi = join(
        3,
        JoinType::Semi,
        leaf(1, vec![1]),
        leaf(2, vec![2]),
        Vec::new(),
        Vec::new(),
    );
    let blocked_parent = join(
        5,
        JoinType::Semi,
        inner_semi,
        leaf(4, vec![4]),
        Vec::new(),
        vec![Expression::default()],
    );

    let (result, changed) = SemiJoinRewriter.Optimize(blocked_parent).unwrap();
    assert!(!changed);
    let JoinNode::Join { left, .. } = result.node else {
        panic!("semi join with other conditions must remain unchanged");
    };
    assert!(matches!(left.node, JoinNode::Projection { .. }));
}
