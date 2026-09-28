// Copyright 2026 AsterSQL.

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

// sessionexpr：会话边界上的表达式构建与求值上下文。
//
// 将 SessionContext 适配为 ExprContext / EvalContext，供表达式包访问会话变量、
// InfoSchema、权限与语句时间等，而不把完整会话子系统引入 expression 叶包。

extern crate self as sessionexpr;

pub use contextutil_crate as contextutil;
pub use exprctx_crate as exprctx;
pub use expropt_crate as expropt;
pub use exprstatic_crate as exprstatic;
pub use infoschema_crate as infoschema;
pub use mathutil_crate as mathutil;
pub use privilege_crate as privilege;
pub use vardef_crate as vardef;
pub use variable_crate as variable;

/// 再导出 auth 身份类型。
pub mod auth {
    pub use auth_crate::parser::auth::auth::{RoleIdentity, UserIdentity};
}

/// 再导出错误级别上下文。
pub mod errctx {
    pub use errctx_crate::errctx::*;
}

/// 再导出 MySQL 常量与权限类型。
pub mod mysql {
    pub use mysql_crate::r#const::*;
    pub use mysql_crate::privs::PrivilegeType;
}

/// 再导出 Datum / FieldType / Scalar 类型。
pub mod types {
    pub use types_crate::datum::*;
    pub use types_crate::field::FieldType;
    pub use types_crate::scalar::*;
}

/// 会话表达式上下文实现。
mod sessionctx;
pub use sessionctx::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "sessionctx_test.rs"]
mod sessionctx_test;
