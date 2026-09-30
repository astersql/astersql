// Copyright 2026 AsterSQL.

// 指标（metrics）注册工具 crate 入口。
//
// 封装各子系统 Prometheus 指标初始化与 Keyspace 常量标签设置，
// 对外重导出 `common` 模块中的注册 API。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

mod common;
pub use common::*;
mod db_labels;
pub use db_labels::GetDBNames;

#[cfg(test)]
#[path = "common_test.rs"]
mod common_test;

#[cfg(test)]
#[path = "go_merge_30_test.rs"]
mod go_merge_30_test;
