// Copyright 2026 AsterSQL.

// 外部工作负载控制器的 gRPC 客户端 crate 入口。
//
// 导出 `client` 模块中的控制器客户端类型与构造函数；
// 测试构建下挂载 `client_test` 与迁移对齐单元测试。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 控制器 gRPC 客户端实现与 protobuf 绑定。
pub mod client;
/// 再导出客户端公共 API，便于 `extworkload_client::New` 等调用。
pub use client::*;

extern crate self as extworkload_client;

/// 客户端往返、拦截器与错误映射的集成测试。
#[cfg(test)]
#[path = "client_test.rs"]
mod client_test;

/// 与 Go 行为对齐的迁移回归测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
