// Copyright 2026 AsterSQL.

// rangerctx crate 根：为 Range 构造提供可分离的会话上下文。
//
// Range（索引/表扫描区间）构造需要类型上下文、错误级别、表达式构建上下文，
// 以及计划缓存/Range 回退相关句柄；本 crate 聚合这些依赖并导出 `RangerContext`。

#![allow(dead_code, non_snake_case)]

/// 计划缓存跟踪与静态告警追加器，供 Range 回退时跳过缓存。
pub mod contextutil {
    pub use contextutil_crate::plancache::*;
    pub use contextutil_crate::warn::*;
}
/// 错误级别上下文（ErrCtx），控制截断/溢出等告警是否升级为错误。
pub mod errctx {
    pub use errctx_crate::errctx::*;
}
/// 表达式求值与构建上下文（EvalContext / BuildContext）。
pub mod exprctx {
    pub use exprctx_crate::*;
}
/// 可序列化的静态表达式上下文，用于 Detach 后的无会话绑定副本。
pub mod exprstatic {
    pub use exprstatic_crate::*;
}
/// 标量类型上下文，提供默认无告警语句上下文。
pub mod types {
    pub use types_crate::scalar::{Context, DefaultStmtNoWarningContext};
}

mod context;
/// 导出 `RangerContext` 及其 Detach 等 API。
pub use context::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "context_test.rs"]
mod context_test;
