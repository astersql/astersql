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

// 内存块池与顺序 Buffer：在固定块内切分小对象，大对象独立分配。
//
// 对应 Go `membuf`：Pool 缓存固定大小块；Buffer 从池渐进取块并 bump-pointer 分配；
// 可选 Limiter 限制已借出块与小对象元数据开销的总量。

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use super::limiter::{ErrCannotAcquireMemory, Limiter};

/// 默认块缓存容量（块个数）。
const defaultPoolSize: usize = 1024;
/// 默认单块大小：1 MiB。
const defaultBlockSize: usize = 1 << 20; // 1 MiB

/// Allocator 对应 Go 的内存分配/释放抽象接口。
pub trait Allocator: Send + Sync {
    /// 分配长度为 n 的清零字节缓冲。
    fn Alloc(&self, n: usize) -> Vec<u8>;
    /// 释放先前由 Alloc 得到的缓冲。
    fn Free(&self, bytes: Vec<u8>);
}

/// stdAllocator 对应 Go 默认分配器；Vec 的 Drop 代替 Go 垃圾回收器处理 Free 空操作。
pub struct stdAllocator;

impl Allocator for stdAllocator {
    fn Alloc(&self, n: usize) -> Vec<u8> {
        vec![0; n]
    }

    fn Free(&self, bytes: Vec<u8>) {
        drop(bytes);
    }
}

/// Pool 对应 Go 的块池。Mutex<VecDeque<_>> 表达固定容量 channel 的并发 FIFO 缓存语义。
pub struct Pool {
    /// 实际申请/释放块内存的分配器。
    allocator: Arc<dyn Allocator>,
    /// 每个固定块的字节数。
    blockSize: usize,
    /// 已归还、可复用的块缓存。
    blockCache: Mutex<VecDeque<Vec<u8>>>,
    /// 缓存中最多保留的块数。
    blockCacheCapacity: usize,
    /// 可选的借用块总量限制器。
    limiter: std::option::Option<Arc<Limiter>>,
}

/// Option 对应 Go 的 Pool functional option。
pub type Option = Box<dyn FnOnce(&mut Pool)>;

/// WithBlockNum 配置池最多缓存多少个块。
pub fn WithBlockNum(num: usize) -> Option {
    Box::new(move |pool| {
        pool.blockCacheCapacity = num;
        pool.blockCache = Mutex::new(VecDeque::with_capacity(num));
    })
}

/// WithBlockSize 配置每个固定块的字节数。
pub fn WithBlockSize(bytes: usize) -> Option {
    Box::new(move |pool| pool.blockSize = bytes)
}

/// WithAllocator 配置块池使用的分配器。
pub fn WithAllocator(allocator: Arc<dyn Allocator>) -> Option {
    Box::new(move |pool| pool.allocator = allocator)
}

/// WithPoolMemoryLimiter 配置返回给 Buffer 的固定块总量限制。
/// 与 Go 一致，超过 blockSize 的独立大对象不计入该限制。
pub fn WithPoolMemoryLimiter(limiter: Arc<Limiter>) -> Option {
    Box::new(move |pool| pool.limiter = Some(limiter))
}

/// NewPool 创建带默认分配器、1 MiB 块和 1024 块缓存上限的池，再按顺序应用选项。
pub fn NewPool(opts: Vec<Option>) -> Arc<Pool> {
    let mut pool = Pool {
        allocator: Arc::new(stdAllocator),
        blockSize: defaultBlockSize,
        blockCache: Mutex::new(VecDeque::with_capacity(defaultPoolSize)),
        blockCacheCapacity: defaultPoolSize,
        limiter: None,
    };
    for opt in opts {
        opt(&mut pool);
    }
    Arc::new(pool)
}

