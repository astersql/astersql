// Copyright 2026 AsterSQL.

// simplesst：简化版 SST（Sorted String Table）读写与多路归并管线。
//
// 供 ingest（批量导入）使用：编码有序 KV、写数据/统计文件、并发预取，
// 以及多文件 MergeSort（归并排序）迭代。`MemoryStorage` 模拟对象存储的整对象可见语义。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
#[cfg(test)]
use std::sync::Mutex;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
#[cfg(test)]
use std::time::Duration;

pub mod byte_reader;
pub mod codec;
pub mod concurrent_reader;
pub mod file;
pub mod iter;
pub mod kv_reader;
pub mod onefile_writer;
pub mod stat_reader;
pub mod util;
pub mod writer;

pub use byte_reader::ByteReader;
pub use codec::RangeProperty;

#[cfg(test)]
mod byte_reader_test;
#[cfg(test)]
mod codec_test;
#[cfg(test)]
mod concurrent_reader_test;
#[cfg(test)]
mod file_test;
#[cfg(test)]
mod iter_test;
#[cfg(test)]
mod onefile_writer_test;
#[cfg(test)]
mod util_test;
#[cfg(test)]
mod writer_test;

/// simplesst 管线共用错误类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// 资源已关闭后仍被使用。
    Closed,
    /// 在记录边界读取时正常到达文件末尾。
    Eof,
    /// 已读到部分记录后到达文件末尾，表示内容损坏或被截断。
    UnexpectedEof(String),
    /// 检测到重复键（DuplicateMode::Error 策略）。
    DuplicateKey { key: Vec<u8>, value: Vec<u8> },
    /// 编码/解码或参数非法。
    InvalidData(String),
    /// IO 类错误；`UnexpectedEof` 常表示正常迭代结束。
    Io(std::io::ErrorKind, String),
    /// 对象路径不存在。
    NotFound(String),
    /// 存储读写锁被毒化。
    Poisoned,
}

impl Error {
    /// 构造表示文件结束的 EOF 错误。
    pub fn eof() -> Self {
        Self::Eof
    }

    /// 构造表示记录被截断的 UnexpectedEOF 错误。
    pub fn unexpected_eof(context: impl Into<String>) -> Self {
        Self::UnexpectedEof(context.into())
    }

    /// 是否为 EOF（正常耗尽输入）。
    pub fn is_eof(&self) -> bool {
        matches!(self, Self::Eof)
    }

    /// 是否为记录内部的意外 EOF。
    pub fn is_unexpected_eof(&self) -> bool {
        matches!(self, Self::UnexpectedEof(_))
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => f.write_str("resource is closed"),
            Self::Eof => f.write_str("end of file"),
            Self::UnexpectedEof(context) => write!(f, "unexpected end of file: {context}"),
            Self::DuplicateKey { key, .. } => write!(f, "duplicate key: {key:?}"),
            Self::InvalidData(message) | Self::NotFound(message) => f.write_str(message),
            Self::Io(_, message) => f.write_str(message),
            Self::Poisoned => f.write_str("storage lock is poisoned"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        if value.kind() == std::io::ErrorKind::UnexpectedEof {
            Self::unexpected_eof(value.to_string())
        } else {
            Self::Io(value.kind(), value.to_string())
        }
    }
}

/// 本 crate 统一的 `Result` 别名。
pub type Result<T> = std::result::Result<T, Error>;

/// 可克隆、确定性的内存对象存储，供移植后的 SST 管线使用。
///
/// 语义与 Go 存储 API 一致：读者仅在写入方提交整对象后才能看见文件。
/// A deterministic, cloneable object store used by the ported SST pipeline.
/// Its operations have the same whole-object visibility semantics used by the
/// Go store API: readers only see a file after the writer commits it.
#[derive(Clone, Debug)]
pub struct MemoryStorage {
    /// path -> 完整对象内容；写覆盖、读克隆。
    files: Arc<RwLock<BTreeMap<String, Vec<u8>>>>,
    #[cfg(test)]
    write_failures: Arc<Mutex<Vec<WriteFailure>>>,
    #[cfg(test)]
    write_attempts: Arc<Mutex<Vec<String>>>,
    #[cfg(test)]
    read_delay_ms: Arc<AtomicU64>,
    #[cfg(test)]
    active_reads: Arc<AtomicUsize>,
    #[cfg(test)]
    max_active_reads: Arc<AtomicUsize>,
}

#[cfg(test)]
#[derive(Clone, Debug)]
struct WriteFailure {
    path_fragment: String,
    remaining: usize,
}

impl Default for MemoryStorage {
    fn default() -> Self {
        Self {
            files: Arc::default(),
            #[cfg(test)]
            write_failures: Arc::default(),
            #[cfg(test)]
            write_attempts: Arc::default(),
            #[cfg(test)]
            read_delay_ms: Arc::default(),
            #[cfg(test)]
            active_reads: Arc::default(),
            #[cfg(test)]
            max_active_reads: Arc::default(),
        }
    }
}

impl MemoryStorage {
    /// 提交（覆盖）整对象；提交前读者看不到本次写入。
    pub fn write(&self, path: impl Into<String>, data: Vec<u8>) -> Result<()> {
        let path = path.into();
        #[cfg(test)]
        {
            self.write_attempts
                .lock()
                .map_err(|_| Error::Poisoned)?
                .push(path.clone());
            let mut failures = self.write_failures.lock().map_err(|_| Error::Poisoned)?;
            if let Some(rule) = failures
                .iter_mut()
                .find(|rule| rule.remaining > 0 && path.contains(&rule.path_fragment))
            {
                rule.remaining -= 1;
                return Err(Error::Io(
                    std::io::ErrorKind::Other,
                    format!("injected write failure for {path}"),
                ));
            }
        }
        self.files
            .write()
            .map_err(|_| Error::Poisoned)?
            .insert(path, data);
        Ok(())
    }

