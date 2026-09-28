// Copyright 2018 PingCAP, Inc.
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

// 统计信息（Statistics）与 Plan Replayer 相关 Prometheus 指标。
//
// 覆盖自动/手动 ANALYZE、伪估计（outdated stats 导致）、同步加载、统计健康度、
// 历史统计与 Plan Replayer 任务。本文件只描述指标元数据，不采集或不注册。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;

// 这段逻辑只描述 Prometheus 指标元数据，不会采集统计信息、注册指标或执行任何 TiDB 业务动作。

// Stats metrics. Go 的包级变量在 实现中保留为可选句柄，表示初始化前的 nil 状态。
/// 自动 ANALYZE 耗时直方图（秒）。ANALYZE 收集表/索引统计供优化器使用。
pub static mut AutoAnalyzeHistogram: Option<prometheus::Histogram> = None;
/// 自动 ANALYZE 次数。
pub static mut AutoAnalyzeCounter: Option<prometheus::CounterVec> = None;
/// 手动 ANALYZE 次数。
pub static mut ManualAnalyzeCounter: Option<prometheus::CounterVec> = None;
/// 统计不准确率分布。
pub static mut StatsInaccuracyRate: Option<prometheus::Histogram> = None;
/// 因统计过期触发伪估计的次数。
pub static mut PseudoEstimation: Option<prometheus::CounterVec> = None;
/// 同步加载统计总次数。
pub static mut SyncLoadCounter: Option<prometheus::Counter> = None;
/// 同步加载超时次数。
pub static mut SyncLoadTimeoutCounter: Option<prometheus::Counter> = None;
/// 同步加载去重次数。
pub static mut SyncLoadDedupCounter: Option<prometheus::Counter> = None;
/// 同步加载延迟（毫秒）。
pub static mut SyncLoadHistogram: Option<prometheus::Histogram> = None;
/// 同步加载期间读统计的延迟（毫秒）。
pub static mut ReadStatsHistogram: Option<prometheus::Histogram> = None;
/// 统计缓存操作计数。
pub static mut StatsCacheCounter: Option<prometheus::CounterVec> = None;
/// 统计缓存数值仪表。
pub static mut StatsCacheGauge: Option<prometheus::GaugeVec> = None;
/// 统计健康度仪表。
pub static mut StatsHealthyGauge: Option<prometheus::GaugeVec> = None;
/// 后台统计增量加载作业耗时。
pub static mut StatsDeltaLoadHistogram: Option<prometheus::Histogram> = None;
/// 后台 stats_meta 更新作业耗时。
pub static mut StatsDeltaUpdateHistogram: Option<prometheus::Histogram> = None;
/// 后台统计使用量更新作业耗时。
pub static mut StatsUsageUpdateHistogram: Option<prometheus::Histogram> = None;
/// 历史统计操作计数。
pub static mut HistoricalStatsCounter: Option<prometheus::CounterVec> = None;
/// Plan Replayer 捕获任务计数（用于复现执行计划问题）。
pub static mut PlanReplayerTaskCounter: Option<prometheus::CounterVec> = None;
/// 已注册的 Plan Replayer 任务数。
pub static mut PlanReplayerRegisterTaskGauge: Option<prometheus::Gauge> = None;

/// 增加手动 ANALYZE 的成功或失败计数。
///
/// Go 在 `AnalyzeExec.Next` 返回时更新该指标。初始化发生在进程启动阶段；在
/// 尚未初始化指标的轻量测试环境中保持无操作，与 nil collector 不可写等价。
pub fn IncManualAnalyzeCounter(result: &str) {
    unsafe {
        if let Some(counter) = ManualAnalyzeCounter.as_ref() {
            counter.with_label_values(&[result]).inc();
        }
    }
}

/// 增加自动 ANALYZE 的成功或失败计数。
///
/// 对应 Go `autoanalyze/exec.AutoAnalyze` 在系统会话执行完成后的计数更新。
pub fn IncAutoAnalyzeCounter(result: &str) {
    unsafe {
        if let Some(counter) = AutoAnalyzeCounter.as_ref() {
            counter.with_label_values(&[result]).inc();
        }
    }
}

