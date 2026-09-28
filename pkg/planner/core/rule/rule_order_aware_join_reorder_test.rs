// Copyright 2026 AsterSQL.

use super::rule_init::{Expr, FieldType, JoinType, LogicalRule, Plan, PlanKind};
use super::rule_order_aware_join_reorder::OrderAwareJoinReorder;
use std::collections::BTreeMap;

fn column(id: i64) -> Expr {
    Expr::Column {
        id,
        field_type: FieldType::SignedInt,
    }
}

fn source(column_id: i64, estimated_rows: f64) -> Plan {
    Plan {
        kind: PlanKind::DataSource {
            table_id: column_id,
            indexes: BTreeMap::from([(1, vec![column_id])]),
            partition: None,
            selected_partitions: None,
        },
        schema: vec![column_id],
        children: Vec::new(),
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows,
        used_stats: BTreeMap::new(),
    }
}

#[test]
fn projection_rewrites_order_column_before_choosing_leading_join_input() {
    let join = Plan {
        kind: PlanKind::Join {
            join_type: JoinType::Inner,
            equal_conditions: Vec::new(),
            other_conditions: Vec::new(),
        },
        schema: vec![1, 2],
        children: vec![source(2, 1.0), source(1, 100.0)],
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 10.0,
        used_stats: BTreeMap::new(),
    };
    let projection = Plan {
        kind: PlanKind::Projection {
            expressions: vec![column(1), column(2)],
        },
        schema: vec![101, 102],
        children: vec![join],
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 10.0,
        used_stats: BTreeMap::new(),
    };
    let plan = Plan {
        kind: PlanKind::Sort {
            by: vec![column(101)],
        },
        schema: vec![101, 102],
        children: vec![projection],
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 10.0,
        used_stats: BTreeMap::new(),
    };

    let (optimized, changed) = OrderAwareJoinReorder.optimize(plan).unwrap();
    assert!(changed);
    let join = &optimized.children[0].children[0];
    assert_eq!(join.children[0].schema, vec![1]);
}
