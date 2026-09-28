// Copyright 2026 AsterSQL.

//! 事务 KV 备份集成测试包入口：导出 client 与 stubs。
//! 与 br_rawkv 结构对称，但场景面向事务键空间。
//! `main` 解析标志并运行；失败 log_panic，对齐 Go 测试二进制。
//! 非生产路径；parity_test 对照 Go 契约。
//! 业务细节在 `client` 模块。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

#[path = "stubs.rs"]
pub mod stubs;

#[path = "client.rs"]
pub mod client;

pub use client::*;
pub use stubs::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "client_test.rs"]
mod client_test;

/// Shared process entrypoint called by the binary wrapper.
/// 跳过程序名解析标志后执行 txn 场景。
pub fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flags = client::parse_flags(&args);
    if let Err(err) = client::run_with_flags(&flags) {
        stubs::log_panic("Error", &[("error", err.msg)]);
    }
}
