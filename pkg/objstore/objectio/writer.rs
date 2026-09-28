// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 对象缓冲写入：按块切分并可选压缩后上传。
//
// `BufferedWriter` 对齐 Go 的 `BufferedWriter`：缓冲超过容量时同步上传满块，
// close 时刷出尾块。无压缩用 `PlainBuffer`；有压缩则走 compressedio::Buffer。

use std::io::{self, Write as IoWrite};
use std::sync::Arc;

use crate::compressedio::{self, CompressType};
use crate::recording::AccessStats;
use crate::{Context, Writer};

/// A no-op flusher for writers which do not own an intermediate buffer.
/// 无中间缓冲的写入器所用的空刷盘器（flush 恒成功）。
pub struct EmptyFlusher;

impl EmptyFlusher {
    /// 空操作刷盘。
    pub fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    /// Go 风格别名，转发到 `flush`。
    #[allow(non_snake_case)]
    pub fn Flush(&mut self) -> io::Result<()> {
        self.flush()
    }
}

/// 统一明文/压缩缓冲的拦截接口，供 BufferedWriter 按容量切块上传。
trait InterceptBuffer {
    fn write(&mut self, data: &[u8]) -> io::Result<usize>;
    fn close(&mut self) -> io::Result<()>;
    fn flush(&mut self) -> io::Result<()>;
    fn len(&self) -> usize;
    fn cap(&self) -> usize;
    fn bytes(&self) -> Vec<u8>;
    fn reset(&mut self);
    fn compressed(&self) -> bool;
}

/// 无压缩时的固定容量内存缓冲。
struct PlainBuffer {
    data: Vec<u8>,
    capacity: usize,
}

impl InterceptBuffer for PlainBuffer {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.data.extend_from_slice(data);
        Ok(data.len())
    }

    fn close(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn len(&self) -> usize {
        self.data.len()
    }

    fn cap(&self) -> usize {
        self.capacity
    }

    fn bytes(&self) -> Vec<u8> {
        self.data.clone()
    }

    fn reset(&mut self) {
        self.data.clear();
    }

    fn compressed(&self) -> bool {
        false
    }
}

impl InterceptBuffer for compressedio::Buffer {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        IoWrite::write(self, data)
    }

    fn close(&mut self) -> io::Result<()> {
        compressedio::Buffer::close(self)
    }

    fn flush(&mut self) -> io::Result<()> {
        compressedio::Buffer::flush(self)
    }

    fn len(&self) -> usize {
        compressedio::Buffer::len(self)
    }

    fn cap(&self) -> usize {
        compressedio::Buffer::cap(self)
    }

    fn bytes(&self) -> Vec<u8> {
        compressedio::Buffer::bytes(self)
    }

    fn reset(&mut self) {
        compressedio::Buffer::reset(self)
    }

    fn compressed(&self) -> bool {
        compressedio::Buffer::compressed(self)
    }
}

/// 按是否压缩选择 PlainBuffer 或 compressedio::Buffer。
fn new_intercept_buffer(
    chunk_size: usize,
    compress_type: CompressType,
) -> Box<dyn InterceptBuffer> {
    if compress_type == CompressType::NoCompression {
        Box::new(PlainBuffer {
            data: Vec::with_capacity(chunk_size),
            capacity: chunk_size,
        })
    } else {
        Box::new(compressedio::new_buffer(chunk_size, compress_type))
    }
}

/// A chunking writer equivalent to Go's `BufferedWriter`.
/// 按块切分的写入器，等价于 Go 的 `BufferedWriter`。
pub struct BufferedWriter {
    buf: Box<dyn InterceptBuffer>,
    writer: Box<dyn Writer>,
    access_rec: Option<Arc<AccessStats>>,
}

