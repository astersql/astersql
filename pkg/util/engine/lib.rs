// Copyright 2026 AsterSQL.

// `util/engine` crate 入口：按 store label 识别 TiFlash 引擎节点。
//
// 对应 Go `pkg/util/engine`。对外导出 `engine` 模块 API；测试配置下挂载
// 表驱动单元测试与迁移回归用例。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// TiFlash / store label 识别实现。
pub mod engine;
/// 重新导出 `engine` 中的公开类型与函数。
pub use engine::*;

/// 表驱动单元测试：HTTP 响应路径下的 TiFlash 判定。
#[cfg(test)]
#[path = "engine_test.rs"]
mod engine_test;

/// 迁移期回归：kvproto 与 HTTP 写/算节点过滤。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
