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

// Chunk 基础追加与空值（NULL）保留行为的单元测试。
//
// 验证按列追加普通值与 NULL 后，行数与按行读取结果正确。
// NULL 在列式布局中由 nullBitmap 标记，不依赖数据槽本身。

/// 断言 Chunk 追加整数与 NULL 后，按行读取能区分空值与有效值。
#[test]
fn chunk_append_preserves_null_and_value_rows() {
    use super::{NewChunkWithCapacity, mysql, types};
    let fields = vec![*types::NewFieldType(mysql::TypeLonglong)];
    let mut chunk = NewChunkWithCapacity(fields, 3);
    // 交错追加有效值与 NULL，覆盖 nullBitmap 与取值路径。
    chunk.AppendInt64(0, 5);
    chunk.AppendNull(0);
    chunk.AppendInt64(0, 9);
    assert_eq!(chunk.NumRows(), 3);
    assert_eq!(chunk.GetRow(0).GetInt64(0), 5);
    assert!(chunk.GetRow(1).IsNull(0));
    assert_eq!(chunk.GetRow(2).GetInt64(0), 9);
}

#[test]
fn go_merge_11_used_memory_excludes_retained_capacity() {
    use super::{NewChunkWithCapacity, mysql, types};
    let mut chunk = NewChunkWithCapacity(vec![*types::NewFieldType(mysql::TypeLonglong)], 8);
    let initial = chunk.UsedMemoryUsage();
    assert!(initial < chunk.MemoryUsage());
    chunk.AppendInt64(0, 42);
    assert!(chunk.UsedMemoryUsage() > initial);
    let allocated = chunk.MemoryUsage();
    chunk.Reset();
    assert_eq!(chunk.UsedMemoryUsage(), initial);
    assert_eq!(chunk.MemoryUsage(), allocated);
}

/// 覆盖 Go TestAppendChunk/TestTruncateTo/TestCopyTo/TestAppendSel 的核心断言。
#[test]
fn append_truncate_copy_and_selection_match_go() {
    use super::{NewChunkWithCapacity, mysql, types};

    let fields = vec![
        *types::NewFieldType(mysql::TypeLonglong),
        *types::NewFieldType(mysql::TypeVarchar),
    ];
    let mut source = NewChunkWithCapacity(fields.clone(), 4);
    source.AppendInt64(0, 1);
    source.AppendString(1, "a");
    source.AppendNull(0);
    source.AppendString(1, "bb");
    source.AppendInt64(0, 3);
    source.AppendNull(1);

    let mut appended = NewChunkWithCapacity(fields.clone(), 8);
    appended.Append(&source, 0, 3);
    appended.Append(&source, 0, 3);
    assert_eq!(appended.NumRows(), 6);
    assert!(appended.GetRow(1).IsNull(0));
    assert_eq!(appended.GetRow(4).GetString(1), "bb");

    appended.TruncateTo(5);
    assert_eq!(appended.NumRows(), 5);
    assert_eq!(appended.GetRow(4).GetString(1), "bb");
    let copied = appended.CopyConstruct();
    assert_eq!(copied.NumRows(), 5);
    assert!(copied.GetRow(1).IsNull(0));

    source.SetSel(Some(vec![2, 0]));
    assert_eq!(source.NumRows(), 2);
    assert_eq!(source.GetRow(0).GetInt64(0), 3);
    source.AppendInt64(0, 4);
    source.AppendString(1, "d");
    assert_eq!(source.Sel().unwrap(), &[2, 0, 3]);
    let selected = source.CopyConstructSel();
    assert_eq!(selected.NumRows(), 3);
    assert_eq!(selected.GetRow(0).GetInt64(0), 3);
    assert_eq!(selected.GetRow(1).GetInt64(0), 1);
    assert_eq!(selected.GetRow(2).GetString(1), "d");
}

