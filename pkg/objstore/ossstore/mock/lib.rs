// Copyright 2026 AsterSQL.

// OSS mock 子 crate 入口：对外导出 API 与凭证提供者的测试替身。
//
// 对应 Go `ossstore/mock`，用 mockall 生成可期望调用的 mock，
// 便于在不访问真实阿里云的情况下测试 Client/Store。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// Go OSS SDK 风格 API 的 mock 定义与请求/结果类型。
pub mod api_mock;
/// 凭证提供者 mock。
pub mod provider_mock;
pub use api_mock::*;
pub use provider_mock::*;

#[cfg(test)]
mod api_mock_test;

#[cfg(test)]
mod provider_mock_test;

/// mock 与 Go 方法集对齐的迁移单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
