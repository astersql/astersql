// Copyright 2026 AsterSQL.

// 工作负载学习（Workload Learning）包入口。
//
// 汇总表读代价指标定义、分析 Handle 与内存缓存等子模块，
// 并在测试配置下挂载对应的单元测试文件。

#![allow(non_snake_case, non_upper_case_globals)]

mod cache;
mod handle;
mod metrics;

pub use cache::*;
pub use handle::*;
pub use metrics::*;

#[cfg(test)]
#[path = "cache_test.rs"]
mod cache_test;
#[cfg(test)]
#[path = "handle_test.rs"]
mod handle_test;
#[cfg(test)]
#[path = "metrics_test.rs"]
mod metrics_test;
