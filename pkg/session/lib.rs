// Copyright 2026 AsterSQL.

// `session` 包：会话（Session）层 crate 入口。
//
// 定义会话错误类型，并导出引导、事务、DML/FTS/hint 运行时、计划缓存、
// 非事务 DML、升级与 TiDB 兼容辅助等子模块；测试模块仅在 `cfg(test)` 下编译。

#![allow(dead_code)]

use std::fmt;

/// 会话层统一错误：以可读消息描述失败原因。
#[derive(Clone, Debug)]
pub struct SessionError {
    message: String,
    source: Option<astersql_errors::SharedError>,
}

impl SessionError {
    /// 由任意可转为字符串的消息构造会话错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    pub fn with_source(message: impl Into<String>, source: astersql_errors::SharedError) -> Self {
        Self {
            message: message.into(),
            source: Some(source),
        }
    }

    pub fn into_shared(self) -> astersql_errors::SharedError {
        self.source
            .unwrap_or_else(|| astersql_errors::New(self.message))
    }
}

impl PartialEq for SessionError {
    fn eq(&self, other: &Self) -> bool {
        self.message == other.message
    }
}

impl Eq for SessionError {}

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

/// 会话层结果别名：默认成功单元类型，失败为 [`SessionError`]。
pub type SessionResult<T = ()> = Result<T, SessionError>;

pub use astersql_util_metricsutil::GetDBNames;

pub mod advisory_locks;
pub mod bootstrap;
pub mod contextimpl;
pub mod ddl_tables;
pub mod dml_runtime;
pub mod fts_runtime;
pub mod global_init;
pub mod hint_runtime;
pub mod mock_bootstrap;
pub mod nontransactional;
pub mod plan_cache_runtime;
pub mod runtime;
pub mod session;
pub mod sync_upgrade;
pub mod testutil;
pub mod tidb;
pub mod txn;
pub mod txnmanager;
pub mod upgrade_def;
pub mod upgrade_run;

pub use ddl_tables::{
    BackfillTables, DDLJobTables, DDLNotifierTables, InitDDLTables, MDLTables, TableBasicInfo,
};

#[cfg(test)]
mod bench_test;
#[cfg(test)]
mod bootstrap_test;
#[cfg(test)]
#[path = "cached_table_runtime_test.rs"]
mod cached_table_runtime_test;
#[cfg(test)]
#[path = "dml_runtime_test.rs"]
mod dml_runtime_test;
#[cfg(test)]
#[path = "fts_runtime_test.rs"]
mod fts_runtime_test;
#[cfg(test)]
mod global_init_test;
#[cfg(test)]
#[path = "hint_runtime_test.rs"]
mod hint_runtime_test;
#[cfg(test)]
mod load_data_runtime_test;
#[cfg(test)]
mod main_test;
#[cfg(test)]
#[path = "mysql_ddl_compat_test.rs"]
mod mysql_ddl_compat_test;
#[cfg(test)]
#[path = "mysql_dml_compat_test.rs"]
mod mysql_dml_compat_test;
#[cfg(test)]
#[path = "mysql_metadata_compat_test.rs"]
mod mysql_metadata_compat_test;
#[cfg(test)]
#[path = "mysql_performance_schema_runtime_test.rs"]
mod mysql_performance_schema_runtime_test;
#[cfg(test)]
#[path = "mysql_privilege_persistence_test.rs"]
mod mysql_privilege_persistence_test;
#[cfg(test)]
mod txn_test;
#[cfg(test)]
mod txnmanager_test;
#[cfg(test)]
include!("mysql_system_schema_metadata_test.rs");
#[cfg(test)]
mod contextimpl_test;
#[cfg(test)]
#[path = "cost_trace_runtime_test.rs"]
mod cost_trace_runtime_test;
#[cfg(test)]
#[path = "mysql_query_compat_test.rs"]
mod mysql_query_compat_test;
#[cfg(test)]
#[path = "mysql_relational_query_compat_test.rs"]
mod mysql_relational_query_compat_test;
#[cfg(test)]
#[path = "mysql_sys_schema_compat_test.rs"]
mod mysql_sys_schema_compat_test;
#[cfg(test)]
#[path = "mysql_system_catalog_compat_test.rs"]
mod mysql_system_catalog_compat_test;
#[cfg(test)]
#[path = "mysql_transaction_compat_test.rs"]
mod mysql_transaction_compat_test;
#[cfg(test)]
#[path = "mysql_type_compat_test.rs"]
mod mysql_type_compat_test;
#[cfg(test)]
mod nontransactional_test;
#[cfg(test)]
#[path = "plan_cache_runtime_test.rs"]
mod plan_cache_runtime_test;
#[cfg(test)]
#[path = "runtime/admin_test.rs"]
mod runtime_admin_test;
#[cfg(test)]
#[path = "runtime/dispatch_test.rs"]
mod runtime_dispatch_test;
#[cfg(test)]
#[path = "runtime_pessimistic_test.rs"]
mod runtime_pessimistic_test;
#[cfg(test)]
#[path = "runtime/session_test.rs"]
mod runtime_session_test;
#[cfg(test)]
#[path = "runtime/source_test.rs"]
mod runtime_source_test;
#[cfg(test)]
#[path = "runtime/statistics_test.rs"]
mod runtime_statistics_test;
#[cfg(test)]
#[path = "runtime/system_query_test.rs"]
mod runtime_system_query_test;
#[cfg(test)]
#[path = "runtime_test.rs"]
mod runtime_test;
#[cfg(test)]
mod session_nextgen_test;
#[cfg(test)]
mod session_test;
#[cfg(test)]
mod sync_upgrade_test;
#[cfg(test)]
mod tidb_test;
#[cfg(test)]
#[path = "update_point_get_test.rs"]
mod update_point_get_test;
#[cfg(test)]
mod upgrade_backfill_test;
#[cfg(test)]
mod upgrade_def_test;
#[cfg(test)]
mod upgrade_run_test;
#[cfg(test)]
mod upgrade_test;

// Share the executable regression with the lib-test surface required by the plan.
#[cfg(test)]
extern crate self as astersql_session;
#[cfg(test)]
#[path = "tests/system_session.rs"]
mod system_session_alignment_test;
