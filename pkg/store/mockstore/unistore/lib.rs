// Copyright 2026 AsterSQL.

// unistore crate 入口：嵌入式 mock TiKV 子系统聚合层。
//
// unistore 在进程内提供 Region（键空间分片）、MVCC（多版本并发控制）、
// Raft 写入与 PD（Placement Driver，集群元数据）等能力，供 TiDB 单测模拟真实 TiKV，
// 无需外部集群。本文件再导出 config/server/tikv 依赖 crate，并声明 cluster、mock、
// pd、raw_handler、rpc、testutil 等子模块。

#![allow(
    ambiguous_glob_reexports,
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// UniStore 配置 crate 再导出。
pub use astersql_store_mockstore_unistore_config as config;
/// UniStore 服务端 crate 再导出。
pub use astersql_store_mockstore_unistore_server as server;
/// UniStore 侧 TiKV 协议/客户端相关 crate 再导出。
pub use astersql_store_mockstore_unistore_tikv as tikv;

/// 嵌入式集群（Region 拓扑与调度）模拟。
pub mod cluster;
/// Mock 存储与测试替身。
pub mod mock;
/// PD 客户端与心跳相关。
pub mod pd;
/// Raw KV 请求处理。
pub mod raw_handler;
/// RPC 入口与消息适配。
pub mod rpc;
/// 测试辅助工具。
pub mod testutil;

pub use cluster::*;
pub use mock::*;
pub use pd::*;
pub use raw_handler::*;
pub use rpc::*;
pub use testutil::*;

/// Embedded UniStore construction and temporary-path lifecycle tests.
#[cfg(test)]
#[path = "mock_test.rs"]
mod mock_test;

/// Cluster 拆分与延迟调度测试。
#[cfg(test)]
#[path = "cluster_test.rs"]
mod cluster_test;
/// 测试入口公共配置。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
/// PD 客户端相关测试。
#[cfg(test)]
#[path = "pd_test.rs"]
mod pd_test;
/// Raw KV 处理相关测试。
#[cfg(test)]
#[path = "raw_handler_test.rs"]
mod raw_handler_test;

/// RPC dispatch and Go parity tests.
#[cfg(test)]
#[path = "rpc_test.rs"]
mod rpc_test;

/// TopSQL resource-tag request mapping parity tests.
#[cfg(test)]
#[path = "testutil_test.rs"]
mod testutil_test;

// Go TestMain performs common setup before m.Run, including filtered test
// runs. Rust's default test harness has no TestMain hook, so register a
// platform loader initializer that runs before libtest starts its workers.
#[cfg(test)]
pub(crate) static TEST_ENVIRONMENT_INITIALIZED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
#[used]
#[cfg_attr(
    target_vendor = "apple",
    unsafe(link_section = "__DATA,__mod_init_func")
)]
#[cfg_attr(
    all(unix, not(target_vendor = "apple")),
    unsafe(link_section = ".init_array")
)]
#[cfg_attr(windows, unsafe(link_section = ".CRT$XCU"))]
static INITIALIZE_TEST_ENVIRONMENT: extern "C" fn() = {
    extern "C" fn initialize() {
        astersql_testkit_testsetup::SetupForCommonTest();
        TEST_ENVIRONMENT_INITIALIZED.store(true, std::sync::atomic::Ordering::Release);
    }
    initialize
};
