// Copyright 2026 AsterSQL.

// 权限（privilege）crate 根模块：聚合依赖门面并再导出权限管理 API。
//
// 本模块通过 `extern crate self` 与子模块再导出，对齐 Go `pkg/privilege` 的包边界，
// 向上层提供 Manager 绑定、会话上下文与权限校验相关类型。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 自引用别名，供测试与子模块以统一 crate 名引用本包。
extern crate self as astersql_privilege;

/// 解析器相关再导出：认证 AST 与 MySQL 权限类型。
pub mod parser {
    /// 认证相关解析产物。
    pub mod auth {
        pub use auth_crate::parser::auth::auth::*;
    }

    /// MySQL 权限类型枚举（PrivilegeType）。
    pub mod mysql {
        pub use mysql_crate::privs::PrivilegeType;
    }
}

/// 会话执行上下文再导出。
pub mod sessionctx {
    pub use sessionctx_crate::ExecutionContext;

    /// 会话变量（SessionVars）再导出。
    pub mod variable {
        pub use variable_crate::session::SessionVars;
    }
}

/// Datum 等基础数据类型再导出。
pub mod types {
    pub use types_crate::datum::Datum;
}

/// SQL 受限执行器等工具再导出。
pub mod util {
    /// 受限 SQL 执行接口（RestrictedSQLExecutor）与 Go 风格错误。
    pub mod sqlexec {
        pub use sqlexec_crate::{GoError, RestrictedSQLExecutor};
    }
}

#[path = "privilege.rs"]
mod privilege_api;

/// 对外可见的 privilege 命名空间：含认证连接与权限 API。
pub mod privilege {
    /// 认证插件使用的连接抽象。
    pub mod conn {
        pub use conn_crate::conn::AuthConn;
    }

    pub use crate::privilege_api::*;
}

pub use privilege_api::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
