// Copyright 2026 AsterSQL.

// 表达式求值上下文（exprctx）crate 入口。
//
// 对应 Go `pkg/expression/exprctx`：为表达式构建与求值提供会话侧上下文，
// 包括参数绑定、可选求值属性（如当前用户、InfoSchema、咨询锁）以及
// 类型/错误处理上下文。本文件负责依赖重导出与子模块装配。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as exprctx;
extern crate self as exprctx_crate;

/// 错误处理上下文（截断、除零等错误级别）的重导出别名。
pub mod errctx {
    pub use errctx_crate::errctx::*;
}
/// MySQL 兼容随机数等数学工具的重导出。
pub mod mathutil {
    pub use mathutil_crate::*;
}
/// SQL Mode 等 MySQL 协议常量。
pub mod mysql {
    pub use parser_mysql::r#const::SQLMode;
}
/// Datum / FieldType / 标量类型上下文的重导出。
pub mod types {
    pub use types_crate::datum::Datum;
    pub use types_crate::field::FieldType;
    pub use types_crate::scalar::{Context, DefaultStmtFlags, Flags};
}
/// 会话变量与用户变量的重导出。
pub mod variable {
    pub use session_variable::SessionVars;
    pub use session_variable::session::UserVars;
}

/// 预处理语句参数值访问。
mod param;
pub use param::*;
/// 可选求值属性键与位图集合。
mod optional;
pub use optional::*;
/// BuildContext / EvalContext 等核心上下文接口与包装器。
mod context;
pub use context::*;

#[cfg(test)]
#[path = "context_override_test.rs"]
mod context_override_test;
#[cfg(test)]
#[path = "context_test.rs"]
mod context_test;
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
#[cfg(test)]
#[path = "optional_test.rs"]
mod optional_test;
