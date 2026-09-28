// Copyright 2026 AsterSQL.

// `util/context` crate 入口。
//
// 聚合上下文 ID、SQL warning 处理与计划缓存（Plan Cache）跟踪相关模块，
// 并重导出错误与 parser terror 依赖。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 本 crate 自引用别名，供测试模块引用。
extern crate self as util_context;

/// AsterSQL 错误类型。
pub use astersql_errors as errors;
/// parser terror 实现（MySQL 风格错误码/文案）。
pub use astersql_parser_terror as terror_impl;

/// 嵌套导出 mysql / terror，对齐 Go 侧 `parser` 包路径习惯。
pub mod parser {
    pub use astersql_parser_mysql as mysql;

    pub mod terror {
        pub use astersql_parser_terror::*;
    }
}

/// 键值上下文与 `GenContextID`。
pub mod context;
/// SQL warning 追加、复制与 SHOW WARNINGS 级别常量。
pub mod warn;
/// 再导出 warn 模块公开符号。
pub use warn::*;
/// 计划缓存启用/跳过跟踪与 range fallback 告警。
pub mod plancache;

/// AsterSQL 迁移补充回归测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
/// 对应 Go `warn_test.go` 的 warning 单元测试。
#[cfg(test)]
#[path = "warn_test.rs"]
mod warn_test;
