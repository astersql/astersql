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

// 全局排序数据文件读取：同步按范围加载与异步 KV 流式迭代。
//
// 从外部存储读取 `encode_kvs` 编码的 data 文件，按 `[start_key, end_key)`
// 过滤键，并受内存上限约束。`CancellationToken` 支持协作式取消。

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;

use crate::{Error, KvPair, Result, Storage};

/// 单文件估计大小达到该并发阈值时启用多缓冲并发读（移植常量，语义对齐 Go）。
pub static readAllDataConcThreshold: u64 = 4;
/// 每个并发读缓冲的字节预算（64MiB）。
pub const ConcurrentReaderBufferSizePerConc: u64 = 64 * 1024 * 1024;

/// 协作式取消令牌：合并/读取循环定期检查，触发后返回 `Error::Cancelled`。
#[derive(Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    /// Share the native object-store cancellation flag with sort/load loops.
    pub fn from_cancellation_flag(flag: Arc<AtomicBool>) -> Self {
        Self(flag)
    }

    /// 标记为已取消。
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// 查询是否已取消。
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// 若已取消则返回 `Error::Cancelled`。
    fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Default)]
/// 内存中按文件暂存的 KV 缓冲：`build` 前分散在 `kvs_per_file`，之后展平到 `kvs`。
pub struct MemKvsAndBuffers {
    /// 展平后的全部 KV（调用 `build` 后有效）。
    pub kvs: Vec<KvPair>,
    /// 已保留 KV 的编码总字节数。
    pub size: usize,
    /// 因早于 `start_key` 而丢弃的编码字节数合计。
    pub dropped_size: usize,
    kvs_per_file: Vec<Vec<KvPair>>,
    dropped_size_per_file: Vec<usize>,
}

impl MemKvsAndBuffers {
    /// 将各文件缓冲合并到 `kvs`，并汇总 `dropped_size`。
    pub fn build(&mut self) {
        let count = self.kvs_per_file.iter().map(Vec::len).sum();
        self.kvs = Vec::with_capacity(count);
        for pairs in self.kvs_per_file.drain(..) {
            self.kvs.extend(pairs);
        }
        self.dropped_size = self.dropped_size_per_file.drain(..).sum();
    }

    /// 清空全部缓冲与计数，便于错误路径回滚。
    pub fn clear(&mut self) {
        self.kvs.clear();
        self.kvs_per_file.clear();
        self.dropped_size_per_file.clear();
        self.size = 0;
        self.dropped_size = 0;
    }
}

/// 按文件列表读取 `[start_key, end_key)` 内的 KV，写入 `output`。
///
/// `data_files`/`stats_files`/`start_offsets`/`estimated_end_offsets` 长度必须一致；
/// 失败时清空 `output`，避免半成品状态泄漏。
pub fn read_all_data(
    token: &CancellationToken,
    store: &dyn Storage,
    data_files: &[String],
    stats_files: &[String],
    start_key: &[u8],
    end_key: &[u8],
    start_offsets: &[u64],
    estimated_end_offsets: &[u64],
    memory_limit: usize,
    output: &mut MemKvsAndBuffers,
) -> Result<()> {
    let file_count = data_files.len();
    // 三组切片长度必须与 data 文件数对齐。
    if stats_files.len() != file_count
        || start_offsets.len() != file_count
        || estimated_end_offsets.len() != file_count
    {
        return Err(Error::InvalidArgument(
            "data, stat, and offset slices must have equal length".into(),
        ));
    }
    output.clear();
    let result = (|| {
        for index in 0..file_count {
            token.check()?;
            let estimated_size = estimated_end_offsets[index]
                .checked_sub(start_offsets[index])
                .ok_or_else(|| Error::InvalidData("end offset precedes start offset".into()))?;
            // 按估计体积决定是否拉高并发读参数（本移植中 _concurrency 暂未真正并行）。
            let expected_concurrency = estimated_size / ConcurrentReaderBufferSizePerConc + 1;
            let concurrency = if expected_concurrency >= readAllDataConcThreshold {
                expected_concurrency
            } else {
                1
            };
            read_one_file(
                token,
                store,
                &data_files[index],
                start_key,
                end_key,
                start_offsets[index],
                concurrency,
                memory_limit,
                output,
            )?;
        }
        Ok(())
    })();
    // 任一文件失败则丢弃已读缓冲，保证调用方看不到部分结果。
    if result.is_err() {
        output.clear();
    }
    result
}

