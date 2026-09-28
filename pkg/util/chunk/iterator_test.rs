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

// 行迭代器与 Go iterator_test.go 的对抗性一致性测试。

use super::iterator::{
    Iterator, NewIterator4Chunk, NewIterator4List, NewIterator4RowContainer, NewIterator4RowPtr,
    NewIterator4Slice, NewMultiIterator,
};
use super::{Chunk, List, NewChunkWithCapacity, Row, RowContainer, mysql, types};

fn int_chunk(values: &[i64]) -> Box<Chunk> {
    let mut chunk = NewChunkWithCapacity(
        vec![*types::NewFieldType(mysql::TypeLonglong)],
        values.len(),
    );
    for value in values {
        chunk.AppendInt64(0, *value);
    }
    chunk
}

fn collect(iterator: &mut dyn Iterator) -> Vec<i64> {
    let mut values = Vec::new();
    let mut row = iterator.Begin();
    while row != iterator.End() {
        values.push(row.GetInt64(0));
        row = iterator.Next();
    }
    values
}

/// 用 Begin/Next/End 遍历 Chunk，收集首列 int64 并与写入顺序比对。
#[test]
fn chunk_iterator_visits_each_row_once() {
    let mut iterator = NewIterator4Chunk(int_chunk(&[3, 5, 8]));
    assert_eq!(collect(iterator.as_mut()), vec![3, 5, 8]);
}

#[test]
fn chunk_iterator_honors_selection_vector() {
    let mut chunk = int_chunk(&(0..1024).collect::<Vec<_>>());
    chunk.SetSel(Some((0..1024).step_by(2).collect()));
    let mut iterator = NewIterator4Chunk(chunk);
    let values = collect(iterator.as_mut());
    assert_eq!(values.len(), 512);
    assert!(values.iter().all(|value| value % 2 == 0));
}

fn assert_cursor_contract(iterator: &mut dyn Iterator, expected: &[i64]) {
    assert_eq!(iterator.Len(), expected.len());
    assert_eq!(iterator.Begin().GetInt64(0), expected[0]);
    for value in expected.iter().take(5) {
        assert_eq!(iterator.Current().GetInt64(0), *value);
        iterator.Next();
    }
    iterator.ReachEnd();
    assert!(iterator.Current().IsEmpty());
    assert_eq!(iterator.Begin().GetInt64(0), expected[0]);
    assert_eq!(collect(iterator), expected);
}

#[test]
fn slice_list_and_row_ptr_iterators_match_go_cursor_contract() {
    let expected: Vec<i64> = (0..10).collect();
    let source = int_chunk(&expected);

    let rows: Vec<Row> = (0..expected.len()).map(|idx| source.GetRow(idx)).collect();
    let mut slice = NewIterator4Slice(rows);
    assert_cursor_contract(slice.as_mut(), &expected);

    let mut list = List::New(vec![*types::NewFieldType(mysql::TypeLonglong)], 1, 2);
    for idx in 0..expected.len() {
        list.AppendRow(source.GetRow(idx));
    }
    let mut list_iterator = NewIterator4List(list);
    assert_cursor_contract(list_iterator.as_mut(), &expected);

    let mut list = List::New(vec![*types::NewFieldType(mysql::TypeLonglong)], 8, 16);
    let ptrs = (0..expected.len())
        .map(|idx| list.AppendRow(source.GetRow(idx)))
        .collect();
    let mut row_ptr_iterator = NewIterator4RowPtr(list, ptrs);
    assert_cursor_contract(row_ptr_iterator.as_mut(), &expected);

    let container = RowContainer::New(vec![*types::NewFieldType(mysql::TypeLonglong)], 2);
    container.Add(*int_chunk(&expected[..5])).unwrap();
    container.Add(*int_chunk(&expected[5..])).unwrap();
    let mut container_iterator = NewIterator4RowContainer(Box::new(container));
    assert_cursor_contract(container_iterator.as_mut(), &expected);
}

#[test]
fn empty_and_multi_iterators_match_go_boundaries() {
    let mut empty_slice = NewIterator4Slice(Vec::new());
    assert_eq!(empty_slice.Begin(), empty_slice.End());
    let mut empty_chunk = NewIterator4Chunk(Box::default());
    assert_eq!(empty_chunk.Begin(), empty_chunk.End());
    let mut empty_list = NewIterator4List(List::New(
        vec![*types::NewFieldType(mysql::TypeLonglong)],
        1,
        1,
    ));
    assert_eq!(empty_list.Begin(), empty_list.End());
    let mut empty_ptrs = NewIterator4RowPtr(
        List::New(vec![*types::NewFieldType(mysql::TypeLonglong)], 1, 1),
        Vec::new(),
    );
    assert_eq!(empty_ptrs.Begin(), empty_ptrs.End());
    let mut empty_container = NewIterator4RowContainer(Box::new(RowContainer::New(
        vec![*types::NewFieldType(mysql::TypeLonglong)],
        1,
    )));
    assert_eq!(empty_container.Begin(), empty_container.End());

    let empty = NewIterator4Chunk(Box::default());
    let first = NewIterator4Chunk(int_chunk(&[0, 1, 2]));
    let second = NewIterator4Chunk(int_chunk(&[3, 4, 5]));
    let mut multi = NewMultiIterator(vec![empty, first, second]);
    assert_eq!(multi.Len(), 6);
    assert_eq!(collect(multi.as_mut()), vec![0, 1, 2, 3, 4, 5]);
    assert!(multi.Error().is_none());
}
