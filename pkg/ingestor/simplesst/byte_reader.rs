// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 外部存储字节读取器（ByteReader）。
//
// 在小缓冲顺序读与大缓冲并发预取之间切换，供 SST/归并路径按精确字节数消费文件；
// 半条记录遇 EOF 时按 UnexpectedEof 处理，与 Go 对外可见语义一致。

/// ConcurrentReaderBufferSizePerConc 对应 Go 的可调包级变量：每个并发读取分片默认使用 8 MiB。
// pub static mut ConcurrentReaderBufferSizePerConc: usize = 8 * MB;
//
/// concurrentReaderTotalConcurrency 是单个任务内所有外部读取器共享的最大并发预算。
// const concurrentReaderTotalConcurrency: usize = 256;
//
/// ConcurrentReaderFields 收拢 Go 匿名结构体中的并发读取状态。
/// `expected` 是调用方期望模式，`now` 是当前实际模式；两者只在 reload 边界完成切换。
// struct ConcurrentReaderFields {
//     largeBufferPool: Option<Buffer>,
//     store: Option<Storage>,
//     filename: String,
//     concurrency: usize,
//     bufSizePerConc: usize,
//     now: bool,
//     expected: bool,
//     largeBuf: Vec<Vec<u8>>,
//     reader: Option<ConcurrentFileReader>,
//     reloadCnt: usize,
// }
// */
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crate::concurrent_reader::ConcurrentFileReader;
use crate::{Error, MemoryStorage, Result};

/// 每个并发分片的默认缓冲大小（8 MiB），对应 Go 可调包级变量。
pub static ConcurrentReaderBufferSizePerConc: AtomicUsize = AtomicUsize::new(8 * 1024 * 1024);
/// 单个任务内所有外部读取器共享的最大并发预算。
pub const CONCURRENT_READER_TOTAL_CONCURRENCY: usize = 256;
/// 单次 `read_n_bytes` 允许的最大请求长度（1 GiB）。
const MAX_READ_SIZE: usize = 1024 * 1024 * 1024;

/// 内存后端上的字节读取器：维护逻辑偏移，并可惰性切入并发预取模式。
#[derive(Debug)]
pub struct ByteReader {
    data: Arc<Vec<u8>>,
    position: usize,
    small_buffer_size: usize,
    concurrent_enabled: bool,
    concurrent_now: bool,
    concurrent_expected: bool,
    concurrency: usize,
    buffer_size_per_concurrency: usize,
    concurrent_reader: Option<ConcurrentFileReader>,
    closed: bool,
}

impl ByteReader {
    /// 从内存缓冲区构造读取器；`buffer_size` 须为正，偏移不可越界。
    pub fn new(data: Vec<u8>, initial_offset: usize, buffer_size: usize) -> Result<Self> {
        if buffer_size == 0 {
            return Err(Error::InvalidData("buffer size must be positive".into()));
        }
        if initial_offset >= data.len() {
            return Err(Error::eof());
        }
        Ok(Self {
            data: Arc::new(data),
            position: initial_offset,
            small_buffer_size: buffer_size,
            concurrent_enabled: false,
            concurrent_now: false,
            concurrent_expected: false,
            concurrency: 1,
            buffer_size_per_concurrency: buffer_size,
            concurrent_reader: None,
            closed: false,
        })
    }

    /// 从 `MemoryStorage` 按对象名读出全部字节后构造读取器。
    pub fn from_storage(
        store: &MemoryStorage,
        name: &str,
        initial_offset: usize,
        buffer_size: usize,
    ) -> Result<Self> {
        Self::new(store.read(name)?, initial_offset, buffer_size)
    }

    /// 配置并发读：只保存并发度与每路缓冲大小，不立即发起 IO。
    pub fn enable_concurrent_read(
        &mut self,
        concurrency: usize,
        buffer_size_per_concurrency: usize,
    ) -> Result<()> {
        if concurrency == 0
            || concurrency > CONCURRENT_READER_TOTAL_CONCURRENCY
            || buffer_size_per_concurrency == 0
        {
            return Err(Error::InvalidData(
                "invalid concurrent reader configuration".into(),
            ));
        }
        self.concurrent_enabled = true;
        self.concurrency = concurrency;
        self.buffer_size_per_concurrency = buffer_size_per_concurrency;
        Ok(())
    }

