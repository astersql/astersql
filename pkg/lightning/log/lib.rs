// Copyright 2026 AsterSQL.

// Lightning 日志 crate 入口。
//
// 聚合 `filter`（级别/字段/过滤 Core）、`log`（Logger 与初始化）与测试用
// `testlogger`，并对外再导出 filter/log 的公共 API。

#![allow(non_snake_case, non_upper_case_globals)]

extern crate self as astersql_lightning_log;

/// 日志级别、字段、JSON 编码与按包路径过滤的 Core。
pub mod filter;
/// Logger、配置初始化、任务起止日志与取消错误判定。
pub mod log;
/// 测试用内存 Logger 与缓冲。
pub mod testlogger;

pub use filter::*;
pub use log::*;

#[cfg(test)]
#[path = "filter_test.rs"]
mod filter_test;
#[cfg(test)]
#[path = "log_test.rs"]
mod log_test;
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
