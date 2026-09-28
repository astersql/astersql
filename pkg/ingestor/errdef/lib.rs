// Copyright 2026 AsterSQL.

// `ingestor::errdef` crate 入口：声明并再导出 ingest 错误定义模块。
//
// 测试模块 `migration_aster_unit_test` 校验错误消息、RFC code 与磁盘满检测链。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 错误常量与辅助类型定义。
pub mod errors;
pub use errors::*;

#[cfg(test)]
#[path = "errors_test.rs"]
mod errors_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
