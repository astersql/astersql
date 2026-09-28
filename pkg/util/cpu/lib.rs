// Copyright 2026 AsterSQL.

// `pkg/util/cpu` crate 入口：重导出 CPU 观测 API，并挂接 mathutil / cgroup / metrics 依赖。
//
// 测试时通过 `TEST_LOCK` 串行化全局状态相关用例，避免观测器原子量互相干扰。

#![allow(non_snake_case, non_upper_case_globals)]

extern crate self as util_cpu;

/// 指数移动平均等数学工具（外部 crate 再导出）。
pub mod mathutil {
    pub use mathutil_crate::*;
}
/// cgroup CPU 配额/用量查询（外部 crate 再导出）。
pub mod cgroup {
    pub use cgroup_crate::*;
}
/// Prometheus 指标占位：EMA CPU 使用率 Gauge。
pub mod metrics {
    /// 可选的全局 EMA CPU 使用率指标；观测器启动后写入。
    pub static mut EMACPUUsageGauge: Option<prometheus::Gauge> = None;
}

mod cpu;
pub use cpu::*;

/// 串行化依赖全局 CPU 状态的测试，避免并发污染。
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
#[path = "cpu_test.rs"]
mod cpu_test;
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
