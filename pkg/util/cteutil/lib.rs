// Copyright 2026 AsterSQL.

// CTE（公共表表达式，Common Table Expression）中间结果存储工具 crate 入口。
//
// 对应 Go `util/cteutil`：为递归/非递归 CTE 执行提供可引用计数的临时存储
//（`Storage` / `StorageRC`），并再导出 chunk、内存/磁盘 tracker 等依赖。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

extern crate self as cteutil;

/// 列式 Chunk / RowContainer 等数据结构。
pub mod chunk {
    pub use chunk_crate::*;
}
/// 磁盘用量追踪器。
pub mod disk {
    pub use disk_crate::tracker::Tracker;
}
/// 内存用量追踪器。
pub mod memory {
    pub use memory_crate::tracker::*;
}
/// 同步原语封装。
pub mod syncutil {
    pub use syncutil_crate::*;
}
/// 字段类型定义。
pub mod types {
    pub use types_crate::field::*;
}
/// MySQL 类型常量等。
pub mod mysql {
    pub use chunk_crate::mysql::*;
}

/// CTE 存储层统一错误类型。
pub mod errors {
    /// 以消息字符串承载的简单错误。
    #[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
    #[error("{0}")]
    pub struct Error(String);

    /// 由任意可转为字符串的消息构造错误。
    pub fn New(message: impl Into<String>) -> Error {
        Error(message.into())
    }

    impl From<chunk_crate::ChunkError> for Error {
        fn from(error: chunk_crate::ChunkError) -> Self {
            New(error.to_string())
        }
    }
}

/// CTE 临时存储实现（`Storage` / `StorageRC`）。
mod storage;
pub use storage::*;

/// 迁移期单元测试：引用计数、读写、溢出与锁语义。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

#[cfg(test)]
#[path = "storage_test.rs"]
mod storage_test;
