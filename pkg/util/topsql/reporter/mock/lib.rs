// Copyright 2026 AsterSQL.

// TopSQL reporter 测试用 mock crate：proto 生成类型与假 pubsub/server。
//
// `tipb` 模块由 tonic include 构建期生成；`pubsub`/`server` 提供单测替身，
// 避免依赖真实 agent 或网络。

extern crate self as topsql_mock;

/// 构建期生成的 tipb protobuf / gRPC 类型。
pub mod tipb {
    tonic::include_proto!("tipb");
}

pub mod pubsub;
pub mod server;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
mod pubsub_test;
