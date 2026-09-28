// Copyright 2026 AsterSQL.

// `chunk` 内部 group1 聚合层：类型/错误/磁盘 I/O/加解密/校验和适配，并 include 核心实现。
//
// 本文件作为 `util/chunk` 的第一垂直组入口：把 `types`、`mysql`、临时文件、CTR 加密、
// checksum、带缓存的 Reader 等依赖收拢为与 Go 相近的模块面，再通过 `include!`
// 注入 `column`/`codec`/`chunk`/`alloc`/`chunk_util`/`chunk_in_disk`。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// 表达式求值类型与标量比较等从外部 `types` crate 再导出。
pub mod types {
    pub use ::types::*;
    pub use types_field::{
        ETDatetime, ETDecimal, ETDuration, ETInt, ETJson, ETReal, ETString, ETTimestamp,
        ETVectorFloat32, EvalType,
    };
    pub use types_scalar::CompareString;
}

pub use types::mysql;

/// group1 局部错误类型，可从 I/O 错误转换。
pub mod errors {
    #[derive(Debug, thiserror::Error)]
    pub enum Error {
        #[error("{0}")]
        Message(String),
        #[error(transparent)]
        Io(#[from] std::io::Error),
    }

    /// 由消息字符串构造错误。
    pub fn New(message: impl Into<String>) -> Error {
        Error::Message(message.into())
    }

    /// 将任意 Display 值包装为消息错误（对齐 Go `errors.Trace` 的轻量用法）。
    pub fn Trace(error: impl std::fmt::Display) -> Error {
        Error::Message(error.to_string())
    }
}

/// 磁盘 Tracker 与临时目录初始化。
pub mod disk {
    pub type Tracker = Box<memory_crate::tracker::Tracker>;
    pub fn NewTracker(label: i32, limit: i64) -> Tracker {
        memory_crate::tracker::NewTracker(label, limit)
    }
    pub use disk_crate::CheckAndInitTempDir;
}

/// 落盘 Chunk 数据相关的内存标签常量。
pub mod memory {
    pub use memory_crate::tracker::LabelForChunkDataInDiskByChunks;
}

pub mod disjointset {
    pub use disjointset_crate::*;
}

pub mod intest {
    pub use intest_crate::Assert;
}

/// 原子指针封装，提供 Go 风格 `Load` / `CompareAndSwap`。
pub mod atomic {
    use std::sync::atomic::{AtomicPtr, Ordering};

    pub struct Pointer<T>(AtomicPtr<T>);
    impl<T> Pointer<T> {
        pub fn new(value: *mut T) -> Self {
            Self(AtomicPtr::new(value))
        }
        pub fn Load(&self) -> *mut T {
            self.0.load(Ordering::Acquire)
        }
        pub fn CompareAndSwap(&self, old: *mut T, new: *mut T) -> bool {
            self.0
                .compare_exchange(old, new, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        }
    }
}

/// 全局配置中与 spill 相关的临时路径与加密方式。
pub mod config {
    pub const SpilledFileEncryptionMethodPlaintext: &str = "plaintext";

    pub struct Security {
        pub SpilledFileEncryptionMethod: String,
    }
    pub struct Config {
        pub TempStoragePath: String,
        pub Security: Security,
    }
    /// 从全局 config crate 投影出本模块需要的字段子集。
    pub fn GetGlobalConfig() -> Config {
        let config = config_crate::get_global_config();
        Config {
            TempStoragePath: config.temp_storage_path.clone(),
            Security: Security {
                SpilledFileEncryptionMethod: config.security.spilled_file_encryption_method.clone(),
            },
        }
    }
}

/// 写/关/按偏移读抽象，以及限定区间的 `SectionReader`。
pub mod io {
    use crate::errors;

    pub trait Writer {
        fn Write(&mut self, data: &[u8]) -> Result<usize, errors::Error>;
    }
    pub trait WriteCloser: Writer + Send {
        fn Close(&mut self) -> Result<(), errors::Error>;
    }
    pub trait ReaderAt: Send + Sync {
        fn ReadAt(&self, data: &mut [u8], offset: i64) -> Result<usize, errors::Error>;
    }

