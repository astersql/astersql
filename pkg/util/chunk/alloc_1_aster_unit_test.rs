// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// Allocator 与磁盘 spill 相关行为的补充单元测试（对齐 Go 语义）。
//
// 覆盖：Chunk/Column 缓存复用、缓存上限与超大变长列丢弃、ReuseHook 一次性回调、
// DataInDiskByChunks 落盘往返与磁盘用量记账。InitChunkAllocSize 会改全局配置，
// 测试间用互斥锁串行化以免互相干扰。

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// 保护全局 `InitChunkAllocSize` 配置，避免并行测试互相覆盖。
static ALLOCATOR_CONFIG_LOCK: Mutex<()> = Mutex::new(());

/// 构造单列 VARCHAR 的字段类型列表，用于变长列分配场景。
fn varchar_fields() -> Vec<Box<types::FieldType>> {
    vec![types::NewFieldType(mysql::TypeVarchar)]
}

/// 验证 Alloc/Reset 后复用同一 Chunk 指针，并保留变长列已扩展的 data capacity。
#[test]
fn allocator_reuses_chunk_and_column_storage_like_go() {
    let _config_guard = ALLOCATOR_CONFIG_LOCK.lock().unwrap();
    InitChunkAllocSize(2, 2);
    let fields = varchar_fields();
    let mut alloc = NewAllocator();
    let first = alloc.Alloc(&fields, 4, 8);
    let first_ptr = std::sync::Arc::as_ptr(&first);
    {
        let mut chunk = first.lock().unwrap();
        assert_eq!(chunk.Capacity(), 4);
        // 写入大块字节迫使变长列扩容，Reset 后 capacity 应被缓存保留。
        chunk.AppendBytes(0, &vec![b'x'; 1024]);
        assert!(chunk.columns[0].data.capacity() >= 1024);
    }
    alloc.Reset();
    assert_eq!(alloc.cached_column_count(VarElemLen), 1);
    assert!(alloc.cached_column_capacity(VarElemLen).unwrap() >= 1024);
    // 再次 Alloc 应拿到同一 Arc 地址；容量按 maxChunkSize 回填。
    let second = alloc.Alloc(&fields, 99, 8);
    assert_eq!(std::sync::Arc::as_ptr(&second), first_ptr);
    let chunk = second.lock().unwrap();
    assert_eq!(chunk.Capacity(), 8);
    assert_eq!(chunk.NumRows(), 0);
    assert!(
        chunk.columns[0].data.capacity() >= 1024,
        "reused capacity was {}",
        chunk.columns[0].data.capacity()
    );
}

/// 验证列缓存上限：超大变长列在 Reset 时被丢弃，不进入 column cache。
#[test]
fn allocator_limits_caches_and_drops_oversized_variable_columns() {
    let _config_guard = ALLOCATOR_CONFIG_LOCK.lock().unwrap();
    InitChunkAllocSize(1, 1);
    let fields = varchar_fields();
    let mut alloc = NewAllocator();
    let chunk = alloc.Alloc(&fields, 1, 1);
    // 20KB 远超默认变长列缓存阈值，Reset 后变长列缓存应为 0。
    chunk.lock().unwrap().AppendBytes(0, &vec![b'a'; 20 * 1024]);
    let _uncached = alloc.Alloc(&fields, 1, 1);
    alloc.Reset();
    assert_eq!(alloc.cached_chunk_count(), 1);
    assert_eq!(alloc.cached_column_count(VarElemLen), 0);
}

/// 验证 ReuseHookAllocator：有复用配额时首次 Alloc 回调一次（对齐 Go sync.Once 语义）。
#[test]
fn reuse_hook_matches_go_once_semantics() {
    let _config_guard = ALLOCATOR_CONFIG_LOCK.lock().unwrap();
    InitChunkAllocSize(0, 0);
    let count = Arc::new(AtomicUsize::new(0));
    let hook_count = Arc::clone(&count);
    // 缓存大小为 0 时不会复用，hook 不应触发。
    let mut no_cache = NewReuseHookAllocator(
        NewAllocator(),
        Box::new(move || {
            hook_count.fetch_add(1, Ordering::SeqCst);
        }),
    );
    let fields = varchar_fields();
    let _ = no_cache.Alloc(&fields, 1, 1);
    assert_eq!(count.load(Ordering::SeqCst), 0);

    InitChunkAllocSize(1, 1);
    let hook_count = Arc::clone(&count);
    let mut cached = NewReuseHookAllocator(
        NewAllocator(),
        Box::new(move || {
            hook_count.fetch_add(1, Ordering::SeqCst);
        }),
    );
    // Go 根据 CheckReuseAllocSize 在第一次 Alloc 前触发，不要延迟到真正命中 free list。
    let _ = cached.Alloc(&fields, 1, 1);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    // 后续 Alloc 不再触发。
    let _ = cached.Alloc(&fields, 1, 1);
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

/// 验证按 Chunk 落盘往返：布局、行内容与 diskTracker 用量记账与 Go 一致。
#[test]
fn chunk_disk_round_trip_preserves_go_layout_and_accounting() {
    let field = *types::NewFieldType(mysql::TypeVarchar);
    let mut chunk = New(vec![field.clone()], 2, 2);
    chunk.AppendString(0, "aster");
    chunk.AppendString(0, "sql");

    let mut disk = NewDataInDiskByChunks(vec![field], "task409-".to_owned());
    disk.Add(&chunk).unwrap();
    assert_eq!(disk.NumChunks(), 1);
    assert_eq!(disk.NumRows(), 2);
    assert!(disk.GetTotalBytesInDisk() > 0);
    // tracker 消耗应与 totalBytesInDisk 一致；Close 后归零。
    assert_eq!(
        disk.GetDiskTracker().BytesConsumed(),
        disk.GetTotalBytesInDisk()
    );

    let restored = disk.GetChunk(0).unwrap();
    assert_eq!(restored.NumRows(), 2);
    assert_eq!(restored.Column(0).GetBytes(0), b"aster");
    assert_eq!(restored.Column(0).GetBytes(1), b"sql");
    disk.Close();
    assert_eq!(disk.GetDiskTracker().BytesConsumed(), 0);
}
