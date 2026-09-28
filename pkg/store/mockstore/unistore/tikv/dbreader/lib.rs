// Copyright 2026 AsterSQL.

// unistore DBReader crate 入口。
//
// 聚合存储引擎（fjall）、协议类型（kvproto）与 MVCC 依赖，并导出
// `db_reader` 中的只读事务抽象；测试配置下挂载迁移单测。

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

pub use fjall;
/// Region 错误协议类型（errorpb）。
pub mod errorpb {
    pub use kvproto::errorpb::*;
}
/// KV 层冲突等错误类型。
pub mod kverrors {
    pub use kverrors_crate::kverrors::*;
}
/// 键范围等 KV 辅助类型。
pub mod kv {
    pub use kvproto::kvrpcpb::KeyRange;
}
/// TiKV KV RPC 协议消息。
pub mod kvrpcpb {
    pub use kvproto::kvrpcpb::*;
}
/// 集群元数据协议（Region/Peer 等）。
pub mod metapb {
    pub use kvproto::metapb::*;
}
/// MVCC 用户元数据与相关工具。
pub mod mvcc {
    pub use mvcc_crate::mvcc::*;
}
/// DBReader 实现：基于 ReadTxn/DBIterator 的 MVCC 只读路径。
pub mod db_reader;
pub use db_reader::*;

/// Aster 迁移单测。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
