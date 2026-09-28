// Copyright 2026 AsterSQL.

// server/internal/util crate 根模块。
//
// 聚合缓冲读连接与 MySQL 协议工具（长度编码、字符集解码、CORS/测试配置），
// 并在测试构建下挂载迁移对照单测与 util 包测试。

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

extern crate self as astersql_server_internal_util;

/// 再导出配置 crate，供本包内 CORS/测试配置构造使用。
pub use config_crate as config;
pub use config_crate::*;

/// 带缓冲的 TCP 读连接（Peek / IsAlive）。
mod buffered_read_conn;
pub use buffered_read_conn::*;
#[cfg(test)]
#[path = "buffered_read_conn_test.rs"]
mod buffered_read_conn_test;
/// MySQL 长度编码、输入解码、CORS 与测试配置工具。
mod util;
pub use util::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
#[cfg(test)]
#[path = "util_test.rs"]
mod util_test;
