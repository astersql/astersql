// Copyright 2026 AsterSQL.

// 对象 I/O（objectio）crate 入口。
//
// 提供缓冲/压缩写入（`writer`）、读写接口（`interface`），并重导出
// compressedio / recording。测试配置下通过 `#[path]` 挂入上层 objstore
// 各后端模块，以保持 Go 外部包测试的本地对象存储边界。

#![allow(non_snake_case, non_upper_case_globals)]

#[cfg(test)]
extern crate self as objectio;
#[cfg(test)]
extern crate self as storeapi;

/// 压缩 I/O 子包重导出。
pub use astersql_objstore_compressedio as compressedio;
/// 压缩类型枚举重导出。
pub use astersql_objstore_compressedio::CompressType;
/// 访问统计（recording）子模块。
pub mod recording {
    pub use astersql_objstore_recording::*;
}

mod interface;
mod writer;
/// 取消上下文与 Reader/Writer trait。
pub use interface::{Context, Reader, Writer};
/// 缓冲写入器与工厂函数。
pub use writer::*;

// `writer_test` is an external-package test in Go.  Keep its local-object-store
// boundary intact while compiling the support layer from its formal source.
// `writer_test` 在 Go 中是外部包测试；保留本地对象存储边界，并从正式源码编译支撑层。
#[cfg(test)]
#[path = "../azblob.rs"]
pub mod azblob;
#[cfg(test)]
#[path = "../batch.rs"]
pub mod batch;
#[cfg(test)]
#[path = "../compress.rs"]
pub mod compress;
#[cfg(test)]
#[path = "../flags.rs"]
pub mod flags;
#[cfg(test)]
#[path = "../gcs.rs"]
pub mod gcs;
#[cfg(test)]
#[path = "../gcs_extra.rs"]
pub mod gcs_extra;
#[cfg(test)]
#[path = "../hdfs.rs"]
pub mod hdfs;
#[cfg(test)]
#[path = "../helper.rs"]
pub mod helper;
#[cfg(test)]
#[path = "../local.rs"]
pub mod local;
#[cfg(all(test, unix))]
#[path = "../local_unix.rs"]
pub mod local_unix;
#[cfg(all(test, windows))]
#[path = "../local_windows.rs"]
pub mod local_windows;
#[cfg(test)]
#[path = "../locking.rs"]
pub mod locking;
#[cfg(test)]
#[path = "../memstore.rs"]
pub mod memstore;
#[cfg(test)]
#[path = "../noop.rs"]
pub mod noop;
#[cfg(test)]
#[path = "../parse.rs"]
pub mod parse;
#[cfg(test)]
#[path = "../storage.rs"]
pub mod storage;
#[cfg(test)]
#[path = "../storeapi/storage.rs"]
mod storeapi_impl;
#[cfg(test)]
pub use storeapi_impl::*;

/// 外部包风格的 writer 集成测试（include `writer_test.rs`）。
#[cfg(test)]
mod writer_test {
    use crate as objstore;
    use crate as objectio;
    use crate::storeapi;
    include!("writer_test.rs");
}

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// 缓冲写入器迁移单元测试。
mod migration_aster_unit_test;
