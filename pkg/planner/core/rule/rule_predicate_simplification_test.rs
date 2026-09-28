// Copyright 2026 AsterSQL.

use super::rule_init::{Expr, FieldType, LogicalRule, Plan, PlanKind, Value};
use super::rule_predicate_simplification::{PredicateSimplification, simplify_cnf};
use std::collections::BTreeMap;

fn scalar(function: &str, args: Vec<Expr>) -> Expr {
    Expr::Scalar {
        function: function.to_owned(),
        args,
        field_type: FieldType::Bool,
    }
}

fn empty_plan(predicates: Vec<Expr>) -> Plan {
    Plan {
        kind: PlanKind::Selection,
        schema: Vec::new(),
        children: Vec::new(),
        predicates,
        keys: Vec::new(),
        estimated_rows: 0.0,
        used_stats: BTreeMap::new(),
    }
}

#[test]
fn optimize_keeps_go_rule_changed_flag_false() {
    let plan = empty_plan(vec![Expr::Constant(Value::Bool(true))]);
    let (optimized, changed) = PredicateSimplification.optimize(plan).unwrap();

    assert!(optimized.predicates.is_empty());
    assert!(
        !changed,
        "Go PredicateSimplification always reports planChanged=false"
    );
}

#[test]
fn mutable_or_duplicates_are_preserved_like_go() {
    let random = scalar("rand", Vec::new());
    let predicate = scalar("or", vec![random.clone(), random.clone()]);

    assert_eq!(simplify_cnf(vec![predicate.clone()]), vec![predicate]);
}

#[test]
fn sql_nullable_self_comparisons_are_not_boolean_folded() {
    let null_equality = scalar(
        "eq",
        vec![Expr::Constant(Value::Null), Expr::Constant(Value::Null)],
    );
    let column = Expr::Column {
        id: 1,
        field_type: FieldType::SignedInt,
    };
    let column_inequality = scalar("ne", vec![column.clone(), column]);

    assert_eq!(
        simplify_cnf(vec![null_equality.clone(), column_inequality.clone()]),
        vec![null_equality, column_inequality]
    );
}
