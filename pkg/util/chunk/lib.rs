// Copyright 2026 AsterSQL.

// `util/chunk` crate 入口：列式 Chunk 运行时、行容器与磁盘落盘相关模块聚合。
//
// 对应 Go `pkg/util/chunk`。本文件导出内存/磁盘 Tracker 适配、统一错误类型，
// 并通过 `include!` / `#[path]` 挂接 Column、Codec、Compare、List、Iterator、
// MutRow、Pool、RowContainer 及各类单元测试。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

pub use rand_crate;

// These adapters are the package's already-integrated dependencies.  The first
// vertical group owns the detailed I/O/encryption adapters; explicit root
// aliases below make those private adapter types available while that harness
// is compiled as a module.
/// 内存用量追踪与查询中断信号，对应 Go `memory` 包中的 Tracker / SQLKiller。
pub mod memory {
    pub use memory_crate::sqlkiller::QueryInterrupted;
    pub use memory_crate::tracker::*;
}

/// 临时目录与磁盘 Tracker 适配，供 Chunk 落盘（spill）使用。
pub mod disk {
    pub type Tracker = Box<memory_crate::tracker::Tracker>;
    /// 构造带标签与字节上限的磁盘用量 Tracker；`limit < 0` 表示不限制。
    pub fn NewTracker(label: i32, limit: i64) -> Tracker {
        memory_crate::tracker::NewTracker(label, limit)
    }
    pub use disk_crate::CheckAndInitTempDir;
}

/// Chunk 子系统统一错误：逻辑消息或 I/O 失败字符串。
#[derive(Clone, Debug, thiserror::Error)]
pub enum ChunkError {
    #[error("{0}")]
    Message(String),
    #[error("{0}")]
    Io(String),
}

impl From<std::io::Error> for ChunkError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

/// 本 crate 默认 `Result`，错误类型为 [`ChunkError`]。
pub type Result<T, E = ChunkError> = std::result::Result<T, E>;

/// 把 crate 内 `io::WriteCloser` 适配为加密层的 `WriteCloser`。
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

/// 把 crate 内 `io::ReaderAt` 适配为加密层的 `ReaderAt`。
struct EncryptReadAdapter(Box<dyn io::ReaderAt>);
impl encrypt_crate::aes_layer::ReaderAt for EncryptReadAdapter {
    fn read_at(&self, data: &mut [u8], offset: u64) -> (usize, Option<String>) {
        match self.0.ReadAt(data, offset as i64) {
            Ok(count) => (count, None),
            Err(error) => (0, Some(error.to_string())),
        }
    }
}

/// 校验和写入适配：错误转为 `(0, Some(msg))` 的 Go 风格草稿接口。
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

/// 校验和随机读适配，同样把 `Result` 展开为 `(count, Option<err>)`。
struct ChecksumReadAdapter(Box<dyn io::ReaderAt>);
impl checksum_crate::ReaderAtDraft for ChecksumReadAdapter {
    fn ReadAt(&self, data: &mut [u8], offset: i64) -> (usize, Option<String>) {
        match self.0.ReadAt(data, offset) {
            Ok(count) => (count, None),
            Err(error) => (0, Some(error.to_string())),
        }
    }
}

#[path = "row.rs"]
pub mod row;
pub use row::Row;

// group1 聚合 Column/Chunk/Codec 等核心实现与 I/O 适配层。
#[path = "internal/group1/lib.rs"]
mod group_1;
pub use group_1::*;

impl From<group_1::errors::Error> for ChunkError {
    fn from(error: group_1::errors::Error) -> Self {
        Self::Message(error.to_string())
    }
}

/// 通过 `include!` 注入比较与上下界查找实现，保持与 Go 同文件语义。
mod compare_impl {
    use crate::*;
    include!("compare.rs");
}
pub use compare_impl::*;

#[path = "list.rs"]
pub mod list;
pub use list::{List, RowPtr};
#[path = "iterator.rs"]
pub mod iterator;
#[path = "mutrow.rs"]
pub mod mutrow;
#[path = "pool.rs"]
pub mod pool;
#[path = "row_container_reader.rs"]
pub mod row_container_reader;

#[path = "row_in_disk.rs"]
pub mod row_in_disk;
pub use row_in_disk::*;
#[path = "row_container.rs"]
pub mod row_container;
pub use row_container::*;

#[cfg(test)]
#[path = "alloc_test.rs"]
mod alloc_test;
#[cfg(test)]
#[path = "chunk_in_disk_test.rs"]
mod chunk_in_disk_test;
#[cfg(test)]
#[path = "chunk_test.rs"]
mod chunk_test;
#[cfg(test)]
#[path = "chunk_util_test.rs"]
mod chunk_util_test;
#[cfg(test)]
#[path = "codec_2_aster_unit_test.rs"]
mod codec_2_aster_unit_test;
#[cfg(test)]
#[path = "codec_test.rs"]
mod codec_test;
#[cfg(test)]
#[path = "column_test.rs"]
mod column_test;
#[cfg(test)]
#[path = "iterator_3_aster_unit_test.rs"]
mod iterator_3_aster_unit_test;
#[cfg(test)]
#[path = "iterator_test.rs"]
mod iterator_test;
#[cfg(test)]
#[path = "list_test.rs"]
mod list_test;
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "mutrow_test.rs"]
mod mutrow_test;
#[cfg(test)]
#[path = "pool_test.rs"]
mod pool_test;
#[cfg(test)]
#[path = "row_container_4_aster_unit_test.rs"]
mod row_container_4_aster_unit_test;
#[cfg(test)]
#[path = "row_container_test.rs"]
mod row_container_test;
#[cfg(test)]
#[path = "row_in_disk_test.rs"]
mod row_in_disk_test;
#[cfg(test)]
#[path = "row_test.rs"]
mod row_test;
