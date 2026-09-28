// Copyright 2026 AsterSQL.

// SQL 解析器驱动层（parser_driver）的 crate 入口。
//
// 向解析器注册字面量构造钩子，并导出 `ValueExpr` 等值表达式类型；
// 测试通过 `#[path]` 挂载迁移单元测试与 Go 对齐用例。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as parser_driver;

/// 复用 `parser_format` 的 Restore 标志与上下文。
pub use parser_format as format;

mod value_expr;
/// 导出值表达式、参数占位符与字面量构造入口。
pub use value_expr::*;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
#[cfg(test)]
#[path = "value_expr_test.rs"]
mod value_expr_test;
