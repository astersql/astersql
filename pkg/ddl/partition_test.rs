// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 分区 DDL 相关测试。
//

use crate::executor::{
    ColumnInfo, Executor, ExecutorError, Ident, MemoryJobBackend, OnExist, PartitionDefinition,
    SessionContext, TableInfo,
};
use std::time::Duration;

use crate::partition::{
    PartitionDefinition as ModelPartitionDefinition, PartitionInfo, PartitionType, PartitionValue,
    build_check_condition_for_list, build_check_condition_for_range,
};

fn model_partition(
    name: &str,
    less_than: Vec<PartitionValue>,
    in_values: Vec<Vec<PartitionValue>>,
) -> ModelPartitionDefinition {
    ModelPartitionDefinition {
        name: name.to_owned(),
        less_than,
        in_values,
        ..ModelPartitionDefinition::default()
    }
}

#[test]
fn exchange_range_validation_selects_rows_outside_the_partition_like_go() {
    let info = PartitionInfo {
        partition_type: PartitionType::Range,
        expression: "a".to_owned(),
        definitions: vec![
            model_partition("p0", vec![PartitionValue::Int(10)], vec![]),
            model_partition("p1", vec![PartitionValue::Int(20)], vec![]),
            model_partition("pmax", vec![PartitionValue::MaxValue], vec![]),
        ],
        ..PartitionInfo::default()
    };

    assert_eq!(
        ("a >= ?".to_owned(), vec![PartitionValue::Int(10)]),
        build_check_condition_for_range(&info, 0).unwrap()
    );
    assert_eq!(
        (
            "a < ? OR a >= ? OR a IS NULL".to_owned(),
            vec![PartitionValue::Int(10), PartitionValue::Int(20)],
        ),
        build_check_condition_for_range(&info, 1).unwrap()
    );
    assert_eq!(
        (
            "a < ? OR a IS NULL".to_owned(),
            vec![PartitionValue::Int(20)],
        ),
        build_check_condition_for_range(&info, 2).unwrap()
    );
}

#[test]
fn exchange_list_validation_uses_null_safe_non_membership_like_go() {
    let info = PartitionInfo {
        partition_type: PartitionType::List,
        expression: "a".to_owned(),
        definitions: vec![model_partition(
            "p0",
            vec![],
            vec![vec![PartitionValue::Int(1)], vec![PartitionValue::Null]],
        )],
        ..PartitionInfo::default()
    };

    assert_eq!(
        "NOT ((a) <=> 1 OR (a) <=> NULL)",
        build_check_condition_for_list(&info, 0).unwrap()
    );
}

/// 加分区后禁止一次删光全部；删掉尾分区后再与普通表 exchange，应成功。
#[test]
fn partition_add_drop_and_exchange_preserve_physical_ids() {
    let mut ddl = Executor::new(MemoryJobBackend::default(), Duration::ZERO);
    let mut session = SessionContext::default();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    ddl.create_table(
        &mut session,
        "test",
        TableInfo::new("p", vec![ColumnInfo::integer("a")]),
        OnExist::Error,
    )
    .unwrap();
    ddl.create_table(
        &mut session,
        "test",
        TableInfo::new("normal", vec![ColumnInfo::integer("a")]),
        OnExist::Error,
    )
    .unwrap();
    let partitioned = Ident::new("test", "p");
    ddl.add_partitions(
        &mut session,
        &partitioned,
        vec![
            PartitionDefinition::new("p0", vec!["10".into()]),
            PartitionDefinition::new("p1", vec!["MAXVALUE".into()]),
        ],
    )
    .unwrap();
    assert!(matches!(
        ddl.drop_partitions(&mut session, &partitioned, &["p0".into(), "p1".into()]),
        Err(ExecutorError::InvalidPartition(_))
    ));
    let removed = ddl
        .drop_partitions(&mut session, &partitioned, &["p1".into()])
        .unwrap();
    assert_eq!(1, removed.len());
    ddl.exchange_partition(
        &mut session,
        &partitioned,
        "p0",
        &Ident::new("test", "normal"),
    )
    .unwrap();
}

#[test]
fn port_9453792437_staging_keeps_table_order_and_full_definitions() {
    use astersql_meta_model::{PartitionDefinition, PartitionInfo, TableInfo};
    use astersql_parser_ast::NewCIStr;
    let defs: Vec<_> = (0..3)
        .map(|i| PartitionDefinition {
            ID: i + 10,
            Name: NewCIStr(&format!("p{i}")),
            InValues: vec![vec![format!("{}", i + 1)]],
            ..Default::default()
        })
        .collect();
    let mut table = TableInfo {
        Partition: Some(PartitionInfo {
            Definitions: defs.clone(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let requested = PartitionInfo {
        Definitions: vec![PartitionDefinition {
            ID: 99,
            Name: NewCIStr("new"),
            InValues: vec![vec!["1".into()], vec!["3".into()]],
            Comment: "retain the full definition".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    crate::partition::stage_reorganize_partition_definitions(
        &mut table,
        &["p2".into(), "p0".into()],
        &requested,
    )
    .unwrap();
    let pi = table.Partition.as_ref().unwrap();
    assert_eq!(
        serde_json::to_value(&pi.Definitions).unwrap(),
        serde_json::to_value(&defs).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&pi.DroppingDefinitions).unwrap(),
        serde_json::to_value([&defs[0], &defs[2]]).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&pi.AddingDefinitions).unwrap(),
        serde_json::to_value(&requested.Definitions).unwrap()
    );
    let before = serde_json::to_value(&table).unwrap();
    assert!(
        crate::partition::stage_reorganize_partition_definitions(
            &mut table,
            &["absent".into()],
            &requested
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&table).unwrap(), before);
}

#[test]
fn port_9453792437_retired_indexes_keep_cleanup_ids_and_adjust_hidden_offsets() {
    use astersql_meta_model::{ColumnInfo, IndexColumn, IndexInfo, SchemaState as S, TableInfo};
    use astersql_parser_ast::NewCIStr;
    let column = |name: &str, offset, hidden| ColumnInfo {
        Name: NewCIStr(name),
        Offset: offset,
        Hidden: hidden,
        ..Default::default()
    };
    let index = |id, offset, hidden: &str, global, state| IndexInfo {
        ID: id,
        Name: NewCIStr(&format!("i{id}")),
        Global: global,
        State: state,
        Columns: vec![IndexColumn {
            Name: NewCIStr(hidden),
            Offset: offset,
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut table = TableInfo {
        ID: 42,
        Columns: vec![
            column("a", 0, false),
            column("h1", 1, true),
            column("h2", 2, true),
        ],
        Indices: vec![
            index(1, 1, "h1", true, S::DeleteOnly),
            index(2, 2, "h2", false, S::DeleteOnly),
            index(3, 0, "a", false, S::Public),
        ],
        ..Default::default()
    };
    let mut args = astersql_meta_model::group_2::TablePartitionArgs::default();
    crate::partition::remove_replaced_partition_indexes(&mut table, &mut args);
    assert_eq!(table.Columns.len(), 1);
    assert_eq!(table.Columns[0].Name.L, "a");
    assert_eq!(table.Indices.len(), 1);
    assert_eq!(table.Indices[0].ID, 3);
    assert_eq!(table.Indices[0].Columns[0].Offset, 0);
    assert_eq!(args.OldGlobalIndexes.len(), 1);
    assert_eq!(args.OldGlobalIndexes[0].TableID, 42);
    assert_eq!(args.OldGlobalIndexes[0].IndexID, 1);
}
