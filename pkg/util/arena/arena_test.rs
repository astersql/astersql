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
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Arena 分配器单元测试（对应 Go `arena_test.go`）。
//
// 覆盖 `SimpleAllocator` 的偏移推进、超容量回退堆分配、`AllocWithLen` 与 `Reset`，
// 以及 `StdAllocator` 返回切片的长度/容量形状。

use super::arena::{Allocator, NewAllocator, StdAllocator};

/// 预分配 arena 总容量。
const ARENA_CAP: usize = 1000;
/// 小容量分配请求。
const ALLOC_CAP_SMALL: usize = 10;
/// 中等容量分配请求。
const ALLOC_CAP_MEDIUM: usize = 20;
/// 超过 arena 容量的分配请求，应走堆回退路径。
const ALLOC_CAP_OUT: usize = 1024;

// test_simple_arena_allocator 对应 Go 的 TestSimpleArenaAllocator。
// 测试重点是小容量分配推进 arena.off，超出 arena 容量时回退到普通分配且不推进 offset。
/// 验证简单 arena：小分配推进 `off`，超容量不推进，`AllocWithLen`/`Reset` 行为正确。
#[test]
fn test_simple_arena_allocator() {
    let mut arena = NewAllocator(ARENA_CAP);
    {
        let slice = arena.Alloc(ALLOC_CAP_SMALL);
        assert_eq!(0, slice.len());
        assert_eq!(ALLOC_CAP_SMALL, slice.capacity());
    }
    assert_eq!(ALLOC_CAP_SMALL, arena.off);

    {
        let slice = arena.Alloc(ALLOC_CAP_MEDIUM);
        assert_eq!(0, slice.len());
        assert_eq!(ALLOC_CAP_MEDIUM, slice.capacity());
    }
    assert_eq!(ALLOC_CAP_SMALL + ALLOC_CAP_MEDIUM, arena.off);

    {
        let slice = arena.Alloc(ALLOC_CAP_OUT);
        assert_eq!(0, slice.len());
        assert_eq!(ALLOC_CAP_OUT, slice.capacity());
    }
    // ALLOC_CAP_OUT 大于预分配 arena 容量；Go 原实现返回新切片，不增加 offset。
    assert_eq!(ALLOC_CAP_SMALL + ALLOC_CAP_MEDIUM, arena.off);

    {
        let slice = arena.AllocWithLen(2, ALLOC_CAP_SMALL);
        assert_eq!(2, slice.len());
        assert_eq!(ALLOC_CAP_SMALL, slice.capacity());
    }
    // AllocWithLen 先走 Alloc 推进 offset，再把可见长度调整为 2。
    assert_eq!(
        ALLOC_CAP_SMALL + ALLOC_CAP_MEDIUM + ALLOC_CAP_SMALL,
        arena.off
    );

    arena.Reset();
    // Reset 只复位 offset；底层 arena 容量仍保持初始化时的 arenaCap。
    assert_eq!(0, arena.off);
    assert_eq!(ARENA_CAP, arena.arena.capacity());
}

// test_std_allocator 对应 Go 的 TestStdAllocator。
// 标准分配器不复用 arena，只检查返回切片的长度和容量形状。
/// 验证标准分配器：`Alloc`/`AllocWithLen` 仅保证长度与容量，不涉及 arena 偏移。
#[test]
fn test_std_allocator() {
    let mut allocator = StdAllocator();
    let slice = allocator.Alloc(ALLOC_CAP_MEDIUM);
    assert_eq!(0, slice.len());
    assert_eq!(ALLOC_CAP_MEDIUM, slice.capacity());
    drop(slice);

    let slice = allocator.AllocWithLen(ALLOC_CAP_SMALL, ALLOC_CAP_MEDIUM);
    assert_eq!(ALLOC_CAP_SMALL, slice.len());
    assert_eq!(ALLOC_CAP_MEDIUM, slice.capacity());
}
