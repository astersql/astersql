// Copyright 2026 AsterSQL.

// `tidbmanager` crate 入口：导出与 TiDB Manager 通信的客户端，并挂载相关测试模块。
//
// Manager 负责编排 TiDB Pod 生命周期；本 crate 提供 `free` 等 HTTP API 封装。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// TiDB Manager HTTP 客户端实现（见 `tidbmanager.rs`）。
pub mod tidbmanager;
pub use tidbmanager::*;

#[cfg(test)]
#[path = "tidbmanager_test.rs"]
mod tidbmanager_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
