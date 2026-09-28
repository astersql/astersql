// Copyright 2026 AsterSQL.
// 权限子系统的认证连接（AuthConn）crate 入口。
//
// 再导出 `conn` 模块中的 `AuthConn` trait，供认证插件与客户端交换协议包。
// 测试子模块通过 path 属性挂载迁移对照用例。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 认证连接接口定义所在子模块。
pub mod conn;
pub use conn::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
