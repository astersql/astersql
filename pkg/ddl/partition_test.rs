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
