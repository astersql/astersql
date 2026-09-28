// Copyright 2026 AsterSQL.

// Store（存储层）crate 入口。
//
// 聚合 etcd 相关工具与存储打开/连接逻辑：根据路径与配置连接到 TiKV 或
// 其他 Store 后端，处理 PD（Placement Driver）引导、重试与错误分类。

#![allow(non_snake_case, non_upper_case_globals)]

/// etcd 客户端与相关辅助逻辑。
mod etcd;
/// 存储打开、重试与错误分类主实现。
mod store;
pub use astersql_store_driver::{
    NetworkPdKeyspaceClient, NetworkSecurity, PdKeyspaceError, PdKeyspaceErrorKind,
};
pub use etcd::*;
pub use store::*;

/// Aster 迁移对照的 store 单元测试。
#[cfg(test)]
mod migration_aster_unit_test {
    include!("migration_aster_unit_test.rs");
}
/// Batch Coprocessor（批量协处理器）相关测试。
#[cfg(test)]
#[path = "batch_coprocessor_test.rs"]
mod batch_coprocessor_test;
/// etcd 模块行为测试。
#[cfg(test)]
#[path = "etcd_test.rs"]
mod etcd_test;
/// 对应 Go TestMain 的测试入口校验。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
/// Store 打开与连接逻辑测试。
#[cfg(test)]
#[path = "store_test.rs"]
mod store_test;
