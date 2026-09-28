// Copyright 2026 AsterSQL.

// Lightning Prometheus 指标子 crate 入口。
//
// 重导出 `promutil` 与 `metric` 模块中的计数器、直方图与上下文包装，
// 供导入（Import）路径上报 chunk/行/字节进度与各阶段耗时。

#![allow(non_snake_case, non_upper_case_globals)]

extern crate self as astersql_lightning_metric;

/// 将 util 侧 `promutil` 工厂与注册表抽象再导出到本 crate。
pub mod promutil {
    pub use astersql_util_promutil::*;
}

pub mod metric;
pub use metric::*;

#[cfg(test)]
#[path = "metric_test.rs"]
mod metric_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
