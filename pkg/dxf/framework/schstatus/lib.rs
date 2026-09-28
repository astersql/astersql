// Copyright 2026 AsterSQL.

// DXF 调度状态（schstatus）包入口。
//
// 聚合调度运行时的状态查询（`status`）与调优参数（`tune`）子模块，
// 并向外再导出其公共 API。DXF（Distributed eXecution Framework）的 owner
// 节点可据此观察任务队列、节点资源需求等调度快照。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 调度状态查询与快照定义。
pub mod status;
/// 调度相关调优（tune）参数。
pub mod tune;
pub use status::*;
pub use tune::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "status_test.rs"]
mod status_test;
