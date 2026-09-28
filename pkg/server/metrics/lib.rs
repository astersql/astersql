// Copyright 2026 AsterSQL.

// server/metrics crate 根模块。
//
// 再导出 Prometheus 查询/断连/空闲/包 IO 等指标句柄，并在测试构建下挂载迁移对照单测。

extern crate self as astersql_server_metrics;

#[path = "metrics.rs"]
mod implementation;

pub use implementation::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