    /// 读取整对象；路径不存在返回 `NotFound`。
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        #[cfg(test)]
        {
            let active = self.active_reads.fetch_add(1, Ordering::AcqRel) + 1;
            self.max_active_reads.fetch_max(active, Ordering::AcqRel);
            let delay = self.read_delay_ms.load(Ordering::Acquire);
            if delay > 0 {
                std::thread::sleep(Duration::from_millis(delay));
            }
        }
        let result = match self.files.read() {
            Ok(files) => files
                .get(path)
                .cloned()
                .ok_or_else(|| Error::NotFound(format!("object not found: {path}"))),
            Err(_) => Err(Error::Poisoned),
        };
        #[cfg(test)]
        self.active_reads.fetch_sub(1, Ordering::AcqRel);
        result
    }

    /// 按偏移与长度切片读取；越界返回 EOF。
    pub fn read_range(&self, path: &str, offset: usize, length: usize) -> Result<Vec<u8>> {
        let data = self.read(path)?;
        let end = offset
            .checked_add(length)
            .ok_or_else(|| Error::InvalidData("range overflow".into()))?;
        if end > data.len() {
            return Err(Error::eof());
        }
        Ok(data[offset..end].to_vec())
    }

    /// 列出当前所有已提交对象路径。
    pub fn list(&self) -> Result<Vec<String>> {
        Ok(self
            .files
            .read()
            .map_err(|_| Error::Poisoned)?
            .keys()
            .cloned()
            .collect())
    }

    /// 测试边界：让接下来匹配路径片段的若干次写入失败。
    #[cfg(test)]
    pub fn fail_next_writes_containing(&self, path_fragment: &str, count: usize) -> Result<()> {
        self.write_failures
            .lock()
            .map_err(|_| Error::Poisoned)?
            .push(WriteFailure {
                path_fragment: path_fragment.into(),
                remaining: count,
            });
        Ok(())
    }

    /// 返回匹配路径片段的写入尝试次数，用于验证重试而非固定成功。
    #[cfg(test)]
    pub fn write_attempts_containing(&self, path_fragment: &str) -> Result<usize> {
        Ok(self
            .write_attempts
            .lock()
            .map_err(|_| Error::Poisoned)?
            .iter()
            .filter(|path| path.contains(path_fragment))
            .count())
    }

    /// 测试边界：为每次对象读取增加固定延迟，使并发上限可稳定观测。
    #[cfg(test)]
    pub fn set_read_delay(&self, delay: Duration) {
        self.read_delay_ms
            .store(delay.as_millis() as u64, Ordering::Release);
    }

    /// 历史最大并发读取数。
    #[cfg(test)]
    pub fn max_concurrent_reads(&self) -> usize {
        self.max_active_reads.load(Ordering::Acquire)
    }

    /// 清零并发读取峰值统计。
    #[cfg(test)]
    pub fn reset_read_metrics(&self) {
        self.active_reads.store(0, Ordering::Release);
        self.max_active_reads.store(0, Ordering::Release);
    }
}
