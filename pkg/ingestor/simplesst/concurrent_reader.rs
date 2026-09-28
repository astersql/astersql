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

// 并发文件范围读取器。
//
// `ConcurrentFileReader` 从当前 offset 起把文件切成固定大小的非重叠区间，
// 并行读取后按文件顺序返回；用于 SST（Sorted String Table）预取热点分片。

/// ConcurrentFileReader 对应 Go 的 concurrentFileReader，从当前 offset 起把文件拆成固定大小的并发请求。
/// offset 会在任务提交时前移，因此一次 read 中的每个请求都拥有互不重叠的文件范围。
// pub struct ConcurrentFileReader {
//     ctx: Context,
//     concurrency: usize,
//     readBufferSize: usize,
//     storage: Storage,
//     name: String,
//     offset: i64,
//     fileSize: i64,
// }
// */
use std::sync::Arc;

use crate::{Error, Result};

/// 并行读取互不重叠的文件区间，并按文件顺序返回分片。
///
/// Reads non-overlapping file ranges in parallel and returns them in file order.
#[derive(Clone, Debug)]
pub struct ConcurrentFileReader {
    /// 完整对象内容的共享缓冲。
    data: Arc<Vec<u8>>,
    /// 单次 read 最多提交的并发分片数。
    concurrency: usize,
    /// 每个分片期望读取的字节数。
    read_buffer_size: usize,
    /// 下一次请求的起始偏移（提交时前移）。
    offset: usize,
    /// 可读区间的逻辑文件长度上界。
    file_size: usize,
}

impl ConcurrentFileReader {
    /// 校验并发度、缓冲大小与可读范围后构造读取器。
    pub fn new(
        data: Arc<Vec<u8>>,
        offset: usize,
        file_size: usize,
        concurrency: usize,
        read_buffer_size: usize,
    ) -> Result<Self> {
        if concurrency == 0 || read_buffer_size == 0 {
            return Err(Error::InvalidData(
                "concurrency and buffer size must be positive".into(),
            ));
        }
        if offset > file_size || file_size > data.len() {
            return Err(Error::InvalidData(
                "reader range is outside the object".into(),
            ));
        }
        Ok(Self {
            data,
            concurrency,
            read_buffer_size,
            offset,
            file_size,
        })
    }

    /// 返回当前尚未读过的文件偏移。
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// 最多提交 `concurrency` 个范围读取；任一失败则整批失败。
    pub fn read(&mut self) -> Result<Vec<Vec<u8>>> {
        if self.offset >= self.file_size {
            return Err(Error::eof());
        }
        // 先在主线程切好互不重叠的 [start, end)，并推进 offset。
        let mut ranges = Vec::with_capacity(self.concurrency);
        for _ in 0..self.concurrency {
            if self.offset >= self.file_size {
                break;
            }
            // 文件末尾缩短到剩余字节，避免越过 file_size。
            let end = self
                .offset
                .saturating_add(self.read_buffer_size)
                .min(self.file_size);
            ranges.push((self.offset, end));
            self.offset = end;
        }
        let data = Arc::clone(&self.data);
        // 对应 Go errgroup：并行拷贝各分片，完成后按提交下标排序还原文件顺序。
        let mut chunks = std::thread::scope(|scope| {
            let handles: Vec<_> = ranges
                .into_iter()
                .enumerate()
                .map(|(index, (start, end))| {
                    let data = Arc::clone(&data);
                    scope.spawn(move || (index, data[start..end].to_vec()))
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .map_err(|_| Error::InvalidData("concurrent range reader panicked".into()))
                })
                .collect::<Result<Vec<_>>>()
        })?;
        chunks.sort_by_key(|(index, _)| *index);
        Ok(chunks.into_iter().map(|(_, chunk)| chunk).collect())
    }
}

/// 对应 Go 构造函数，仅保存读取参数并委托 `ConcurrentFileReader::new`。
pub fn newConcurrentFileReader(
    data: Arc<Vec<u8>>,
    offset: usize,
    file_size: usize,
    concurrency: usize,
    read_buffer_size: usize,
) -> Result<ConcurrentFileReader> {
    ConcurrentFileReader::new(data, offset, file_size, concurrency, read_buffer_size)
}
/*

/// newConcurrentFileReader 对应 Go 构造函数，只保存读取参数，不打开文件也不提前发起网络请求。
pub fn newConcurrentFileReader(
    ctx: &Context,
    storage: &Storage,
    name: &str,
    offset: i64,
    fileSize: i64,
    concurrency: usize,
    readBufferSize: usize,
) -> Result<ConcurrentFileReader, Error> {
    Ok(ConcurrentFileReader {
        ctx: ctx.clone(),
        concurrency,
        readBufferSize,
        storage: storage.clone(),
        name: name.to_owned(),
        offset,
        fileSize,
    })
}

impl ConcurrentFileReader {
    /// read 对应 Go 的 read：最多提交 concurrency 个范围读取，并按文件顺序返回装满的缓冲切片。
    pub fn read(&mut self, bufs: &mut [Vec<u8>]) -> Result<Vec<Vec<u8>>, Error> {
        if self.offset >= self.fileSize {
            return Err(Error::eof());
        }

        let mut requests = Vec::with_capacity(self.concurrency);
        for index in 0..self.concurrency {
            if self.offset >= self.fileSize {
                break;
            }

            // 文件末尾请求会缩短到剩余字节数，避免越过 fileSize；其它请求使用完整 readBufferSize。
            let remaining = (self.fileSize - self.offset) as usize;
            let readSize = self.readBufferSize.min(remaining);
            bufs[index].truncate(readSize);
            let offset = self.offset;
            self.offset += readSize as i64;
            requests.push((index, offset, readSize));
        }

        // Go 使用 errgroup 同时等待所有范围请求；任一任务失败则整批失败，不返回部分缓冲。
        let mut group = TaskGroup::new();
        for (index, offset, readSize) in requests.iter().copied() {
            let ctx = self.ctx.clone();
            let storage = self.storage.clone();
            let name = self.name.clone();
            let buf = &mut bufs[index][..readSize];
            group.Go(move || {
                readDataInRange(&ctx, &storage, &name, offset, buf).map_err(|err| {
                    // 保留 Go errors.Annotatef 的诊断字段，明确指出失败分片的偏移和请求大小。
                    err.annotate(format!("offset: {offset}, readSize: {readSize}"))
                })?;
                Ok(())
            });
        }
        group.Wait()?;

        // errgroup 成功后才按提交顺序暴露缓冲；即使请求完成顺序不同，调用方看到的仍是文件顺序。
        Ok(requests
            .into_iter()
            .map(|(index, _, readSize)| bufs[index][..readSize].to_vec())
            .collect())
    }
}
*/
