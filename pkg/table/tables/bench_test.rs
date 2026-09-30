// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 表记录批量操作的基准语义回归测试。
//
// Go 版本分别测量流水线 DML 下的新增、更新和删除性能；这里使用轻量内存表
// 分别复现三个单批工作负载，覆盖基准依赖的记录、聚簇句柄和更新列掩码语义。

use crate::index::{ColumnInfo, SchemaState, TableInfo};
use crate::mutation_checker::Datum;
use crate::tables::{Column, TableCommon};

/// 与 Go 流水线 DML 基准保持一致的单批记录数。
const BATCH_SIZE: usize = 5_000;

/// 构造与 Go 基准相同的两列表：聚簇整数主键 `a` 和字符串列 `b`。
fn benchmark_table() -> TableCommon {
    let primary_key = ColumnInfo {
        id: 1,
        name: "a".to_owned(),
        needs_restored_data: false,
        field_type: 8, // MySQL BIGINT
        collation: String::new(),
    };
    let value = ColumnInfo {
        id: 2,
        name: "b".to_owned(),
        needs_restored_data: false,
        field_type: 253, // MySQL VARSTRING
        collation: "utf8mb4_bin".to_owned(),
    };
    TableCommon::new(
        TableInfo {
            id: 1,
            columns: vec![primary_key.clone(), value.clone()],
        },
        1,
        vec![
            Column {
                info: primary_key,
                offset: 0,
                state: SchemaState::Public,
                hidden: false,
                generated: false,
                generated_stored: false,
                primary_key: true,
                common_handle: false,
                default_value: None,
                origin_default_value: None,
            },
            Column {
                info: value,
                offset: 1,
                state: SchemaState::Public,
                hidden: false,
                generated: false,
                generated_stored: false,
                primary_key: false,
                common_handle: false,
                default_value: None,
                origin_default_value: None,
            },
        ],
        vec![],
        vec![],
        false,
    )
    .unwrap()
}

fn record(value: usize, text: &str) -> Vec<Datum> {
    vec![
        Datum::Int(value as i64),
        Datum::Bytes(text.as_bytes().to_vec()),
    ]
}

#[test]
fn add_record_pipelined_dml_batch_preserves_records_and_clustered_handles() {
    let mut table = benchmark_table();
    for value in 0..BATCH_SIZE {
        assert_eq!(
            table.add_record(record(value, "test"), Some(value as i64)),
            Ok(value as i64)
        );
    }
    assert_eq!(table.iter_records().count(), BATCH_SIZE);
    assert_eq!(
        table.row_with_columns(0, &[0, 1]).unwrap(),
        record(0, "test")
    );
    assert_eq!(
        table
            .row_with_columns(BATCH_SIZE as i64 - 1, &[0, 1])
            .unwrap(),
        record(BATCH_SIZE - 1, "test")
    );
}

#[test]
fn remove_record_pipelined_dml_batch_removes_every_clustered_handle() {
    let mut table = benchmark_table();
    for value in 0..BATCH_SIZE {
        table
            .add_record(record(value, "test"), Some(value as i64))
            .unwrap();
    }
    for value in 0..BATCH_SIZE {
        table
            .remove_record(value as i64, &record(value, "test"))
            .unwrap();
    }
    assert_eq!(table.iter_records().count(), 0);
}

#[test]
fn update_record_pipelined_dml_batch_only_touches_value_column() {
    let mut table = benchmark_table();
    for value in 0..BATCH_SIZE {
        table
            .add_record(record(value, "test"), Some(value as i64))
            .unwrap();
    }
    for value in 0..BATCH_SIZE {
        table
            .update_record(
                value as i64,
                &record(value, "test"),
                record(value, "updated"),
                &[false, true],
            )
            .unwrap();
    }
    assert_eq!(
        table
            .row_with_columns(BATCH_SIZE as i64 - 1, &[0, 1])
            .unwrap(),
        record(BATCH_SIZE - 1, "updated")
    );
}
