// Copyright 2026 AsterSQL.

// `intset` crate：快速整数集合 FastIntSet 及其测试入口。
//
// FastIntSet 用于优化器/执行器等场景中的整型集合运算（列号、位图等），
// 小范围用位图、超出阈值后切换到更大表示，接口对齐 Go 版 `util/intset`。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// FastIntSet 实现模块。
pub mod fast_int_set;
/// 对外重导出 FastIntSet 相关 API，保持与 Go 包扁平导出一致。
pub use fast_int_set::*;

#[cfg(test)]
mod fast_int_set_bench_test;
#[cfg(test)]
mod fast_int_set_test;

/// 迁移对照单测：行为与 Go 实现逐项对齐。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
