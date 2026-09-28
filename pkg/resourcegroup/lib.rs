// Copyright 2026 AsterSQL.

// 资源组（Resource Group）crate 入口。
//
// 导出 runaway 检查与消费上报相关类型，供会话/执行层在查询生命周期中
// 做配额判定与 RU（请求单元）计量。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

pub mod checker;
pub use checker::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
