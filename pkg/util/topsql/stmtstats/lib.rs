// Copyright 2026 AsterSQL.

// stmtstats crate 入口：语句级 TopSQL/TopRU 统计、KV 计数与聚合器。
//
// 聚合各会话 `StatementStats`，定期推送给 Collector / RUCollector；
// RU（Resource Unit）为 TiDB 资源计量单位。对应 Go `pkg/util/topsql/stmtstats`。

#![allow(non_snake_case, non_upper_case_globals, static_mut_refs)]

/// 重导出 TopSQL/TopRU 全局状态依赖。
pub use topsql_state_dependency as topsql_state;

/// 执行详情的薄封装重导出。
pub mod execdetails {
    pub use execdetails_dependency::execdetails::util::RUDetails;
}

/// 上报侧 Prometheus 指标。
pub use reporter_metrics_dependency::{metrics, reporter_metrics};

mod rustats;
pub use rustats::*;
mod stmtstats;
pub use stmtstats::*;
mod kv_exec_count;
pub use kv_exec_count::*;
mod aggregator;
pub use aggregator::*;

/// 测试用：初始化 reporter metrics 父指标与计数器句柄。
pub fn init_reporter_metrics_for_tests() {
    metrics::init_parent_metrics();
    reporter_metrics::init();
}

/// 测试用：读取因超限丢弃的 RU key 数与 RU 总量计数。
pub fn reporter_drop_metrics_for_tests() -> (f64, f64) {
    unsafe {
        (
            reporter_metrics::IgnoreExceedRUKeysCounter
                .as_ref()
                .expect("drop counter initialized")
                .get(),
            reporter_metrics::IgnoreExceedRUTotalCounter
                .as_ref()
                .expect("drop RU counter initialized")
                .get(),
        )
    }
}

#[path = "test_support.rs"]
#[doc(hidden)]
pub mod stmtstats_tests;
#[path = "test_guard.rs"]
#[doc(hidden)]
pub mod test_support;

#[cfg(test)]
#[path = "aggregator_1_aster_unit_test.rs"]
mod aggregator_1_aster_unit_test;
#[cfg(test)]
#[path = "aggregator_bench_test.rs"]
mod aggregator_bench_test;
#[cfg(test)]
#[path = "aggregator_test.rs"]
mod aggregator_test;
#[cfg(test)]
#[path = "kv_exec_count_test.rs"]
mod kv_exec_count_test;
#[cfg(test)]
#[path = "stmtstats_test.rs"]
mod stmtstats_test;
