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

// Row Table Builder 单元测试。
//
// 覆盖 join key 序列化与分区、过滤行丢弃/保留、恢复 chunk 分区形状校验、
// 最大元素长度检查，以及非 2 的幂分区数拒绝。

use crate::join_table_meta::{FieldKind, FieldType, new_table_meta};
use crate::row_table_builder::{
    Chunk, RowTableBuilder, Value, calculate_fake_length, calculate_row_data_length,
};

/// 构造可空/非空的有符号整型字段类型。
fn int_type(nullable: bool) -> FieldType {
    FieldType {
        kind: FieldKind::SignedInt,
        fixed_length: Some(8),
        nullable,
    }
}

/// 构造可空/非空的文本字段类型（utf8mb4_bin）。
fn text_type(nullable: bool) -> FieldType {
    FieldType {
        kind: FieldKind::Text {
            collation: "utf8mb4_bin".into(),
        },
        fixed_length: None,
        nullable,
    }
}

/// 构造含 Int + Text 两列的 Join 表元数据。
fn meta(need_used_flag: bool) -> crate::join_table_meta::JoinTableMeta {
    let types = vec![int_type(true), text_type(true)];
    new_table_meta(
        &[0],
        &types,
        &[types[0].clone()],
        &[types[0].clone()],
        &[1],
        &[0, 1],
        need_used_flag,
    )
    .unwrap()
}

/// 含有效 key、NULL key、NULL 文本列的三行测试 chunk。
fn chunk() -> Chunk {
    vec![
        vec![Value::Int(1), Value::Text("one".into())],
        vec![Value::Null, Value::Text("null".into())],
        vec![Value::Int(3), Value::Null],
    ]
}

/// 序列化 key、分区写入后行字节按 8 对齐，有效 key 计数正确。
#[test]
fn row_table_builder_serializes_keys_partitions_and_aligns_rows() {
    let metadata = meta(true);
    let mut builder = RowTableBuilder::new(vec![0], 4, true, false, true, 1).unwrap();
    let table = builder.process_chunk(&chunk(), &metadata, None, 0).unwrap();
    assert_eq!(table.row_count(), 3);
    assert_eq!(table.valid_key_count(), 2);
    assert_eq!(builder.serialized_keys.len(), 3);
    assert_eq!(builder.partition_indices.len(), 3);
    assert!(
        table
            .segments()
            .iter()
            .flat_map(|segment| &segment.rows)
            .all(|row| row.bytes.len() % 8 == 0)
    );
}

/// `keep_filtered_rows` 控制过滤失败行是丢弃还是保留（valid_keys 标记）。
#[test]
fn row_table_builder_filter_can_drop_or_preserve_invalid_rows() {
    let metadata = meta(false);
    let filter = [true, false, true];
    let mut dropping = RowTableBuilder::new(vec![0], 2, true, true, false, 1).unwrap();
    let dropped = dropping
        .process_chunk(&chunk(), &metadata, Some(&filter), 0)
        .unwrap();
    assert_eq!(dropped.row_count(), 2);
    assert_eq!(dropped.valid_key_count(), 2);

    let mut keeping = RowTableBuilder::new(vec![0], 2, true, true, true, 1).unwrap();
    let kept = keeping
        .process_chunk(&chunk(), &metadata, Some(&filter), 0)
        .unwrap();
    assert_eq!(kept.row_count(), 3);
    assert_eq!(kept.valid_key_count(), 2);
    assert_eq!(keeping.valid_keys, vec![true, false, true]);
}

