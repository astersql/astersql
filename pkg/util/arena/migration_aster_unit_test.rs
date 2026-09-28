// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// See the License for the specific language governing permissions and
// limitations under the License.

// Arena 迁移补充单元测试。
//
// 相对 Go 原测试，额外覆盖：arena 指针连续性、等于/超出容量边界回退、
// `ArenaBuffer` 追加溢出迁移到自有存储，以及标准分配器零初始化形状。

use super::arena::{Allocator, NewAllocator, StdAllocator};

/// 连续小分配应落在同一预分配 arena 上，且 `Reset` 后可复用原偏移处的旧字节内容。
#[test]
fn simple_allocator_reuses_the_preallocated_arena() {
    let mut allocator = NewAllocator(32);
    let arena_start = allocator.arena.as_ptr();

    let mut first = allocator.AllocWithLen(2, 8);
    assert!(first.is_arena_backed());
    assert_eq!(first.as_ptr(), arena_start);
    assert_eq!(first.len(), 2);
    assert_eq!(first.capacity(), 8);
    first.copy_from_slice(&[17, 29]);
    drop(first);
    assert_eq!(allocator.off, 8);

    let second = allocator.Alloc(6);
    assert_eq!(second.as_ptr(), unsafe { arena_start.add(8) });
    assert_eq!(second.len(), 0);
    assert_eq!(second.capacity(), 6);
    drop(second);
    assert_eq!(allocator.off, 14);

    allocator.Reset();
    let reused = allocator.AllocWithLen(2, 8);
    assert_eq!(reused.as_ptr(), arena_start);
    assert_eq!(&*reused, &[17, 29]);
}

/// 对齐 Go：`end < len(arena)` 才走 arena；容量等于或大于 arena 长度时回退堆且 `off` 不变。
#[test]
fn simple_allocator_matches_go_fallback_boundaries() {
    let mut allocator = NewAllocator(16);
    let arena_start = allocator.arena.as_ptr();

    let exact_capacity = allocator.Alloc(16);
    assert!(!exact_capacity.is_arena_backed());
    assert_ne!(exact_capacity.as_ptr(), arena_start);
    assert_eq!(exact_capacity.capacity(), 16);
    drop(exact_capacity);
    assert_eq!(allocator.off, 0);

    let oversized = allocator.Alloc(24);
    assert_ne!(oversized.as_ptr(), arena_start);
    assert_eq!(oversized.capacity(), 24);
    drop(oversized);
    assert_eq!(allocator.off, 0);

    let within_arena = allocator.Alloc(15);
    assert_eq!(within_arena.as_ptr(), arena_start);
    drop(within_arena);
    assert_eq!(allocator.off, 15);
}

/// 追加超过 arena 借用容量时，缓冲应脱离 arena 并保留已有字节。
#[test]
fn arena_buffer_moves_to_owned_storage_when_append_exceeds_capacity() {
    let mut allocator = NewAllocator(32);
    let arena_start = allocator.arena.as_ptr();

    let mut buffer = allocator.AllocWithLen(2, 4);
    buffer.copy_from_slice(&[1, 2]);
    buffer.extend_from_slice(&[3, 4]);
    assert!(buffer.is_arena_backed());
    assert_eq!(buffer.as_ptr(), arena_start);
    assert_eq!(&*buffer, &[1, 2, 3, 4]);

    buffer.push(5);
    assert!(!buffer.is_arena_backed());
    assert_ne!(buffer.as_ptr(), arena_start);
    assert_eq!(&*buffer, &[1, 2, 3, 4, 5]);
}

/// 标准分配器返回的长度/容量与 Go 一致，且 `AllocWithLen` 可见区为零字节。
#[test]
fn standard_allocator_matches_go_slice_shapes() {
    let mut allocator = StdAllocator();

    let empty = allocator.Alloc(20);
    assert_eq!(empty.len(), 0);
    assert_eq!(empty.capacity(), 20);

    let initialized = allocator.AllocWithLen(10, 20);
    assert_eq!(initialized.len(), 10);
    assert_eq!(initialized.capacity(), 20);
    assert!(initialized.iter().all(|byte| *byte == 0));
}
