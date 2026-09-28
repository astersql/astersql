// Copyright 2026 AsterSQL.

use std::collections::BTreeMap;

use super::rule_column_pruning::ColumnPruner;
use super::rule_init::{LogicalRule, Plan, PlanKind};

fn plan(kind: PlanKind, schema: &[i64], children: Vec<Plan>) -> Plan {
    Plan {
        kind,
        schema: schema.to_vec(),
        children,
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 1.0,
        used_stats: BTreeMap::new(),
    }
}

#[test]
fn column_pruner_preserves_go_rule_contract() {
    let child = plan(
        PlanKind::DataSource {
            table_id: 1,
            indexes: BTreeMap::new(),
            partition: None,
            selected_partitions: None,
        },
        &[1, 2],
        Vec::new(),
    );
    let root = plan(PlanKind::Selection, &[1], vec![child]);

    let rule = ColumnPruner;
    let (optimized, plan_changed) = rule.optimize(root).unwrap();

    assert_eq!(rule.name(), "column_prune");
    assert_eq!(optimized.children[0].schema, vec![1]);
    assert!(!plan_changed, "Go ColumnPruner always reports false");
}
