// Copyright 2026 AsterSQL.

// TopSQL collector crate 入口：导出 CPU 采集实现。
//
// 对应 Go `util/topsql/collector`；`cpu` 模块提供 SQLCPUCollector 等 API。

extern crate self as topsql_collector;

/// CPU profile 采集与聚合实现。
mod cpu;
/// 再导出 cpu 模块公共类型与函数。
pub use cpu::*;
