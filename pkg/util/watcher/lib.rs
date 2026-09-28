// Copyright 2026 AsterSQL.

// watcher crate 根：轮询式文件系统监视器。
//
// 导出事件模型（`event`）与监视器本体（`watcher`）；测试下挂载迁移对照
// 与功能单测模块。常用于 binlog 文件等路径变更感知。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 文件变更事件与操作位定义。
pub mod event;
/// 轮询 Watcher：Add/Remove/Start/Close 与事件投递。
pub mod watcher;
pub use event::*;
pub use watcher::*;

/// 与 Go 行为对照的迁移单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

/// Watcher 功能单测（创建/修改/重命名/跨目录移动等）。
#[cfg(test)]
#[path = "watcher_test.rs"]
mod watcher_test;
