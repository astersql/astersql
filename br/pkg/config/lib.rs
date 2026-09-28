// Copyright 2026 AsterSQL.

//! BR 配置 crate 入口：汇总 EBS 元数据与 TiKV 配置解析，对外 re-export。
//! 对照 Go `br/pkg/config` 包；测试模块仅在 `cfg(test)` 下挂载。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

/// EBS 全备元数据与校验（`ebs.go`）。
#[path = "ebs.rs"]
pub mod ebs;

/// TiKV 配置 JSON 字段解析（`kv.go`）。
#[path = "kv.rs"]
pub mod kv;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "ebs_test.rs"]
mod ebs_test;

// 公开 API 扁平导出，调用方可不经子模块路径使用类型与函数。
pub use ebs::*;
pub use kv::*;
