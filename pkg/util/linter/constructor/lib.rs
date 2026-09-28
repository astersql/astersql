// Copyright 2026 AsterSQL.

// 构造函数 linter 标记包入口。
//
// 对应 Go `pkg/util/linter/constructor`。再导出 `Constructor` 标记类型，
// 供静态检查限制结构体仅能在白名单构造函数中创建。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 构造函数标记类型定义。
pub mod constructorflag;
pub use constructorflag::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
