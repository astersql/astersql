// Copyright 2026 AsterSQL.

// 会话 API（sessionapi）门面 crate。
//
// 聚合解析 AST、鉴权身份、连接、会话扩展、结果字段解析、会话管理器、
// 执行上下文、会话状态、结果集与事务信息等依赖的 re-export，
// 并对外暴露 `Session` trait（见 `session` 子模块）。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as sessionapi;

/// SQL 语句 AST 节点类型 re-export。
pub mod ast {
    pub use ast_crate::ast::StmtNode;
}
/// 用户身份（UserIdentity）相关类型 re-export。
pub mod auth {
    pub use auth_crate::parser::auth::auth::UserIdentity;
}
/// 鉴权连接（AuthConn）接口 re-export。
pub mod conn {
    pub use conn_crate::AuthConn;
}
/// 会话扩展点（SessionExtensions）re-export。
pub mod extension {
    pub use extension_crate::SessionExtensions;
}
/// 结果列元数据（ResultField）re-export。
pub mod resolve {
    pub use resolve_crate::ResultField;
}
/// 会话管理器（Manager）re-export。
pub mod sessmgr {
    pub use sessmgr_crate::Manager;
}
/// 会话/执行上下文与错误类型 re-export。
pub mod sessionctx {
    pub use sessionctx_crate::{Context, ExecutionContext, GoError, SessionStatesHandler};
}
/// 会话状态类型枚举 re-export。
pub mod sessionstates {
    pub use sessionstates_crate::SessionStateType;
}
/// SQL 执行结果集（RecordSet）re-export。
pub mod sqlexec {
    pub use sqlexec_crate::RecordSet;
}
/// 事务摘要信息（TxnInfo）re-export。
pub mod txninfo {
    pub use txninfo_crate::txn_info::TxnInfo;
}

/// 会话 trait 与相关类型定义。
mod session;
pub use session::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "session_test.rs"]
mod session_test;
