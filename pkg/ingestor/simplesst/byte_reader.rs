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
/*

/// ByteReader 对应 Go 的 byteReader，在小缓冲顺序读取和大缓冲并发读取之间切换。
/// `curBufIdx` 与 `curBufOffset` 始终指向下一段未消费数据，返回切片会在后续 reload 后失效。
pub struct ByteReader {
    ctx: Context,
    storageReader: ObjectReader,
    curBuf: Vec<Vec<u8>>,
    curBufIdx: usize,
    curBufOffset: usize,
    smallBuf: Vec<u8>,
    concurrentReader: ConcurrentReaderFields,
    logger: Logger,
    mergeSortReadCounter: Option<Counter>,
}

/// openStoreReaderAndSeek 对应 Go 的存储打开辅助函数，从指定偏移建立带预取大小的读取器。
pub fn openStoreReaderAndSeek(
    ctx: &Context,
    store: &Storage,
    name: &str,
    initFileOffset: u64,
    prefetchSize: usize,
) -> Result<ObjectReader, Error> {
    // Go 通过 ReaderOption 同时设置 StartOffset 与 PrefetchSize；打开动作本身会发出一次 GET。
    store.Open(
        ctx,
        name,
        ReaderOption {
            StartOffset: Some(initFileOffset as i64),
            PrefetchSize: prefetchSize,
        },
    )
}

/// newByteReader 对应 Go 构造函数，为顺序读取分配小缓冲并立即装入首批数据。
pub fn newByteReader(
    ctx: &Context,
    storageReader: ObjectReader,
    bufSize: usize,
) -> Result<ByteReader, Error> {
    let smallBuf = vec![0; bufSize];
    let mut reader = ByteReader {
        ctx: ctx.clone(),
        storageReader,
        curBuf: vec![smallBuf.clone()],
        curBufIdx: 0,
        curBufOffset: 0,
        smallBuf,
        concurrentReader: ConcurrentReaderFields {
            largeBufferPool: None,
            store: None,
            filename: String::new(),
            concurrency: 0,
            bufSizePerConc: 0,
            now: false,
            expected: false,
            largeBuf: Vec::new(),
            reader: None,
            reloadCnt: 0,
        },
        logger: logger(ctx),
        mergeSortReadCounter: None,
    };

    // Go 的命名返回值配合 defer：首次 reload 失败时仍会关闭已经打开的 storageReader。
    if let Err(err) = reader.reload() {
        let _ = reader.Close();
        return Err(err);
    }
    Ok(reader)
}

impl ByteReader {
    /// enableConcurrentRead 对应 Go 的配置阶段，只保存存储、文件名、并发度和内存池，不立即发起 IO。
    pub fn enableConcurrentRead(
        &mut self,
        store: &Storage,
        filename: &str,
        concurrency: usize,
        bufSizePerConc: usize,
        bufferPool: &mut Buffer,
    ) {
        self.concurrentReader.store = Some(store.clone());
        self.concurrentReader.filename = filename.to_owned();
        self.concurrentReader.concurrency = concurrency;
        self.concurrentReader.bufSizePerConc = bufSizePerConc;
        self.concurrentReader.largeBufferPool = Some(bufferPool.clone());
    }

    /// switchConcurrentMode 保留 sortedReader 的模式切换语义。
    /// 开启采用惰性切换；关闭则立即释放大缓冲，并把顺序读取器校准到尚未消费的位置。
    pub fn switchConcurrentMode(&mut self, useConcurrent: bool) -> Result<(), Error> {
        if self.concurrentReader.store.is_none() {
            self.logger.Warn("concurrent reader is not enabled, skip switching");
            return Ok(());
        }

        // 必须先更新 expected，下一次 reload 才能观察到 false -> true 的切换请求。
        self.concurrentReader.expected = useConcurrent;
        if useConcurrent || !self.concurrentReader.now {
            return Ok(());
        }

        // 关闭并发模式要立刻归还内存；reloadCnt 和旧缓冲偏移共同确定顺序读取器需要回退的距离。
        let (reloadCnt, offsetInOldBuf) = self.closeConcurrentReader();
        let largeBufSize = self.concurrentReader.bufSizePerConc * self.concurrentReader.concurrency;
        let delta = offsetInOldBuf as i64 + (reloadCnt.saturating_sub(1) * largeBufSize) as i64;
        self.storageReader.Seek(delta, SeekFrom::Current)?;

        match self.reload() {
            // Go 忽略这里的 EOF，交给下一次 readNBytes 按正常文件尾处理。
            Err(err) if err.is_eof() => Ok(()),
            result => result,
        }
    }

    /// switchToConcurrentReader 在顺序读取器预取数据耗尽后创建分段读取器并申请每路缓冲。
    fn switchToConcurrentReader(&mut self) -> Result<(), Error> {
        // 此函数只在 reload 边界调用，所以顺序读取器当前位置就是并发读取的准确起点。
        let currOffset = self.storageReader.Seek(0, SeekFrom::Current)?;
        let fileSize = self.storageReader.GetFileSize()?;
        let fields = &mut self.concurrentReader;
        fields.reader = Some(newConcurrentFileReader(
            &self.ctx,
            fields.store.as_ref().expect("concurrent store configured"),
            &fields.filename,
            currOffset,
            fileSize,
            fields.concurrency,
            fields.bufSizePerConc,
        )?);

        fields.largeBuf = Vec::with_capacity(fields.concurrency);
        for _ in 0..fields.concurrency {
            // TryAllocBytes 可能返回错误，也可能成功但不给出内存；两种情况都与 Go 一样立即失败。
            let buf = fields
                .largeBufferPool
                .as_mut()
                .expect("concurrent buffer pool configured")
                .TryAllocBytes(fields.bufSizePerConc)?
                .ok_or_else(|| Error::message(format!("alloc large buffer failed, size {}", fields.bufSizePerConc)))?;
            fields.largeBuf.push(buf);
        }

        self.curBuf = fields.largeBuf.clone();
        self.curBufIdx = 0;
        self.curBufOffset = 0;
        fields.now = true;
        Ok(())
    }

    /// readNBytes 读取恰好 n 个字节；若跨越底层缓冲区，则复制并展平到辅助缓冲区。
    /// 与 Go 相同，只有在尚未读到任何字节时允许返回 EOF，半条记录遇到文件尾会转成 UnexpectedEOF。
    pub fn readNBytes(&mut self, mut n: usize) -> Result<Vec<u8>, Error> {
        if n == 0 {
            return Err(Error::message("illegal n (0) when reading from external storage"));
        }
        if n > GB {
            return Err(Error::message(format!(
                "read {n} bytes from external storage, exceed max limit {GB}"
            )));
        }

        let requested = n;
        let (readLen, chunks) = self.next(n);
        if readLen == n && chunks.len() == 1 {
            return Ok(chunks[0].clone());
        }

        // Go 在跨缓冲区时按已消费长度写入 auxBuf；这里用 Vec 顺序追加保持相同内容。
        let mut auxBuf = Vec::with_capacity(requested);
        for chunk in chunks {
            n -= chunk.len();
            auxBuf.extend_from_slice(&chunk);
        }
        let mut hasRead = readLen > 0;

        while n > 0 {
            if let Err(err) = self.reload() {
                if err.is_eof() && hasRead {
                    return Err(Error::unexpected_eof_with_file(&self.concurrentReader.filename));
                }
                return Err(err);
            }
            let (count, chunks) = self.next(n);
            hasRead |= count > 0;
            for chunk in chunks {
                n -= chunk.len();
                auxBuf.extend_from_slice(&chunk);
            }
        }
        Ok(auxBuf)
    }

    /// next 最多从当前已加载缓冲中消费 n 个字节，不触发 IO，并返回实际数量和分段视图的拥有型。
    fn next(&mut self, mut n: usize) -> (usize, Vec<Vec<u8>>) {
        let mut retCnt = 0;
        let mut ret = Vec::with_capacity(self.curBuf.len().saturating_sub(self.curBufIdx) + 1);

        while self.curBufIdx < self.curBuf.len() && n > 0 {
            let cur = &self.curBuf[self.curBufIdx];
            if self.curBufOffset + n <= cur.len() {
                ret.push(cur[self.curBufOffset..self.curBufOffset + n].to_vec());
                retCnt += n;
                self.curBufOffset += n;
                if self.curBufOffset == cur.len() {
                    self.curBufIdx += 1;
                    self.curBufOffset = 0;
                }
                break;
            }

            let available = cur.len() - self.curBufOffset;
            ret.push(cur[self.curBufOffset..].to_vec());
            retCnt += available;
            n -= available;
            self.curBufIdx += 1;
            self.curBufOffset = 0;
        }
        (retCnt, ret)
    }

    /// reload 装入下一批数据，并只在缓冲边界执行顺序模式到并发模式的转换。
    fn reload(&mut self) -> Result<(), Error> {
        if !self.concurrentReader.now && self.concurrentReader.expected {
            self.logger.Info("switch reader mode: concurrent=true");
            self.switchToConcurrentReader()?;
        }

        if self.concurrentReader.now {
            self.concurrentReader.reloadCnt += 1;
            let buffers = self
                .concurrentReader
                .reader
                .as_mut()
                .expect("concurrent reader initialized")
                .read(&mut self.concurrentReader.largeBuf)?;
            self.curBuf = buffers;
            self.curBufIdx = 0;
            self.curBufOffset = 0;
        } else {
            // Go 使用统一重试策略包装小缓冲 IO；闭包返回 retryable 标志决定是否再次读取。
            runWithRetry(DefaultMaxRetries, RetryInterval, || self.readFromStorageReader())?;
        }

        // 原实现用 defer 统计本次装载后的总字节数，错误路径也会上报当前缓冲规模。
        if let Some(counter) = self.mergeSortReadCounter.as_mut() {
            counter.Add(self.curBuf.iter().map(Vec::len).sum::<usize>() as f64);
        }
        Ok(())
    }

    /// readFromStorageReader 对应 Go 的 io.ReadFull 重试回调，区分正常 EOF、短读、取消和其它 IO 错误。
    fn readFromStorageReader(&mut self) -> Result<bool, Error> {
        match self.storageReader.ReadFull(&mut self.curBuf[0]) {
            Ok(_) => {}
            Err(ReadFullError::Eof) => {
                // 推进索引使后续读取继续观察到 EOF，而不是重复消费旧缓冲。
                self.curBufIdx = self.curBuf.len();
                return Err(Error::eof());
            }
            Err(ReadFullError::UnexpectedEof { read: 0 }) => {
                self.logger.Warn("encounter (0, ErrUnexpectedEOF) during read, retry it");
                return Ok(true);
            }
            Err(ReadFullError::UnexpectedEof { read }) => {
                // 非零短读是文件最后一批数据，只缩短可见长度，不再重试。
                self.curBuf[0].truncate(read);
            }
            Err(ReadFullError::Canceled) => return Err(Error::canceled()),
            Err(ReadFullError::Other(err)) => {
                self.logger.WarnError("other error during read", &err);
                return Err(err);
            }
        }
        self.curBufIdx = 0;
        self.curBufOffset = 0;
        Ok(false)
    }

    /// closeConcurrentReader 丢弃尚未消费的大缓冲，销毁内存池，并返回顺序读取器校准所需信息。
    fn closeConcurrentReader(&mut self) -> (usize, usize) {
        let fields = &mut self.concurrentReader;
        let dropBytes = fields.bufSizePerConc * (self.curBuf.len() - self.curBufIdx) - self.curBufOffset;
        self.logger.InfoFields(
            "drop data in closeConcurrentReader",
            fields.reloadCnt,
            dropBytes,
            self.curBufIdx,
        );

        // Go failpoint 在测试中断言一次模式期间最多 reload 一次；生产路径不执行该断言。
        failpoint_assert_reload_at_most_once(fields.reloadCnt);
        if let Some(pool) = fields.largeBufferPool.as_mut() {
            pool.Destroy();
        }
        fields.largeBuf.clear();
        fields.now = false;

        let reloadCnt = fields.reloadCnt;
        fields.reloadCnt = 0;
        let offsetInOldBuffer = self.curBufOffset + self.curBufIdx * fields.bufSizePerConc;
        self.curBuf = vec![self.smallBuf.clone()];
        self.curBufIdx = 0;
        self.curBufOffset = 0;
        (reloadCnt, offsetInOldBuffer)
    }

    /// Close 对应 Go 的资源收尾：并发模式先归还大缓冲，随后关闭底层对象存储读取器。
    pub fn Close(&mut self) -> Result<(), Error> {
        if self.concurrentReader.now {
            self.closeConcurrentReader();
        }
        self.storageReader.Close()
    }
}
*/
