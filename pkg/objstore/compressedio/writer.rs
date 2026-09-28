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

// 按压缩类型构造压缩 Writer：封装 Gzip / Snappy / Zstd 编码器并统一关闭语义。

use std::io::{self, Write};

use flate2::Compression;
use flate2::write::GzEncoder;
use snap::write::FrameEncoder;
use zstd::stream::write::Encoder as ZstdEncoder;

use super::{CompressType, Flusher};

/// The Rust equivalent of Go's io.WriteCloser plus compressedio.Flusher.
/// 对应 Go 的 `io.WriteCloser` 与 `compressedio.Flusher`：可写、可刷盘、可关闭。
pub trait Writer: Write + Flusher {
    /// 结束压缩流并释放底层编码器（关闭后不可再写）。
    fn close(&mut self) -> io::Result<()>;
}

/// Gzip 编码器包装；`encoder` 为 None 表示已关闭。
struct GzipWriter<W: Write> {
    encoder: Option<GzEncoder<W>>,
}

impl<W: Write> Write for GzipWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        open_mut(&mut self.encoder)?.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Flusher::flush(self)
    }
}

impl<W: Write> Flusher for GzipWriter<W> {
    fn flush(&mut self) -> io::Result<()> {
        open_mut(&mut self.encoder)?.flush()
    }
}

impl<W: Write> Writer for GzipWriter<W> {
    fn close(&mut self) -> io::Result<()> {
        // take 后 try_finish 写出 gzip footer。
        if let Some(mut encoder) = self.encoder.take() {
            encoder.try_finish()?;
        }
        Ok(())
    }
}

/// Snappy 分帧编码器包装。
struct SnappyWriter<W: Write> {
    encoder: Option<FrameEncoder<W>>,
}

impl<W: Write> Write for SnappyWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        open_mut(&mut self.encoder)?.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Flusher::flush(self)
    }
}

impl<W: Write> Flusher for SnappyWriter<W> {
    fn flush(&mut self) -> io::Result<()> {
        open_mut(&mut self.encoder)?.flush()
    }
}

impl<W: Write> Writer for SnappyWriter<W> {
    fn close(&mut self) -> io::Result<()> {
        if let Some(mut encoder) = self.encoder.take() {
            encoder.flush()?;
        }
        Ok(())
    }
}

/// Zstd 流式编码器包装。
struct ZstdWriter<W: Write> {
    encoder: Option<ZstdEncoder<'static, W>>,
}

impl<W: Write> Write for ZstdWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        open_mut(&mut self.encoder)?.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Flusher::flush(self)
    }
}

impl<W: Write> Flusher for ZstdWriter<W> {
    fn flush(&mut self) -> io::Result<()> {
        open_mut(&mut self.encoder)?.flush()
    }
}

impl<W: Write> Writer for ZstdWriter<W> {
    fn close(&mut self) -> io::Result<()> {
        // finish 写出 zstd 帧结束标记并消费编码器。
        if let Some(encoder) = self.encoder.take() {
            encoder.finish()?;
        }
        Ok(())
    }
}

/// 关闭后访问编码器时返回 BrokenPipe。
fn open_mut<T>(value: &mut Option<T>) -> io::Result<&mut T> {
    value
        .as_mut()
        .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "compressed writer is closed"))
}

/// 按压缩类型创建 Writer；无压缩返回 None；Zstd 创建失败时打日志并返回 None。
pub fn new_writer(compress_type: CompressType, writer: Box<dyn Write>) -> Option<Box<dyn Writer>> {
    match compress_type {
        CompressType::Gzip => Some(Box::new(GzipWriter {
            encoder: Some(GzEncoder::new(writer, Compression::default())),
        })),
        CompressType::Snappy => Some(Box::new(SnappyWriter {
            encoder: Some(FrameEncoder::new(writer)),
        })),
        CompressType::Zstd => match ZstdEncoder::new(writer, 0) {
            Ok(encoder) => Some(Box::new(ZstdWriter {
                encoder: Some(encoder),
            })),
            Err(error) => {
                eprintln!("Met error when creating new writer for Zstd type file: {error}");
                None
            }
        },
        CompressType::NoCompression => None,
    }
}
