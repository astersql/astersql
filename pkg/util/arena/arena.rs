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

// Arena 分配器：预分配一块连续字节缓冲，按偏移切出短生命周期切片，降低频繁堆分配开销。
//
// 对应 Go `pkg/util/arena`。Arena 在此指“竞技场式”内存池：先申请大块，再线性推进
// `off` 切分子区间；容量不足时回退到独立 `Vec` 分配。返回的 `ArenaBuffer` 模拟 Go
// slice 的可见长度与容量；溢出追加时会从 arena 借用迁移到自有存储。

use std::ops::{Deref, DerefMut};

/// 缓冲底层存储：要么借用 arena 子切片，要么拥有独立 `Vec`。
enum BufferStorage<'a> {
    /// 借用自 `SimpleAllocator.arena` 的一段可变字节。
    Arena(&'a mut [u8]),
    /// 堆上自有缓冲（容量不足或超 arena 边界时使用）。
    Owned(Vec<u8>),
}

/// A byte buffer with Go-slice-like visible length and capacity.
///
/// Arena-backed buffers borrow their allocation from `SimpleAllocator`. When an
/// append exceeds that capacity, the buffer moves its visible bytes into an
/// owned `Vec`, matching Go's append behavior.
///
/// 类 Go slice 的字节缓冲：`len` 为可见长度，`capacity` 为可写上限。
/// 由 arena 支持时借用 `SimpleAllocator` 的分配；追加超过容量时把可见字节
/// 迁入自有 `Vec`，行为对齐 Go 的 `append`。
pub struct ArenaBuffer<'a> {
    storage: BufferStorage<'a>,
    len: usize,
    capacity: usize,
}

impl<'a> ArenaBuffer<'a> {
    /// 从 arena 子切片构造：可见长度为 0，容量为切片长度。
    fn from_arena(storage: &'a mut [u8]) -> Self {
        let capacity = storage.len();
        Self {
            storage: BufferStorage::Arena(storage),
            len: 0,
            capacity,
        }
    }

    /// 构造堆上自有缓冲，可见长度为 0。
    fn owned(capacity: usize) -> Self {
        Self {
            storage: BufferStorage::Owned(Vec::with_capacity(capacity)),
            len: 0,
            capacity,
        }
    }

    /// 返回可见长度（对应 Go slice 的 `len`）。
    pub fn len(&self) -> usize {
        self.len
    }

    /// 可见长度是否为 0。
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 返回容量（对应 Go slice 的 `cap`）。
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 是否仍借用 arena（尚未因溢出迁移到自有 `Vec`）。
    pub fn is_arena_backed(&self) -> bool {
        matches!(self.storage, BufferStorage::Arena(_))
    }

    /// 底层字节起始指针（只读）。
    pub fn as_ptr(&self) -> *const u8 {
        match &self.storage {
            BufferStorage::Arena(storage) => storage.as_ptr(),
            BufferStorage::Owned(storage) => storage.as_ptr(),
        }
    }

    /// 底层字节起始指针（可变）。
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        match &mut self.storage {
            BufferStorage::Arena(storage) => storage.as_mut_ptr(),
            BufferStorage::Owned(storage) => storage.as_mut_ptr(),
        }
    }

    /// 设置可见长度；owned 存储会 `resize` 填 0，arena 存储只改 `len`。
    fn set_len(&mut self, length: usize) {
        assert!(
            length <= self.capacity,
            "length ({length}) exceeds capacity ({})",
            self.capacity
        );
        if let BufferStorage::Owned(storage) = &mut self.storage {
            storage.resize(length, 0);
        }
        self.len = length;
    }

    /// 追加单字节；容量内直接写入，溢出时 owned 继续 push，arena 则拷贝迁移到新 `Vec`。
    pub fn push(&mut self, byte: u8) {
        // 容量未满：按存储类型原地写入并推进可见长度。
        if self.len < self.capacity {
            match &mut self.storage {
                BufferStorage::Arena(storage) => storage[self.len] = byte,
                BufferStorage::Owned(storage) => storage.push(byte),
            }
            self.len += 1;
            return;
        }

        // 容量已满：owned 交给 Vec 扩容；arena 需拷贝可见区后切换为 Owned。
        match &mut self.storage {
            BufferStorage::Owned(storage) => {
                storage.push(byte);
                self.len = storage.len();
                self.capacity = storage.capacity();
            }
            BufferStorage::Arena(storage) => {
                // 至少 2 倍旧容量，保证与常见切片扩容策略一致。
                let mut owned = Vec::with_capacity(self.capacity.saturating_mul(2).max(1));
                owned.extend_from_slice(&storage[..self.len]);
                owned.push(byte);
                self.len = owned.len();
                self.capacity = owned.capacity();
                self.storage = BufferStorage::Owned(owned);
            }
        }
    }

    /// 逐字节追加切片内容（内部走 `push`，因此也可能触发 arena→owned 迁移）。
    pub fn extend_from_slice(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.push(*byte);
        }
    }
}