impl BufferedWriter {
    /// 将数据写入缓冲；超过容量则 flush 并上传，直到剩余可放入缓冲。
    fn write_inner(&mut self, ctx: &Context, mut data: &[u8]) -> io::Result<usize> {
        let mut written_total = 0;
        while self.buf.len() + data.len() > self.buf.cap() {
            // A compressor may emit a header larger than the requested chunk
            // capacity. Go's bytes.Buffer grows its Cap in that case; the Rust
            // dependency reports the configured capacity, so avoid underflow
            // and upload the emitted bytes before consuming more input.
            // 压缩器可能输出大于配置容量的头部；避免下溢，先上传已产出字节再继续吃输入。
            let to_fill = self.buf.cap().saturating_sub(self.buf.len());
            if to_fill > 0 {
                let written = self.buf.write(&data[..to_fill])?;
                written_total += written;
                data = &data[written..];
                if self.buf.compressed() {
                    continue;
                }
            }

            // The Go implementation deliberately ignores compressor flush
            // errors here and lets the upload/close path report failures.
            // Go 侧故意忽略此处 flush 错误，由上传/close 路径上报失败。
            let _ = self.buf.flush();
            self.upload_chunk(ctx)?;
        }

        let written = self.buf.write(data)?;
        written_total += written;
        Ok(written_total)
    }

    /// 若缓冲非空则取出字节并写入底层 Writer。
    fn upload_chunk(&mut self, ctx: &Context) -> io::Result<()> {
        if self.buf.len() == 0 {
            return Ok(());
        }
        let data = self.buf.bytes();
        self.buf.reset();
        self.writer.write(ctx, &data)?;
        Ok(())
    }

    /// 返回底层 Writer 引用。
    pub fn get_writer(&self) -> &dyn Writer {
        self.writer.as_ref()
    }

    /// Go 风格别名，转发到 `get_writer`。
    #[allow(non_snake_case)]
    pub fn GetWriter(&self) -> &dyn Writer {
        self.get_writer()
    }
}

impl Writer for BufferedWriter {
    fn write(&mut self, ctx: &Context, data: &[u8]) -> io::Result<usize> {
        let result = self.write_inner(ctx, data);
        // 按实际接受字节数记入访问统计（失败时记 0）。
        let accepted = result.as_ref().copied().unwrap_or(0);
        AccessStats::rec_write(self.access_rec.as_deref(), accepted);
        result
    }

    fn close(&mut self, ctx: &Context) -> io::Result<()> {
        let _ = self.buf.close();
        self.upload_chunk(ctx)?;
        self.writer.close(ctx)
    }
}

/// 构造无统计的缓冲上传 Writer（装箱为 `dyn Writer`）。
pub fn new_uploader_writer(
    writer: Box<dyn Writer>,
    chunk_size: usize,
    compress_type: CompressType,
) -> Box<dyn Writer> {
    Box::new(new_buffered_writer(writer, chunk_size, compress_type, None))
}

/// 构造 `BufferedWriter`；`access_rec` 可选记录写入流量。
pub fn new_buffered_writer(
    writer: Box<dyn Writer>,
    chunk_size: usize,
    compress_type: CompressType,
    access_rec: Option<Arc<AccessStats>>,
) -> BufferedWriter {
    BufferedWriter {
        writer,
        buf: new_intercept_buffer(chunk_size, compress_type),
        access_rec,
    }
}

/// Go 风格别名，转发到 `new_uploader_writer`。
#[allow(non_snake_case)]
pub fn NewUploaderWriter(
    writer: Box<dyn Writer>,
    chunk_size: usize,
    compress_type: CompressType,
) -> Box<dyn Writer> {
    new_uploader_writer(writer, chunk_size, compress_type)
}

/// Go 风格别名，转发到 `new_buffered_writer`。
#[allow(non_snake_case)]
pub fn NewBufferedWriter(
    writer: Box<dyn Writer>,
    chunk_size: usize,
    compress_type: CompressType,
    access_rec: Option<Arc<AccessStats>>,
) -> BufferedWriter {
    new_buffered_writer(writer, chunk_size, compress_type, access_rec)
}
