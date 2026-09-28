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

// Iterator / List / MutRow / Pool / RowContainerReader 的 Aster 单元测试集。
//
// 覆盖切片游标边界、跨 Chunk 的 List/RowPtr 迭代、MultiIterator 过滤空输入、
// List Reset/Clear 的内存记账、MutRow 多种类型布局，以及 Reader 流式读取与错误传播。

use super::iterator::{
    Iterator, NewIterator4Chunk, NewIterator4List, NewIterator4RowPtr, NewIterator4Slice,
    NewMultiIterator,
};
use super::mutrow::{GoAny, MutRowFromDatums, MutRowFromValues};
use super::pool::NewPool;
use super::row_container_reader::{NewRowContainerReader, RowContainerReader, RowContainerSource};
use super::types::{self, FieldType};
use super::{Chunk, ChunkError, List, RowContainer};
use std::sync::Arc;

/// 构造 int64（Longlong）字段类型。
fn int_field() -> FieldType {
    *types::NewFieldType(super::mysql::TypeLonglong)
}
/// 用给定值填充单列 int Chunk。
fn int_chunk(values: &[i64], capacity: usize) -> Box<Chunk> {
    let mut chunk = super::New(vec![int_field()], capacity, capacity);
    for value in values {
        chunk.AppendInt64(0, *value);
    }
    chunk
}

/// 从 `Begin` 遍历到 `End`，收集首列 int64。
fn collect(iter: &mut dyn Iterator) -> Vec<i64> {
    let mut rows = Vec::new();
    let mut row = iter.Begin();
    while row != iter.End() {
        rows.push(row.GetInt64(0));
        row = iter.Next();
    }
    rows
}

/// 验证切片迭代器的 Current/Begin/Next/ReachEnd 游标边界与 Go 一致。
#[test]
fn iterator_slice_preserves_go_cursor_boundaries() {
    let source = int_chunk(&[10, 20, 30], 3);
    let rows = (0..3).map(|idx| source.GetRow(idx)).collect();
    let mut iter = NewIterator4Slice(rows);
    assert!(iter.Current().IsEmpty());
    assert_eq!(iter.Begin().GetInt64(0), 10);
    assert_eq!(iter.Current().GetInt64(0), 10);
    assert_eq!(iter.Next().GetInt64(0), 20);
    iter.ReachEnd();
    assert!(iter.Current().IsEmpty());
    assert_eq!(iter.Begin().GetInt64(0), 10);
    assert_eq!(collect(iter.as_mut()), vec![10, 20, 30]);
}

/// List 与 RowPtr 迭代器应跨多个小 Chunk 正确遍历全部行。
#[test]
fn list_and_row_ptr_iterators_cross_chunk_boundaries() {
    let fields = vec![int_field()];
    let source = int_chunk(&[1, 2, 3, 4, 5], 5);
    let mut list = List::New(fields, 2, 2);
    let pointers: Vec<_> = (0..5)
        .map(|idx| list.AppendRow(source.GetRow(idx)))
        .collect();
    assert_eq!(list.NumChunks(), 3);
    assert_eq!(list.Len(), 5);

    let mut list_iter = NewIterator4List(list);
    assert_eq!(collect(list_iter.as_mut()), vec![1, 2, 3, 4, 5]);

    let mut second = List::New(vec![int_field()], 2, 2);
    let second_ptrs: Vec<_> = (0..5)
        .map(|idx| second.AppendRow(source.GetRow(idx)))
        .collect();
    let mut ptr_iter = NewIterator4RowPtr(second, second_ptrs);
    assert_eq!(collect(ptr_iter.as_mut()), vec![1, 2, 3, 4, 5]);
    assert_eq!(pointers[4].ChkIdx, 2);
}

/// MultiIterator 丢弃空子迭代器，但仍按非空输入累计 Len 与遍历顺序。
#[test]
fn multi_iterator_filters_empty_inputs_and_keeps_length() {
    let empty = NewIterator4Chunk(Box::default());
    let first = NewIterator4Chunk(int_chunk(&[1, 2], 2));
    let second = NewIterator4Chunk(int_chunk(&[3, 4], 2));
    let mut iter = NewMultiIterator(vec![empty, first, second]);
    assert_eq!(iter.Len(), 4);
    assert_eq!(collect(iter.as_mut()), vec![1, 2, 3, 4]);
    assert!(iter.Error().is_none());
}

/// Reset 回收 Chunk 到 freelist 且 Tracker 仍保留用量；Clear 后用量归零。
#[test]
fn list_reset_reuses_chunks_and_memory_accounting_matches_go() {
    let source = int_chunk(&[7, 8, 9], 3);
    let mut list = List::New(vec![int_field()], 2, 2);
    for idx in 0..3 {
        list.AppendRow(source.GetRow(idx));
    }
    assert_eq!(list.NumChunks(), 2);
    list.Reset();
    assert_eq!(list.NumChunks(), 0);
    assert_eq!(list.Len(), 0);
    assert!(list.GetMemTracker().BytesConsumed() > 0);
    for idx in 0..3 {
        list.AppendRow(source.GetRow(idx));
    }
    assert_eq!(list.NumChunks(), 2);
    list.Clear();
    assert_eq!(list.GetMemTracker().BytesConsumed(), 0);
}