    /// 在底层 `ReaderAt` 上划定 `[offset, offset+length)` 窗口顺序读取。
    pub struct SectionReader {
        reader: Box<dyn ReaderAt>,
        offset: i64,
        length: i64,
        cursor: i64,
    }
    pub fn NewSectionReader(reader: Box<dyn ReaderAt>, offset: i64, length: i64) -> SectionReader {
        SectionReader {
            reader,
            offset,
            length,
            cursor: 0,
        }
    }
    impl SectionReader {
        /// 尽量填满缓冲；中途 EOF 返回意外结束错误。
        pub fn read_full(&mut self, data: &mut [u8]) -> Result<usize, errors::Error> {
            let wanted = data.len().min((self.length - self.cursor).max(0) as usize);
            let mut read = 0;
            while read < wanted {
                let count = self.reader.ReadAt(
                    &mut data[read..wanted],
                    self.offset + self.cursor + read as i64,
                )?;
                if count == 0 {
                    return Err(errors::New("unexpected EOF"));
                }
                read += count;
            }
            self.cursor += read as i64;
            Ok(read)
        }
    }
}

/// 临时文件创建/删除，以及实现 `io` trait 的文件封装。
pub mod os {
    use crate::{errors, io};
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    /// 可克隆的文件句柄（内层 `Arc<Mutex<File>>`），支持按偏移读写。
    #[derive(Clone)]
    pub struct File {
        inner: Arc<Mutex<std::fs::File>>,
        path: Arc<PathBuf>,
    }
    impl File {
        pub fn Name(&self) -> String {
            self.path.to_string_lossy().into_owned()
        }
        pub fn Close(&self) -> Result<(), errors::Error> {
            self.inner.lock().unwrap().sync_all().map_err(Into::into)
        }
    }
    impl io::Writer for File {
        fn Write(&mut self, data: &[u8]) -> Result<usize, errors::Error> {
            self.inner.lock().unwrap().write(data).map_err(Into::into)
        }
    }
    impl io::WriteCloser for File {
        fn Close(&mut self) -> Result<(), errors::Error> {
            File::Close(self)
        }
    }
    impl io::ReaderAt for File {
        fn ReadAt(&self, data: &mut [u8], offset: i64) -> Result<usize, errors::Error> {
            let mut file = self.inner.lock().unwrap();
            file.seek(SeekFrom::Start(offset as u64))?;
            file.read(data).map_err(Into::into)
        }
    }
    /// 在目录下创建带前缀的临时文件并 `keep` 为持久路径。
    pub fn CreateTemp(directory: String, prefix: &str) -> Result<File, errors::Error> {
        std::fs::create_dir_all(&directory)?;
        let named = tempfile::Builder::new()
            .prefix(prefix)
            .tempfile_in(directory)?;
        let (file, path) = named.keep().map_err(|error| error.error)?;
        Ok(File {
            inner: Arc::new(Mutex::new(file)),
            path: Arc::new(path),
        })
    }
    pub fn Remove(path: String) -> Result<(), errors::Error> {
        std::fs::remove_file(path).map_err(Into::into)
    }
}

/// 桥接本模块 `WriteCloser` 到加密 crate 的写入适配器。
struct EncryptWriteAdapter(Box<dyn io::WriteCloser>);
impl encrypt_crate::aes_layer::WriteCloser for EncryptWriteAdapter {
    fn write(&mut self, data: &[u8]) -> (usize, Option<String>) {
        match self.0.Write(data) {
            Ok(count) => (count, None),
            Err(error) => (0, Some(error.to_string())),
        }
    }
    fn close(&mut self) -> Option<String> {
        self.0.Close().err().map(|error| error.to_string())
    }
}
/// 桥接本模块 `ReaderAt` 到加密 crate 的读取适配器。
struct EncryptReadAdapter(Box<dyn io::ReaderAt>);
impl encrypt_crate::aes_layer::ReaderAt for EncryptReadAdapter {
    fn read_at(&self, data: &mut [u8], offset: u64) -> (usize, Option<String>) {
        match self.0.ReadAt(data, offset as i64) {
            Ok(count) => (count, None),
            Err(error) => (0, Some(error.to_string())),
        }
    }
}

/// CTR 模式加解密 Writer/Reader 封装，供 spill 文件加密。
pub mod encrypt {
    use crate::{EncryptReadAdapter, EncryptWriteAdapter, errors, io};
    use std::sync::{Arc, Mutex};

