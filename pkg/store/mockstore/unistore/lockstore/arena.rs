// Copyright 2019-present PingCAP, Inc.
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

// arena 分块内存池：地址编码、块分配、增长与延迟复用。
//
// arena 把多个固定大小的字节块串成可分配空间，用 `arenaAddr`（高 32 位块号+1、低 32 位偏移）
// 定位数据。`free` 后块进入 pending，等待 `reuseSafeDuration` 再写，降低无锁读者读到被覆盖数据的概率。

// 对应 arena.go，实现 arena 地址、块分配、增长与延迟复用。

use std::cell::{Cell, UnsafeCell};
use std::sync::Arc;
use std::time::{Duration, Instant};

// arenaAddr 对应 Go 的 uint64 地址编码，高 32 位保存 block index+1，低 32 位保存块内偏移。
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
/// arena 地址：高 32 位为 block index+1，低 32 位为块内偏移。
pub struct arenaAddr(pub u64);

/// 8 字节对齐掩码（低 3 位清零）。
pub const alignMask: usize = (1usize << 32) - 8; // 29 bit 1 and 3 bit 0.
/// 分配失败时的非法块内偏移。
pub const nullBlockOffset: u32 = u32::MAX;
/// 空地址哨兵（block index 编码为 0）。
pub const nullArenaAddr: arenaAddr = arenaAddr(0);

// Time waited until we reuse the empty block.
// Data corruption can happen under this time sequence.
// 1. a reader reads a node.
// 2. a writer delete the node, then free the block, put it into the writable queue
// 	and this block become the first writable block.
// 3. The writer insert another node, overwrite the block we just freed.
// 4. The reader reads the key/value of that delete node.
// But because the time between 1 and 4 is very short, this is very unlikely to happen but it can happen.
// So we wait for a while so the reader can finish reading before we overwrite the empty block.
// reuseSafeDuration 保留 Go 的 100ms 延迟复用窗口，用来降低无锁读者读到被覆盖块的概率。
/// 空块延迟复用安全窗口（100ms）。
pub const reuseSafeDuration: Duration = Duration::from_millis(100);

impl arenaAddr {
    // blockIdx 对应 Go 的 arenaAddr.blockIdx；nullArenaAddr 不应调用本方法，否则会发生下溢。
    /// 解码块下标（存储值减一）。
    pub fn blockIdx(self) -> usize {
        ((self.0 >> 32) as usize) - 1
    }

    // blockOffset 对应 Go 的 arenaAddr.blockOffset，取低 32 位作为块内偏移。
    /// 解码块内偏移。
    pub fn blockOffset(self) -> u32 {
        self.0 as u32
    }
}

// newArenaAddr 对应 Go 的地址打包函数，blockIdx 存储时加一以便 0 能表示空地址。
/// 打包块下标与偏移为 arenaAddr。
pub fn newArenaAddr(blockIdx: usize, blockOffset: u32) -> arenaAddr {
    arenaAddr(((blockIdx as u64 + 1) << 32) | blockOffset as u64)
}

// arena 对应 Go 的 arena 定位器，负责在多个 arenaBlock 之间分配和延迟复用。
/// arena 定位器：管理多块缓冲、可写队列与待复用块。
pub struct arena {
    pub blockSize: usize,
    pub blocks: Vec<Arc<arenaBlock>>,
    pub writableQueue: Vec<usize>,
    pub pendingBlocks: Vec<pendingBlock>,
}

// pendingBlock 对应 Go 的待复用块记录，直到 reusableTime 后才重新进入 writableQueue。
#[derive(Clone)]
/// 待复用块：到达 reusableTime 后才可重新写入。
pub struct pendingBlock {
    pub blockIdx: usize,
    pub reusableTime: Instant,
}

// newArenaLocator 对应 Go 的构造函数，初始只创建一个可写 block。
/// 构造仅含一个可写块的 arena。
pub fn newArenaLocator(blockSize: usize) -> arena {
    arena {
        blockSize,
        blocks: vec![newArenaBlock(blockSize)],
        writableQueue: vec![0],
        pendingBlocks: Vec::new(),
    }
}

impl arena {
    // get 对应 Go 的 arena.get，按 arenaAddr 与 size 返回块内数据切片。
    /// 按地址取只读切片。
    pub fn get(&self, addr: arenaAddr, size: usize) -> &[u8] {
        if addr.blockIdx() >= self.blocks.len() {
            panic!(
                "arena.get out of range: len(blocks)={}, block={}, offset={}, size={}",
                self.blocks.len(),
                addr.blockIdx(),
                addr.blockOffset(),
                size
            );
        }
        self.blocks[addr.blockIdx()].get(addr.blockOffset(), size)
    }

    // get_mut 是为 newNode 写入节点内存补出的可变切片形状，对应 Go 中同一底层 []byte。
    /// 按地址取可变切片（供节点写入）。
    pub fn get_mut(&mut self, addr: arenaAddr, size: usize) -> &mut [u8] {
        if addr.blockIdx() >= self.blocks.len() {
            panic!(
                "arena.get out of range: len(blocks)={}, block={}, offset={}, size={}",
                self.blocks.len(),
                addr.blockIdx(),
                addr.blockOffset(),
                size
            );
        }
        self.blocks[addr.blockIdx()].get_mut(addr.blockOffset(), size)
    }

