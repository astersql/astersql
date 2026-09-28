// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 对象存储压缩包装：在读写路径上透明应用 Gzip/Snappy/Zstd。
//
// `WithCompression` 包装下层 `Storage`；`CompressionStorage` 在无压缩时退化为透传。
// 流式 Open/Create 通过拦截 Reader/Writer 完成解压与压缩。

use std::cell::RefCell;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, bail};
use flate2::read::MultiGzDecoder;
use snap::read::FrameDecoder;
use zstd::stream::read::Decoder as ZstdDecoder;

use crate::azblob::lock_unpoisoned;
use crate::{objectio, storeapi};

/// 流式压缩写缓冲块大小（5 MiB），与 Go 侧硬编码一致。
const HARDCODED_CHUNK_SIZE: usize = 5 * 1024 * 1024;

/// 带压缩类型与解压配置的存储装饰器。
pub struct WithCompression<S: storeapi::Storage> {
    storage: S,
    compress_type: objectio::CompressType,
    decompress_config: objectio::compressedio::DecompressConfig,
}

impl<S: storeapi::Storage> WithCompression<S> {
    /// 构造压缩包装；`compress_type` 为 `NoCompression` 时写路径仍可走压缩分支短路。
    pub fn new(
        storage: S,
        compress_type: objectio::CompressType,
        decompress_config: objectio::compressedio::DecompressConfig,
    ) -> Self {
        Self {
            storage,
            compress_type,
            decompress_config,
        }
    }

    /// 拆出底层存储，丢弃压缩配置。
    pub fn into_inner(self) -> S {
        self.storage
    }

    /// 借用底层存储。
    pub fn inner(&self) -> &S {
        &self.storage
    }
}

/// 可能是纯透传或已包装压缩的存储枚举，便于统一实现 `Storage`。
pub enum CompressionStorage<S: storeapi::Storage> {
    Plain(S),
    Compressed(WithCompression<S>),
}

/// 按压缩类型选择 `Plain` 或 `Compressed` 变体。
pub fn with_compression<S: storeapi::Storage>(
    storage: S,
    compress_type: objectio::CompressType,
    config: objectio::compressedio::DecompressConfig,
) -> CompressionStorage<S> {
    if compress_type == objectio::CompressType::NoCompression {
        CompressionStorage::Plain(storage)
    } else {
        CompressionStorage::Compressed(WithCompression::new(storage, compress_type, config))
    }
}

impl<S: storeapi::Storage> storeapi::Storage for WithCompression<S> {
    fn WriteFile(&self, ctx: &objectio::Context, name: &str, data: &[u8]) -> Result<()> {
        if self.compress_type == objectio::CompressType::NoCompression {
            return self.storage.WriteFile(ctx, name, data);
        }
        // 先压到内存缓冲，再把压缩字节写入底层对象。
        let buffer = SharedBuffer::default();
        let mut writer =
            objectio::compressedio::new_writer(self.compress_type, Box::new(buffer.clone()))
                .ok_or_else(|| anyhow::anyhow!("compression writer is unavailable"))?;
        writer.write_all(data)?;
        writer.close()?;
        self.storage.WriteFile(ctx, name, &buffer.bytes())
    }

    fn ReadFile(&self, ctx: &objectio::Context, name: &str) -> Result<Vec<u8>> {
        let data = self.storage.ReadFile(ctx, name)?;
        if self.compress_type == objectio::CompressType::NoCompression {
            return Ok(data);
        }
        let mut reader = new_decompress_reader(
            self.compress_type,
            self.decompress_config.clone(),
            Box::new(Cursor::new(data)),
        )?
        .ok_or_else(|| anyhow::anyhow!("decompression reader is unavailable"))?;
        let mut output = Vec::new();
        reader.read_to_end(&mut output)?;
        Ok(output)
    }

    fn FileExists(&self, ctx: &objectio::Context, name: &str) -> Result<bool> {
        self.storage.FileExists(ctx, name)
    }

    fn DeleteFile(&self, ctx: &objectio::Context, name: &str) -> Result<()> {
        self.storage.DeleteFile(ctx, name)
    }

    fn Open(
        &self,
        ctx: &objectio::Context,
        path: &str,
        option: Option<&storeapi::ReaderOption>,
    ) -> Result<Box<dyn objectio::Reader>> {
        let reader = self.storage.Open(ctx, path, option)?;
        intercept_decompress_reader(reader, self.compress_type, self.decompress_config.clone())
    }

    fn DeleteFiles(&self, ctx: &objectio::Context, names: &[String]) -> Result<()> {
        self.storage.DeleteFiles(ctx, names)
    }