/// Go 对保留的过滤行使用递增假 hash，使它们在所有分区间轮询分布。
#[test]
fn filtered_rows_are_balanced_across_partitions_like_go() {
    let metadata = meta(false);
    let rows = (0..8)
        .map(|value| vec![Value::Int(value), Value::Text(value.to_string())])
        .collect::<Vec<_>>();
    let filter = [false; 8];
    let mut builder = RowTableBuilder::new(vec![0], 4, false, true, true, 1).unwrap();

    let table = builder
        .process_chunk(&rows, &metadata, Some(&filter), 0)
        .unwrap();

    assert_eq!(builder.hash_values, vec![0, 1, 2, 3, 0, 1, 2, 3]);
    assert_eq!(builder.partition_indices, vec![0, 1, 2, 3, 0, 1, 2, 3]);
    assert_eq!(
        table
            .segments()
            .iter()
            .map(|segment| segment.rows.len())
            .collect::<Vec<_>>(),
        vec![2, 2, 2, 2]
    );
}

/// Null map 的 bit 下标来自 row table 中的保存列顺序，而不是原 schema 下标。
#[test]
fn sparse_saved_columns_use_compact_null_map_positions() {
    let types = (0..10).map(|_| int_type(true)).collect::<Vec<_>>();
    let metadata = new_table_meta(
        &[0],
        &types,
        &[types[0].clone()],
        &[types[0].clone()],
        &[],
        &[9],
        false,
    )
    .unwrap();
    let mut row = vec![Value::Int(0); 10];
    row[9] = Value::Null;
    let mut builder = RowTableBuilder::new(vec![0], 1, false, false, false, 1).unwrap();

    let table = builder
        .process_chunk(&vec![row], &metadata, None, 0)
        .unwrap();
    let encoded = &table.segments()[0].rows[0];

    let saved_position = metadata
        .row_columns_order
        .iter()
        .position(|column| *column == 9)
        .unwrap();
    assert!(metadata.is_column_null(encoded, saved_position));
}

/// 恢复 chunk 时分区数不匹配应报错；匹配后可重新计算分区下标。
#[test]
fn restored_chunk_checks_partition_shape_and_regenerates_partition() {
    let metadata = meta(false);
    let mut builder = RowTableBuilder::new(vec![0], 4, true, false, true, 1).unwrap();
    assert!(
        builder
            .process_restored_chunk(&chunk(), &metadata, 2)
            .is_err()
    );
    let restored = builder
        .process_restored_chunk(&chunk(), &metadata, 4)
        .unwrap();
    assert_eq!(restored.row_count(), 3);
    let (hash, partition) = builder.regenerate_hash_and_partition(0xf0, 4).unwrap();
    assert_eq!(hash, 0xf0);
    assert_eq!(partition, 3);
}

/// 最大元素长度检查与 8 字节假填充对齐一致。
#[test]
fn row_table_builder_reports_maximum_element_size() {
    let metadata = meta(false);
    let builder = RowTableBuilder::new(vec![0], 1, true, false, true, 1).unwrap();
    let rows = chunk();
    let (fits, maximum) = builder.check_max_element_size(&rows, &metadata, usize::MAX);
    assert!(fits);
    assert!(maximum >= calculate_row_data_length(&metadata, &rows[0]));
    assert!(
        !builder
            .check_max_element_size(&rows, &metadata, maximum - 1)
            .0
    );
    assert_eq!((maximum + calculate_fake_length(maximum)) % 8, 0);
}

/// 分区数必须为非零 2 的幂，否则构造失败。
#[test]
fn row_table_builder_rejects_non_power_of_two_partition_count() {
    assert!(RowTableBuilder::new(vec![0], 0, false, false, false, 0).is_err());
    assert!(RowTableBuilder::new(vec![0], 3, false, false, false, 0).is_err());
}

#[test]
/// Rows with fewer columns than the metadata schema fail before producing a partial table.
fn row_table_builder_rejects_short_rows_without_partial_state() {
    let metadata = meta(false);
    let mut builder = RowTableBuilder::new(vec![0], 2, true, false, true, 1).unwrap();
    let short = vec![vec![Value::Int(1)]];
    assert!(builder.process_chunk(&short, &metadata, None, 0).is_err());
    assert_eq!(builder.hash_values.len(), 1);
    assert_eq!(builder.valid_keys.len(), 1);
}
