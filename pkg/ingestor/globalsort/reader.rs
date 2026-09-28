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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;

use crate::{Error, KvPair, Result, Storage, decode_kvs};

/// 单文件估计大小达到该并发阈值时启用多缓冲并发读（移植常量，语义对齐 Go）。
pub static readAllDataConcThreshold: u64 = 4;
/// 每个并发读缓冲的字节预算（64MiB）。
pub const ConcurrentReaderBufferSizePerConc: u64 = 64 * 1024 * 1024;

/// 协作式取消令牌：合并/读取循环定期检查，触发后返回 `Error::Cancelled`。
#[derive(Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
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
    let bytes = storage.read(data_file)?;
    let pairs = decode_kvs(
        &bytes,
        usize::try_from(start_offset).map_err(|_| {
            Error::InvalidArgument("start offset does not fit in memory address space".into())
        })?,
    )?;
    let mut kept = Vec::new();
    let mut size = 0usize;
    let mut dropped = 0usize;
    for pair in pairs {
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
    receiver: mpsc::Receiver<Result<KvPair>>,
    handle: Option<JoinHandle<()>>,
}

impl Iterator for AsyncKvReader {
    type Item = Result<KvPair>;

    fn next(&mut self) -> Option<Self::Item> {
        self.receiver.recv().ok()
    }
}

/// 丢弃时 join 后台线程，避免泄漏。
impl Drop for AsyncKvReader {
    fn drop(&mut self) {
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
    let (sender, receiver) = mpsc::channel();
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
        receiver,
        handle: Some(handle),
    }
}

/// 将单个文件的全部 KV 推入 channel；接收端断开视为取消。
fn read_one_kv_file_to_channel(
    token: &CancellationToken,
    store: &dyn Storage,
    file: &str,
    sender: &mpsc::Sender<Result<KvPair>>,
) -> Result<()> {
    for pair in decode_kvs(&store.read(file)?, 0)? {
        token.check()?;
        sender.send(Ok(pair)).map_err(|_| Error::Cancelled)?;
    }
    Ok(())
}
