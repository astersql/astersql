// Copyright 2026 AsterSQL.
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

// `cteutil` 迁移期单元测试：对齐 Go CTE Storage 生命周期与读写语义。
//
// 覆盖：引用计数 Open/Deref、Add/Get、空 Chunk、Reopen、SwapData、
// 内存溢出落盘（spill）以及 Lock/Unlock 互斥。

use std::sync::{Arc, mpsc};
use std::time::Duration;

use super::*;

/// 构造指定 MySQL 类型的单列表头。
fn fields(tp: u8) -> Vec<types::FieldType> {
    vec![*types::NewFieldType(tp)]
}

/// 将整数序列写入新 Chunk（列 0）。
fn int_chunk(values: &[i64]) -> Box<chunk::Chunk> {
    let mut chunk = chunk::NewChunkWithCapacity(fields(mysql::TypeLong), values.len().max(1));
    for value in values {
        chunk.AppendInt64(0, *value);
    }
    chunk
}

/// 校验未打开报错、多次 OpenAndRef、Deref 清状态与 done/iter/error 复位。
#[test]
fn storage_reference_lifecycle_matches_go() {
    let mut storage = NewStorageRowContainer(fields(mysql::TypeLong), 1);

    assert_eq!(
        storage.DerefAndClose().unwrap_err().to_string(),
        "Storage not opend yet"
    );
    storage.OpenAndRef().unwrap();
    storage.OpenAndRef().unwrap();
    storage.SetDone();
    storage.SetIter(9);
    storage.SetError(errors::New("fill failed"));

    storage.DerefAndClose().unwrap();
    assert!(storage.Done());
    storage.DerefAndClose().unwrap();
    assert!(!storage.Done());
    assert_eq!(storage.GetIter(), 0);
    assert!(storage.Error().is_none());
    assert_eq!(
        storage.DerefAndClose().unwrap_err().to_string(),
        "Storage not opend yet"
    );
}

/// 打开后 Add，再按 Chunk/RowPtr 读取与 Go 一致。
#[test]
fn add_get_chunk_and_row_match_go() {
    let mut storage = NewStorageRowContainer(fields(mysql::TypeLong), 10);
    let input = int_chunk(&[0, 1, 2, 3, 4]);

    assert_eq!(
        storage.Add(&input).unwrap_err().to_string(),
        "Storage is not valid"
    );
    storage.OpenAndRef().unwrap();
    storage.Add(&input).unwrap();

    assert_eq!(storage.NumChunks(), 1);
    assert_eq!(storage.NumRows(), 5);
    let output = storage.GetChunk(0).unwrap();
    assert_eq!(output.GetRow(3).GetInt64(0), 3);
    let row = storage
        .GetRow(chunk::RowPtr {
            ChkIdx: 0,
            RowIdx: 4,
        })
        .unwrap();
    assert_eq!(row.GetInt64(0), 4);
}

/// 空 Chunk 被忽略，不增加 NumChunks/NumRows。
#[test]
fn empty_chunk_is_ignored() {
    let mut storage = NewStorageRowContainer(fields(mysql::TypeLong), 4);
    storage.OpenAndRef().unwrap();
    let empty = chunk::NewChunkWithCapacity(fields(mysql::TypeLong), 4);

    storage.Add(&empty).unwrap();
    assert_eq!(storage.NumChunks(), 0);
    assert_eq!(storage.NumRows(), 0);
}

/// Reopen 清空数据与可变状态，并可再次写入。
#[test]
fn reopen_clears_data_and_mutable_state() {
    let mut storage = NewStorageRowContainer(fields(mysql::TypeLong), 4);
    storage.OpenAndRef().unwrap();
    storage.Add(&int_chunk(&[7, 8])).unwrap();
    storage.SetDone();
    storage.SetIter(3);
    storage.SetError(errors::New("old error"));

    storage.Reopen().unwrap();
    assert_eq!(storage.NumChunks(), 0);
    assert_eq!(storage.NumRows(), 0);
    assert!(!storage.Done());
    assert_eq!(storage.GetIter(), 0);
    assert!(storage.Error().is_none());

    for _ in 0..100 {
        storage.Reopen().unwrap();
    }
    storage.Add(&int_chunk(&[11])).unwrap();
    assert_eq!(storage.GetChunk(0).unwrap().GetRow(0).GetInt64(0), 11);
}

/// SwapData 交换底层数据/schema，但保留各自 done/iter 等元数据。
#[test]
fn swap_data_preserves_each_storage_metadata() {
    let mut integers = NewStorageRowContainer(fields(mysql::TypeLong), 10);
    integers.OpenAndRef().unwrap();
    integers.Add(&int_chunk(&[1, 2])).unwrap();
    integers.SetDone();
    integers.SetIter(17);

    let text_fields = fields(mysql::TypeVarString);
    let mut strings = NewStorageRowContainer(text_fields.clone(), 2);
    strings.OpenAndRef().unwrap();
    let mut text = chunk::NewChunkWithCapacity(text_fields, 2);
    text.AppendString(0, "one");
    text.AppendString(0, "two");
    strings.Add(&text).unwrap();

    integers.SwapData(&mut strings).unwrap();
    assert_eq!(integers.GetChunk(0).unwrap().GetRow(1).GetString(0), "two");
    assert_eq!(strings.GetChunk(0).unwrap().GetRow(0).GetInt64(0), 1);
    assert!(integers.Done());
    assert_eq!(integers.GetIter(), 17);
    assert!(!strings.Done());
    assert_eq!(strings.GetIter(), 0);
}

/// spill 后内存字节归零、磁盘字节增加，且读结果不变。
#[test]
fn spill_action_moves_rows_to_disk_without_changing_results() {
    let mut storage = NewStorageRowContainer(fields(mysql::TypeLong), 4);
    storage.OpenAndRef().unwrap();
    storage.Add(&int_chunk(&[5, 6, 7])).unwrap();
    assert!(storage.GetMemBytes() > 0);
    assert_eq!(storage.GetDiskBytes(), 0);

    let action = storage.ActionSpillForTest();
    action.Action(storage.GetMemTracker());
    action.WaitForTest();

    assert_eq!(storage.GetMemBytes(), 0);
    assert!(storage.GetDiskBytes() > 0);
    assert_eq!(storage.GetChunk(0).unwrap().GetRow(2).GetInt64(0), 7);
}

/// Lock 阻塞其他调用方直到 Unlock（对齐 Go 显式互斥约定）。
#[test]
fn lock_blocks_other_callers_until_unlock() {
    let storage = Arc::new(NewStorageRowContainer(fields(mysql::TypeLong), 1));
    storage.Lock();

    let contender = Arc::clone(&storage);
    let (acquired_tx, acquired_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        contender.Lock();
        acquired_tx.send(()).unwrap();
        contender.Unlock();
    });

    assert!(acquired_rx.recv_timeout(Duration::from_millis(50)).is_err());
    storage.Unlock();
    acquired_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    worker.join().unwrap();
}
