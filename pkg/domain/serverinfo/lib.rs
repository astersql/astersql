// Copyright 2026 AsterSQL.

// serverinfo crate 入口：节点 ServerInfo 与 etcd 同步。
//
// 导出 `info`（数据结构/JSON）与 `syncer`（etcd 登记、拓扑刷新、陈旧清理），
// 测试配置下挂载 `syncer_test`。

#![allow(
    dead_code,
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    unused_imports,
    unused_variables
)]

/// ServerInfo / TopologyInfo 类型与编解码。
mod info;
mod real_etcd;
/// Syncer：向 etcd 写入/查询服务器与拓扑信息。
mod syncer;

pub use info::*;
pub use real_etcd::*;
pub use syncer::*;

/// ServerInfo JSON、克隆与 Go 行为一致性单测。
#[cfg(test)]
#[path = "info_test.rs"]
mod info_test;

/// Syncer 拓扑、陈旧清理与跨 keyspace 假定身份相关单测。
#[cfg(test)]
#[path = "syncer_test.rs"]
mod syncer_test;
