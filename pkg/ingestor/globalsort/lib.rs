// Copyright 2026 AsterSQL.

// 全局排序（globalsort）crate：外部存储上的 KV 归并、拆分、读取与导入引擎。
//
// 提供内存/对象存储抽象、KV 编解码、重复键策略与冲突统计等公共类型，
// 供 Lightning/Ingest 在全局排序后向 TiKV 导入 SST（Sorted String Table）时使用。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::sync::{Arc, RwLock};

pub mod engine;
pub mod kvgroup;
pub mod merge;
pub mod merge_v2;
pub mod reader;
pub mod split;
pub mod testutil;
pub mod util;
pub use util::*;

#[cfg(test)]
#[path = "bench_test.rs"]
mod bench_test;
#[cfg(test)]
#[path = "merge_test.rs"]
mod merge_test;
#[cfg(test)]
#[path = "merge_v2_test.rs"]
mod merge_v2_test;
#[cfg(test)]
#[path = "misc_bench_test.rs"]
mod misc_bench_test;
#[cfg(test)]
#[path = "reader_test.rs"]
mod reader_test;
#[cfg(test)]
#[path = "sort_test.rs"]
mod sort_test;
#[cfg(test)]
#[path = "split_test.rs"]
mod split_test;
#[cfg(test)]
#[path = "testutil_test.rs"]
mod testutil_test;
#[cfg(test)]
#[path = "util_test.rs"]
mod util_test;

/// 全局排序与导入路径上的统一错误类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// 操作被取消（CancellationToken / Context）。
    Cancelled,
    /// 资源已关闭，不可再使用。
    Closed,
    /// 发现重复键；携带冲突的键值。
    DuplicateKey {
        key: Vec<u8>,
        value: Vec<u8>,
    },
    InvalidArgument(String),
    InvalidData(String),
    NotFound(String),
    /// 内存配额不足；`requested` 为申请量，`limit` 为上限。
    OutOfMemory {
        requested: usize,
        limit: usize,
    },
    /// 共享锁被 poison（持锁线程 panic）。
    Poisoned,
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("operation cancelled"),
            Self::Closed => f.write_str("resource is closed"),
            Self::DuplicateKey { key, .. } => write!(f, "duplicate key found: {}", hex(key)),
            Self::InvalidArgument(message)
            | Self::InvalidData(message)
            | Self::NotFound(message) => f.write_str(message),
            Self::OutOfMemory { requested, limit } => {
                write!(
                    f,
                    "cannot acquire memory: requested {requested}, limit {limit}"
                )
            }
            Self::Poisoned => f.write_str("shared state lock is poisoned"),
        }
    }
}

impl std::error::Error for Error {}
/// 本 crate 的 `Result` 别名。
pub type Result<T> = std::result::Result<T, Error>;

/// 将字节切片格式化为小写十六进制字符串。
pub fn hex(value: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(value.len() * 2);
    for byte in value {
        result.push(DIGITS[(byte >> 4) as usize] as char);
        result.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    result
}

/// 一对编码后的键值。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KvPair {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

impl KvPair {
    /// 键与值的原始字节总长度（不含长度头）。
    pub fn encoded_size(&self) -> usize {
        self.key.len() + self.value.len()
    }
}

/// 遇到重复键时的处理策略。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OnDuplicateKey {
    #[default]
    /// 忽略语义在部分路径仍会报错，需与具体调用方一致。
    Ignore,
    /// 丢弃重复键并记录冲突信息。
    Record,
    /// 丢弃所有出现次数大于 1 的键。
    Remove,
    /// 发现重复立即返回错误。
    Error,
}

/// 重复键冲突统计。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConflictInfo {
    pub count: u64,
    pub files: Vec<String>,
}

impl ConflictInfo {
    /// 累加另一份冲突统计；计数按 Go `uint64` 规则回绕。
    pub fn merge(&mut self, other: &Self) {
        self.count = self.count.wrapping_add(other.count);
        self.files.extend(other.files.iter().cloned());
    }
}

/// 文件内一段有序范围的属性摘要。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RangeProperty {
    pub first_key: Vec<u8>,
    pub last_key: Vec<u8>,
    pub size: u64,
    pub keys: u64,
}

/// 数据文件与统计文件的配对及其范围属性。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FilePair {
    pub data_file: String,
    pub stat_file: String,
    pub properties: Vec<RangeProperty>,
}

/// 多文件写出后的文件列表统计。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MultipleFilesStat {
    pub filenames: Vec<FilePair>,
}

/// Writer 关闭时回传的写出摘要。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WriterSummary {
    pub min: Vec<u8>,
    pub max: Vec<u8>,
    pub total_size: u64,
    pub total_count: u64,
    pub multiple_files_stats: Vec<MultipleFilesStat>,
    pub conflict_info: ConflictInfo,
}

/// An object upload remains unpublished until finish succeeds. Production
/// stores implement this with their streaming/multipart writer.
pub trait ObjectWriter: std::io::Write {
    fn finish(self: Box<Self>) -> Result<()>;
}

struct BufferedObjectWriter<'a, S: Storage + ?Sized> {
    store: &'a S,
    path: String,
    bytes: Vec<u8>,
}
impl<S: Storage + ?Sized> std::io::Write for BufferedObjectWriter<'_, S> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<S: Storage + ?Sized> ObjectWriter for BufferedObjectWriter<'_, S> {
    fn finish(self: Box<Self>) -> Result<()> {
        self.store.write(&self.path, self.bytes)
    }
}

