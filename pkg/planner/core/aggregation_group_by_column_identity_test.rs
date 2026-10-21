// Copyright 2026 AsterSQL.

use super::{
    NewPBPlanBuilder, PBAggregateFunction, PBColumnInfo, PBColumnRef, PBExecutor, PBTableInfo,
    PlannerContext,
};

fn table() -> PBTableInfo {
    PBTableInfo {
        id: 1,
        database: "information_schema".to_owned(),
        name: "cluster_tidb_statements_stats".to_owned(),
        cluster_table: true,
        columns: ["digest", "index_names", "exec_count", "last_seen"]
            .into_iter()
            .enumerate()
            .map(|(offset, name)| PBColumnInfo {
                id: offset as i64 + 1,
                name: name.to_owned(),
                field_type: "value".to_owned(),
                unique_id: offset as i64 + 11,
            })
            .collect(),
    }
}

#[test]
fn aggregation_group_by_column_identity() {
    for stream in [false, true] {
        for filtered in [false, true] {
            let mut executors = vec![PBExecutor::TableScan {
                table_id: 1,
                columns: vec![1, 2, 3, 4],
                desc: false,
            }];
            if filtered {
                executors.push(PBExecutor::Selection {
                    conditions: vec!["last_seen >= 2000-01-01".to_owned()],
                });
            }
            executors.push(PBExecutor::Aggregation {
                functions: vec![
                    PBAggregateFunction::new("sum", PBColumnRef::new(2)),
                    PBAggregateFunction::new("max", PBColumnRef::new(1)),
                    PBAggregateFunction::new("first", PBColumnRef::new(0)),
                ],
                group_by: vec![PBColumnRef::new(0)],
                stream,
            });

            let mut builder = NewPBPlanBuilder(PlannerContext::default(), vec![table()], vec![]);
            builder.Build(&executors).unwrap();
            let aggregation = builder.last_aggregation().unwrap();

            assert_eq!(aggregation.group_by[0].unique_id, Some(11));
            assert_eq!(aggregation.functions[0].argument.unique_id, Some(13));
            assert_eq!(aggregation.functions[1].argument.unique_id, Some(12));
            assert_eq!(aggregation.functions[2].argument.unique_id, Some(11));
            assert_eq!(
                aggregation.group_by[0].unique_id, aggregation.functions[2].argument.unique_id,
                "the GROUP BY digest must retain the input digest identity"
            );
            assert_ne!(
                aggregation.group_by[0].unique_id, aggregation.functions[1].argument.unique_id,
                "same-position aggregate output must not replace the GROUP BY input identity"
            );
        }
    }
}
