// Copyright 2026 AsterSQL.

use std::collections::BTreeMap;

use super::rule_constant_propagation::ConstantPropagationSolver;
use super::rule_init::{Expr, FieldType, JoinType, LogicalRule, Plan, PlanKind, Value};

fn plan(kind: PlanKind, schema: &[i64], children: Vec<Plan>, predicates: Vec<Expr>) -> Plan {
    Plan {
        kind,
        schema: schema.to_vec(),
        children,
        predicates,
        keys: Vec::new(),
        estimated_rows: 1.0,
        used_stats: BTreeMap::new(),
    }
}

fn column(id: i64) -> Expr {
    Expr::Column {
        id,
        field_type: FieldType::SignedInt,
    }
}

fn comparison(function: &str, id: i64, value: i64) -> Expr {
    Expr::Scalar {
        function: function.to_owned(),
        args: vec![column(id), Expr::Constant(Value::Int(value))],
        field_type: FieldType::Bool,
    }
}

fn source(id: i64) -> Plan {
    plan(
        PlanKind::DataSource {
            table_id: id,
            indexes: BTreeMap::new(),
            partition: None,
            selected_partitions: None,
        },
        &[id],
        Vec::new(),
        Vec::new(),
    )
}

#[test]
fn inner_join_pulls_comparison_above_join_and_preserves_go_changed_flag() {
    let predicate = comparison("gt", 1, 1);
    let left = plan(
        PlanKind::Selection,
        &[1],
        vec![source(1)],
        vec![predicate.clone()],
    );
    let join = plan(
        PlanKind::Join {
            join_type: JoinType::Inner,
            equal_conditions: vec![comparison("eq", 1, 2)],
            other_conditions: Vec::new(),
        },
        &[1, 2],
        vec![left, source(2)],
        Vec::new(),
    );

    let rule = ConstantPropagationSolver;
    let (optimized, changed) = rule.optimize(join).unwrap();

    assert_eq!(rule.name(), "constant_propagation");
    assert!(
        !changed,
        "Go ConstantPropagationSolver always reports false"
    );
    assert!(matches!(optimized.kind, PlanKind::Selection));
    assert_eq!(optimized.predicates, vec![predicate]);
    assert!(matches!(optimized.children[0].kind, PlanKind::Join { .. }));
}

#[test]
fn outer_join_only_pulls_from_preserved_side_and_unsupported_join_is_unchanged() {
    let left_predicate = comparison("ge", 1, 10);
    let right_predicate = comparison("lt", 2, 20);
    let selected_left = plan(
        PlanKind::Selection,
        &[1],
        vec![source(1)],
        vec![left_predicate.clone()],
    );
    let selected_right = plan(
        PlanKind::Selection,
        &[2],
        vec![source(2)],
        vec![right_predicate],
    );

    let left_join = plan(
        PlanKind::Join {
            join_type: JoinType::LeftOuter,
            equal_conditions: Vec::new(),
            other_conditions: Vec::new(),
        },
        &[1, 2],
        vec![selected_left.clone(), selected_right.clone()],
        Vec::new(),
    );
    let semi_join = plan(
        PlanKind::Join {
            join_type: JoinType::Semi,
            equal_conditions: Vec::new(),
            other_conditions: Vec::new(),
        },
        &[1],
        vec![selected_left, selected_right],
        Vec::new(),
    );

    let (optimized_left, _) = ConstantPropagationSolver.optimize(left_join).unwrap();
    let (optimized_semi, _) = ConstantPropagationSolver
        .optimize(semi_join.clone())
        .unwrap();

    assert_eq!(optimized_left.predicates, vec![left_predicate]);
    assert_eq!(optimized_semi, semi_join);
}

#[test]
fn projection_remaps_candidate_column_and_non_go_comparison_is_rejected() {
    let selection = plan(
        PlanKind::Selection,
        &[1],
        vec![source(1)],
        vec![comparison("eq", 1, 7), comparison("ne", 1, 8)],
    );
    let projection = plan(
        PlanKind::Projection {
            expressions: vec![column(1)],
        },
        &[11],
        vec![selection],
        Vec::new(),
    );
    let join = plan(
        PlanKind::Join {
            join_type: JoinType::Inner,
            equal_conditions: Vec::new(),
            other_conditions: Vec::new(),
        },
        &[11, 2],
        vec![projection, source(2)],
        Vec::new(),
    );

    let (optimized, _) = ConstantPropagationSolver.optimize(join).unwrap();

    assert_eq!(optimized.predicates, vec![comparison("eq", 11, 7)]);
}
