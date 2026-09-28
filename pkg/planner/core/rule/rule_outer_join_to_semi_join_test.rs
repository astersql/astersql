// Copyright 2026 AsterSQL.

use std::collections::BTreeMap;

use super::rule_init::{Expr, FieldType, JoinType, LogicalRule, Plan, PlanKind};
use super::rule_outer_join_to_semi_join::OuterJoinToSemiJoin;

fn column(id: i64) -> Expr {
    Expr::Column {
        id,
        field_type: FieldType::SignedInt,
    }
}

fn source(schema: &[i64]) -> Plan {
    Plan {
        kind: PlanKind::DataSource {
            table_id: schema.first().copied().unwrap_or_default(),
            indexes: BTreeMap::new(),
            partition: None,
            selected_partitions: None,
        },
        schema: schema.to_vec(),
        children: Vec::new(),
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 1.0,
        used_stats: BTreeMap::new(),
    }
}

fn selection(join: Plan, inner_column: i64) -> Plan {
    Plan {
        kind: PlanKind::Selection,
        schema: join.schema.clone(),
        children: vec![join],
        predicates: vec![Expr::Scalar {
            function: "is_null".into(),
            args: vec![column(inner_column)],
            field_type: FieldType::Bool,
        }],
        keys: Vec::new(),
        estimated_rows: 1.0,
        used_stats: BTreeMap::new(),
    }
}

#[test]
fn equal_condition_must_be_column_op_column() {
    let malformed_equality = Expr::Scalar {
        function: "eq".into(),
        args: vec![
            Expr::Scalar {
                function: "plus".into(),
                args: vec![column(2), Expr::Constant(super::rule_init::Value::Int(1))],
                field_type: FieldType::SignedInt,
            },
            column(1),
        ],
        field_type: FieldType::Bool,
    };
    let join = Plan {
        kind: PlanKind::Join {
            join_type: JoinType::LeftOuter,
            equal_conditions: vec![malformed_equality],
            other_conditions: Vec::new(),
        },
        schema: vec![1, 2],
        children: vec![source(&[1]), source(&[2])],
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 1.0,
        used_stats: BTreeMap::new(),
    };
    let plan = selection(join, 2);

    let (optimized, changed) = OuterJoinToSemiJoin.optimize(plan.clone()).unwrap();
    assert!(!changed);
    assert_eq!(optimized, plan);
}

#[test]
fn right_outer_join_swaps_children_and_equality_arguments() {
    let join = Plan {
        kind: PlanKind::Join {
            join_type: JoinType::RightOuter,
            equal_conditions: vec![Expr::Scalar {
                function: "eq".into(),
                args: vec![column(2), column(1)],
                field_type: FieldType::Bool,
            }],
            other_conditions: Vec::new(),
        },
        schema: vec![2, 1],
        children: vec![source(&[2]), source(&[1])],
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 1.0,
        used_stats: BTreeMap::new(),
    };

    let (optimized, changed) = OuterJoinToSemiJoin.optimize(selection(join, 2)).unwrap();
    assert!(changed);
    let rewritten = &optimized.children[0];
    assert_eq!(rewritten.children[0].schema, vec![1]);
    assert_eq!(rewritten.children[1].schema, vec![2]);
    let PlanKind::Join {
        join_type,
        equal_conditions,
        ..
    } = &rewritten.kind
    else {
        panic!("expected rewritten join");
    };
    assert_eq!(*join_type, JoinType::AntiSemi);
    assert_eq!(
        equal_conditions[0],
        Expr::Scalar {
            function: "eq".into(),
            args: vec![column(1), column(2)],
            field_type: FieldType::Bool,
        }
    );
}