#[allow(clippy::too_many_arguments)]
/// 读取单个 data 文件：解码、按键窗口过滤，并检查内存上限。
pub fn read_one_file(
    token: &CancellationToken,
    storage: &dyn Storage,
    data_file: &str,
    start_key: &[u8],
    end_key: &[u8],
    start_offset: u64,
    _concurrency: u64,
    memory_limit: usize,
    output: &mut MemKvsAndBuffers,
) -> Result<()> {
    token.check()?;
    let mut stream = storage.open_at(data_file, start_offset)?;
    let mut kept = Vec::new();
    let mut size = 0usize;
    let mut dropped = 0usize;
    while let Some(pair) = read_stream_pair(stream.as_mut(), storage.record_format())? {
        token.check()?;
        // 半开区间 [start_key, end_key)：小于 start 丢弃，大于等于 end 停止。
        if pair.key.as_slice() < start_key {
            dropped = dropped.saturating_add(pair.encoded_size());
            continue;
        }
        if pair.key.as_slice() >= end_key {
            break;
        }
        let next_size = output
            .size
            .checked_add(size)
            .and_then(|current| current.checked_add(pair.encoded_size()))
            .ok_or_else(|| Error::OutOfMemory {
                requested: usize::MAX,
                limit: memory_limit,
            })?;
        // 累计编码大小超过 memory_limit 则报 OutOfMemory。
        if next_size > memory_limit {
            return Err(Error::OutOfMemory {
                requested: next_size,
                limit: memory_limit,
            });
        }
        size += pair.encoded_size();
        kept.push(pair);
    }
    output.size += size;
    output.kvs_per_file.push(kept);
    output.dropped_size_per_file.push(dropped);
    Ok(())
}

/// 后台线程异步解码多文件并经 channel 产出的 KV 迭代器。
pub struct AsyncKvReader {
    receiver: Option<mpsc::Receiver<Result<KvPair>>>,
    handle: Option<JoinHandle<()>>,
}

impl Iterator for AsyncKvReader {
    type Item = Result<KvPair>;

    fn next(&mut self) -> Option<Self::Item> {
        self.receiver.as_ref()?.recv().ok()
    }
}

/// 丢弃时 join 后台线程，避免泄漏。
impl Drop for AsyncKvReader {
    fn drop(&mut self) {
        // Disconnect first: a producer blocked on the bounded output must be
        // woken before joining when a consumer stops early.
        self.receiver.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// 启动后台线程，按文件顺序异步解码并流式返回 KV。
pub fn ReadKVFilesAsync(
    token: CancellationToken,
    store: Arc<dyn Storage>,
    files: Vec<String>,
) -> AsyncKvReader {
    let (sender, receiver) = mpsc::sync_channel(1);
    let handle = std::thread::spawn(move || {
        for file in files {
            if token.is_cancelled() {
                let _ = sender.send(Err(Error::Cancelled));
                break;
            }
            let result = read_one_kv_file_to_channel(&token, store.as_ref(), &file, &sender);
            if let Err(error) = result {
                let _ = sender.send(Err(error));
                break;
            }
        }
    });
    AsyncKvReader {
        receiver: Some(receiver),
        handle: Some(handle),
    }
}

/// 将单个文件的全部 KV 推入 channel；接收端断开视为取消。
fn read_one_kv_file_to_channel(
    token: &CancellationToken,
    store: &dyn Storage,
    file: &str,
    sender: &mpsc::SyncSender<Result<KvPair>>,
) -> Result<()> {
    let mut stream = store.open(file)?;
    while let Some(pair) = read_stream_pair(stream.as_mut(), store.record_format())? {
        token.check()?;
        sender.send(Ok(pair)).map_err(|_| Error::Cancelled)?;
    }
    Ok(())
}

pub(crate) fn io_error(error: std::io::Error) -> Error {
    if matches!(
        error
            .get_ref()
            .and_then(|cause| cause.downcast_ref::<Error>()),
        Some(Error::Cancelled)
    ) {
        return Error::Cancelled;
    }
    Error::InvalidData(error.to_string())
}
/// Read exactly one length-prefixed KV. EOF is valid only before its header.
pub(crate) fn read_stream_pair(
    reader: &mut dyn std::io::Read,
    format: crate::RecordFormat,
) -> Result<Option<KvPair>> {
    let mut header = [0_u8; 16];
    let size = match format {
        crate::RecordFormat::LegacyLittleEndian32 => 8,
        crate::RecordFormat::GoBigEndian64 => 16,
    };
    match reader.read(&mut header[..1]).map_err(io_error)? {
        0 => return Ok(None),
        _ => {}
    }
    reader.read_exact(&mut header[1..size]).map_err(io_error)?;
    let (key_len, value_len) = match format {
        crate::RecordFormat::LegacyLittleEndian32 => (
            u32::from_le_bytes(header[..4].try_into().unwrap()) as u64,
            u32::from_le_bytes(header[4..8].try_into().unwrap()) as u64,
        ),
        crate::RecordFormat::GoBigEndian64 => (
            u64::from_be_bytes(header[..8].try_into().unwrap()),
            u64::from_be_bytes(header[8..].try_into().unwrap()),
        ),
    };
    let allocate = |length: u64| -> Result<Vec<u8>> {
        let length =
            usize::try_from(length).map_err(|_| Error::InvalidData("KV length overflow".into()))?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| Error::OutOfMemory {
                requested: length,
                limit: isize::MAX as usize,
            })?;
        bytes.resize(length, 0);
        Ok(bytes)
    };
    let mut pair = KvPair {
        key: allocate(key_len)?,
        value: allocate(value_len)?,
    };
    reader.read_exact(&mut pair.key).map_err(io_error)?;
    reader.read_exact(&mut pair.value).map_err(io_error)?;
    Ok(Some(pair))
}

/// Go stat files contain length-prefixed RangeProperty bodies. Retain only one
/// body while decoding; large files are never fetched or expanded as a unit.
pub(crate) struct StreamStatsReader {
    stream: Box<dyn std::io::Read>,
}
pub(crate) struct FileProperty {
    pub range: crate::RangeProperty,
    pub offset: u64,
}
impl StreamStatsReader {
    pub(crate) fn open(store: &dyn Storage, path: &str) -> Result<Self> {
        Ok(Self {
            stream: store.open(path)?,
        })
    }
    pub(crate) fn next(&mut self) -> Result<Option<FileProperty>> {
        let mut length = [0_u8; 4];
        if self.stream.read(&mut length[..1]).map_err(io_error)? == 0 {
            return Ok(None);
        }
        self.stream.read_exact(&mut length[1..]).map_err(io_error)?;
        let size = u32::from_be_bytes(length) as usize;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size)
            .map_err(|_| Error::OutOfMemory {
                requested: size,
                limit: isize::MAX as usize,
            })?;
        bytes.resize(size, 0);
        self.stream.read_exact(&mut bytes).map_err(io_error)?;
        let mut body = std::io::Cursor::new(bytes);
        fn key(body: &mut std::io::Cursor<Vec<u8>>) -> Result<Vec<u8>> {
            let mut size = [0_u8; 4];
            body.read_exact(&mut size).map_err(io_error)?;
            let size = u32::from_be_bytes(size) as usize;
            if size
                > body
                    .get_ref()
                    .len()
                    .saturating_sub(body.position() as usize)
            {
                return Err(Error::InvalidData("truncated stat property key".into()));
            }
            let mut key = vec![0; size];
            body.read_exact(&mut key).map_err(io_error)?;
            Ok(key)
        }
        fn number(body: &mut std::io::Cursor<Vec<u8>>) -> Result<u64> {
            let mut bytes = [0_u8; 8];
            body.read_exact(&mut bytes).map_err(io_error)?;
            Ok(u64::from_be_bytes(bytes))
        }
        let first_key = key(&mut body)?;
        let last_key = key(&mut body)?;
        let size = number(&mut body)?;
        let keys = number(&mut body)?;
        let offset = number(&mut body)?;
        Ok(Some(FileProperty {
            range: crate::RangeProperty {
                first_key,
                last_key,
                size,
                keys,
            },
            offset,
        }))
    }
}