impl Pool {
    /// acquire 先阻塞取得一个块的配额，再从缓存或分配器取得实际内存。
    fn acquire(&self) -> Vec<u8> {
        // 配额先占后取块，保证 Limiter 统计覆盖实际借出的内存。
        if let Some(limiter) = &self.limiter {
            limiter.Acquire(self.blockSize);
        }
        self.takeBlock()
    }

    /// takeBlock 对应 Go 非阻塞 select：缓存为空时立即新分配，不等待归还。
    fn takeBlock(&self) -> Vec<u8> {
        if let Some(block) = self.blockCache.lock().unwrap().pop_front() {
            return block;
        }
        self.allocator.Alloc(self.blockSize)
    }

    /// release 尝试把块放回有界缓存；缓存已满时交给分配器释放，最后归还 Limiter 配额。
    fn release(&self, block: Vec<u8>) {
        // 先尝试入缓存；溢出的块在解锁后再 Free，缩短持锁时间。
        let mut pending = Some(block);
        {
            let mut cache = self.blockCache.lock().unwrap();
            if cache.len() < self.blockCacheCapacity {
                cache.push_back(pending.take().unwrap());
            }
        }
        if let Some(block) = pending {
            self.allocator.Free(block);
        }
        if let Some(limiter) = &self.limiter {
            limiter.Release(self.blockSize);
        }
    }

    /// Destroy 对应关闭 Go channel 后排空缓存；这里一次取出缓存，避免持锁调用外部分配器。
    pub fn Destroy(&self) {
        // 先 swap 出全部缓存块，再在无锁状态下逐个 Free。
        let blocks = {
            let mut cache = self.blockCache.lock().unwrap();
            std::mem::take(&mut *cache)
        };
        for block in blocks {
            self.allocator.Free(block);
        }
    }

    /// TotalSize 只统计仍在 Pool 缓存中的块，不包含已借给 Buffer 的内存。
    pub fn TotalSize(&self) -> i64 {
        (self.blockCache.lock().unwrap().len() * self.blockSize) as i64
    }

    /// NewBuffer 创建渐进取得块、销毁时统一归还的 Buffer。
    pub fn NewBuffer(self: &Arc<Self>, opts: Vec<BufferOption>) -> Buffer {
        let mut buffer = Buffer {
            pool: Arc::clone(self),
            blocks: Vec::new(),
            blockCntLimit: -1,
            curBlockIdx: -1,
            curIdx: 0,
            smallObjOverhead: 0,
            smallObjOverheadCache: 0,
        };
        for opt in opts {
            opt(&mut buffer);
        }
        if buffer.blocks.capacity() == 0 {
            buffer.blocks = Vec::with_capacity(128);
        }
        buffer
    }
}

/// Buffer 对应 Go 的块内顺序分配器。
/// Rust 通过 curBlockIdx 访问 blocks，避免额外保存一个可变切片别名；其逻辑等价于 Go 的 curBlock 字段。
pub struct Buffer {
    /// 借出/归还固定块的所属池。
    pool: Arc<Pool>,
    /// 当前持有的固定块列表。
    blocks: Vec<Vec<u8>>,
    /// 允许持有的最大块数；`-1` 表示不限制。
    blockCntLimit: isize,
    /// 当前写块在 `blocks` 中的下标；`-1` 表示尚未取得块。
    curBlockIdx: isize,
    /// 当前块内下一个可分配字节的偏移。
    curIdx: usize,
    /// 已向 Limiter 登记的小对象元数据开销总量。
    smallObjOverhead: usize,
    /// 本地尚未摊销完的元数据配额余额。
    smallObjOverheadCache: usize,
}

/// BufferOption 对应 Go 的 Buffer functional option。
pub type BufferOption = Box<dyn FnOnce(&mut Buffer)>;

/// WithBufferMemoryLimit 将近似字节上限向上对齐为块数，并预留块索引空间。
pub fn WithBufferMemoryLimit(limit: u64) -> BufferOption {
    Box::new(move |buffer| {
        let count = getBlockCnt(limit, buffer.pool.blockSize as u64) as usize;
        buffer.blockCntLimit = count as isize;
        buffer.blocks = Vec::with_capacity(count);
    })
}

