// Copyright 2026 AsterSQL.

// 生成列表达式工具 crate 入口。
//
// 对应 Go `pkg/util/generatedexpr`：导出 `ParseExpression` /
// `SimpleResolveName`，并经依赖桥接 AST、字符集、表元数据与解析器。
// 测试配置下挂载 Go 对齐用例、`TestMain` 风格公共 setup 与迁移回归。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// AST 节点与表达式类型（来自 parser 依赖）。
pub mod ast {
    pub use parser_dependency::ast::*;
}
/// 默认字符集与排序规则查询。
pub mod charset {
    pub use parser_dependency::charset::charset::GetDefaultCharsetAndCollate;
}
/// 错误构造与传播类型。
pub mod errors {
    pub use parser_dependency::errors::*;
}
/// 表/列元数据（`TableInfo` / `ColumnInfo`）。
pub mod model {
    pub use model_dependency::group_1::{ColumnInfo, TableInfo};
}
/// 解析器连接字符集/排序规则选项。
pub mod parser {
    pub use parser_dependency::terror::Error as TerrorError;
    pub use parser_dependency::{CharsetConnection, CollationConnection, ErrParse};
}
/// 完整 parser 依赖再导出。
pub mod parser_core {
    pub use parser_dependency::*;
}
/// 解析器获取/销毁工具。
pub mod parserutil {
    pub use parserutil_dependency::parser::*;
}
/// 生成列表达式解析与列名解析实现。
pub mod generated_expr;
pub use generated_expr::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// AsterSQL 迁移补充回归测试。
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "gen_expr_test.rs"]
/// 对应 Go `TestParseExpression` 的解析用例。
mod gen_expr_test;

#[cfg(test)]
#[path = "main_test.rs"]
pub(crate) mod main_test;
