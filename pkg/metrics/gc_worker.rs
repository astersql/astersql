// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// GC（垃圾回收）Worker 相关 Prometheus 指标。
//
// GC Worker 清理过期的 MVCC（多版本并发控制）版本；按 Region（数据分片）扫描锁
// 并执行销毁。本模块统计 worker 动作、各阶段耗时、配置值、失败次数以及
// unsafe destroy range 等危险操作的失败计数。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;

//

// Metrics for the GC worker.
/// GC Worker 各类动作累计次数。
pub static mut GCWorkerCounter: Option<prometheus::CounterVec> = None;
/// GC 各阶段耗时直方图（stage 标签）。
pub static mut GCHistogram: Option<prometheus::HistogramVec> = None;
/// 当前 GC 相关配置值 Gauge。
pub static mut GCConfigGauge: Option<prometheus::GaugeVec> = None;
/// GC 任务失败次数。
pub static mut GCJobFailureCounter: Option<prometheus::CounterVec> = None;
/// 按 Region 粒度的 GC 动作结果计数。
pub static mut GCActionRegionResultCounter: Option<prometheus::CounterVec> = None;
/// 同一 Region 内多次扫锁（锁过多）的次数。
pub static mut GCRegionTooManyLocksCounter: Option<prometheus::Counter> = None;
/// unsafe destroy range 失败计数（由外部 TiKV client collector 注入）。
pub static mut GCUnsafeDestroyRangeFailuresCounterVec: Option<prometheus::CounterVec> = None;

/// 初始化 GC Worker 本地定义的指标 collector。
// InitGCWorkerMetrics 对应 Go 初始化函数：创建 worker 动作、耗时、配置、失败与 region 结果指标。
pub fn InitGCWorkerMetrics() {
    let _init_guard = crate::metrics::PACKAGE_INIT_LOCK
        .lock()
        .expect("metrics init lock poisoned");
    let worker_counter = metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "tikvclient",
            Name: "gc_worker_actions_total",
            Help: "Counter of gc worker actions.",
        },
        vec!["type"],
    );

    let gc_histogram = metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "tikvclient",
            Name: "gc_seconds",
            Help: "Bucketed histogram of gc duration.",
            // 1 秒起始、倍数 2、20 个桶，保持 Go 的约 6 天覆盖范围。
            Buckets: prometheus::ExponentialBuckets(1.0, 2.0, 20),
        },
        vec!["stage"],
    );

    let config_gauge = metricscommon::NewGaugeVec(
        prometheus::GaugeOpts {
            Namespace: "tidb",
            Subsystem: "tikvclient",
            Name: "gc_config",
            Help: "Gauge of GC configs.",
        },
        vec!["type"],
    );

    let job_failure_counter = metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "tikvclient",
            Name: "gc_failure",
            Help: "Counter of gc job failure.",
        },
        vec!["type"],
    );

    let region_result_counter = metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "tikvclient",
            Name: "gc_action_result",
            Help: "Counter of gc action result on region level.",
        },
        vec!["type"],
    );

    let too_many_locks_counter = metricscommon::NewCounter(prometheus::CounterOpts {
        Namespace: "tidb",
        Subsystem: "tikvclient",
        Name: "gc_region_too_many_locks",
        Help: "Counter of gc scan lock request more than once in the same region.",
    });

    // Go aliases client-go's package-initialized collector. The Rust TiKV client
    // does not export that collector, so initialize an equivalent local handle;
    // `init_gc_unsafe_destroy_range_metric` may replace it with an upstream one.
    let unsafe_destroy_range_failures_counter = metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "tikvclient",
            Name: "gc_unsafe_destroy_range_failures",
            Help: "Counter of unsafe destroyrange failures",
        },
        vec!["type"],
    );

    // Go 由 InitMetrics 串行调用；static mut 只机械表示包级赋值，不提供同步保证。
    unsafe {
        GCWorkerCounter = Some(worker_counter);
        GCHistogram = Some(gc_histogram);
        GCConfigGauge = Some(config_gauge);
        GCJobFailureCounter = Some(job_failure_counter);
        GCActionRegionResultCounter = Some(region_result_counter);
        GCRegionTooManyLocksCounter = Some(too_many_locks_counter);
        GCUnsafeDestroyRangeFailuresCounterVec = Some(unsafe_destroy_range_failures_counter);
    }
}

/// GCHistogram 的 stage 标签：表示一整轮 GC 的总耗时。
// StageTotal is used in the "stage" label of GCHistogram to represent the total time of a turn of GC.
pub const StageTotal: &str = "total";

/// 将 TiKV client 侧的 unsafe-destroy-range 失败 Counter 接到本包静态槽位。
/// 调用方传入上游 collector（client-go 无对应 Rust crate），此处不重新实现指标本身。
/// Wires the TiKV client's unsafe-destroy-range failure collector into this
/// package. The caller supplies the upstream collector because client-go has no
/// Rust crate counterpart; the collector itself is not reimplemented here.
pub fn init_gc_unsafe_destroy_range_metric(counter: prometheus::CounterVec) {
    unsafe {
        GCUnsafeDestroyRangeFailuresCounterVec = Some(counter);
    }
}