/// GetAlignedSize 返回按 blockSize 向上对齐后的字节数。
pub fn GetAlignedSize(size: u64, blockSize: u64) -> u64 {
    getBlockCnt(size, blockSize).wrapping_mul(blockSize)
}

/// getBlockCnt 对应 ceil(size / blockSize)；调用者沿用 Go 对 blockSize 非零的前提。
fn getBlockCnt(size: u64, blockSize: u64) -> u64 {
    size.wrapping_add(blockSize).wrapping_sub(1) / blockSize
}

/// 一批 256 KiB 配额可摊销大量切片头或 SliceLocation 的元数据开销。
const smallObjOverheadBatch: usize = 256 * 1024;
/// `Vec<u8>` 切片头大小，用于估算小对象元数据开销。
const sizeOfSlice: usize = std::mem::size_of::<Vec<u8>>();

/// AllocatedBytes 区分独立大对象与借用自块池的切片，同时保留 Go `[]byte` 的统一返回概念。
pub enum AllocatedBytes<'a> {
    /// 超过块大小时独立拥有的大对象。
    Owned(Vec<u8>),
    /// 落在 Buffer 固定块内的借用切片。
    Borrowed(&'a mut [u8]),
}

impl<'a> AllocatedBytes<'a> {
    /// 取得可变字节视图，便于统一写入。
    fn as_mut_slice(&mut self) -> &mut [u8] {
        match self {
            Self::Owned(bytes) => bytes.as_mut_slice(),
            Self::Borrowed(bytes) => bytes,
        }
    }
}

/// SliceLocation 对应不含指针的紧凑切片位置，降低大量小对象给 Go GC 带来的扫描成本。
#[derive(Clone, Copy, Default)]
pub struct SliceLocation {
    /// 所属块在 Buffer.blocks 中的下标。
    bufIdx: i32,
    /// 块内起始偏移。
    offset: i32,
    /// 切片字节长度。
    pub Length: i32,
}

/// `SliceLocation` 结构体大小，用于元数据配额记账。
const sizeOfSliceLocation: usize = std::mem::size_of::<SliceLocation>();

impl Buffer {
    /// recordSmallObjOverhead 按 256 KiB 批量从 Limiter 取得小对象元数据配额。
    fn recordSmallObjOverhead(&mut self, n: usize) {
        // 本地余额不足时再批量 Acquire，摊销频繁小对象的限制器开销。
        if n > self.smallObjOverheadCache {
            self.pool
                .limiter
                .as_ref()
                .unwrap()
                .Acquire(smallObjOverheadBatch);
            self.smallObjOverheadCache += smallObjOverheadBatch;
            self.smallObjOverhead += smallObjOverheadBatch;
        }
        self.smallObjOverheadCache -= n;
    }

    /// releaseSmallObjOverhead 一次归还此前按批取得的全部元数据配额并清零本地账本。
    fn releaseSmallObjOverhead(&mut self) {
        self.pool
            .limiter
            .as_ref()
            .unwrap()
            .Release(self.smallObjOverhead);
        self.smallObjOverhead = 0;
        self.smallObjOverheadCache = 0;
    }

    /// Reset 复用已持有块，从首块重新开始分配；调用前必须丢弃此前返回的所有引用。
    pub fn Reset(&mut self) {
        if self.pool.limiter.is_some() {
            self.releaseSmallObjOverhead();
        }
        if !self.blocks.is_empty() {
            self.curBlockIdx = 0;
            self.curIdx = 0;
        }
    }

    /// Destroy 归还所有块和元数据配额，并恢复未分配状态。
    pub fn Destroy(&mut self) {
        if self.pool.limiter.is_some() {
            self.releaseSmallObjOverhead();
        }
        for block in self.blocks.drain(..) {
            self.pool.release(block);
        }
        self.curBlockIdx = -1;
        self.curIdx = 0;
    }