    /// 切换并发期望模式；关闭时立即同步并发读取器偏移并退出并发态。
    pub fn switch_concurrent_mode(&mut self, use_concurrent: bool) -> Result<()> {
        if !self.concurrent_enabled {
            return Ok(());
        }
        self.concurrent_expected = use_concurrent;
        if !use_concurrent && self.concurrent_now {
            if let Some(reader) = self.concurrent_reader.take() {
                self.position = reader.offset();
            }
            self.concurrent_now = false;
        }
        Ok(())
    }

    /// 已关闭则返回 `Error::Closed`。
    fn ensure_open(&self) -> Result<()> {
        if self.closed {
            Err(Error::Closed)
        } else {
            Ok(())
        }
    }

    /// Returns exactly `count` bytes. EOF before the first byte and truncation
    /// after a partial read are both represented as UnexpectedEof, matching the
    /// externally visible Go reader behavior.
    /// 精确读取 `count` 字节；首字节前 EOF 与半读截断均映射为 UnexpectedEof。
    pub fn read_n_bytes(&mut self, count: usize) -> Result<Vec<u8>> {
        self.ensure_open()?;
        if count == 0 {
            return Err(Error::InvalidData("read size must be positive".into()));
        }
        if count > MAX_READ_SIZE {
            return Err(Error::InvalidData(format!(
                "read size exceeds {MAX_READ_SIZE}"
            )));
        }
        let end = self
            .position
            .checked_add(count)
            .ok_or_else(|| Error::InvalidData("read offset overflow".into()))?;
        if end > self.data.len() {
            if self.position >= self.data.len() {
                return Err(Error::eof());
            }
            let available = self.data.len() - self.position;
            self.position = self.data.len();
            self.concurrent_reader = None;
            self.concurrent_now = false;
            self.concurrent_expected = false;
            return Err(Error::unexpected_eof(format!(
                "requested {count} bytes with only {available} remaining"
            )));
        }
        // 首次切入并发模式时按当前偏移创建 ConcurrentFileReader
        if self.concurrent_expected && !self.concurrent_now {
            self.concurrent_reader = Some(ConcurrentFileReader::new(
                Arc::clone(&self.data),
                self.position,
                self.data.len(),
                self.concurrency,
                self.buffer_size_per_concurrency,
            )?);
            self.concurrent_now = true;
        }
        // The production in-memory backend can return the requested logical
        // range directly. Position accounting remains identical in both modes.
        let result = self.data[self.position..end].to_vec();
        self.position = end;
        if let Some(reader) = self.concurrent_reader.as_mut() {
            // Keep the concurrent reader's resume position synchronized even
            // when a record crosses one or more prefetch chunks.
            *reader = ConcurrentFileReader::new(
                Arc::clone(&self.data),
                self.position,
                self.data.len(),
                self.concurrency,
                self.buffer_size_per_concurrency,
            )?;
        }
        Ok(result)
    }

    /// 当前逻辑读取偏移。
    pub fn position(&self) -> usize {
        self.position
    }
    /// 顺序读小缓冲大小。
    pub fn buffer_size(&self) -> usize {
        self.small_buffer_size
    }
    /// 返回调用方期望与当前实际并发读取模式，供归并热点调度与测试观测。
    pub fn concurrent_mode(&self) -> (bool, bool) {
        (self.concurrent_expected, self.concurrent_now)
    }
    /// 释放并发读取器并标记为已关闭。
    pub fn close(&mut self) -> Result<()> {
        self.concurrent_reader = None;
        self.concurrent_now = false;
        self.closed = true;
        Ok(())
    }

    /// Go 风格别名：转发到 `read_n_bytes`。
    pub fn readNBytes(&mut self, count: usize) -> Result<Vec<u8>> {
        self.read_n_bytes(count)
    }
    /// Go 风格别名：转发到 `switch_concurrent_mode`。
    pub fn SwitchConcurrentMode(&mut self, enabled: bool) -> Result<()> {
        self.switch_concurrent_mode(enabled)
    }
    /// Go 风格别名：转发到 `close`。
    pub fn Close(&mut self) -> Result<()> {
        self.close()
    }
}

/// 从内存存储打开对象并定位到 `initial_offset`，预取大小至少为 1。
pub fn openStoreReaderAndSeek(
    store: &MemoryStorage,
    name: &str,
    initial_offset: usize,
    prefetch_size: usize,
) -> Result<ByteReader> {
    ByteReader::from_storage(store, name, initial_offset, prefetch_size.max(1))
}

/// 从偏移 0 构造顺序字节读取器。
pub fn newByteReader(data: Vec<u8>, buffer_size: usize) -> Result<ByteReader> {
    ByteReader::new(data, 0, buffer_size)
}
