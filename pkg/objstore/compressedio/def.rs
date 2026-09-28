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

// 压缩格式枚举、解压配置与类型名解析。
//
// 定义 TiDB 对象存储支持的 `CompressType`、文件后缀、`Flusher` 与 `DecompressConfig`。

use std::fmt;
use std::io;

/// TiDB 对象存储支持的压缩格式；数值与 Go 侧常量对齐。
/// The compression formats supported by TiDB object storage.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompressType {
    NoCompression = 0,
    Gzip = 1,
    Snappy = 2,
    Zstd = 3,
}

impl CompressType {
    /// 该压缩格式对应的文件名后缀；无压缩返回空串。
    pub fn file_suffix(self) -> &'static str {
        match self {
            Self::NoCompression => "",
            Self::Gzip => ".gz",
            Self::Snappy => ".snappy",
            Self::Zstd => ".zst",
        }
    }
}

/// 可刷新压缩器自有缓冲的 Writer 能力抽象。
/// A writer that can flush compressor-owned buffers.
pub trait Flusher {
    fn flush(&mut self) -> io::Result<()>;
}

/// 解压选项；目前仅 zstd 使用并发度提示。
/// Decompression options. Only zstd interprets the concurrency hint.
#[derive(Clone, Debug, Default)]
pub struct DecompressConfig {
    pub zstd_decode_concurrency: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 解析未知压缩类型名时返回的错误。
pub struct ParseCompressTypeError(String);

impl fmt::Display for ParseCompressTypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseCompressTypeError {}

/// 将配置字符串解析为 `CompressType`（支持别名如 `gz`/`zst`）。
pub fn parse_compress_type(compress_type: &str) -> Result<CompressType, ParseCompressTypeError> {
    match compress_type {
        "" | "no-compression" => Ok(CompressType::NoCompression),
        "gzip" | "gz" => Ok(CompressType::Gzip),
        "snappy" => Ok(CompressType::Snappy),
        "zstd" | "zst" => Ok(CompressType::Zstd),
        other => Err(ParseCompressTypeError(format!(
            "unknown compress type {other}"
        ))),
    }
}
