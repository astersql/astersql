// Copyright 2026 AsterSQL.

// UCA allkeys 表生成器 crate 入口。
//
// 对应 Go `ucadata/generator`：解析 Unicode Collation Algorithm 的 allkeys 文本，
// 生成 MapTable4 / LongRuneMap 等权重表源码；`magic` 提供十六进制查表等辅助常量。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

// 生成器主逻辑：解析 CET、隐式权重、模板渲染。
#[path = "main.rs"]
pub mod generator;
/// 长权重哨兵与 reverseHexTable 等查表常量。
pub mod magic;
pub use generator::*;
pub use magic::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
