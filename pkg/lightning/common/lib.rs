// Copyright 2026 AsterSQL.

// Lightning `common` crate 入口。
//
// 聚合导入公共能力：自增分配、连接池、重复键检测、错误归一化、键适配器、
// 重试、安全、存储路径与通用工具，并再导出供其它 lightning 子模块使用。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unused_variables
)]
/// 表级自增 / RowID / AUTO_RANDOM 分配器。
mod common;
/// gRPC / 客户端连接池。
mod conn;
/// 重复键检测与写批。
mod dupdetect;
/// 统一错误类型与归一化。
mod errors;
/// 重复检测用键编解码适配器。
mod key_adapter;
/// 一次性错误（OnceError）封装。
mod once_error;
/// 暂停 / 恢复控制。
mod pause;
/// 可重试错误判定与退避。
mod retry;
/// TLS / 安全相关配置辅助。
mod security;
/// 外部存储路径抽象。
mod storage;
/// Unix 平台存储实现细节。
#[cfg(unix)]
mod storage_unix;
/// Windows 平台存储实现细节。
#[cfg(any(windows, test))]
mod storage_windows;
/// 通用工具函数（上下文、RowID 编码等）。
mod util;
pub use common::*;
pub use conn::*;
pub use dupdetect::*;
pub use errors::*;
pub use key_adapter::*;
pub use once_error::*;
pub use pause::*;
pub use retry::*;
pub use security::*;
pub use storage::*;
pub use util::*;

#[cfg(test)]
#[path = "common_test.rs"]
mod common_test;
#[cfg(test)]
#[path = "conn_test.rs"]
mod conn_test;
#[cfg(test)]
#[path = "dupdetect_test.rs"]
mod dupdetect_test;
#[cfg(test)]
#[path = "errors_test.rs"]
mod errors_test;
#[cfg(test)]
#[path = "key_adapter_test.rs"]
mod key_adapter_test;
#[cfg(test)]
#[path = "once_error_test.rs"]
mod once_error_test;
#[cfg(test)]
#[path = "pause_test.rs"]
mod pause_test;
#[cfg(test)]
#[path = "retry_test.rs"]
mod retry_test;
#[cfg(test)]
#[path = "security_test.rs"]
mod security_test;
#[cfg(test)]
#[path = "storage_test.rs"]
mod storage_test;
#[cfg(test)]
#[path = "storage_unix_test.rs"]
mod storage_unix_test;
#[cfg(test)]
#[path = "storage_windows_test.rs"]
mod storage_windows_test;
#[cfg(test)]
#[path = "util_test.rs"]
mod util_test;
