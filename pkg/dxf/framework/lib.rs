// Copyright 2026 AsterSQL.
// DXF framework crate 入口。
//
// 再导出 `doc` 模块中的包级文档与公共类型；测试时挂载迁移相关单元测试。
// DXF（Distributed eXecution Framework）负责分布式后台任务的统一调度与资源管理。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 包级文档与 DXF 概念说明（调度、slot、任务步骤等）。
pub mod doc;
/// 将 `doc` 中的公共项提升到 crate 根，便于外部直接引用。
pub use doc::*;

#[cfg(test)]
/// AsterSQL 迁移相关的框架层单元测试（仅 test 配置编译）。
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
