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

// `List`（多 Chunk 行列表）跨块追加与行指针稳定性的单元测试。
//
// 对应 Go `list_test.go`：验证当单 Chunk 容量不足以容纳全部行时，
// `List` 会切分到多个 Chunk，且通过 `RowPtr` 仍能正确取回原始行值。

/// 验证追加行跨过 Chunk 边界后，行指针仍指向正确数据。
#[test]
fn list_crosses_chunk_boundaries_and_keeps_row_pointers() {
    use super::{List, NewChunkWithCapacity, mysql, types};
    // 构造三行源数据，List 容量为 2，第三行应落入第二个 Chunk。
    let fields = vec![*types::NewFieldType(mysql::TypeLonglong)];
    let mut source = NewChunkWithCapacity(fields.clone(), 3);
    for value in [1, 2, 3] {
        source.AppendInt64(0, value);
    }
    let mut list = List::New(fields, 2, 2);
    let pointers: Vec<_> = (0..3).map(|i| list.AppendRow(source.GetRow(i))).collect();
    assert_eq!(list.NumChunks(), 2);
    assert_eq!(list.GetRow(pointers[2]).GetInt64(0), 3);
}

/// 对齐 Go `TestList`：覆盖基本追加、Reset 后复用、Add 后只读分块与遍历顺序。
#[test]
fn list_append_reset_add_and_walk_match_go() {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::{List, NewChunkWithCapacity, mysql, types};

    let fields = vec![*types::NewFieldType(mysql::TypeLonglong)];
    let mut source = NewChunkWithCapacity(fields.clone(), 1);
    source.AppendInt64(0, 1);
    let source_row = source.GetRow(0);
    let mut list = List::New(fields.clone(), 2, 2);

    for _ in 0..5 {
        list.AppendRow(source_row.clone());
    }
    assert_eq!(list.NumChunks(), 3);
    assert_eq!(list.Len(), 5);

    list.Reset();
    assert_eq!(list.NumChunks(), 0);
    assert_eq!(list.Len(), 0);
    for _ in 0..5 {
        list.AppendRow(source_row.clone());
    }
    assert_eq!(list.NumChunks(), 3);

    list.Reset();
    let mut added = NewChunkWithCapacity(fields, 32);
    added.AppendNull(0);
    list.Add(added);
    let ptr = list.AppendRow(source_row);
    assert_eq!((ptr.ChkIdx, ptr.RowIdx), (1, 0));
    assert_eq!(list.GetRow(ptr).GetInt64(0), 1);

    list.Reset();
    for value in 0..5 {
        let mut chunk = NewChunkWithCapacity(list.FieldTypes().to_vec(), 1);
        chunk.AppendInt64(0, value);
        list.AppendRow(chunk.GetRow(0));
    }
    let values = Rc::new(RefCell::new(Vec::new()));
    let walked = Rc::clone(&values);
    list.Walk(Box::new(move |row| {
        walked.borrow_mut().push(row.GetInt64(0));
        Ok(())
    }))
    .unwrap();
    assert_eq!(*values.borrow(), vec![0, 1, 2, 3, 4]);
}

/// 对齐 Go `TestListMemoryUsage`：最后一个 Chunk 在 Reset 时记账，复用时扣除，Clear 清零。
#[test]
fn list_memory_accounting_matches_go() {
    use super::{List, NewChunkWithCapacity, mysql, types};

    let fields = vec![*types::NewFieldType(mysql::TypeLonglong)];
    let mut source = NewChunkWithCapacity(fields.clone(), 2);
    source.AppendInt64(0, 7);
    let source_usage = source.MemoryUsage();
    let source_row = source.GetRow(0);
    let mut list = List::New(fields, 2, 4);

    assert_eq!(list.GetMemTracker().BytesConsumed(), 0);
    list.AppendRow(source_row);
    assert_eq!(list.GetMemTracker().BytesConsumed(), 0);
    let list_usage = list.GetChunk(0).MemoryUsage();

    list.Reset();
    assert_eq!(list.GetMemTracker().BytesConsumed(), list_usage);
    list.Add(source);
    assert_eq!(
        list.GetMemTracker().BytesConsumed(),
        list_usage + source_usage
    );

    list.Clear();
    assert_eq!(list.GetMemTracker().BytesConsumed(), 0);
    assert_eq!(list.NumChunks(), 0);
    assert_eq!(list.Len(), 0);
}
