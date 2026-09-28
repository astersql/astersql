// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// Chunk 工具函数（按 selection 拷贝行）的单元测试。
//
// selection 向量用 bool 标记哪些物理行应被拷贝到目标列；
// Join/过滤等算子常用该路径只传递命中行。

/// 断言 CopySelectedRows 只拷贝 selected 为 true 的行。
#[test]
fn selected_rows_copy_only_requested_positions() {
    use super::{CopySelectedRows, newFixedLenColumn};
    let mut source = newFixedLenColumn(8, 3);
    source.AppendInt64(10);
    source.AppendInt64(20);
    source.AppendInt64(30);
    let mut destination = newFixedLenColumn(8, 2);
    // 中间行被过滤，目标列应只含 10 与 30。
    CopySelectedRows(&mut destination, &source, &[true, false, true]);
    assert_eq!(destination.Int64s(), vec![10, 30]);
}

/// 覆盖 Go TestCopySelectedJoinRowsDirect 及 selection-vector 错误分支。
#[test]
fn selected_join_rows_copy_fixed_variable_null_and_virtual_rows() {
    use super::{CopySelectedJoinRowsDirect, NewChunkWithCapacity, mysql, types};

    let fields = vec![
        *types::NewFieldType(mysql::TypeLonglong),
        *types::NewFieldType(mysql::TypeVarchar),
    ];
    let mut source = NewChunkWithCapacity(fields.clone(), 3);
    source.AppendInt64(0, 1);
    source.AppendString(1, "a");
    source.AppendNull(0);
    source.AppendString(1, "b");
    source.AppendInt64(0, 3);
    source.AppendNull(1);
    let mut destination = NewChunkWithCapacity(fields, 3);
    assert!(CopySelectedJoinRowsDirect(&source, &[true, false, true], &mut destination,).unwrap());
    assert_eq!(destination.NumRows(), 2);
    assert_eq!(destination.GetRow(0).GetInt64(0), 1);
    assert_eq!(destination.GetRow(1).GetInt64(0), 3);
    assert!(destination.GetRow(1).IsNull(1));

    source.SetSel(Some(vec![0]));
    assert!(
        CopySelectedJoinRowsDirect(&source, &[true], &mut destination)
            .unwrap_err()
            .to_string()
            .contains("selection vector")
    );

    let mut source = NewChunkWithCapacity(Vec::<types::FieldType>::new(), 3);
    source.SetNumVirtualRows(3);
    let mut destination = NewChunkWithCapacity(Vec::<types::FieldType>::new(), 3);
    assert!(CopySelectedJoinRowsDirect(&source, &[true, false, true], &mut destination,).unwrap());
    assert_eq!(destination.NumRows(), 2);
}

/// 覆盖 Go TestCopySelectedJoinRows 的同 outer-row 批量复制路径。
#[test]
fn selected_join_rows_with_same_outer_rows_match_go() {
    use super::{CopySelectedJoinRowsWithSameOuterRows, NewChunkWithCapacity, mysql, types};

    let fields = vec![
        *types::NewFieldType(mysql::TypeLonglong),
        *types::NewFieldType(mysql::TypeVarchar),
    ];
    let mut source = NewChunkWithCapacity(fields.clone(), 3);
    for inner in [10, 20, 30] {
        source.AppendInt64(0, inner);
        source.AppendString(1, "outer");
    }
    let mut destination = NewChunkWithCapacity(fields, 3);
    assert!(
        CopySelectedJoinRowsWithSameOuterRows(
            &source,
            0,
            1,
            1,
            1,
            &[false, true, true],
            &mut destination,
        )
        .unwrap()
    );
    assert_eq!(destination.Column(0).Int64s(), vec![20, 30]);
    assert_eq!(destination.GetRow(0).GetString(1), "outer");
    assert_eq!(destination.GetRow(1).GetString(1), "outer");
}

/// 覆盖 Go TestCopySelectedVirtualNum 的零列、零 inner 列与零 outer 列路径。
#[test]
fn selected_virtual_rows_match_go_edge_cases() {
    use super::{
        CopySelectedJoinRowsDirect, CopySelectedJoinRowsWithSameOuterRows, NewChunkWithCapacity,
        mysql, types,
    };

    let mut source = NewChunkWithCapacity(Vec::<types::FieldType>::new(), 3);
    source.SetNumVirtualRows(3);
    let mut destination = NewChunkWithCapacity(Vec::<types::FieldType>::new(), 3);
    assert!(CopySelectedJoinRowsDirect(&source, &[true, false, true], &mut destination).unwrap());
    assert_eq!(destination.NumRows(), 2);

    let fields = vec![*types::NewFieldType(mysql::TypeLonglong)];
    let mut source = NewChunkWithCapacity(fields.clone(), 3);
    for value in [3, 3, 3] {
        source.AppendInt64(0, value);
    }
    let mut destination = NewChunkWithCapacity(fields, 3);
    assert!(
        CopySelectedJoinRowsWithSameOuterRows(
            &source,
            1,
            0,
            0,
            1,
            &[true, false, true],
            &mut destination,
        )
        .unwrap()
    );
    assert_eq!(destination.NumRows(), 2);
    assert_eq!(destination.Column(0).Int64s(), vec![3, 3]);
}

/// 覆盖 Go TestMergeInputIdxToOutputIdxes：同源输入列必须合并后再交换。
#[test]
fn column_swap_merges_referred_input_columns_like_go() {
    use super::{ColumnSwapHelper, NewChunkWithCapacity, mysql, types};
    use std::collections::HashMap;

    let mut mappings = HashMap::new();
    mappings.insert(0, vec![0, 1]);
    mappings.insert(1, vec![2, 3]);
    let helper = ColumnSwapHelper::New(mappings);

    let input_fields = vec![
        *types::NewFieldType(mysql::TypeLonglong),
        *types::NewFieldType(mysql::TypeLonglong),
    ];
    let mut input = NewChunkWithCapacity(input_fields, 1);
    input.AppendInt64(0, 99);
    input.MakeRef(0, 1);

    let output_fields = vec![*types::NewFieldType(mysql::TypeLonglong); 4];
    let mut output = NewChunkWithCapacity(output_fields, 1);
    helper.SwapColumns(&mut input, &mut output).unwrap();

    for idx in 1..4 {
        assert!(output.Column(0).same_ref(output.Column(idx)));
    }
    assert_eq!(output.GetRow(0).GetInt64(0), 99);
    let merged = helper.mergedInputIdxToOutputIdxes.Load();
    assert!(!merged.is_null());
    let merged = unsafe { &*merged };
    assert_eq!(merged.len(), 1);
    let mut output_indexes = merged.get(&0).unwrap().clone();
    output_indexes.sort_unstable();
    assert_eq!(output_indexes, vec![0, 1, 2, 3]);
}