    pub type CtrCipher = encrypt_crate::aes_layer::CtrCipher;
    pub fn NewCtrCipher() -> Result<CtrCipher, errors::Error> {
        encrypt_crate::aes_layer::NewCtrCipher().map_err(Into::into)
    }
    type Inner = encrypt_crate::aes_layer::Writer<EncryptWriteAdapter>;
    #[derive(Clone)]
    pub struct Writer(Arc<Mutex<Inner>>);
    pub fn NewWriter(file: &crate::os::File, cipher: &CtrCipher) -> Writer {
        Writer(Arc::new(Mutex::new(encrypt_crate::aes_layer::NewWriter(
            EncryptWriteAdapter(Box::new(file.clone())),
            cipher,
        ))))
    }
    impl Writer {
        pub fn GetCache(&self) -> Vec<u8> {
            self.0.lock().unwrap().GetCache().to_vec()
        }
        pub fn GetCacheDataOffset(&self) -> i64 {
            self.0.lock().unwrap().GetCacheDataOffset()
        }
    }
    impl io::Writer for Writer {
        fn Write(&mut self, data: &[u8]) -> Result<usize, errors::Error> {
            let (count, error) = self.0.lock().unwrap().Write(data);
            error.map_or_else(|| Ok(count), |error| Err(errors::New(error)))
        }
    }
    impl io::WriteCloser for Writer {
        fn Close(&mut self) -> Result<(), errors::Error> {
            self.0
                .lock()
                .unwrap()
                .Close()
                .map_or_else(|| Ok(()), |error| Err(errors::New(error)))
        }
    }
    pub struct Reader(encrypt_crate::aes_layer::Reader<EncryptReadAdapter>);
    pub fn NewReader(file: &crate::os::File, cipher: &CtrCipher) -> Reader {
        Reader(encrypt_crate::aes_layer::NewReader(
            EncryptReadAdapter(Box::new(file.clone())),
            cipher.clone(),
        ))
    }
    impl io::ReaderAt for Reader {
        fn ReadAt(&self, data: &mut [u8], offset: i64) -> Result<usize, errors::Error> {
            let (count, error) = self.0.ReadAt(data, offset);
            error.map_or_else(|| Ok(count), |error| Err(errors::New(error)))
        }
    }
}

/// 校验和层写入适配：错误转为 `(count, Option<msg>)`。
struct ChecksumWriteAdapter(Box<dyn io::WriteCloser>);
impl checksum_crate::WriteCloserDraft for ChecksumWriteAdapter {
    fn Write(&mut self, data: &[u8]) -> (usize, Option<String>) {
        match self.0.Write(data) {
            Ok(count) => (count, None),
            Err(error) => (0, Some(error.to_string())),
        }
    }
    fn Close(&mut self) -> Option<String> {
        self.0.Close().err().map(|error| error.to_string())
    }
}
struct ChecksumReadAdapter(Box<dyn io::ReaderAt>);
impl checksum_crate::ReaderAtDraft for ChecksumReadAdapter {
    fn ReadAt(&self, data: &mut [u8], offset: i64) -> (usize, Option<String>) {
        match self.0.ReadAt(data, offset) {
            Ok(count) => (count, None),
            Err(error) => (0, Some(error.to_string())),
        }
    }
}

/// 带校验和的 Writer/Reader，错误从草稿接口再映射回 `errors::Error`。
pub mod checksum {
    use crate::{ChecksumReadAdapter, ChecksumWriteAdapter, errors, io};
    use std::sync::{Arc, Mutex};

