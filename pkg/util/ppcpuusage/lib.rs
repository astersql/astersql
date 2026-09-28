// Copyright 2026 AsterSQL.

// `util/ppcpuusage` crate 入口：按 SQL 汇总 TiDB/TiKV CPU 用量。
//
// 对应 Go `pkg/util/ppcpuusage`。对外再导出 `cpuusages` 中的类型与方法。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// CPU 用量结构体与 SQL 级并发合并实现。
pub mod cpuusages;
/// 再导出 CPUUsages / SQLCPUUsages 等公共 API。
pub use cpuusages::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// AsterSQL 迁移补充的 ppcpuusage 单元测试。
mod migration_aster_unit_test;
