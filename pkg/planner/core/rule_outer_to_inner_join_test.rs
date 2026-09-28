// Copyright 2026 AsterSQL.

use crate::rule_join_reorder::{JoinNode, JoinPlan};
use crate::rule_outer_to_inner_join::ConvertOuterToInnerJoin;
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

fn predicate(name: &str, column: usize) -> Expression {
    Expression {
        name: name.into(),
        column: Some(column),
        ..Expression::default()
    }
}

fn join(join_type: JoinType, left: JoinPlan, right: JoinPlan, on: Vec<Expression>) -> JoinPlan {
    JoinPlan {
        id: 10,
        schema: left.schema.iter().chain(&right.schema).copied().collect(),
        row_count: 1.0,
        node: JoinNode::Join {
            join_type,
            left: Box::new(left),
            right: Box::new(right),
            equal_conditions: Vec::new(),
            other_conditions: on,
            preferred_method: None,
        },
    }
}

#[test]
fn optimize_matches_go_name_and_changed_contract() {
    let selection = JoinPlan {
        id: 11,
        schema: vec![1, 2],
        row_count: 1.0,
        node: JoinNode::Selection {
            conditions: vec![predicate("gt", 2)],
            child: Box::new(join(
                JoinType::LeftOuter,
                leaf(1, 1),
                leaf(2, 2),
                Vec::new(),
            )),
        },
    };

    let (result, changed) = ConvertOuterToInnerJoin.Optimize(selection).unwrap();
    assert!(!changed, "Go rule deliberately reports planChanged=false");
    assert_eq!(
        ConvertOuterToInnerJoin.Name(),
        "convert_outer_to_inner_joins"
    );
    let JoinNode::Selection { child, .. } = result.node else {
        panic!("selection must remain root")
    };
    assert!(matches!(
        child.node,
        JoinNode::Join {
            join_type: JoinType::Inner,
            ..
        }
    ));
}

#[test]
fn outer_join_on_clause_reaches_only_null_producing_child() {
    let nested = join(JoinType::LeftOuter, leaf(3, 2), leaf(4, 3), Vec::new());
    let root = join(
        JoinType::LeftOuter,
        leaf(1, 1),
        nested,
        vec![predicate("on_gt", 3)],
    );

    let (result, _) = ConvertOuterToInnerJoin.Optimize(root).unwrap();
    let JoinNode::Join { right, .. } = result.node else {
        panic!("root must remain join")
    };
    assert!(matches!(
        right.node,
        JoinNode::Join {
            join_type: JoinType::Inner,
            ..
        }
    ));
}
