// Copyright 2026 AsterSQL.

// TiKV store driver crate 入口。
//
// 导出 `tikv_driver` 中的 TiKVDriver / tikvStore 等类型，并在测试配置下挂载
// 客户端追踪、配置、生命周期、快照拦截、SQL fail、事务等相关测试模块。

#![allow(non_snake_case, non_camel_case_types, dead_code)]

/// 官方 TiKV 事务客户端的同步运行时边界。
pub mod client_runtime;
pub use astersql_store_copr::{
    NetworkPdKeyspaceClient, NetworkSecurity, PdKeyspaceError, PdKeyspaceErrorKind,
};
pub use client_runtime::{ClientConfig, ClientRuntime, ClientRuntimeError, KeyspaceConfig};

/// canonical `pkg/kv` 的 client-rust 事务/快照适配。
mod kv_adapter;
mod runaway_adapter;
pub mod sst_import;

/// TiKV 驱动与存储实现（Open、事务、GC Worker 等）。
mod tikv_driver;
pub use tikv_driver::*;

#[cfg(test)]
#[path = "client_runtime_test.rs"]
mod client_runtime_test;
/// InjectTraceClient 追踪注入测试。
#[cfg(test)]
#[path = "client_test.rs"]
mod client_test;
/// 默认配置与路径解析测试。
#[cfg(test)]
#[path = "config_test.rs"]
mod config_test;
/// 标准 DAG Coprocessor transport 与 canonical Client 适配测试。
#[cfg(test)]
#[path = "coprocessor_adapter_test.rs"]
mod coprocessor_adapter_test;
/// Driver / Store 生命周期测试。
#[cfg(test)]
#[path = "driver_lifecycle_test.rs"]
mod driver_lifecycle_test;
/// client-rust canonical KV/MVCC 适配测试。
#[cfg(test)]
#[path = "kv_adapter_test.rs"]
mod kv_adapter_test;
/// 测试辅助（快照准备与清理）主测试。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
/// 本地源码 PD/TiKV 黑盒端到端验收。
#[cfg(test)]
#[path = "real_tikv_test.rs"]
mod real_tikv_test;
/// 快照拦截器相关测试。
#[cfg(test)]
#[path = "snap_interceptor_test.rs"]
mod snap_interceptor_test;
/// SQL failpoint 相关测试。
#[cfg(test)]
#[path = "sql_fail_test.rs"]
mod sql_fail_test;
#[cfg(test)]
#[path = "test_state.rs"]
mod test_state;
/// 进程级测试状态锁的健壮性测试。
#[cfg(test)]
#[path = "test_state_test.rs"]
mod test_state_test;
/// 事务驱动相关测试。
#[cfg(test)]
#[path = "txn_test.rs"]
mod txn_test;

mod read_request;
pub use tikv_client::{PointResponseStats, ReadAttempt, ReadOptions, ReadStats};

/// PD token-response runtime state used by session paging decisions.
pub use tikv_client::{proto::resource_manager as resource_manager_proto, resource_group_runtime};
