// Copyright 2026 AsterSQL.

//! RawKV 备份集成测试包入口：导出 client 与 stubs。
//! 解析命令行标志后跑 rawkv 客户端场景；失败走 log_panic。
//! 非生产路径，仅供测试二进制包装器使用。
//! parity_test 校验与 Go 侧契约对齐。
//! 具体场景逻辑在 `client` 模块。

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
/// 解析 argv（跳过程序名）后执行；错误以 panic 日志收尾。
pub fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flags = client::parse_flags(&args);
    if let Err(err) = client::run_with_flags(&flags) {
        stubs::log_panic("Error", &[("error", err.msg)]);
    }
}
