// Copyright 2026 AsterSQL.

// LDAP 认证子 crate：Simple Bind 与 SASL 挑战循环实现。
//
// 提供连接池、DN 规范化/搜索、TLS/StartTLS，以及 SCRAM/GSSAPI 等
// SASL 方法的协议适配入口。`migration_aster_unit_test` /
// `ldap_common_test` 在测试配置下挂载。

#![allow(non_snake_case)]

extern crate self as astersql_privilege_ldap;

/// 再导出 `native_tls`，供测试构造 TLS acceptor。
pub use native_tls;

/// SASL 认证方法名常量。
#[path = "const.rs"]
pub mod constants;
/// LDAP 连接池、配置与用户搜索公共实现。
pub mod ldap_common;
/// LDAP SASL 多轮认证实现。
pub mod sasl;
/// LDAP Simple Bind 密码认证实现。
pub mod simple;

#[cfg(test)]
mod simple_test;

/// 迁移对照用单元测试（与 Go 行为对齐）。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

/// LDAP 公共连接/TLS 行为测试。
#[cfg(test)]
#[path = "ldap_common_test.rs"]
mod ldap_common_test;
