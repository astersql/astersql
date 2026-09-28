// Copyright 2026 AsterSQL.

// `pkg/extension`：TiDB 扩展（extension）框架的 Rust 映射入口。
//
// 扩展可在运行时注册自定义系统变量、动态权限、UDF、认证插件、
// 会话事件处理器与 bootstrap SQL。本文件负责：
// - 声明依赖类型的再导出子模块（auth、mysql、types、chunk 等）
// - 串联 util / auth / function / session / manifest / extensions / registry
// - 在测试配置下挂载各单元测试模块
//
// 无业务运行时逻辑，仅做模块组装与符号再导出。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

extern crate self as astersql_extension;

/// etcd 客户端类型，供扩展实现 crate 在不重复声明版本的情况下使用。
pub use etcd_client;

/// 外部扩展注册落地模块（对应 Go 的空 import 包）。
#[path = "_import/import.rs"]
pub mod extension_import;

/// 用户/角色身份类型再导出。
pub mod auth_identity {
    pub use parser_auth::parser::auth::auth::{RoleIdentity, UserIdentity};
}
/// MySQL 协议常量与权限类型再导出。
pub mod mysql {
    pub use parser_mysql::r#const::DefaultAuthPlugins;
    pub use parser_mysql::privs::PrivilegeType;
}
/// 鉴权连接类型再导出。
pub mod privilege_conn {
    pub use privilege_conn_dependency::conn::AuthConn as RawAuthConn;
}
/// 表达式求值类型与 Datum 再导出。
pub mod types {
    pub use types_dependency::datum::Datum;
    pub use types_dependency::field::EvalType;
}
/// 列式执行引擎中的行（chunk::Row）再导出。
pub mod chunk {
    pub use chunk_dependency::Row;
}
/// 系统变量注册/查询 API 再导出。
pub mod variable {
    pub use variable_dependency::{
        ConnectionInfo, GetSysVar, RegisterSysVar, SysVar, UnregisterSysVar,
    };
}
/// SQL 摘要（digest）类型再导出。
pub mod parser {
    pub use parser_root::digester_impl::Digest;
}
/// AST 节点再导出。
pub mod ast {
    pub use parser_ast::*;
}
/// 语句上下文中的表条目再导出。
pub mod stmtctx {
    pub use stmtctx_dependency::TableEntry;
}

pub mod util;
pub use util::*;
pub mod auth;
pub use auth::*;
pub mod function;
pub use function::*;
pub mod session;
pub use session::*;
pub mod manifest;
pub use manifest::*;
pub mod extensions;
pub use extensions::*;
pub mod registry;
pub use registry::*;

#[cfg(test)]
#[path = "auth_1_aster_unit_test.rs"]
mod auth_1_aster_unit_test;

#[cfg(test)]
mod auth_test;
#[cfg(test)]
mod bootstrap_test;
#[cfg(test)]
mod event_listener_test;
#[cfg(test)]
mod function_test;
#[cfg(test)]
mod main_test;
#[cfg(test)]
mod manifest_test;
#[cfg(test)]
mod registry_test;
