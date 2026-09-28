// Copyright 2026 AsterSQL.

// `util/zeropool` crate 入口：零分配倾向的类型安全对象池。
//
// 对应 Go `pkg/util/zeropool`。再导出 `pool` 模块中的 `Pool`/`New`，
// 并挂载迁移补充与原版行为对齐的单元测试。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 对象池实现。
pub mod pool;
pub use pool::*;

/// AsterSQL 迁移补充的 zeropool 单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

/// 与 Go 原版 TestPool / Benchmark 对应的测试。
#[cfg(test)]
#[path = "pool_test.rs"]
mod pool_test;
