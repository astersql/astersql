// Copyright 2026 AsterSQL.

// CPU 剖析（CPU profile）工具 crate 入口。
//
// 对应 Go `util/cpuprofile`：聚合并行采样器、pprof HTTP/API 封装，以及测试用
// CPU 负载模拟。pprof 是 Google 的性能剖析数据格式，常用于采集与展示 CPU 采样。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals
)]

// 自引用别名，供子模块与测试以 crate 名导入本包符号。
extern crate self as cpuprofile_testutil;
extern crate self as util_cpuprofile;
/// 并行 CPU 剖析器核心实现。
mod cpuprofile;
pub use cpuprofile::*;
/// pprof 采集器与 HTTP 处理封装。
mod pprof_api;
pub use pprof_api::*;
/// 测试用 CPU 负载与取消令牌工具。
#[path = "testutil/util.rs"]
mod testutil;
pub use testutil::*;

#[cfg(test)]
mod cpuprofile_test;

#[cfg(test)]
mod pprof_api_test;

/// 迁移期单元测试：全局剖析器、采集器与 HTTP 行为对齐 Go。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

/// testutil 侧模拟负载标签分组的迁移测试。
#[cfg(test)]
#[path = "testutil/migration_aster_unit_test.rs"]
mod testutil_migration_aster_unit_test;
