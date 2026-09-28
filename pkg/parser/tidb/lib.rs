// Copyright 2026 AsterSQL.

// TiDB 解析器扩展 crate 入口。
//
// 对照 Go `pkg/parser/tidb`：对外再导出特性标识（feature ID）相关符号，
// 并在测试配置下挂载迁移对齐单元测试。特性标识用于标记 DDL/语法中的 TiDB 专有能力。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 特性标识与可解析性检查模块。
pub mod features;
pub use features::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// 与 Go 侧 feature ID 常量及 CanParseFeature 行为对齐的单元测试。
mod migration_aster_unit_test;