/// 覆盖 Go TestChunkSizeControl、投影追加与虚拟行计数。
#[test]
fn required_rows_projection_and_virtual_rows_match_go() {
    use super::mutrow::{GoAny, MutRowFromValues};
    use super::{NewChunkWithCapacity, mysql, types};

    let fields = vec![*types::NewFieldType(mysql::TypeLonglong)];
    let mut controlled = NewChunkWithCapacity(fields.clone(), 4);
    controlled.SetRequiredRows(2, 4);
    controlled.AppendInt64(0, 1);
    assert!(!controlled.IsFull());
    controlled.AppendInt64(0, 2);
    assert!(controlled.IsFull());
    controlled.SetRequiredRows(99, 4);
    assert_eq!(controlled.RequiredRows(), 4);
    assert!(!controlled.IsFull());
    controlled.AppendInt64(0, 3);
    controlled.AppendInt64(0, 4);
    assert!(controlled.IsFull());
    controlled.GrowAndReset(8);
    assert_eq!(controlled.RequiredRows(), 8);
    assert_eq!(controlled.NumRows(), 0);

    let source = MutRowFromValues(vec![
        GoAny::Int64(0),
        GoAny::Int64(1),
        GoAny::Int64(2),
        GoAny::Int64(3),
    ]);
    let two_fields = vec![
        *types::NewFieldType(mysql::TypeLonglong),
        *types::NewFieldType(mysql::TypeLonglong),
    ];
    let mut projected = NewChunkWithCapacity(two_fields, 2);
    assert_eq!(
        projected.AppendRowByColIdxs(source.ToRow(), Some(&[3, 1])),
        2
    );
    assert_eq!(projected.GetRow(0).GetInt64(0), 3);
    assert_eq!(projected.GetRow(0).GetInt64(1), 1);

    let mut virtual_chunk = NewChunkWithCapacity(Vec::<types::FieldType>::new(), 2);
    assert_eq!(
        virtual_chunk.AppendRowByColIdxs(source.ToRow(), Some(&[])),
        0
    );
    assert_eq!(virtual_chunk.GetNumVirtualRows(), 1);
    assert_eq!(virtual_chunk.NumRows(), 1);
}

/// Go distinguishes a nil column slice from an initialized empty slice: a
/// zero-column chunk created by `New` still participates in renew/grow logic.
#[test]
fn initialized_zero_column_chunks_renew_and_grow_like_go() {
    use super::{New, types};

    let mut chunk = New(Vec::<types::FieldType>::new(), 2, 8);
    let cloned = chunk.CloneEmpty(6);
    assert_eq!(cloned.Capacity(), 6);
    assert_eq!(cloned.RequiredRows(), 6);

    chunk.SetNumVirtualRows(2);
    chunk.GrowAndReset(8);
    assert_eq!(chunk.Capacity(), 4);
    assert_eq!(chunk.RequiredRows(), 8);
    assert_eq!(chunk.NumRows(), 0);
}

/// 覆盖 Go TestSwapColumn/TestMakeRefTo/TestToString 的引用与格式化语义。
#[test]
fn column_references_swaps_and_formatting_match_go() {
    use super::{NewChunkWithCapacity, mysql, types};

    let float_fields = vec![
        *types::NewFieldType(mysql::TypeFloat),
        *types::NewFieldType(mysql::TypeFloat),
    ];
    let mut left = NewChunkWithCapacity(float_fields.clone(), 1);
    left.AppendFloat32(0, 1.0);
    left.MakeRef(0, 1);
    assert!(left.Column(0).same_ref(left.Column(1)));

    let mut right = NewChunkWithCapacity(float_fields, 1);
    right.AppendFloat32(0, 9.0);
    right.MakeRef(0, 1);
    left.swapColumn(0, &mut right, 0).unwrap();
    assert!(left.Column(0).same_ref(left.Column(1)));
    assert!(right.Column(0).same_ref(right.Column(1)));
    assert_eq!(left.GetRow(0).GetFloat32(0), 9.0);
    assert_eq!(right.GetRow(0).GetFloat32(0), 1.0);

    let fields = vec![
        *types::NewFieldType(mysql::TypeLonglong),
        *types::NewFieldType(mysql::TypeVarchar),
    ];
    let mut formatted = NewChunkWithCapacity(fields.clone(), 2);
    formatted.AppendInt64(0, 42);
    formatted.AppendString(1, "aster");
    formatted.AppendInt64(0, -1);
    formatted.AppendNull(1);
    assert_eq!(formatted.ToString(fields), "42, aster\n-1, NULL\n");
}
