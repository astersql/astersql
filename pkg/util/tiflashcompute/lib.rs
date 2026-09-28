// Copyright 2026 AsterSQL.

// TiFlash Compute 工具 crate 入口：分发策略与拓扑获取（topo fetcher）。
//
// 再导出 `config` / `vardef`，并公开 `dispatch_policy` 与 `topo_fetcher`
// 模块，供 MPP/自动扩缩相关逻辑选择计算节点与查询拓扑。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

pub use config;
pub use vardef;

/// 任务分发策略（轮询 / 一致性哈希）。
pub mod dispatch_policy;
/// 自动扩缩拓扑获取器（AWS / mock / test / 全局初始化）。
pub mod topo_fetcher;
pub use dispatch_policy::*;
pub use topo_fetcher::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