// InitStatsMetrics 对应 Go 的指标初始化入口；每个赋值保留原名称、标签与桶配置。
/// 初始化统计信息与 Plan Replayer 相关全部指标句柄。
pub unsafe fn InitStatsMetrics() {
    AutoAnalyzeHistogram = Some(metricscommon::NewHistogram(prometheus::HistogramOpts {
        Namespace: "tidb",
        Subsystem: "statistics",
        Name: "auto_analyze_duration_seconds",
        Help: "Bucketed histogram of processing time (s) of auto analyze.",
        // 指数桶覆盖约 10ms 到 24h，与 Go 的 ExponentialBuckets 参数一致。
        Buckets: prometheus::ExponentialBuckets(0.01, 2.0, 24),
    }));
    AutoAnalyzeCounter = Some(metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "statistics",
            Name: "auto_analyze_total",
            Help: "Counter of auto analyze.",
        },
        &[LblType],
    ));
    ManualAnalyzeCounter = Some(metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "statistics",
            Name: "manual_analyze_total",
            Help: "Counter of manual analyze.",
        },
        &[LblType],
    ));
    StatsInaccuracyRate = Some(metricscommon::NewHistogram(prometheus::HistogramOpts {
        Namespace: "tidb",
        Subsystem: "statistics",
        Name: "stats_inaccuracy_rate",
        Help: "Bucketed histogram of stats inaccuracy rate.",
        Buckets: prometheus::ExponentialBuckets(0.01, 2.0, 14),
    }));
    PseudoEstimation = Some(metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "statistics",
            Name: "pseudo_estimation_total",
            Help: "Counter of pseudo estimation caused by outdated stats.",
        },
        &[LblType],
    ));

    // 同步加载的总数、超时和去重计数分别建模，避免把 Go 的三个独立时序合并。
    SyncLoadCounter = Some(metricscommon::NewCounter(prometheus::CounterOpts {
        Namespace: "tidb",
        Subsystem: "statistics",
        Name: "sync_load_total",
        Help: "Counter of sync load.",
    }));
    SyncLoadTimeoutCounter = Some(metricscommon::NewCounter(prometheus::CounterOpts {
        Namespace: "tidb",
        Subsystem: "statistics",
        Name: "sync_load_timeout_total",
        Help: "Counter of sync load timeout.",
    }));
    SyncLoadDedupCounter = Some(metricscommon::NewCounter(prometheus::CounterOpts {
        Namespace: "tidb",
        Subsystem: "statistics",
        Name: "sync_load_dedup_total",
        Help: "Counter of deduplicated sync load.",
    }));
    SyncLoadHistogram = Some(metricscommon::NewHistogram(prometheus::HistogramOpts {
        Namespace: "tidb",
        Subsystem: "statistics",
        Name: "sync_load_latency_millis",
        Help: "Bucketed histogram of latency time (ms) of sync load.",
        Buckets: prometheus::ExponentialBuckets(1.0, 2.0, 22),
    }));
    ReadStatsHistogram = Some(metricscommon::NewHistogram(prometheus::HistogramOpts {
        Namespace: "tidb",
        Subsystem: "statistics",
        Name: "read_stats_latency_millis",
        Help: "Bucketed histogram of latency time (ms) of stats read during sync-load.",
        Buckets: prometheus::ExponentialBuckets(1.0, 2.0, 22),
    }));
    StatsHealthyGauge = Some(metricscommon::NewGaugeVec(
        prometheus::GaugeOpts {
            Namespace: "tidb",
            Subsystem: "statistics",
            Name: "stats_healthy",
            Help: "Gauge of stats healthy",
        },
        &[LblType],
    ));
    HistoricalStatsCounter = Some(metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "statistics",
            Name: "historical_stats",
            Help: "counter of the historical stats operation",
        },
        &[LblType, LblResult],
    ));
    PlanReplayerTaskCounter = Some(metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "plan_replayer",
            Name: "task",
            Help: "counter of plan replayer captured task",
        },
        &[LblType, LblResult],
    ));
    PlanReplayerRegisterTaskGauge = Some(metricscommon::NewGauge(prometheus::GaugeOpts {
        Namespace: "tidb",
        Subsystem: "plan_replayer",
        Name: "register_task",
        Help: "gauge of plan replayer registered task",
    }));

    // 三个后台统计作业共享相同桶边界，但保留不同指标名与帮助文本。
    StatsDeltaLoadHistogram = Some(metricscommon::NewHistogram(prometheus::HistogramOpts {
        Namespace: "tidb",
        Subsystem: "statistics",
        Name: "stats_delta_load_duration_seconds",
        Help: "Bucketed histogram of processing time for the background statistics loading job",
        Buckets: prometheus::ExponentialBuckets(0.01, 2.0, 24),
    }));
    StatsDeltaUpdateHistogram = Some(metricscommon::NewHistogram(prometheus::HistogramOpts {
        Namespace: "tidb",
        Subsystem: "statistics",
        Name: "stats_delta_update_duration_seconds",
        Help: "Bucketed histogram of processing time for the background stats_meta update job",
        Buckets: prometheus::ExponentialBuckets(0.01, 2.0, 24),
    }));
    StatsUsageUpdateHistogram = Some(metricscommon::NewHistogram(prometheus::HistogramOpts {
        Namespace: "tidb",
        Subsystem: "statistics",
        Name: "stats_usage_update_duration_seconds",
        Help: "Bucketed histogram of processing time for the background stats usage update job",
        Buckets: prometheus::ExponentialBuckets(0.01, 2.0, 24),
    }));
    StatsCacheCounter = Some(metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "statistics",
            Name: "stats_cache_op",
            Help: "Counter for statsCache operation",
        },
        &[LblType],
    ));
    StatsCacheGauge = Some(metricscommon::NewGaugeVec(
        prometheus::GaugeOpts {
            Namespace: "tidb",
            Subsystem: "statistics",
            Name: "stats_cache_val",
            Help: "gauge of stats cache value",
        },
        &[LblType],
    ));
}