    // alloc 对应 Go 的 arena.alloc，优先使用 writableQueue 尾部 block，满块会被弹出。
    /// 分配 size 字节；优先复用 pending，否则可能返回 nullArenaAddr。
    pub fn alloc(&mut self, size: usize) -> arenaAddr {
        loop {
            if self.writableQueue.is_empty() {
                if !self.pendingBlocks.is_empty() {
                    let pending = &self.pendingBlocks[0];
                    // 到达安全时间后，pending block 才能重新写入，避免并发读者读到刚被覆盖的数据。
                    if Instant::now() >= pending.reusableTime {
                        let pending = self.pendingBlocks.remove(0);
                        self.writableQueue.push(pending.blockIdx);
                        continue;
                    }
                }
                return nullArenaAddr;
            }

            let availIdx = *self.writableQueue.last().unwrap();
            let blockOffset = self.blocks[availIdx].alloc(size);
            if blockOffset != nullBlockOffset {
                return newArenaAddr(availIdx, blockOffset);
            }
            self.writableQueue.pop();
        }
    }

    // free decrease the arena block reference and makes the block reusable.
    // We don't know if there is concurrent reader who may reference the deleted entry.
    // So we must make sure the old data is not referenced for long time, and we only overwrite
    // it after a safe amount of time.
    // free 对应 Go 的引用计数递减与延迟复用逻辑；这里保留单写多读场景下的安全窗口。
    /// 递减引用；无引用且曾写满则进入延迟复用队列。
    pub fn free(&mut self, addr: arenaAddr) {
        let blockIdx = addr.blockIdx();
        let arena = &self.blocks[blockIdx];
        arena.refCount.set(arena.refCount.get() - 1);
        // No reference, the arenaBlock can be reused.
        if arena.refCount.get() == 0 && arena.length.get() > arena.buf_len() {
            self.pendingBlocks.push(pendingBlock {
                blockIdx,
                reusableTime: Instant::now() + reuseSafeDuration,
            });
            arena.length.set(0);
        }
    }

    // grow 对应 Go 的 arena.grow，复制定位器元数据并追加一个新块；旧 block 继续被现有节点引用。
    /// 复制定位器并追加新块（旧块仍被现有节点引用）。
    pub fn grow(&self) -> arena {
        let mut newLoc = arena {
            blockSize: self.blockSize,
            blocks: Vec::with_capacity(self.blocks.len() + 1),
            writableQueue: Vec::new(),
            pendingBlocks: self.pendingBlocks.clone(),
        };
        newLoc.blocks.extend(self.blocks.iter().cloned());
        let availIdx = newLoc.blocks.len();
        newLoc.blocks.push(newArenaBlock(self.blockSize));
        newLoc.writableQueue.push(availIdx);
        newLoc
    }

    // growInPlace keeps every block buffer allocation stable while extending the
    // locator. This is the Rust equivalent used by the single-writer path: moving
    // arenaBlock values does not move their Vec-backed node bytes.
    /// 原地追加新块，保持既有 block 缓冲地址稳定。
    pub fn growInPlace(&mut self) {
        let availIdx = self.blocks.len();
        self.blocks.push(newArenaBlock(self.blockSize));
        self.writableQueue.push(availIdx);
    }
}

// arenaBlock 对应 Go 的单个连续字节块，refCount 统计仍被节点引用的分配数量。
/// 单个连续字节块及其引用计数与已用长度。
pub struct arenaBlock {
    buf: UnsafeCell<Vec<u8>>,
    refCount: Cell<u64>,
    length: Cell<usize>,
}

// One writer mutates allocation metadata and unpublished byte ranges. Readers
// only inspect published node ranges, matching Go's single-writer contract.
unsafe impl Send for arenaBlock {}
unsafe impl Sync for arenaBlock {}

// newArenaBlock 对应 Go 的块构造函数，预分配固定大小的字节缓冲区。
/// 预分配固定大小字节缓冲的块。
pub fn newArenaBlock(blockSize: usize) -> Arc<arenaBlock> {
    Arc::new(arenaBlock {
        buf: UnsafeCell::new(vec![0; blockSize]),
        refCount: Cell::new(0),
        length: Cell::new(0),
    })
}

impl arenaBlock {
    fn buf_len(&self) -> usize {
        unsafe { (&*self.buf.get()).len() }
    }

    // get 对应 Go 的 arenaBlock.get，返回 offset 起始的指定长度切片。
    /// 块内只读切片。
    pub fn get(&self, offset: u32, size: usize) -> &[u8] {
        let start = offset as usize;
        unsafe { &(&*self.buf.get())[start..start + size] }
    }

    // get_mut 保留 Go 中通过 arena 字节切片写入 node header、key、value 的可变访问方式。
    /// 块内可变切片。
    pub fn get_mut(&self, offset: u32, size: usize) -> &mut [u8] {
        let start = offset as usize;
        unsafe { &mut (&mut *self.buf.get())[start..start + size] }
    }

    // alloc 对应 Go 的 8 字节对齐分配；分配失败时 length 仍保持越界状态，供 free 判断块曾被写满。
    /// 8 字节对齐分配；失败返回 nullBlockOffset。
    pub fn alloc(&self, size: usize) -> u32 {
        // The returned addr should be aligned in 8 bytes.
        let offset = (self.length.get() + 7) & alignMask;
        self.length.set(offset + size);
        if self.length.get() > self.buf_len() {
            return nullBlockOffset;
        }
        self.refCount.set(self.refCount.get() + 1);
        offset as u32
    }
}
