// Copyright 2026 AsterSQL.

// `util/mvmap` crate 入口：多值哈希表（MVMap）。
//
// 对应 Go `pkg/util/mvmap`。通过 `include!` 引入实现；测试模块覆盖基准、
// TestMain 边界、Go 原测试与 AsterSQL 迁移补充用例。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

// 内联 MVMap 核心实现（含 fnv 子模块）。
include!("mvmap.rs");

/// Put/Get 性能基准与冒烟测试。
#[cfg(test)]
#[path = "bench_test.rs"]
mod bench_test;

/// 对齐 Go 原 `TestMVMap` / `TestFNVHash` 的行为测试。
#[cfg(test)]
#[path = "mvmap_test.rs"]
mod mvmap_test;

/// AsterSQL 迁移补充的 MVMap 单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