    /// TotalSize 返回 Buffer 当前持有的固定块总字节数。
    pub fn TotalSize(&self) -> i64 {
        (self.blocks.len() * self.pool.blockSize) as i64
    }

    /// 当前写块的可用长度；尚未取得块时视为 0。
    fn currentBlockLen(&self) -> usize {
        if self.curBlockIdx < 0 {
            0
        } else {
            self.blocks[self.curBlockIdx as usize].len()
        }
    }

    /// AllocBytes 分配 n 字节；大对象绕过池和 Limiter，小对象从当前固定块切出。
    pub fn AllocBytes(&mut self, n: usize) -> std::option::Option<AllocatedBytes<'_>> {
        // 超过块大小：独立分配，不占用池与 Limiter。
        if n > self.pool.blockSize {
            return Some(AllocatedBytes::Owned(vec![0; n]));
        }

        let location = self.allocateLocation(n)?;
        if location.bufIdx < 0 {
            return None;
        }
        if self.pool.limiter.is_some() {
            self.recordSmallObjOverhead(sizeOfSlice);
        }
        Some(AllocatedBytes::Borrowed(self.GetSlice(&location)))
    }

    /// TryAllocBytes 是非阻塞版本；配额不能立即取得时返回 ErrCannotAcquireMemory，且不改变 Buffer 状态。
    pub fn TryAllocBytes(
        &mut self,
        n: usize,
    ) -> Result<std::option::Option<AllocatedBytes<'_>>, String> {
        if n > self.pool.blockSize {
            return Ok(Some(AllocatedBytes::Owned(vec![0; n])));
        }
        let Some(location) = self.tryAllocLocation(n)? else {
            return Ok(None);
        };
        if location.bufIdx < 0 {
            return Ok(None);
        }
        Ok(Some(AllocatedBytes::Borrowed(self.GetSlice(&location))))
    }

    /// 在当前块或新块中占位 n 字节，返回紧凑位置；超块大小或触达块数上限时返回 None。
    fn allocateLocation(&mut self, n: usize) -> std::option::Option<SliceLocation> {
        if n > self.pool.blockSize {
            return None;
        }

        // 当前块剩余不足时取下一块；若已达 blockCntLimit 则失败。
        if self.curIdx + n > self.currentBlockLen() {
            if self.blockCntLimit >= 0 && self.curBlockIdx + 1 >= self.blockCntLimit {
                return None;
            }
            self.addBlock();
        }

        let block_idx = self.curBlockIdx as usize;
        let start = self.curIdx;
        self.curIdx += n;
        let location = SliceLocation {
            bufIdx: block_idx as i32,
            offset: start as i32,
            Length: n as i32,
        };
        Some(location)
    }

    /// 非阻塞占位：先 TryAcquire 所需配额，失败则不修改 Buffer 状态。
    fn tryAllocLocation(&mut self, n: usize) -> Result<std::option::Option<SliceLocation>, String> {
        let need_block = self.curIdx + n > self.currentBlockLen();
        if need_block && self.blockCntLimit >= 0 && self.curBlockIdx + 1 >= self.blockCntLimit {
            return Ok(None);
        }

        if let Some(limiter) = &self.pool.limiter {
            let mut need_bytes = 0;
            // 仅当确实需要新块（而非 Reset 后复用）时才计入块配额。
            if need_block && self.curBlockIdx >= self.blocks.len() as isize - 1 {
                need_bytes += self.pool.blockSize;
            }
            if sizeOfSlice > self.smallObjOverheadCache {
                need_bytes += smallObjOverheadBatch;
            }
            // 所有状态修改都在 TryAcquire 成功之后，保留 Go 的失败原子性保证。
            if need_bytes > 0 && !limiter.TryAcquire(need_bytes) {
                return Err(ErrCannotAcquireMemory.to_owned());
            }

            if need_block {
                self.addBlockWithReservedLimiterQuota();
            }
            if sizeOfSlice > self.smallObjOverheadCache {
                self.smallObjOverheadCache += smallObjOverheadBatch;
                self.smallObjOverhead += smallObjOverheadBatch;
            }
            self.smallObjOverheadCache -= sizeOfSlice;
        } else if need_block {
            self.addBlock();
        }

        let start = self.curIdx;
        self.curIdx += n;
        Ok(Some(SliceLocation {
            bufIdx: self.curBlockIdx as i32,
            offset: start as i32,
            Length: n as i32,
        }))
    }

    /// AllocBytesWithSliceLocation 强制从池内分配，并同时返回无指针位置句柄；失败时 bytes 为 None。
    pub fn AllocBytesWithSliceLocation(
        &mut self,
        n: usize,
    ) -> (std::option::Option<&mut [u8]>, SliceLocation) {
        let Some(location) = self.allocateLocation(n) else {
            return (None, SliceLocation::default());
        };
        if location.bufIdx < 0 {
            return (None, location);
        }
        if self.pool.limiter.is_some() {
            self.recordSmallObjOverhead(sizeOfSliceLocation);
        }
        (Some(self.GetSlice(&location)), location)
    }

    /// addBlock 优先切换到 Reset 后保留的下一块，否则阻塞取得新块及其 Limiter 配额。
    fn addBlock(&mut self) {
        if self.switchToNextBlock() {
            return;
        }
        self.appendBlock(self.pool.acquire());
    }

    /// 配额已由 TryAcquire 预留，因此这里只取缓存块或分配新块，不能再次 Acquire。
    fn addBlockWithReservedLimiterQuota(&mut self) {
        if self.switchToNextBlock() {
            return;
        }
        self.appendBlock(self.pool.takeBlock());
    }

    /// switchToNextBlock 复用 Buffer 已持有但 Reset 后尚未重新用到的块。
    fn switchToNextBlock(&mut self) -> bool {
        if self.curBlockIdx < self.blocks.len() as isize - 1 {
            self.curBlockIdx += 1;
            self.curIdx = 0;
            return true;
        }
        false
    }

    /// appendBlock 把新块加入 Buffer，并把当前写指针切到块首。
    fn appendBlock(&mut self, block: Vec<u8>) {
        self.blocks.push(block);
        self.curBlockIdx = self.blocks.len() as isize - 1;
        self.curIdx = 0;
    }

    /// GetSlice 根据紧凑位置重新取得可变切片；调用者必须保证位置来自当前 Buffer 且尚未失效。
    pub fn GetSlice(&mut self, location: &SliceLocation) -> &mut [u8] {
        let block = &mut self.blocks[location.bufIdx as usize];
        let start = location.offset as usize;
        let end = start + location.Length as usize;
        &mut block[start..end]
    }

    /// AddBytes 在 Buffer 中分配同长度区域并复制输入。
    pub fn AddBytes(&mut self, bytes: &[u8]) -> std::option::Option<AllocatedBytes<'_>> {
        let mut target = self.AllocBytes(bytes.len())?;
        target.as_mut_slice().copy_from_slice(bytes);
        Some(target)
    }

    /// TryAddBytes 是 AddBytes 的非阻塞版本，原样传播配额错误或容量上限导致的 None。
    pub fn TryAddBytes(
        &mut self,
        bytes: &[u8],
    ) -> Result<std::option::Option<AllocatedBytes<'_>>, String> {
        let Some(mut target) = self.TryAllocBytes(bytes.len())? else {
            return Ok(None);
        };
        target.as_mut_slice().copy_from_slice(bytes);
        Ok(Some(target))
    }
}

#[cfg(test)]
#[path = "buffer_test.rs"]
mod buffer_test;
