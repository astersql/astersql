// Copyright 2026 AsterSQL.

// 对象存储（objstore）crate 入口：统一抽象与各后端实现的模块汇总。
//
// 对应 TiDB/BR 侧的对象存储层，对外提供：
// - `objectio` / `storeapi` 读写接口再导出；
// - 本地、内存、S3/GCS/AzBlob/HDFS 等后端与解析、压缩、分布式锁等子模块；
// - 测试通过 `include!` / `#[path]` 编入本 crate。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as objstore;

/// 对象 IO 抽象再导出。
pub use objectio;
/// 存储 API 再导出。
pub use storeapi;

/// Azure Blob 后端。
pub mod azblob;
/// 批量对象操作。
pub mod batch;
/// 压缩读写封装。
pub mod compress;
/// 后端相关 CLI/配置 flags。
pub mod flags;
/// GCS（Google Cloud Storage）后端。
pub mod gcs;
/// GCS 额外辅助逻辑。
pub mod gcs_extra;
/// HDFS 后端。
pub mod hdfs;
/// 通用辅助：URI 校验、目录反序列化、上传 worker 计数等。
pub mod helper;
/// 本地文件系统后端（`file://`）。
pub mod local;
/// Unix 下创建目录（含 umask 处理）。
pub mod local_unix;
/// Windows 下创建目录。
pub mod local_windows;
/// 基于对象存储的远程互斥/读写锁。
pub mod locking;
/// 内存对象存储（测试与轻量场景）。
pub mod memstore;
/// 空操作（noop）存储实现。
pub mod noop;
/// 存储 URI/Backend 解析。
pub mod parse;
/// Storage trait、选项与工厂等核心类型。
pub mod storage;

#[cfg(test)]
/// AzBlob 相关 Aster 单元测试夹具。
mod azblob_1_aster_unit_test {
    /// 将 azblob 组依赖符号再导出到测试作用域。
    mod azblob_test_support {
        pub use crate::azblob::*;
        pub use crate::batch::*;
        pub use crate::compress::*;
        pub use crate::flags::*;
        pub use crate::gcs::*;
        pub use crate::gcs_extra::*;
        pub use crate::hdfs::*;
    }
    include!("azblob_1_aster_unit_test.rs");
}

#[cfg(test)]
/// helper/local/locking/memstore 等 Aster 单元测试夹具。
mod helper_2_aster_unit_test {
    /// 将第二组 objstore 依赖符号再导出到测试作用域。
    mod helper_test_support {
        pub use crate::helper::*;
        pub use crate::local::*;
        pub use crate::locking::*;
        pub use crate::memstore::*;
        pub use crate::noop::*;
        pub use crate::parse::*;
        pub use crate::storage::*;
    }
    include!("helper_2_aster_unit_test.rs");
}

#[cfg(test)]
#[path = "azblob_test.rs"]
mod azblob_test;
#[cfg(test)]
#[path = "batch_test.rs"]
mod batch_test;
#[cfg(test)]
#[path = "compress_test.rs"]
mod compress_test;
#[cfg(test)]
#[path = "flags_test.rs"]
mod flags_test;
#[cfg(test)]
#[path = "gcs_test.rs"]
mod gcs_test;
#[cfg(test)]
#[path = "hdfs_test.rs"]
mod hdfs_test;
#[cfg(test)]
#[path = "helper_test.rs"]
mod helper_test;
#[cfg(test)]
#[path = "local_test.rs"]
mod local_test;
#[cfg(test)]
#[path = "locking_test.rs"]
mod locking_test;
#[cfg(test)]
#[path = "memstore_test.rs"]
mod memstore_test;
#[cfg(test)]
#[path = "parse_test.rs"]
mod parse_test;
#[cfg(test)]
#[path = "storage_test.rs"]
mod storage_test;
