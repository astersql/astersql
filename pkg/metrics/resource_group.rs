// Copyright 2023 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 资源组（Resource Group） runaway 相关 Prometheus 指标定义。
//
// Runaway 用于检测并处置超出资源组配额的查询：checker 做判定，flusher 批量落盘/上报，
// syncer 在多节点间同步 watch 窗口。本文件只构造指标句柄，不执行 runaway 业务逻辑。

use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;

// Metrics
// Query duration by query is QueryDurationHistogram in `server.go`.
// Option 保留 Go 包级指标指针在初始化前为 nil 的状态。
/// Runaway 检查触发次数，按资源组、检查类型与动作区分。
pub static mut RUNAWAY_CHECKER_COUNTER: Option<prometheus::CounterVec> = None;
/// Flusher 操作总次数，按名称与结果区分。
pub static mut RUNAWAY_FLUSHER_COUNTER: Option<prometheus::CounterVec> = None;
/// 向 flusher 追加记录的次数。
pub static mut RUNAWAY_FLUSHER_ADD_COUNTER: Option<prometheus::CounterVec> = None;
/// Flusher 单次批量大小分布。
pub static mut RUNAWAY_FLUSHER_BATCH_SIZE_HISTOGRAM: Option<prometheus::HistogramVec> = None;
/// Flusher 单次操作耗时（秒）。
pub static mut RUNAWAY_FLUSHER_DURATION_HISTOGRAM: Option<prometheus::HistogramVec> = None;
/// 相邻两次 flusher 操作的间隔（秒）。
pub static mut RUNAWAY_FLUSHER_INTERVAL_HISTOGRAM: Option<prometheus::HistogramVec> = None;
/// Syncer 读操作耗时（秒）。
pub static mut RUNAWAY_SYNCER_DURATION_HISTOGRAM: Option<prometheus::HistogramVec> = None;
/// 相邻两次 syncer 读操作的间隔（秒）。
pub static mut RUNAWAY_SYNCER_INTERVAL_HISTOGRAM: Option<prometheus::HistogramVec> = None;
/// Syncer 扫描窗口下界检查点（Unix 毫秒）。
pub static mut RUNAWAY_SYNCER_CHECKPOINT_GAUGE: Option<prometheus::GaugeVec> = None;
/// Syncer 操作总次数，按类型与结果区分。
pub static mut RUNAWAY_SYNCER_COUNTER: Option<prometheus::CounterVec> = None;

// init_resource_group_metrics 对应 Go 的 InitResourceGroupMetrics，按 checker、flusher、syncer 顺序构造指标。
// static mut 只表达 Go 的包级赋值；真正并发初始化时应由 OnceLock 或统一注册流程保护。
/// 初始化资源组 runaway 相关全部指标句柄（不注册到 Prometheus registry）。
pub unsafe fn init_resource_group_metrics() {
    RUNAWAY_CHECKER_COUNTER = Some(metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "server",
            Name: "query_runaway_check",
            Help: "Counter of query triggering runaway check.",
        },
        // 资源组、检查类型和采取的动作共同区分一次 runaway 判定。
        &[LblResourceGroup, LblType, LblAction],
    ));

    RUNAWAY_FLUSHER_COUNTER = Some(metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "server",
            Name: "runaway_flusher_total",
            Help: "Counter of runaway flusher operations.",
        },
        &[LblName, LblResult],
    ));

    RUNAWAY_FLUSHER_ADD_COUNTER = Some(metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "server",
            Name: "runaway_flusher_add_total",
            Help: "Counter of records added to runaway flusher.",
        },
        &[LblName],
    ));

    RUNAWAY_FLUSHER_BATCH_SIZE_HISTOGRAM = Some(metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "server",
            Name: "runaway_flusher_batch_size",
            Help: "Batch size of runaway flusher operations.",
            // 1、2、4……512，保留 Go 的十个批大小桶。
            Buckets: prometheus::ExponentialBuckets(1.0, 2.0, 10),
        },
        &[LblName],
    ));

    RUNAWAY_FLUSHER_DURATION_HISTOGRAM = Some(metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "server",
            Name: "runaway_flusher_duration_seconds",
            Help: "Duration of runaway flusher operations in seconds.",
            // 1ms 起翻倍 15 桶，覆盖到约 16 秒。
            Buckets: prometheus::ExponentialBuckets(0.001, 2.0, 15),
        },
        &[LblName],
    ));

    RUNAWAY_FLUSHER_INTERVAL_HISTOGRAM = Some(metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "server",
            Name: "runaway_flusher_interval_seconds",
            Help: "Interval between runaway flusher operations in seconds.",
            // 0.1s 起翻倍 12 桶，覆盖到约 200 秒。
            Buckets: prometheus::ExponentialBuckets(0.1, 2.0, 12),
        },
        &[LblName],
    ));

    RUNAWAY_SYNCER_DURATION_HISTOGRAM = Some(metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "server",
            Name: "runaway_syncer_duration_seconds",
            Help: "Duration of runaway syncer read operations in seconds.",
            Buckets: prometheus::ExponentialBuckets(0.001, 2.0, 15),
        },
        &[LblType],
    ));

    RUNAWAY_SYNCER_INTERVAL_HISTOGRAM = Some(metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "server",
            Name: "runaway_syncer_interval_seconds",
            Help: "Interval between runaway syncer read operations in seconds.",
            Buckets: prometheus::ExponentialBuckets(0.1, 2.0, 12),
        },
        &[LblType],
    ));

    RUNAWAY_SYNCER_CHECKPOINT_GAUGE = Some(metricscommon::NewGaugeVec(
        prometheus::GaugeOpts {
            Namespace: "tidb",
            Subsystem: "server",
            Name: "runaway_syncer_checkpoint",
            Help: "Current lower-bound checkpoint of runaway syncer: Unix milliseconds of the next scan window for start_time (watch) or done_time (watch_done).",
        },
        // type 区分 watch 的 start_time 与 watch_done 的 done_time 扫描窗口。
        &[LblType],
    ));

    RUNAWAY_SYNCER_COUNTER = Some(metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "server",
            Name: "runaway_syncer_total",
            Help: "Counter of runaway syncer operations.",
        },
        &[LblType, LblResult],
    ));
}