    fn WalkDir(
        &self,
        ctx: &objectio::Context,
        option: Option<&storeapi::WalkOption>,
        callback: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        self.storage.WalkDir(ctx, option, callback)
    }

    fn URI(&self) -> String {
        self.storage.URI()
    }

    fn Create(
        &self,
        ctx: &objectio::Context,
        path: &str,
        option: Option<&storeapi::WriterOption>,
    ) -> Result<Box<dyn objectio::Writer>> {
        let writer = self.storage.Create(ctx, path, option)?;
        if self.compress_type == objectio::CompressType::NoCompression {
            return Ok(writer);
        }
        // 流式 Create：用固定块大小缓冲后再压缩写出。
        Ok(Box::new(objectio::new_buffered_writer(
            writer,
            HARDCODED_CHUNK_SIZE,
            self.compress_type,
            None,
        )))
    }

    fn Rename(
        &self,
        ctx: &objectio::Context,
        old_file_name: &str,
        new_file_name: &str,
    ) -> Result<()> {
        self.storage.Rename(ctx, old_file_name, new_file_name)
    }

    fn PresignFile(
        &self,
        ctx: &objectio::Context,
        file_name: &str,
        expire: Duration,
    ) -> Result<String> {
        self.storage.PresignFile(ctx, file_name, expire)
    }

    fn Close(&self) {
        self.storage.Close();
    }
}

impl<S: storeapi::Storage> storeapi::Storage for CompressionStorage<S> {
    fn WriteFile(&self, ctx: &objectio::Context, name: &str, data: &[u8]) -> Result<()> {
        match self {
            Self::Plain(storage) => storage.WriteFile(ctx, name, data),
            Self::Compressed(storage) => storage.WriteFile(ctx, name, data),
        }
    }
    fn ReadFile(&self, ctx: &objectio::Context, name: &str) -> Result<Vec<u8>> {
        match self {
            Self::Plain(storage) => storage.ReadFile(ctx, name),
            Self::Compressed(storage) => storage.ReadFile(ctx, name),
        }
    }
    fn FileExists(&self, ctx: &objectio::Context, name: &str) -> Result<bool> {
        match self {
            Self::Plain(storage) => storage.FileExists(ctx, name),
            Self::Compressed(storage) => storage.FileExists(ctx, name),
        }
    }
    fn DeleteFile(&self, ctx: &objectio::Context, name: &str) -> Result<()> {
        match self {
            Self::Plain(storage) => storage.DeleteFile(ctx, name),
            Self::Compressed(storage) => storage.DeleteFile(ctx, name),
        }
    }
    fn Open(
        &self,
        ctx: &objectio::Context,
        path: &str,
        option: Option<&storeapi::ReaderOption>,
    ) -> Result<Box<dyn objectio::Reader>> {
        match self {
            Self::Plain(storage) => storage.Open(ctx, path, option),
            Self::Compressed(storage) => storage.Open(ctx, path, option),
        }
    }
    fn DeleteFiles(&self, ctx: &objectio::Context, names: &[String]) -> Result<()> {
        match self {
            Self::Plain(storage) => storage.DeleteFiles(ctx, names),
            Self::Compressed(storage) => storage.DeleteFiles(ctx, names),
        }
    }
    fn WalkDir(
        &self,
        ctx: &objectio::Context,
        option: Option<&storeapi::WalkOption>,
        callback: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        match self {
            Self::Plain(storage) => storage.WalkDir(ctx, option, callback),
            Self::Compressed(storage) => storage.WalkDir(ctx, option, callback),
        }
    }
    fn URI(&self) -> String {
        match self {
            Self::Plain(storage) => storage.URI(),
            Self::Compressed(storage) => storage.URI(),
        }
    }
    fn Create(
        &self,
        ctx: &objectio::Context,
        path: &str,
        option: Option<&storeapi::WriterOption>,
    ) -> Result<Box<dyn objectio::Writer>> {
        match self {
            Self::Plain(storage) => storage.Create(ctx, path, option),
            Self::Compressed(storage) => storage.Create(ctx, path, option),
        }
    }
    fn Rename(&self, ctx: &objectio::Context, old: &str, new: &str) -> Result<()> {
        match self {
            Self::Plain(storage) => storage.Rename(ctx, old, new),
            Self::Compressed(storage) => storage.Rename(ctx, old, new),
        }
    }
    fn PresignFile(&self, ctx: &objectio::Context, file: &str, expire: Duration) -> Result<String> {
        match self {
            Self::Plain(storage) => storage.PresignFile(ctx, file, expire),
            Self::Compressed(storage) => storage.PresignFile(ctx, file, expire),
        }
    }
    fn Close(&self) {
        match self {
            Self::Plain(storage) => storage.Close(),
            Self::Compressed(storage) => storage.Close(),
        }
    }
}

