// Copyright 2026 AsterSQL.

// server crate 根模块：MySQL 协议服务端入口。
//
// 聚合连接处理、驱动、HTTP 状态/管理接口、RPC、备用切换与连接统计等子模块；
// 测试构建下再挂载各 *_test 与 TiDB 兼容回归。

#![allow(dead_code)]

/// 单连接协议状态机与读写循环。
pub mod conn;
/// 预处理语句（COM_STMT_*）相关逻辑。
pub mod conn_stmt;
/// 预处理语句参数绑定与类型转换。
pub mod conn_stmt_params;
/// 存储引擎/会话驱动抽象。
pub mod driver;
/// TiDB 兼容驱动实现。
pub mod driver_tidb;
/// 服务端扩展点。
pub mod extension;
/// 从数据包中抽取字段/结果集辅助。
pub mod extract;
mod extract_runtime;
/// HTTP 管理/诊断 handler。
pub mod http_handler;
/// HTTP status 端口与健康检查。
pub mod http_status;
/// 测试用 mock 连接。
pub mod mock_conn;
pub mod pg_conn;
#[cfg(test)]
#[path = "pg_conn_test.rs"]
mod pg_conn_test;
/// PostgreSQL 3.2 startup framing.
pub mod pg_protocol;
pub mod pg_result;
/// gRPC / RPC 服务端。
pub mod rpc_server;
/// 真实 TCP PacketIO 与 canonical ConcreteSession 生产适配。
pub mod runtime;
/// 监听、接受连接与生命周期管理。
pub mod server;
/// 主备（standby）相关逻辑。
pub mod standby;
/// 连接/查询统计。
pub mod stat;
/// 按用户维度的连接数限制与计数。
pub mod user_connections;

/// 内部子包再导出（如握手/命令包解析）。
pub mod internal {
    /// 握手与命令包解析。
    pub mod parse {
        pub use astersql_server_internal_parse::*;
    }
}

#[cfg(test)]
#[path = "conn_stmt_params_test.rs"]
mod conn_stmt_params_test;
#[cfg(test)]
#[path = "conn_stmt_test.rs"]
mod conn_stmt_test;
#[cfg(test)]
#[path = "conn_test.rs"]
mod conn_test;
#[cfg(test)]
#[path = "driver_test.rs"]
mod driver_test;
#[cfg(test)]
#[path = "driver_tidb_test.rs"]
mod driver_tidb_test;
#[cfg(test)]
#[path = "extension_test.rs"]
mod extension_test;
#[cfg(test)]
#[path = "extract_test.rs"]
mod extract_test;
#[cfg(test)]
#[path = "http_status_test.rs"]
mod http_status_test;
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "mock_conn_test.rs"]
mod mock_conn_test;
#[cfg(test)]
#[path = "mysql_catalog_protocol_test.rs"]
mod mysql_catalog_protocol_test;
#[cfg(test)]
#[path = "mysql_compat_manifest_test.rs"]
mod mysql_compat_manifest_test;
#[cfg(test)]
#[path = "mysql_compat_test_support.rs"]
mod mysql_compat_test_support;
#[cfg(test)]
#[path = "mysql_error_protocol_test.rs"]
mod mysql_error_protocol_test;
#[cfg(test)]
#[path = "mysql_metadata_protocol_test.rs"]
mod mysql_metadata_protocol_test;
#[cfg(test)]
#[path = "mysql_prepared_protocol_test.rs"]
mod mysql_prepared_protocol_test;
#[cfg(test)]
#[path = "mysql_protocol_compat_test.rs"]
mod mysql_protocol_compat_test;
#[cfg(test)]
#[path = "mysql_protocol_stress_test.rs"]
mod mysql_protocol_stress_test;
#[cfg(test)]
#[path = "mysql_type_protocol_test.rs"]
mod mysql_type_protocol_test;
#[cfg(test)]
#[path = "rpc_server_test.rs"]
mod rpc_server_test;
#[cfg(test)]
#[path = "runtime_test.rs"]
mod runtime_test;
#[cfg(test)]
#[path = "server_test.rs"]
mod server_test;
#[cfg(test)]
#[path = "standby_test.rs"]
mod standby_test;
#[cfg(test)]
#[path = "stat_test.rs"]
mod stat_test;
#[cfg(test)]
#[path = "tidb_library_test.rs"]
mod tidb_library_test;
#[cfg(test)]
#[path = "tidb_test.rs"]
mod tidb_test;
#[cfg(test)]
#[path = "user_connections_test.rs"]
mod user_connections_test;

#[cfg(test)]
#[path = "pg_protocol_test.rs"]
mod pg_protocol_test;

#[cfg(test)]
#[path = "pg_query_test.rs"]
mod pg_query_test;

#[cfg(test)]
#[path = "pg_types_test.rs"]
mod pg_types_test;

#[cfg(test)]
#[path = "pg_error_test.rs"]
mod pg_error_test;

#[cfg(test)]
#[path = "pg_extended_test.rs"]
mod pg_extended_test;

mod pg_extended;

#[cfg(test)]
#[path = "pg_client_integration_test.rs"]
mod pg_client_integration_test;
