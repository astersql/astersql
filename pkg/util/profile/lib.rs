// Copyright 2026 AsterSQL.

// `pkg/util/profile` crate 入口：性能剖析采集与火焰图展示。
//
// 对外再导出 `profile` 模块的 `Collector` 等 API；`flamegraph` 为内部实现。
// 测试模块覆盖迁移单测、火焰图 fixture 与 ProfileGraph 分发。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals
)]
mod flamegraph;
mod profile;
pub use profile::*;

#[cfg(test)]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "flamegraph_test.rs"]
mod flamegraph_test;

#[cfg(test)]
#[path = "profile_test.rs"]
mod profile_test;