/// Find the largest Go property offset whose first key precedes each ascending
/// job key. At most 64 metadata readers run concurrently, matching simplesst.
pub fn get_read_ranges_from_props(
    token: &CancellationToken,
    store: &dyn Storage,
    keys: &[Vec<u8>],
    files: &[String],
) -> Result<Vec<Vec<u64>>> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    if !keys.windows(2).all(|pair| pair[0] <= pair[1]) {
        return Err(Error::InvalidArgument(
            "property seek keys must be ascending".into(),
        ));
    }
    let mut ranges = vec![vec![0; files.len()]; keys.len()];
    if store.record_format() != crate::RecordFormat::GoBigEndian64 {
        return Ok(ranges);
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let columns = std::sync::Mutex::new(vec![None; files.len()]);
    let failure = std::sync::Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..files.len().min(64) {
            scope.spawn(|| {
                loop {
                    if failure.lock().unwrap().is_some() {
                        break;
                    }
                    let file = next.fetch_add(1, Ordering::Relaxed);
                    if file >= files.len() {
                        break;
                    }
                    let result = (|| {
                        token.check()?;
                        let mut reader = StreamStatsReader::open(store, &files[file])?;
                        let mut offsets = vec![0; keys.len()];
                        let mut index = 0;
                        while let Some(property) = reader.next()? {
                            token.check()?;
                            while property.range.first_key > keys[index] {
                                index += 1;
                                if index == keys.len() {
                                    return Ok(offsets);
                                }
                                offsets[index] = offsets[index - 1];
                            }
                            offsets[index] = property.offset;
                        }
                        for remaining in index + 1..keys.len() {
                            offsets[remaining] = offsets[index];
                        }
                        Ok(offsets)
                    })();
                    match result {
                        Ok(offsets) => columns.lock().unwrap()[file] = Some(offsets),
                        Err(error) => {
                            let mut first = failure.lock().unwrap();
                            if first.is_none() {
                                *first = Some(error);
                            }
                            break;
                        }
                    }
                }
            });
        }
    });
    if let Some(error) = failure.into_inner().map_err(|_| Error::Poisoned)? {
        return Err(error);
    }
    for (file, column) in columns
        .into_inner()
        .map_err(|_| Error::Poisoned)?
        .into_iter()
        .enumerate()
    {
        let column = column.ok_or_else(|| {
            Error::InvalidData("property reader stopped before completion".into())
        })?;
        for (key, offset) in column.into_iter().enumerate() {
            ranges[key][file] = offset;
        }
    }
    Ok(ranges)
}
