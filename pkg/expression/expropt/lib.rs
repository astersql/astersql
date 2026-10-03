// Copyright 2026 AsterSQL.

// `expropt`：表达式可选求值属性（Optional Eval Prop）的注册表与各属性 Provider/Reader。
//
// 可选属性把会话用户、InfoSchema、KV Store、权限、序列等依赖从核心 `EvalContext`
// 中解耦；表达式只声明所需键，运行时从上下文按键取 Provider。本 crate 聚合各子模块
// 并再导出外部依赖别名，便于表达式与会话层统一引用。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

pub use exprctx_crate as exprctx;
pub use infoschema_crate as infoschema;
pub use intest_crate as intest;

/// 认证相关类型再导出（用户/角色身份）。
pub mod auth {
    pub use auth_crate::parser::auth::auth::{RoleIdentity, UserIdentity};
}

/// MySQL 权限类型再导出。
pub mod mysql {
    pub use mysql_crate::privs::PrivilegeType;
}

/// 会话变量与全局变量访问相关类型再导出。
pub mod variable {
    pub use variable_crate::session::SessionVars;
    pub use variable_crate::{Context, GlobalVarAccessor, VariableError};
}

// Shared optional-property registry and reader contract.
// 共享的可选属性注册表与 Reader 契约。
mod optional;
pub use optional::*;

mod advisory_lock;
pub use advisory_lock::*;
mod current_user;
pub use current_user::*;
mod ddlowner;
pub use ddlowner::*;
#[path = "infoschema.rs"]
mod infoschema_provider;
pub use infoschema_provider::*;
mod kvstore;
pub use kvstore::*;
#[path = "priv.rs"]
mod priv_provider;
pub use priv_provider::*;
mod sequence;
pub use sequence::*;
mod sessionvars;
pub use sessionvars::*;
#[path = "sqlexec.rs"]
mod sqlexec_provider;
pub use sqlexec_provider::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "optional_test.rs"]
mod optional_test;

#[cfg(test)]
#[path = "sessionvars_test.rs"]
mod sessionvars_test;

mod sessioncontext;
pub use sessioncontext::*;

pub use inference;
