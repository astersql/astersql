// Copyright 2026 AsterSQL.

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

// 静态表达式上下文（exprstatic）crate 入口。
//
// 提供可脱离会话生命周期独立持有的 `EvalContext` / `ExprContext`，
// 便于计划缓存、常量折叠等场景在无会话绑定时求值。
// 本文件再导出依赖包符号，并挂接 `evalctx` / `exprctx` 实现与单元测试。

extern crate self as exprstatic;
extern crate self as exprstatic_crate;

/// 字符集元数据（charset）再导出。
pub mod charset {
    pub use charset_crate::charset::*;
}
/// 上下文工具（告警、计划缓存跟踪等）再导出。
pub mod contextutil {
    pub use contextutil_crate::*;
}
/// 错误级别上下文（errctx）再导出。
pub mod errctx {
    pub use errctx_crate::errctx::*;
}
/// 表达式上下文 trait / 可选属性定义再导出。
pub mod exprctx {
    pub use exprctx_crate::*;
}
/// 可选求值属性 Provider/Reader 再导出。
pub mod expropt {
    pub use expropt_crate::*;
}
/// 数学与随机数工具再导出。
pub mod mathutil {
    pub use mathutil_crate::*;
}
/// MySQL 字符集与常量再导出。
pub mod mysql {
    pub use mysql_crate::charset::*;
    pub use mysql_crate::r#const::*;
}
/// Datum / FieldType / 标量类型再导出。
pub mod types {
    pub use types_crate::datum::Datum;
    pub use types_crate::field::FieldType;
    pub use types_crate::scalar::*;
}
/// 系统变量名与默认值（vardef）再导出。
pub mod vardef {
    pub use vardef_crate::*;
}
/// 会话变量相关符号再导出。
pub mod variable {
    pub use variable_crate::*;
}

/// 静态求值上下文实现。
mod evalctx;
pub use evalctx::*;
/// 静态表达式构建上下文实现（与 `exprctx` 再导出模块名区分）。
#[path = "exprctx.rs"]
mod exprctx_impl;
pub use exprctx_impl::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "evalctx_test.rs"]
mod evalctx_test;

#[cfg(test)]
#[path = "exprctx_test.rs"]
mod exprctx_test;
