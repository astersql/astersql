// Copyright 2026 AsterSQL.
// TLS 工具 crate 入口：导出版本/密码套件命名与安全传输开关。
//
// 对应 Go `util/tls`；测试通过 path 属性挂载 migration 与 tls_test。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// TLS 版本与 cipher suite 名称映射实现模块。
pub mod tls;
/// 再导出 tls 模块公共 API。
pub use tls::*;

#[cfg(test)]
// Aster 迁移一致性单测。
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
// 对应 Go TestVersionName 等用例。
#[path = "tls_test.rs"]
mod tls_test;
