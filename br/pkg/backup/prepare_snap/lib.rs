// Copyright 2026 AsterSQL.

//! `prepare_snap` crate 入口：对齐 Go 包 `br/pkg/backup/prepare_snap`。
//!
//! 本模块只做子模块装配与对外 re-export，不承载业务算法。
//! 生产路径依赖 `env`（PD/TiKV 抽象）、`errors`（链路错误）与
//! `prepare`（快照准备主流程）；`stream` 提供 TiKV PrepareSnapshot 双向流封装。
//! 测试模块在 `cfg(test)` 下挂载，避免污染正常编译产物。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

/// 环境抽象：PD、RegionCache、StoreManager 与退避策略等边界。
#[path = "env.rs"]
pub mod env;

/// 本地错误类型与 Go 语义对齐的错误判定辅助。
#[path = "errors.rs"]
pub mod errors;

/// 快照准备主流程：`Preparer` / `New` 及驱动逻辑。
#[path = "prepare.rs"]
pub mod prepare;

/// PrepareSnapshot gRPC 双向流的发送/接收封装。
#[path = "stream.rs"]
pub mod stream;

/// 与 Go 行为/常量对照的 parity 测试（仅测试构建）。
#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

/// prepare 流程单元测试（仅测试构建）。
#[cfg(test)]
#[path = "prepare_test.rs"]
mod prepare_test;

/// 环境适配层的聚焦回归测试（仅测试构建）。
#[cfg(test)]
#[path = "env_test.rs"]
mod env_test;

/// Error identity and wrapping parity tests (only in test builds).
#[cfg(test)]
#[path = "errors_test.rs"]
mod errors_test;

// 对外暴露环境抽象与错误/准备 API，调用方无需深入子模块路径。
pub use env::*;
pub use errors::{Error, Result, convertErr, leaseExpired, retryLimitExceeded, unsupported};
pub use prepare::{New, Preparer};
