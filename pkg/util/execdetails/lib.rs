// Copyright 2026 AsterSQL.

// `execdetails` 包集成入口：把内部 group1 / ruv2_metrics / util 子 crate 的公共 API
// 再导出为统一模块，并在测试配置下挂接单元测试。
//
// 对应 Go `pkg/util/execdetails`。执行明细（exec details）记录 SQL 执行过程中的
// 耗时、RPC、扫描与资源消耗（RU）等信息，供慢查询与观测使用。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    renamed_and_removed_lints,
    static_mut_refs
)]

extern crate self as execdetails_integration;

/// 再导出 group1 中的执行明细与 runtime stats API。
pub mod execdetails {
    pub use execdetails_group_1::*;
}
/// 再导出 RUv2 指标相关 API。
pub mod ruv2_metrics {
    pub use execdetails_ruv2::*;
}
/// 再导出 util 子模块（上下文键、流量与 RU 明细等）。
pub mod util {
    pub use execdetails_util::*;
}

#[cfg(test)]
#[path = "execdetails_test.rs"]
mod execdetails_test;
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "tiflash_stats_test.rs"]
mod tiflash_stats_test;

#[cfg(test)]
#[path = "go_merge_20_test.rs"]
mod go_merge_20_test;
