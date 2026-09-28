// Copyright 2026 AsterSQL.

// `utilparser` crate 根：导出 AST/parser 工具，并挂接测试子模块。
//
// 对应 Go `pkg/util/parser`。`parser_core` 再导出完整 `astersql_parser`；
// 本 crate 自身提供轻量匹配器、对象池与 SQL restore 辅助。

extern crate self as utilparser;

/// 再导出完整 SQL 解析器（`astersql_parser`）供本包与测试使用。
pub mod parser_core {
    pub use astersql_parser::*;
}

#[path = "ast.rs"]
pub mod ast;

#[path = "parser.rs"]
pub mod parser;

pub use ast::*;
pub use parser::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

#[cfg(test)]
#[path = "parser_test.rs"]
mod parser_test;

#[cfg(test)]
#[path = "ast_test.rs"]
mod ast_test;
