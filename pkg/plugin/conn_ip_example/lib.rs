// Copyright 2026 AsterSQL.

// `conn_ip_example` crate 入口：按连接 IP 做审计示例的插件库。
//
// 对外重导出 `conn_ip_example` 模块中的清单、回调与连接计数等符号；
// 测试文件通过 `#[path]` 挂到本 crate，便于与 Go 包测试布局对齐。

#![allow(dead_code)]

/// 连接 IP 审计示例插件实现模块。
pub mod conn_ip_example;

pub use conn_ip_example::*;

#[cfg(test)]
#[path = "conn_ip_example_test.rs"]
mod conn_ip_example_test;
