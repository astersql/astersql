// Copyright 2026 AsterSQL.

// `astersql-parser-opcode` crate 入口：导出表达式运算符枚举与 format 依赖。
//
// 运算符（opcode）描述 AST 中二元/一元运算的种类；本 crate 只提供编号与文本元数据，
// 不执行表达式求值。

#![allow(non_snake_case, non_upper_case_globals)]

/// 再导出 `astersql_parser_format`，供 Restore 写回 SQL 文本时使用。
pub mod format {
    pub use astersql_parser_format::*;
}

/// 运算符定义与 Format/Restore 实现所在子模块。
pub mod opcode;
pub use opcode::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
