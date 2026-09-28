// Copyright 2026 AsterSQL.

use crate::rule_join_reorder::{JoinEdge, JoinNode, JoinPlan, extractJoinGroup};
use crate::rule_join_reorder_projection_inline::{
    canInlineProjection, canInlineProjectionBasic, tryInlineProjectionForJoinGroup,
};
use crate::task::{Expression, JoinType};

fn leaf(id: usize, column: usize) -> JoinPlan {
    JoinPlan {
        id,
        node: JoinNode::Leaf {
            name: format!("t{id}"),
            predicates: Vec::new(),
            unique_keys: Vec::new(),
            correlated_columns: Vec::new(),
        },
        schema: vec![column],
        row_count: 1.0,
    }
}

fn join() -> JoinPlan {
    JoinPlan {
        id: 3,
        node: JoinNode::Join {
            join_type: JoinType::Inner,
            left: Box::new(leaf(1, 10)),
            right: Box::new(leaf(2, 20)),
            equal_conditions: vec![JoinEdge {
                left_column: 10,
                right_column: 20,
                null_equal: false,
            }],
            other_conditions: Vec::new(),
            preferred_method: None,
        },
        schema: vec![10, 20],
        row_count: 1.0,
    }
}

fn projection(expressions: Vec<Expression>, schema: Vec<usize>) -> JoinPlan {
    JoinPlan {
        id: 4,
        node: JoinNode::Projection {
            expressions,
            child: Box::new(join()),
        },
        schema,
        row_count: 1.0,
    }
}

fn column(column: usize) -> Expression {
    Expression {
        name: format!("col_{column}"),
        column: Some(column),
        ..Default::default()
    }
}

#[test]
fn basic_gate_rejects_constant_only_expressions() {
    let plan = projection(
        vec![Expression {
            name: "constant_1".to_owned(),
            ..Default::default()
        }],
        vec![100],
    );
    assert!(!canInlineProjectionBasic(&plan));
}

#[test]
fn same_leaf_columns_may_be_projected_more_than_once() {
    let plan = projection(vec![column(10), column(10)], vec![100, 101]);
    let JoinNode::Projection { child, .. } = &plan.node else {
        unreachable!();
    };
    assert!(canInlineProjection(&plan, &extractJoinGroup(child)));
}

#[test]
fn unknown_columns_are_unsafe_and_fall_back_to_an_atomic_leaf() {
    let plan = projection(vec![column(999)], vec![100]);
    let JoinNode::Projection { child, .. } = &plan.node else {
        unreachable!();
    };
    assert!(!canInlineProjection(&plan, &extractJoinGroup(child)));

    let (result, handled) = tryInlineProjectionForJoinGroup(&plan);
    assert!(handled);
    let result = result.expect("unsafe projection must become one atomic leaf");
    assert_eq!(result.group.joinNodePlans.len(), 1);
    assert_eq!(result.group.joinNodePlans[0].id, plan.id);
}
