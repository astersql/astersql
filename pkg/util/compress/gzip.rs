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

// Gzip 压缩读写对象池，对齐 Go `sync.Pool` 复用语义。
//
// 对应 Go `pkg/util/compress`。通过 `ReusablePool` 复用 `GzipWriter` /
// `GzipReader`，降低频繁创建编解码器的开销。

#![allow(dead_code)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use std::io::{self, Cursor, Read, Write};
use std::sync::{LazyLock, Mutex};

/// 可复用对象池：工厂函数 + Get/Put，语义对齐 Go `sync.Pool`。
/// A small reusable-object pool with the same factory and Get/Put semantics
/// used by the Go package's `sync.Pool` values.
pub struct ReusablePool<T> {
    items: Mutex<Vec<T>>,
    new: fn() -> T,
}

impl<T> ReusablePool<T> {
    /// 用工厂函数构造空池。
    pub const fn new(new: fn() -> T) -> Self {
        Self {
            items: Mutex::new(Vec::new()),
            new,
        }
    }

    /// 取出一个对象；池空则调用工厂新建。
    pub fn get(&self) -> T {
        self.items
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop()
            .unwrap_or_else(|| (self.new)())
    }

    /// 归还对象到池中供后续复用。
    pub fn put(&self, item: T) {
        self.items
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(item);
    }
}

/// 可重置的 Gzip 编码器封装，供对象池复用。
pub struct GzipWriter {
    encoder: GzEncoder<Vec<u8>>,
}

impl GzipWriter {
    /// 创建占位编码器（对齐 Go `gzip.NewWriter(io.Discard)`，真正压缩前会替换目标缓冲）。
    fn new() -> Self {
        Self {
            // Like gzip.NewWriter(io.Discard), the initial destination is only
            // a placeholder and is replaced before useful work.
            encoder: GzEncoder::new(Vec::new(), Compression::default()),
        }
    }

    /// 将输入压缩为 gzip 字节序列并返回。
    pub fn compress(&mut self, input: &[u8]) -> io::Result<Vec<u8>> {
        self.encoder = GzEncoder::new(Vec::new(), Compression::default());
        self.encoder.write_all(input)?;
        self.encoder.try_finish()?;
        Ok(self.encoder.get_ref().clone())
    }
}

/// 可重置的 Gzip 解码器封装，供对象池复用。
pub struct GzipReader {
    decoder: Option<GzDecoder<Cursor<Vec<u8>>>>,
}

impl GzipReader {
    /// 创建未绑定输入的解码器（对齐 Go 零值 `gzip.Reader`，使用前需 `reset`）。
    fn new() -> Self {
        // This is the Rust equivalent of Go's zero-value gzip.Reader: it must
        // be reset with an input stream before it produces data.
        Self { decoder: None }
    }

    /// 用新的压缩输入重置解码器状态。
    pub fn reset(&mut self, input: Vec<u8>) {
        self.decoder = Some(GzDecoder::new(Cursor::new(input)));
    }
}

impl Read for GzipReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        match self.decoder.as_mut() {
            Some(decoder) => decoder.read(output),
            None => Ok(0),
        }
    }
}

/// 全局 `GzipWriter` 对象池（对齐 Go `GzipWriterPool`）。
// GzipWriterPool is a sync.Pool of gzip.Writer.
pub static GzipWriterPool: LazyLock<ReusablePool<GzipWriter>> =
    LazyLock::new(|| ReusablePool::new(GzipWriter::new));

/// 全局 `GzipReader` 对象池（对齐 Go `GzipReaderPool`）。
// GzipReaderPool is a sync.Pool of gzip.Reader.
pub static GzipReaderPool: LazyLock<ReusablePool<GzipReader>> =
    LazyLock::new(|| ReusablePool::new(GzipReader::new));
