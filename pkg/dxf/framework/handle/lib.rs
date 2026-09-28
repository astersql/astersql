// Copyright 2026 AsterSQL.

// DXF Handle crate 入口：对外暴露任务控制与调度状态查询 API。
//
// 子模块：
// - `handle`：提交/等待/取消任务、重试、云存储 URI、计量等
// - `status`：调度状态、忙碌节点、所需节点数计算等

#![allow(non_snake_case, non_upper_case_globals)]

/// 任务/子任务协议类型。
pub use astersql_dxf_framework_proto as proto;
/// 调度状态（Schedule Status）类型。
pub use astersql_dxf_framework_schstatus as schstatus;
/// 任务存储与历史页等类型。
pub use astersql_dxf_framework_storage as storage;

/// 任务控制实现。
mod handle;
/// 调度状态聚合与节点需求计算。
mod status;

pub use handle::*;
pub use status::*;

#[cfg(test)]
mod handle_test;
#[cfg(test)]
mod status_test;
#[cfg(test)]
mod status_testkit_test;
