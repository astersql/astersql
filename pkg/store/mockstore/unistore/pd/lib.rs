// Copyright 2026 AsterSQL.

// unistore 用 PD（Placement Driver）gRPC 客户端 crate 入口。
//
// PD 负责集群元数据、Region 路由与 TSO（全局时间戳）分配。本 crate 提供
// 对接真实/外部 PD 的 gRPC 客户端实现，并在测试配置下挂载迁移单测。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// PD gRPC 客户端实现（对应 Go `client.go`）。
pub mod client;
pub use client::*;

/// Aster 迁移单测：URL 规范化、重试策略与心跳语义等。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