impl Deref for ArenaBuffer<'_> {
    type Target = [u8];

    /// 解引用为可见长度范围内的字节切片。
    fn deref(&self) -> &Self::Target {
        match &self.storage {
            BufferStorage::Arena(storage) => &storage[..self.len],
            BufferStorage::Owned(storage) => storage.as_slice(),
        }
    }
}

impl DerefMut for ArenaBuffer<'_> {
    /// 可变解引用为可见长度范围内的字节切片。
    fn deref_mut(&mut self) -> &mut Self::Target {
        match &mut self.storage {
            BufferStorage::Arena(storage) => &mut storage[..self.len],
            BufferStorage::Owned(storage) => storage.as_mut_slice(),
        }
    }
}

// Allocator pre-allocates memory to reduce memory allocation cost.
// It is not thread-safe.
/// 分配器接口：预分配内存以降低分配成本；非线程安全。
pub trait Allocator {
    // Alloc allocates memory with 0 len and capacity cap.
    /// 分配长度为 0、容量为 `capacity` 的缓冲。
    fn Alloc(&mut self, capacity: usize) -> ArenaBuffer<'_>;

    // AllocWithLen allocates memory with length and capacity.
    /// 分配指定可见长度与容量的缓冲。
    fn AllocWithLen(&mut self, length: usize, capacity: usize) -> ArenaBuffer<'_>;

    // Reset resets arena offset.
    // Make sure all the allocated memory are not used any more.
    /// 复位 arena 偏移；调用前须确保已分配缓冲不再使用。
    fn Reset(&mut self);
}

// SimpleAllocator is a simple implementation of ArenaAllocator.
/// 简单 Arena 分配器：持有预分配 `arena` 与当前偏移 `off`。
pub struct SimpleAllocator {
    /// 预分配的连续字节池。
    pub arena: Vec<u8>,
    /// 下一次可切分的起始偏移。
    pub off: usize,
}

/// 标准分配器：不预分配 arena，每次直接 `Vec` 分配。
pub struct stdAllocator {}

impl Allocator for stdAllocator {
    fn Alloc(&mut self, capacity: usize) -> ArenaBuffer<'_> {
        ArenaBuffer::owned(capacity)
    }

    fn AllocWithLen(&mut self, length: usize, capacity: usize) -> ArenaBuffer<'_> {
        let mut data = ArenaBuffer::owned(capacity);
        data.set_len(length);
        data
    }

    fn Reset(&mut self) {}
}

// Go 的 `var _ Allocator = &stdAllocator{}` 是接口实现断言；Rust 通过 impl Allocator for stdAllocator 表达。

// StdAllocator implements Allocator but do not pre-allocate memory.
/// 构造不预分配内存的标准分配器。
pub fn StdAllocator() -> stdAllocator {
    stdAllocator {}
}

// NewAllocator creates an Allocator with a specified capacity.
/// 创建指定容量的 `SimpleAllocator`，`off` 从 0 开始。
pub fn NewAllocator(capacity: usize) -> SimpleAllocator {
    SimpleAllocator {
        arena: vec![0; capacity],
        off: 0,
    }
}

impl Allocator for SimpleAllocator {
    // Alloc implements Allocator.AllocBytes interface.
    /// 若 `off + capacity` 仍落在 arena 内则切分子切片并推进 `off`，否则回退到堆分配且不推进 `off`。
    fn Alloc(&mut self, capacity: usize) -> ArenaBuffer<'_> {
        // Go 条件为 end < len(arena)（不含等于），边界处同样走堆分配。
        if self
            .off
            .checked_add(capacity)
            .is_some_and(|end| end < self.arena.len())
        {
            let start = self.off;
            self.off += capacity;
            return ArenaBuffer::from_arena(&mut self.arena[start..start + capacity]);
        }

        ArenaBuffer::owned(capacity)
    }

    // AllocWithLen implements Allocator.AllocWithLen interface.
    /// 先按容量 `Alloc`，再把可见长度设为 `length`。
    fn AllocWithLen(&mut self, length: usize, capacity: usize) -> ArenaBuffer<'_> {
        let mut slice = self.Alloc(capacity);
        slice.set_len(length);
        slice
    }

    // Reset implements Allocator.Reset interface.
    /// 仅将 `off` 置 0，不清空 arena 内容。
    fn Reset(&mut self) {
        // Reset 只复位 offset，不清空 arena；调用者必须确保旧切片不再使用，和 Go 注释一致。
        self.off = 0;
    }
}
