// Copyright 2026 AsterSQL.

//! RawKV 恢复客户端包入口：导出 `rawkv_client` 实现。
//! 对应 Go `br/pkg/restore/internal/rawkv`，面向非事务 RawKV 路径的
//! 批量写入与扫描；与 TiDB 事务 KV 恢复通道分离。
//! 测试经 `#[path]` 挂载，不与实现混编。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

#[path = "rawkv_client.rs"]
pub mod rawkv_client;

// 扁平再导出，调用方直接使用 RawKV 客户端类型与构造函数。
pub use rawkv_client::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "rawkv_client_test.rs"]
mod rawkv_client_test;
