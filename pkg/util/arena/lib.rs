// Copyright 2026 AsterSQL.

// Arena 工具 crate 入口。
//
// 导出 `arena` 模块中的分配器与缓冲类型；测试配置下挂载 Go 对应测试、
// `TestMain` 兼容入口以及迁移回归用例。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// Arena 分配器与 `ArenaBuffer` 实现。
pub mod arena;
pub use arena::*;

#[cfg(test)]
#[path = "arena_test.rs"]
/// 对应 Go `arena_test.go` 的单元测试。
mod arena_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// AsterSQL 迁移补充回归测试。
mod migration_aster_unit_test;
