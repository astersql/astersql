// Copyright 2026 AsterSQL.

// sqlexec crate 根：受限 SQL 执行、结果集与会话侧执行边界抽象。
//
// 对应 Go `pkg/util/sqlexec`。对外再导出 chunk/AST/resolve 等依赖，
// 并挂载 `RestrictedSQLExecutor`、`SimpleRecordSet` 等核心类型。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 列式结果块（Chunk）相关类型。
pub use chunk_crate as chunk;
/// SQL AST 节点类型。
pub use parser_ast as ast;
/// 名称解析与结果字段类型。
pub use resolve_crate as resolve;
/// 系统过程跟踪（TrackSysProc）相关类型。
pub use sysproctrack_crate as sysproctrack;

/// 取消/超时上下文；映射 Go `context.Context`。
pub mod context {
    pub use kv_crate::Context;
}
/// SQL 解析参数。
pub mod parser {
    pub use parser_crate::ParseParam;
}
/// terror 错误调用约定（对应 Go terror.Call）。
pub mod terror {
    pub use parser_terror::*;
}
/// 会话变量（SessionVars）。
pub mod variable {
    pub use variable_crate::session::SessionVars;
}
/// 后台日志工具。
pub mod logutil {
    pub use logutil_crate::log::*;
}
/// Datum / FieldType 等类型系统原语。
pub mod types {
    pub use types_crate::datum::*;
}

// 受限 SQL 执行器、RecordSet 与 Drain/ExecSQL 辅助函数。
mod restricted_sql_executor;
pub use restricted_sql_executor::*;
// 内存中已构造完整内容的简单结果集。
mod simple_record_set;
pub use simple_record_set::*;
#[cfg(test)]
mod go_merge_34_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
