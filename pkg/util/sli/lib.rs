// Copyright 2026 AsterSQL.

// 事务写入吞吐 SLI（Service Level Indicator，服务级别指标）crate 入口。
//
// 对应 Go `pkg/util/sli`：累计单笔事务的写入大小/键数/耗时，并在提交时上报
// 小事务耗时或写入吞吐指标。本文件提供测试互斥锁、failpoint 与 metrics 桩。

#![allow(non_snake_case, non_upper_case_globals)]

extern crate self as astersql_util_sli;

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// 测试互斥锁：串行化依赖全局 metrics/failpoint 状态的用例。
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// 获取测试守卫；锁被毒化时仍接管，避免单测互相污染。
pub fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 测试用 failpoint：模拟 Go `failpoint.Inject("CheckTxnWriteThroughput", ...)`。
pub mod failpoint {
    use super::{AtomicBool, Ordering};

    /// 是否注入 `CheckTxnWriteThroughput`（为真时 commit 后不 Reset，便于断言状态）。
    static CHECK_TXN_WRITE_THROUGHPUT: AtomicBool = AtomicBool::new(false);

    /// 按名称查询是否启用对应 failpoint。
    pub fn inject(name: &str) -> bool {
        name == "CheckTxnWriteThroughput" && CHECK_TXN_WRITE_THROUGHPUT.load(Ordering::SeqCst)
    }

    /// 启用 `CheckTxnWriteThroughput` failpoint。
    pub fn enable() {
        CHECK_TXN_WRITE_THROUGHPUT.store(true, Ordering::SeqCst);
    }

    /// 关闭 `CheckTxnWriteThroughput` failpoint。
    pub fn disable() {
        CHECK_TXN_WRITE_THROUGHPUT.store(false, Ordering::SeqCst);
    }
}

/// Prometheus 指标桩：记录相对 baseline 的观测次数与求和，供单测断言。
pub mod metrics {
    use prometheus::{Histogram, HistogramOpts};
    use std::sync::LazyLock;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SMALL: LazyLock<Histogram> = LazyLock::new(|| {
        Histogram::with_opts(HistogramOpts::new(
            "small_txn_write_duration_seconds",
            "small transaction write duration",
        ))
        .unwrap()
    });
    static THROUGHPUT: LazyLock<Histogram> = LazyLock::new(|| {
        Histogram::with_opts(HistogramOpts::new(
            "txn_write_throughput",
            "transaction write throughput",
        ))
        .unwrap()
    });
    /// reset 时记录的样本计数基线（小事务耗时）。
    static SMALL_BASE_COUNT: AtomicU64 = AtomicU64::new(0);
    /// reset 时记录的样本求和基线（小事务耗时，按 f64 bits 存）。
    static SMALL_BASE_SUM: AtomicU64 = AtomicU64::new(0);
    /// reset 时记录的样本计数基线（写入吞吐）。
    static THROUGHPUT_BASE_COUNT: AtomicU64 = AtomicU64::new(0);
    /// reset 时记录的样本求和基线（写入吞吐，按 f64 bits 存）。
    static THROUGHPUT_BASE_SUM: AtomicU64 = AtomicU64::new(0);

    /// Histogram 观察者包装，保留 Go 风格 `Observe` 方法名。
    pub struct Observer(&'static LazyLock<Histogram>);

    impl Observer {
        /// 向底层 Histogram 写入一个观测值。
        pub fn Observe(&self, value: f64) {
            self.0.observe(value);
        }
    }

    /// 小事务写入耗时指标（秒）。
    pub static SmallTxnWriteDuration: Observer = Observer(&SMALL);
    /// 事务写入吞吐指标（字节/秒）。
    pub static TxnWriteThroughput: Observer = Observer(&THROUGHPUT);

    /// 将当前 Histogram 计数/求和记为基线，后续观测按增量返回。
    pub fn reset() {
        SMALL_BASE_COUNT.store(SMALL.get_sample_count(), Ordering::SeqCst);
        SMALL_BASE_SUM.store(SMALL.get_sample_sum().to_bits(), Ordering::SeqCst);
        THROUGHPUT_BASE_COUNT.store(THROUGHPUT.get_sample_count(), Ordering::SeqCst);
        THROUGHPUT_BASE_SUM.store(THROUGHPUT.get_sample_sum().to_bits(), Ordering::SeqCst);
    }

    /// 返回自上次 reset 以来小事务耗时的（次数, 求和增量）。
    pub fn small_txn_observations() -> (u64, f64) {
        (
            SMALL.get_sample_count() - SMALL_BASE_COUNT.load(Ordering::SeqCst),
            SMALL.get_sample_sum() - f64::from_bits(SMALL_BASE_SUM.load(Ordering::SeqCst)),
        )
    }

    /// 返回自上次 reset 以来写入吞吐的（次数, 求和增量）。
    pub fn throughput_observations() -> (u64, f64) {
        (
            THROUGHPUT.get_sample_count() - THROUGHPUT_BASE_COUNT.load(Ordering::SeqCst),
            THROUGHPUT.get_sample_sum()
                - f64::from_bits(THROUGHPUT_BASE_SUM.load(Ordering::SeqCst)),
        )
    }
}

#[path = "sli.rs"]
mod sli;

pub use sli::TxnWriteThroughputSLI;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