/// MutRow 从 Values/Datums 构造、SetValue/Clone 互不干扰，行为对齐 Go。
#[test]
fn mutrow_values_datums_nulls_and_clone_match_go() {
    let mut row = MutRowFromValues(vec![
        GoAny::String("abc".into()),
        GoAny::Int64(123),
        GoAny::Nil,
    ]);
    assert_eq!(row.ToRow().GetBytes(0), b"abc");
    assert_eq!(row.ToRow().GetInt64(1), 123);
    assert!(row.ToRow().IsNull(2));

    row.SetValues(vec![
        GoAny::String("longer".into()),
        GoAny::Int64(456),
        GoAny::Bytes(vec![1, 2]),
    ]);
    assert_eq!(row.ToRow().GetBytes(0), b"longer");
    assert_eq!(row.ToRow().GetInt64(1), 456);
    assert_eq!(row.ToRow().GetBytes(2), vec![1, 2]);

    let mut clone = row.Clone();
    clone.SetValue(0, GoAny::String("clone".into()));
    assert_eq!(row.ToRow().GetBytes(0), b"longer");
    assert_eq!(clone.ToRow().GetBytes(0), b"clone");

    let datums = vec![
        types::NewIntDatum(-7),
        types::NewBytesDatum(b"datum".to_vec()),
    ];
    let datum_row = MutRowFromDatums(datums);
    assert_eq!(datum_row.ToRow().GetInt64(0), -7);
    assert_eq!(datum_row.ToRow().GetBytes(1), b"datum");
}

/// JSON/Enum/Set/Duration/Vector 在 MutRow 中的字节布局与 Go 序列化一致。
#[test]
fn mutrow_preserves_json_enum_set_duration_and_vector_layouts() {
    let json = types::BinaryJSON {
        TypeCode: 3,
        Value: vec![9, 8],
    };
    let vector = types::ParseVectorFloat32("[1.5,-2]").unwrap();
    let row = MutRowFromValues(vec![
        GoAny::BinaryJSON(json),
        GoAny::Enum(types::Enum {
            Name: "yes".into(),
            Value: 4,
        }),
        GoAny::Set(types::Set {
            Name: "a,b".into(),
            Value: 3,
        }),
        GoAny::Duration(types::Duration {
            Duration: 1234,
            Fsp: types::DefaultFsp,
        }),
        GoAny::VectorFloat32(vector.Clone()),
    ]);
    assert_eq!(row.ToRow().GetBytes(0), vec![3, 9, 8]);
    assert_eq!(&row.ToRow().GetBytes(1)[..8], &4_u64.to_le_bytes());
    assert_eq!(&row.ToRow().GetBytes(2)[8..], b"a,b");
    assert_eq!(row.ToRow().GetInt64(3), 1234);
    assert_eq!(row.ToRow().GetBytes(4), vector.ZeroCopySerialize());
}

/// Pool 按字段类型返回正确 elemBuf 形状，PutChunk 清空列并回收入池。
#[test]
fn pool_returns_all_column_shapes_and_clears_returned_chunk() {
    let pool = NewPool(8);
    let fields = vec![
        *types::NewFieldType(super::mysql::TypeVarchar),
        *types::NewFieldType(super::mysql::TypeFloat),
        *types::NewFieldType(super::mysql::TypeLonglong),
        *types::NewFieldType(super::mysql::TypeDatetime),
        *types::NewFieldType(super::mysql::TypeNewDecimal),
    ];
    let mut chunk = pool.GetChunk(&fields);
    assert_eq!(chunk.NumCols(), 5);
    assert_eq!(
        chunk
            .columns
            .iter()
            .map(|col| col.elemBuf.len())
            .collect::<Vec<_>>(),
        vec![0, 4, 8, super::sizeTime, 40]
    );
    pool.PutChunk(&fields, &mut chunk);
    assert!(chunk.columns.is_empty());
    assert_eq!(pool.cached_columns(), 5);
}

/// RowContainerReader 顺序读完后 Close；注入错误时经 Error() 暴露。
#[test]
fn row_container_reader_streams_closes_and_propagates_errors() {
    /// 可配置在指定 Chunk 索引注入错误的测试数据源。
    struct TestSource {
        chunks: Vec<Box<Chunk>>,
        fail_at: Option<usize>,
    }

    impl RowContainerSource for TestSource {
        fn NumChunks(&self) -> usize {
            self.chunks.len()
        }

        fn NumRowsOfChunk(&self, index: usize) -> usize {
            self.chunks[index].NumRows()
        }

        fn RowsOfChunk(&self, index: usize) -> Result<Vec<super::Row>, ChunkError> {
            if self.fail_at == Some(index) {
                return Err(ChunkError::Message("injected chunk error".to_owned()));
            }
            Ok((0..self.chunks[index].NumRows())
                .map(|row| self.chunks[index].GetRow(row))
                .collect())
        }
    }

    let container = Arc::new(RowContainer::New(vec![int_field()], 2));
    container.Add(*int_chunk(&[1, 2], 2)).unwrap();
    container.Add(*int_chunk(&[3], 1)).unwrap();
    let mut reader = NewRowContainerReader(container);
    let mut values = Vec::new();
    while reader.Current() != reader.End() {
        values.push(reader.Current().GetInt64(0));
        reader.Next();
    }
    reader.Close();
    assert_eq!(values, vec![1, 2, 3]);
    assert!(reader.Error().is_none());

    let failing = Arc::new(TestSource {
        chunks: vec![int_chunk(&[1], 1), int_chunk(&[2], 1)],
        fail_at: Some(1),
    });
    let mut reader = NewRowContainerReader(failing);
    while reader.Current() != reader.End() {
        reader.Next();
    }
    assert_eq!(reader.Error().unwrap().to_string(), "injected chunk error");
    reader.Close();
}
