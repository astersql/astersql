// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// Schema 单元测试。
//
// 覆盖列/键的克隆与字符串化、前缀列索引、唯一性判定、列组抽取与 MergeSchema。

use crate::*;

/// 构造仅含 UniqueID 的测试列。
fn column(unique_id: i64) -> Column {
    Column::new(
        *types::NewFieldType(mysql::TypeLonglong),
        unique_id,
        unique_id,
        unique_id as isize - 1,
    )
}

/// 生成连续 UniqueID 的测试 Schema。
fn schema(first: i64, count: i64) -> Schema {
    NewSchema((first..first + count).map(column).collect())
}

#[test]
/// 验证 Clone/String 保留列与键，且底层存储独立。
fn clone_and_string_preserve_columns_keys_and_independent_storage() {
    let mut schema = schema(1, 5);
    assert_eq!(
        schema.String(),
        "Column: [Column#1,Column#2,Column#3,Column#4,Column#5] PKOrUK: [] NullableUK: []"
    );
    schema.SetKeys(vec![vec![column(1)], vec![column(2)]]);
    schema.SetUniqueKeys(vec![vec![column(3)]]);
    let cloned = schema.Clone();
    assert_eq!(cloned.String(), schema.String());
    assert_ne!(cloned.Columns.as_ptr(), schema.Columns.as_ptr());
    assert_ne!(cloned.PKOrUK.as_ptr(), schema.PKOrUK.as_ptr());
    assert_ne!(cloned.NullableUK.as_ptr(), schema.NullableUK.as_ptr());
}

#[test]
/// 验证 ColumnIndex 优先完整列，并覆盖缺失列与前缀列。
fn retrieve_contains_and_column_index_cover_missing_and_prefix_columns() {
    let mut prefix = column(1);
    prefix.IsPrefix = true;
    let full = column(1);
    let schema = NewSchema(vec![prefix, full, column(2)]);
    assert_eq!(schema.ColumnIndex(&column(1)), Some(1));
    assert_eq!(schema.RetrieveColumn(&column(1)).unwrap().UniqueID, 1);
    assert!(schema.Contains(&column(2)));
    assert!(!schema.Contains(&column(99)));
    assert!(schema.RetrieveColumn(&column(99)).is_none());
}

#[test]
/// 验证 strong/nullable 唯一键集合与 ColumnsIndices。
fn unique_keys_and_column_indices_follow_strong_and_nullable_sets() {
    let mut schema = schema(1, 5);
    schema.SetKeys(vec![vec![column(1)], vec![column(2), column(3)]]);
    schema.SetUniqueKeys(vec![vec![column(4)]]);
    assert!(schema.IsUnique(true, &[column(1)]));
    assert!(schema.IsUnique(true, &[column(3), column(2), column(5)]));
    assert!(!schema.IsUnique(true, &[column(4)]));
    assert!(schema.IsUnique(false, &[column(4)]));
    assert_eq!(
        schema.ColumnsIndices(&[column(1), column(3), column(5)]),
        Some(vec![0, 2, 4])
    );
    assert_eq!(schema.ColumnsIndices(&[column(1), column(99)]), None);
}

#[test]
/// 验证按偏移取列与列组抽取（跳过缺失组）。
fn columns_by_indices_and_group_extraction_preserve_order_and_skip_missing_groups() {
    let schema = schema(1, 5);
    let selected = schema.ColumnsByIndices(&[3, 1, 3]);
    assert_eq!(
        selected
            .iter()
            .map(|column| column.UniqueID)
            .collect::<Vec<_>>(),
        vec![4, 2, 4]
    );
    let (groups, offsets) = schema.ExtractColGroups(&[
        vec![column(1), column(2)],
        vec![column(99)],
        vec![column(5)],
    ]);
    assert_eq!(groups, vec![vec![0, 1], vec![4]]);
    assert_eq!(offsets, vec![0, 2]);
}

#[test]
/// 验证 MergeSchema 在空/单侧/双侧下的列顺序，且不合并键。
fn merge_schema_covers_none_one_side_and_both_side_column_order() {
    let left = schema(1, 3);
    let right = schema(4, 2);
    assert!(MergeSchema(None, None).is_none());
    assert_eq!(
        MergeSchema(Some(&left), None).unwrap().String(),
        left.String()
    );
    assert_eq!(
        MergeSchema(None, Some(&right)).unwrap().String(),
        right.String()
    );
    let merged = MergeSchema(Some(&left), Some(&right)).unwrap();
    assert_eq!(
        merged
            .Columns
            .iter()
            .map(|column| column.UniqueID)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5]
    );
    assert!(merged.PKOrUK.is_empty());
    assert!(merged.NullableUK.is_empty());
}

#[test]
/// 对齐 Go TestGetUsedList：忽略模式外列和重复输入，并按 schema 顺序返回命中位。
fn get_used_list_matches_go_for_external_and_duplicate_columns() {
    let schema = schema(1, 5);
    let used_columns = vec![column(4), column(6), column(7), column(2), column(4)];

    assert_eq!(
        GetUsedList(
            &exprstatic::NewEvalContext(Vec::new()),
            used_columns,
            &schema,
        ),
        vec![false, true, false, true, false]
    );
}

#[test]
/// 覆盖 Go GetExtraHandleColumn 的末列、倒数第二列与缺失分支。
fn extra_handle_is_found_in_the_two_go_supported_positions() {
    let mut last = schema(1, 3);
    last.Columns[2].ID = model::ExtraHandleID;
    assert_eq!(last.GetExtraHandleColumn().unwrap().UniqueID, 3);

    let mut penultimate = schema(1, 3);
    penultimate.Columns[1].ID = model::ExtraHandleID;
    assert_eq!(penultimate.GetExtraHandleColumn().unwrap().UniqueID, 2);

    assert!(schema(1, 3).GetExtraHandleColumn().is_none());
}
