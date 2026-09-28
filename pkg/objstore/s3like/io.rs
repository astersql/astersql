// Copyright 2026 PingCAP, Inc.
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

// S3 兼容存储的对象读写适配层。
//
// `S3ObjectReader` 在可 seek 的对象流上实现重试重开、小范围跳过与预取；
// `AsyncWriter` 通过管道把写入侧与后台 `Uploader` 解耦，对应 Go 侧并发上传路径。

#![allow(non_snake_case)]

use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use anyhow::Result;

use crate::{
    MAX_ERROR_RETRIES, MAX_SKIP_OFFSET_BY_READ, RangeInfo, ReadCloser, RecordRetryableError,
    Storage, Uploader,
};

/// 带范围信息与自动重开的 S3 对象读取器。
pub struct S3ObjectReader {
    /// 所属存储实例（用于 reopen 时重新 open）。
    storage: Arc<Storage>,
    /// 对象相对键名。
    name: String,
    /// 当前底层可读流。
    reader: Box<dyn ReadCloser>,
    /// 当前逻辑读位置（对象内绝对偏移）。
    pos: i64,
    /// 当前打开范围与对象总大小。
    rangeInfo: RangeInfo,
    /// 请求上下文（取消会中断重试）。
    ctx: storeapi::Context,
    /// 预取缓冲区大小；0 表示关闭预取。
    prefetchSize: usize,
}

impl S3ObjectReader {
    /// 构造读取器；初始位置取自 `rangeInfo.Start`。
    pub(crate) fn new(
        storage: Arc<Storage>,
        name: String,
        reader: Box<dyn ReadCloser>,
        rangeInfo: RangeInfo,
        ctx: storeapi::Context,
        prefetchSize: usize,
    ) -> Self {
        let pos = rangeInfo.Start;
        Self {
            storage,
            name,
            reader,
            pos,
            rangeInfo,
            ctx,
            prefetchSize,
        }
    }

    /// 关闭旧流并从当前 `pos` 重新打开对象（可选预取包装）。
    fn reopen(&mut self) -> io::Result<()> {
        // end==Size 表示读到对象末尾；open 约定用 0 表示“到 EOF”。
        let mut end = self.rangeInfo.End.wrapping_add(1);
        if end == self.rangeInfo.Size {
            end = 0;
        }
        let _ = self.reader.close();
        let (mut reader, range) = self
            .storage
            .open(&self.ctx, &self.name, self.pos, end)
            .map_err(io::Error::other)?;
        if self.prefetchSize > 0 {
            reader = prefetch::reader::NewReader(reader, range.RangeSize(), self.prefetchSize);
        }
        self.reader = reader;
        Ok(())
    }

    /// 通过连续 read 丢弃 `amount` 字节，用于短距离前向 seek。
    fn discard_exact(&mut self, mut amount: i64) -> io::Result<()> {
        let mut buffer = [0_u8; 8192];
        while amount > 0 {
            let wanted = amount.min(buffer.len() as i64) as usize;
            let read = self.read(&mut buffer[..wanted])?;
            if read == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            amount -= read as i64;
        }
        Ok(())
    }
}

impl Read for S3ObjectReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let mut max_count = self.rangeInfo.End.wrapping_add(1).wrapping_sub(self.pos);
        if max_count == 0 {
            return Ok(0);
        }
        if max_count > output.len() as i64 {
            max_count = output.len() as i64;
        }
        let maxCount = max_count as usize;
        let mut retries = 0;
        loop {
            match self.reader.read(&mut output[..maxCount]) {
                Ok(n) => {
                    objectio::recording::AccessStats::rec_read(
                        self.storage.accessRec.as_deref(),
                        n,
                    );
                    self.pos = self.pos.wrapping_add(n as i64);
                    return Ok(n);
                }
                // 可重试错误：记指标后 reopen，直至达到上限或上下文取消。
                Err(err) if !self.ctx.is_cancelled() && retries < MAX_ERROR_RETRIES => {
                    RecordRetryableError(&err.to_string());
                    // Go keeps the read error in the named return value when reopening fails.
                    // Preserve that error instead of replacing it with the secondary open error.
                    if self.reopen().is_err() {
                        return Err(err);
                    }
                    retries += 1;
                }
                Err(err) => {
                    objectio::recording::AccessStats::rec_read(
                        self.storage.accessRec.as_deref(),
                        0,
                    );
                    return Err(err);
                }
            }
        }
    }
}