    type Inner = checksum_crate::Writer<ChecksumWriteAdapter>;
    #[derive(Clone)]
    pub struct Writer(Arc<Mutex<Inner>>);
    pub fn NewWriter(writer: Box<dyn io::WriteCloser>) -> Writer {
        Writer(Arc::new(Mutex::new(checksum_crate::NewWriter(
            ChecksumWriteAdapter(writer),
        ))))
    }
    impl Writer {
        pub fn GetCache(&self) -> Vec<u8> {
            self.0.lock().unwrap().GetCache().to_vec()
        }
        pub fn GetCacheDataOffset(&self) -> i64 {
            self.0.lock().unwrap().GetCacheDataOffset()
        }
    }
    impl io::Writer for Writer {
        fn Write(&mut self, data: &[u8]) -> Result<usize, errors::Error> {
            let (count, error) = self.0.lock().unwrap().Write(data);
            error.map_or_else(|| Ok(count), |error| Err(errors::New(error)))
        }
    }
    impl io::WriteCloser for Writer {
        fn Close(&mut self) -> Result<(), errors::Error> {
            self.0
                .lock()
                .unwrap()
                .Close()
                .map_or_else(|| Ok(()), |error| Err(errors::New(error)))
        }
    }
    pub struct Reader(checksum_crate::Reader<ChecksumReadAdapter>);
    pub fn NewReader(reader: Box<dyn io::ReaderAt>) -> Reader {
        Reader(checksum_crate::NewReader(ChecksumReadAdapter(reader)))
    }
    impl io::ReaderAt for Reader {
        fn ReadAt(&self, data: &mut [u8], offset: i64) -> Result<usize, errors::Error> {
            let (count, error) = self.0.ReadAt(data, offset);
            error.map_or_else(|| Ok(count), |error| Err(errors::New(error)))
        }
    }
}

/// 尾部内存缓存 + 磁盘前缀的合成 Reader：偏移落在缓存区则直接拷贝。
pub struct ReaderWithCache {
    reader: Box<dyn io::ReaderAt>,
    cache: Vec<u8>,
    cache_offset: i64,
}
/// 构造带尾缓存的 Reader；`cache_offset` 为缓存对应的文件逻辑起点。
pub fn NewReaderWithCache(
    reader: impl io::ReaderAt + 'static,
    cache: Vec<u8>,
    cache_offset: i64,
) -> ReaderWithCache {
    ReaderWithCache {
        reader: Box::new(reader),
        cache,
        cache_offset,
    }
}
impl io::ReaderAt for ReaderWithCache {
    fn ReadAt(&self, data: &mut [u8], offset: i64) -> Result<usize, errors::Error> {
        // 完全落在缓存：从 cache 切片拷贝。
        if offset >= self.cache_offset {
            let start = (offset - self.cache_offset) as usize;
            if start >= self.cache.len() {
                return Ok(0);
            }
            let count = data.len().min(self.cache.len() - start);
            data[..count].copy_from_slice(&self.cache[start..start + count]);
            return Ok(count);
        }
        // 先读磁盘到 cache 边界，不足则再拼缓存尾部。
        let disk_len = data.len().min((self.cache_offset - offset) as usize);
        let count = self.reader.ReadAt(&mut data[..disk_len], offset)?;
        if count < disk_len || disk_len == data.len() {
            return Ok(count);
        }
        let cached = (data.len() - disk_len).min(self.cache.len());
        data[disk_len..disk_len + cached].copy_from_slice(&self.cache[..cached]);
        Ok(disk_len + cached)
    }
}

/// Go 风格 failpoint 适配：命中时把 `return(...)` 参数投影为布尔值。
pub mod failpoint {
    #[derive(Clone, Copy)]
    pub struct Value(bool);
    impl Value {
        pub fn as_bool(self) -> bool {
            self.0
        }
    }
    pub fn Inject(name: &str, action: impl FnOnce(Value)) {
        let _ = fail::eval(name, |argument| {
            let value =
                argument.is_some_and(|argument| matches!(argument.trim(), "1" | "true" | "on"));
            action(Value(value));
        });
    }
}

pub mod rand {
    use rand_crate::Rng;
    pub use rand_crate::random;
    pub fn Int31n(limit: i32) -> i32 {
        rand_crate::rng().random_range(0..limit)
    }
}

pub mod time {
    pub type Duration = std::time::Duration;
    pub const Millisecond: Duration = Duration::from_millis(1);
    pub fn Sleep(duration: Duration) {
        std::thread::sleep(duration);
    }
}

pub mod terror {
    pub fn Call<T>(_result: T) {}
    pub fn Log<T>(_result: T) {}
}

/// 统一复用 crate 的完整行视图，避免维护第二套不完整的平行类型。
pub use crate::row::Row;

/// 按字段类型与容量从 crate 级全局池取得 Chunk。
pub fn getChunkFromPool(init_cap: usize, fields: Vec<types::FieldType>) -> Box<Chunk> {
    crate::pool::getChunkFromPool(init_cap, &fields)
}
/// 清空 Chunk 并把列归还 crate 级全局池。
pub fn putChunkFromPool(init_cap: usize, fields: Vec<types::FieldType>, mut chunk: Chunk) {
    crate::pool::putChunkFromPool(init_cap, &fields, &mut chunk);
}
pub const sizeUint32: usize = std::mem::size_of::<u32>();

// 以下模块通过 include! 注入同目录上级的核心源文件。
mod column_impl {
    use crate::*;
    include!("../../column.rs");
}
pub use column_impl::*;

mod codec_impl {
    use crate::*;
    include!("../../codec.rs");
}
pub use codec_impl::*;

mod chunk_impl {
    use crate::*;
    include!("../../chunk.rs");
}
pub use chunk_impl::*;

mod alloc_impl {
    include!("../../alloc.rs");
    use crate::*;
}
pub use alloc_impl::*;

mod chunk_util_impl {
    use crate::*;
    include!("../../chunk_util.rs");
}
pub use chunk_util_impl::*;

mod chunk_in_disk_impl {
    use crate::*;
    include!("../../chunk_in_disk.rs");
}
pub use chunk_in_disk_impl::*;

#[cfg(test)]
#[path = "../../alloc_1_aster_unit_test.rs"]
mod alloc_1_aster_unit_test;
