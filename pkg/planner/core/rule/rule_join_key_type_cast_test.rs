// Copyright 2026 AsterSQL.

use std::collections::BTreeMap;

use super::rule_init::{Expr, FieldType, JoinType, LogicalRule, Plan, PlanKind};
use super::rule_join_key_type_cast::JoinKeyTypeCast;

fn column(id: i64, field_type: FieldType) -> Expr {
    Expr::Column { id, field_type }
}

fn source(schema: Vec<i64>) -> Plan {
    Plan {
        kind: PlanKind::Other,
        schema,
        children: vec![],
        predicates: vec![],
        keys: vec![],
        estimated_rows: 1.0,
        used_stats: BTreeMap::new(),
    }
}

fn projection(output_id: i64, input_id: i64, input_type: FieldType) -> Plan {
    Plan {
        kind: PlanKind::Projection {
            expressions: vec![Expr::Cast {
                expr: Box::new(column(input_id, input_type)),
                target: FieldType::Float,
            }],
        },
        schema: vec![output_id],
        children: vec![source(vec![input_id])],
        predicates: vec![],
        keys: vec![],
        estimated_rows: 1.0,
        used_stats: BTreeMap::new(),
    }
}

fn join(join_type: JoinType) -> Plan {
    let text = FieldType::Text {
        charset: "utf8mb4".into(),
        collation: "utf8mb4_bin".into(),
    };
    Plan {
        kind: PlanKind::Join {
            join_type,
            equal_conditions: vec![Expr::Scalar {
                function: "eq".into(),
                args: vec![column(10, FieldType::Float), column(20, FieldType::Float)],
                field_type: FieldType::Bool,
            }],
            other_conditions: vec![],
        },
        schema: vec![10, 20],
        children: vec![
            projection(10, 1, FieldType::SignedInt),
            projection(20, 2, text),
        ],
        predicates: vec![],
        keys: vec![],
        estimated_rows: 1.0,
        used_stats: BTreeMap::new(),
    }
}

#[test]
fn rewrites_projected_signed_int_and_text_keys_with_guard() {
    let (optimized, changed) = JoinKeyTypeCast.optimize(join(JoinType::Inner)).unwrap();
    assert!(changed);

    let PlanKind::Join {
        equal_conditions, ..
    } = &optimized.kind
    else {
        panic!("expected join")
    };
    let Expr::Scalar { args, .. } = &equal_conditions[0] else {
        panic!("expected equality")
    };
    assert_eq!(args[0], column(1, FieldType::SignedInt));
    assert!(matches!(
        args[1],
        Expr::Column {
            field_type: FieldType::SignedInt,
            ..
        }
    ));

    let PlanKind::Projection { expressions } = &optimized.children[0].kind else {
        panic!("expected left projection")
    };
    assert_eq!(expressions.last(), Some(&column(1, FieldType::SignedInt)));
    let PlanKind::Projection { expressions } = &optimized.children[1].kind else {
        panic!("expected right projection")
    };
    assert!(matches!(
        expressions.last(),
        Some(Expr::Cast {
            target: FieldType::SignedInt,
            ..
        })
    ));
    assert!(matches!(
        optimized.children[1].children[0].kind,
        PlanKind::Selection
    ));
    assert_eq!(optimized.children[1].children[0].predicates.len(), 1);
}

#[test]
fn skips_when_text_is_on_preserved_outer_side() {
    let mut plan = join(JoinType::LeftOuter);
    plan.children.swap(0, 1);
    if let PlanKind::Join {
        equal_conditions, ..
    } = &mut plan.kind
    {
        if let Expr::Scalar { args, .. } = &mut equal_conditions[0] {
            args.swap(0, 1);
        }
    }
    let (optimized, changed) = JoinKeyTypeCast.optimize(plan.clone()).unwrap();
    assert!(!changed);
    assert_eq!(optimized, plan);
}

#[test]
fn does_not_rewrite_unprojected_mixed_numeric_keys() {
    let mut plan = join(JoinType::Inner);
    plan.children = vec![source(vec![10]), source(vec![20])];
    if let PlanKind::Join {
        equal_conditions, ..
    } = &mut plan.kind
    {
        *equal_conditions = vec![Expr::Scalar {
            function: "eq".into(),
            args: vec![
                column(10, FieldType::SignedInt),
                column(20, FieldType::Float),
            ],
            field_type: FieldType::Bool,
        }];
    }
    let (optimized, changed) = JoinKeyTypeCast.optimize(plan.clone()).unwrap();
    assert!(!changed);
    assert_eq!(optimized, plan);
}
