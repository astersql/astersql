// Copyright 2026 AsterSQL.

//! Crate entry for `dumpling/cli` (Go package `github.com/pingcap/tidb/dumpling/cli`).
//!
//! 本 crate 是 dumpling CLI 的库入口，对应 Go 包 `dumpling/cli`。
//! 版本元数据与展示逻辑集中在 `versions` 子模块；此处只负责挂载模块并再导出公开符号，
//! 使调用方无需关心子文件拆分，即可使用与 Go 同名的版本查询与日志 API。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

// 显式路径挂载，与 Go 同包内的 `versions.go` 语义对齐，避免目录重构时隐式模块解析漂移。
#[path = "versions.rs"]
mod versions;

// 再导出全部版本相关公开 API，保持与 Go 包级可见性一致。
pub use versions::*;

// 仅在测试配置下挂载与 Go 行为对照的 parity 测试，避免进入正式产物。
#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;
