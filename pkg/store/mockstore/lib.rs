// Copyright 2026 AsterSQL.

// mockstore crate 入口：聚合嵌入式 UniStore、redirector、tikv 与 unistore 子模块。
//
// mockstore 用于单元/集成测试中模拟 KV 存储与 Coprocessor（下推计算）行为，
// 避免依赖真实 TiKV 集群。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// 嵌入式 UniStore 实现再导出（进程内 KV + MVCC）。
pub use astersql_store_mockstore_unistore as embedded_unistore;

/// mockstore 核心工厂与选项。
pub mod mockstore;
/// 请求转发/重定向相关。
pub mod redirector;
/// Tikv 风格 mock 客户端与相关类型。
pub mod tikv;
/// UniStore 适配与导出。
pub mod unistore;

pub use mockstore::*;
pub use redirector::*;

/// 集群 Region 拆分测试。
#[cfg(test)]
#[path = "cluster_test.rs"]
mod cluster_test;
/// 测试入口公共配置。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
/// mockstore factory and option parity tests.
#[cfg(test)]
#[path = "mockstore_test.rs"]
mod mockstore_test;
/// Tikv mock 相关测试。
#[cfg(test)]
#[path = "tikv_test.rs"]
mod tikv_test;