/// 空正文占位：seek 到对象末尾后替换为不再产生数据的流。
struct EmptyBody(Cursor<Vec<u8>>);

impl Read for EmptyBody {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl ReadCloser for EmptyBody {
    fn close(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for S3ObjectReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        // 统一换算为对象内绝对偏移，并拒绝溢出与负偏移。
        let realOffset = match position {
            SeekFrom::Start(offset) => i64::try_from(offset).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "seek offset overflows i64")
            })?,
            SeekFrom::Current(offset) => self.pos.wrapping_add(offset),
            SeekFrom::End(offset) => self.rangeInfo.Size.wrapping_add(offset),
        };
        if realOffset < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "Seek in '{}': invalid offset to seek '{}'.",
                    self.name, realOffset
                ),
            ));
        }
        if realOffset == self.pos {
            return Ok(realOffset as u64);
        }
        // 越过对象末尾：换成空流并夹到 Size。
        if realOffset >= self.rangeInfo.Size {
            let _ = self.reader.close();
            self.reader = Box::new(EmptyBody(Cursor::new(Vec::new())));
            self.pos = self.rangeInfo.Size;
            return Ok(self.pos as u64);
        }
        // 短距离前向：直接读丢弃，避免重新建连。
        let forward = realOffset.wrapping_sub(self.pos);
        if realOffset > self.pos && forward <= MAX_SKIP_OFFSET_BY_READ {
            self.discard_exact(forward)?;
            return Ok(realOffset as u64);
        }

        // 远距离或后退：关闭后从新偏移重新 open。
        self.reader.close()?;
        let (mut reader, range) = self
            .storage
            .open(&self.ctx, &self.name, realOffset, 0)
            .map_err(io::Error::other)?;
        if self.prefetchSize > 0 {
            reader = prefetch::reader::NewReader(reader, range.RangeSize(), self.prefetchSize);
        }
        self.reader = reader;
        self.rangeInfo = range;
        self.pos = realOffset;
        Ok(realOffset as u64)
    }
}

impl objectio::Reader for S3ObjectReader {
    fn close(&mut self) -> io::Result<()> {
        self.reader.close()
    }
    fn file_size(&self) -> io::Result<i64> {
        Ok(self.rangeInfo.Size)
    }
}

/// 管道驱动的异步分片上传 Writer：主线程写管道，后台线程执行 Upload。
pub struct AsyncWriter {
    /// 管道写端；close 时 drop 以通知读端 EOF。
    writer: Option<os_pipe::PipeWriter>,
    /// 后台上传任务句柄。
    upload: Option<JoinHandle<Result<()>>>,
}

impl AsyncWriter {
    /// 创建管道并 spawn 上传线程。
    pub(crate) fn new(ctx: storeapi::Context, uploader: Box<dyn Uploader>) -> io::Result<Self> {
        let (mut reader, writer) = os_pipe::pipe()?;
        let upload = thread::spawn(move || uploader.Upload(&ctx, &mut reader));
        Ok(Self {
            writer: Some(writer),
            upload: Some(upload),
        })
    }
}

impl objectio::Writer for AsyncWriter {
    fn write(&mut self, _ctx: &objectio::Context, data: &[u8]) -> io::Result<usize> {
        self.writer
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "writer is closed"))?
            .write(data)
    }

    fn close(&mut self, _ctx: &objectio::Context) -> io::Result<()> {
        // 先关闭写端，再 join 上传线程以传播上传错误。
        self.writer.take();
        match self.upload.take() {
            Some(handle) => handle
                .join()
                .map_err(|_| io::Error::other("multipart uploader panicked"))?
                .map_err(io::Error::other),
            None => Ok(()),
        }
    }
}