#[derive(Clone)]
/// 可共享的对象 Reader，供解压流与 Seek 同时持有同一底层句柄。
struct SharedObjectReader(Rc<RefCell<Box<dyn objectio::Reader>>>);

impl Read for SharedObjectReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.0.borrow_mut().read(output)
    }
}

impl Seek for SharedObjectReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.0.borrow_mut().seek(position)
    }
}

/// 解压拦截 Reader：读走解压流，Seek 仅支持探测当前位置。
struct CompressReader {
    reader: Box<dyn Read>,
    source: SharedObjectReader,
}

impl Read for CompressReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.reader.read(output)
    }
}

impl Seek for CompressReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        // 仅允许 Current(0) 探测位置，其他 Seek 与 Go 一样不支持。
        if position == SeekFrom::Current(0) {
            self.source.seek(position)
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("compressReader doesn't support Seek now: {position:?}"),
            ))
        }
    }
}

impl objectio::Reader for CompressReader {
    fn close(&mut self) -> io::Result<()> {
        self.source.0.borrow_mut().close()
    }

    fn file_size(&self) -> io::Result<i64> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "compressReader doesn't support GetFileSize now",
        ))
    }
}

/// 在已有 Reader 外包一层解压；无压缩时原样返回。
pub fn intercept_decompress_reader(
    reader: Box<dyn objectio::Reader>,
    compress_type: objectio::CompressType,
    config: objectio::compressedio::DecompressConfig,
) -> Result<Box<dyn objectio::Reader>> {
    if compress_type == objectio::CompressType::NoCompression {
        return Ok(reader);
    }
    let source = SharedObjectReader(Rc::new(RefCell::new(reader)));
    let decompressor = new_decompress_reader(compress_type, config, Box::new(source.clone()))?
        .ok_or_else(|| anyhow::anyhow!("decompression reader is unavailable"))?;
    Ok(Box::new(CompressReader {
        reader: decompressor,
        source,
    }))
}

/// 带字节上限的解压拦截；`limit==0` 退化为普通拦截，负数报错。
pub fn new_limited_intercept_reader(
    reader: Box<dyn objectio::Reader>,
    compress_type: objectio::CompressType,
    config: objectio::compressedio::DecompressConfig,
    limit: i64,
) -> Result<Box<dyn objectio::Reader>> {
    if limit < 0 {
        bail!("compressReader doesn't support negative limit, n: {limit}");
    }
    if limit == 0 {
        return intercept_decompress_reader(reader, compress_type, config);
    }
    let source = SharedObjectReader(Rc::new(RefCell::new(reader)));
    let limited: Box<dyn Read> = Box::new(source.clone().take(limit as u64));
    let body = if compress_type == objectio::CompressType::NoCompression {
        limited
    } else {
        new_decompress_reader(compress_type, config, limited)?
            .ok_or_else(|| anyhow::anyhow!("decompression reader is unavailable"))?
    };
    Ok(Box::new(CompressReader {
        reader: body,
        source,
    }))
}

#[derive(Clone, Default)]
/// 线程安全内存缓冲，供整文件压缩后再一次性 WriteFile。
struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

impl SharedBuffer {
    fn bytes(&self) -> Vec<u8> {
        lock_unpoisoned(&self.0).clone()
    }
}

impl Write for SharedBuffer {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        lock_unpoisoned(&self.0).extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 按压缩类型构造解压 `Read`；无压缩返回 `None`。
fn new_decompress_reader(
    compress_type: objectio::CompressType,
    config: objectio::compressedio::DecompressConfig,
    reader: Box<dyn Read>,
) -> io::Result<Option<Box<dyn Read>>> {
    match compress_type {
        objectio::CompressType::NoCompression => Ok(None),
        objectio::CompressType::Gzip => {
            // Go's gzip reader validates the header at construction and reads
            // concatenated members by default.
            let decoder = MultiGzDecoder::new(reader);
            if decoder.header().is_none() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid gzip header",
                ));
            }
            Ok(Some(Box::new(decoder)))
        }
        objectio::CompressType::Snappy => Ok(Some(Box::new(FrameDecoder::new(reader)))),
        objectio::CompressType::Zstd => {
            let _decode_concurrency = config.zstd_decode_concurrency;
            Ok(Some(Box::new(ZstdDecoder::new(reader)?)))
        }
    }
}
