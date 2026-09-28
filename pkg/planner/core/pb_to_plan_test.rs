// Copyright 2026 AsterSQL.

use super::{NewPBPlanBuilder, PBColumnInfo, PBExecutor, PBTableInfo, PlanKind, PlannerContext};

fn table(name: &str) -> PBTableInfo {
    PBTableInfo {
        id: 1,
        database: "information_schema".to_owned(),
        name: name.to_owned(),
        cluster_table: true,
        columns: vec![
            PBColumnInfo {
                id: 1,
                name: "a".to_owned(),
                field_type: "longlong".to_owned(),
            },
            PBColumnInfo {
                id: 2,
                name: "b".to_owned(),
                field_type: "varchar".to_owned(),
            },
        ],
    }
}

#[test]
fn table_scan_schema_preserves_duplicate_requested_columns_in_table_order() {
    let table = table("cluster_config");
    let builder = NewPBPlanBuilder(PlannerContext::default(), vec![table.clone()], vec![]);
    let requested = vec![
        table.columns[1].clone(),
        table.columns[0].clone(),
        table.columns[0].clone(),
    ];

    assert_eq!(
        builder.buildTableScanSchema(&table, &requested),
        vec!["a".to_owned(), "a".to_owned(), "b".to_owned()]
    );
}

#[test]
fn selection_is_not_pushed_through_projection() {
    let mut builder = NewPBPlanBuilder(
        PlannerContext::default(),
        vec![table("cluster_slow_query")],
        vec![],
    );
    let plan = builder
        .Build(&[
            PBExecutor::TableScan {
                table_id: 1,
                columns: vec![1],
                desc: false,
            },
            PBExecutor::Projection {
                expressions: vec!["a".to_owned()],
            },
            PBExecutor::Selection {
                conditions: vec!["a = 1".to_owned()],
            },
        ])
        .unwrap();

    assert!(matches!(plan.kind, PlanKind::Selection { .. }));
    assert!(matches!(plan.children[0].kind, PlanKind::Projection));
    assert!(
        !plan.children[0].children[0]
            .operator_info
            .contains("pushed:[a = 1]")
    );
}

#[test]
fn selection_remains_for_cluster_table_without_predicate_extractor() {
    let mut builder = NewPBPlanBuilder(
        PlannerContext::default(),
        vec![table("cluster_config")],
        vec![],
    );
    let plan = builder
        .Build(&[
            PBExecutor::TableScan {
                table_id: 1,
                columns: vec![1],
                desc: false,
            },
            PBExecutor::Selection {
                conditions: vec!["a = 1".to_owned()],
            },
        ])
        .unwrap();

    assert!(matches!(plan.kind, PlanKind::Selection { .. }));
    assert_eq!(plan.children.len(), 1);
}

#[test]
fn broadcast_query_rejects_non_whitelisted_statements() {
    let mut builder = NewPBPlanBuilder(PlannerContext::default(), vec![], vec![]);
    let error = builder
        .Build(&[PBExecutor::BroadcastQuery {
            query: "select 1".to_owned(),
        }])
        .unwrap_err();

    assert_eq!(error, "unexpected statement select 1 in broadcast query");
}