/// 对象/内存存储抽象：读写、按前缀列举与批量删除。
/// Production simplesst files use the Go u64 big-endian length headers. The
/// old memory-fixture codec remains explicit for backwards compatibility.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RecordFormat {
    #[default]
    LegacyLittleEndian32,
    GoBigEndian64,
}

pub trait Storage: Send + Sync {
    /// Production stores obtain this from object metadata, without fetching
    /// the complete body. The fallback preserves existing memory fixtures.
    fn file_size(&self, path: &str) -> Result<u64> {
        Ok(self.read(path)?.len() as u64)
    }

    fn record_format(&self) -> RecordFormat {
        RecordFormat::LegacyLittleEndian32
    }
    /// Compatibility fallback for memory fixtures. Real object stores must
    /// override this so readers hold bounded range buffers rather than objects.
    fn open(&self, path: &str) -> Result<Box<dyn std::io::Read>> {
        Ok(Box::new(std::io::Cursor::new(self.read(path)?)))
    }
    /// Open a data object at a record boundary. Production transports use an
    /// object byte range; the compatibility fallback skips through a fixed buffer.
    fn open_at(&self, path: &str, offset: u64) -> Result<Box<dyn std::io::Read>> {
        use std::io::Read;
        let mut stream = self.open(path)?;
        let skipped = std::io::copy(&mut stream.by_ref().take(offset), &mut std::io::sink())
            .map_err(|error| Error::InvalidData(error.to_string()))?;
        if skipped != offset {
            return Err(Error::InvalidData("start offset exceeds file size".into()));
        }
        Ok(stream)
    }
    /// Compatibility fallback for existing in-memory implementations.
    fn create(&self, path: &str) -> Result<Box<dyn ObjectWriter + '_>> {
        Ok(Box::new(BufferedObjectWriter {
            store: self,
            path: path.to_owned(),
            bytes: Vec::new(),
        }))
    }

    fn read(&self, path: &str) -> Result<Vec<u8>>;
    fn write(&self, path: &str, value: Vec<u8>) -> Result<()>;
    fn delete_files(&self, paths: &[String]) -> Result<()>;
    fn list_prefix(&self, prefix: &str) -> Result<Vec<String>>;
}

/// 进程内 `BTreeMap` 实现的存储，供单测使用。
#[derive(Clone, Default)]
pub struct MemoryStorage {
    files: Arc<RwLock<BTreeMap<String, Vec<u8>>>>,
}

impl Storage for MemoryStorage {
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        self.files
            .read()
            .map_err(|_| Error::Poisoned)?
            .get(path)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("object not found: {path}")))
    }

    fn write(&self, path: &str, value: Vec<u8>) -> Result<()> {
        self.files
            .write()
            .map_err(|_| Error::Poisoned)?
            .insert(path.to_owned(), value);
        Ok(())
    }

    fn delete_files(&self, paths: &[String]) -> Result<()> {
        let mut files = self.files.write().map_err(|_| Error::Poisoned)?;
        for path in paths {
            files.remove(path);
        }
        Ok(())
    }

    fn list_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(self
            .files
            .read()
            .map_err(|_| Error::Poisoned)?
            .keys()
            .filter(|path| path.starts_with(prefix))
            .cloned()
            .collect())
    }
}

/// 生成严格大于 `key` 的下一键（末尾追加 0 字节）。
pub fn next_key(key: &[u8]) -> Vec<u8> {
    let mut result = key.to_vec();
    result.push(0);
    result
}

/// 将 KV 列表编码为长度前缀二进制格式（小端 u32 键长/值长 + 载荷）。
pub fn encode_kvs(kvs: &[KvPair]) -> Vec<u8> {
    let capacity = kvs.iter().map(|pair| pair.encoded_size() + 8).sum();
    let mut output = Vec::with_capacity(capacity);
    for pair in kvs {
        output.extend_from_slice(&(pair.key.len() as u32).to_le_bytes());
        output.extend_from_slice(&(pair.value.len() as u32).to_le_bytes());
        output.extend_from_slice(&pair.key);
        output.extend_from_slice(&pair.value);
    }
    output
}

/// 自 `start_offset` 起解码长度前缀 KV 流；截断或溢出返回 `InvalidData`。
pub fn decode_kvs(data: &[u8], start_offset: usize) -> Result<Vec<KvPair>> {
    let mut cursor = start_offset;
    let mut output = Vec::new();
    while cursor < data.len() {
        if data.len() - cursor < 8 {
            return Err(Error::InvalidData("truncated KV length header".into()));
        }
        let key_len = u32::from_le_bytes(data[cursor..cursor + 4].try_into().unwrap()) as usize;
        let value_len =
            u32::from_le_bytes(data[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
        cursor += 8;
        let end = cursor
            .checked_add(key_len)
            .and_then(|value| value.checked_add(value_len))
            .ok_or_else(|| Error::InvalidData("KV length overflow".into()))?;
        if end > data.len() {
            return Err(Error::InvalidData("truncated KV payload".into()));
        }
        output.push(KvPair {
            key: data[cursor..cursor + key_len].to_vec(),
            value: data[cursor + key_len..end].to_vec(),
        });
        cursor = end;
    }
    Ok(output)
}

/// 半开键范围 `[start, end)`。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyRange {
    pub start: Vec<u8>,
    pub end: Vec<u8>,
}
