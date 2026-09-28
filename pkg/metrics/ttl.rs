// Copyright 2022 PingCAP, Inc.
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

// TTL（Time To Live，按时间自动清理过期行）相关 Prometheus 指标定义与初始化。
//
// 本文件只声明指标句柄并在 `InitTTLMetrics` 中注册元数据（名称、帮助文本、桶与标签），
// 不执行扫描、删除过期行或同步定时器等业务逻辑。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;

// 这段逻辑不会扫描或删除过期行、同步定时器、访问数据库或注册 Prometheus 指标，只描述指标元数据。

// TTL metrics. Option 对应 Go 包变量初始化前的 nil 状态。
/// TTL 查询处理耗时直方图（秒），按 SQL 类型与结果标签分桶。
pub static mut TTLQueryDuration: Option<prometheus::HistogramVec> = None;
/// 已处理过期行计数器，按 SQL 类型与结果标签累计。
pub static mut TTLProcessedExpiredRowsCounter: Option<prometheus::CounterVec> = None;
/// 处于指定状态的 TTL Job（作业）数量 Gauge。
pub static mut TTLJobStatus: Option<prometheus::GaugeVec> = None;
/// 处于指定状态的 TTL Task（任务）数量 Gauge。
pub static mut TTLTaskStatus: Option<prometheus::GaugeVec> = None;
/// 各阶段耗时累计计数器，按类型与阶段标签划分。
pub static mut TTLPhaseTime: Option<prometheus::CounterVec> = None;
/// TTL 插入行数累计计数器。
pub static mut TTLInsertRowsCount: Option<prometheus::Counter> = None;
/// TTL 水位延迟（秒）Gauge；水位表示清理进度边界。
pub static mut TTLWatermarkDelay: Option<prometheus::GaugeVec> = None;
/// TTL 事件总计数向量，后续会拆出子句柄。
pub static mut TTLEventCounter: Option<prometheus::CounterVec> = None;
/// 同步单个定时器事件的计数句柄（来自 `TTLEventCounter`）。
pub static mut TTLSyncTimerCounter: Option<prometheus::Counter> = None;
/// 全量刷新定时器事件的计数句柄（来自 `TTLEventCounter`）。
pub static mut TTLFullRefreshTimersCounter: Option<prometheus::Counter> = None;

// InitTTLMetrics 对应 Go 的 TTL 指标初始化入口，保留各指标名称、帮助文本、桶和标签维度。
/// 初始化全部 TTL Prometheus 指标，并缓存常用事件标签的 Counter 子句柄。
pub unsafe fn InitTTLMetrics() {
    TTLQueryDuration = Some(metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "server",
            Name: "ttl_query_duration",
            Help: "Bucketed histogram of processing time (s) of handled TTL queries.",
            // 20 个指数桶从 10ms 起步，覆盖约 1.45 小时。
            Buckets: prometheus::ExponentialBuckets(0.01, 2.0, 20),
        },
        &[LblSQLType, LblResult],
    ));

    TTLProcessedExpiredRowsCounter = Some(metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "server",
            Name: "ttl_processed_expired_rows",
            Help: "The count of expired rows processed in TTL jobs",
        },
        &[LblSQLType, LblResult],
    ));
    TTLJobStatus = Some(metricscommon::NewGaugeVec(
        prometheus::GaugeOpts {
            Namespace: "tidb",
            Subsystem: "server",
            Name: "ttl_job_status",
            Help: "The jobs count in the specified status",
        },
        &[LblType],
    ));
    TTLTaskStatus = Some(metricscommon::NewGaugeVec(
        prometheus::GaugeOpts {
            Namespace: "tidb",
            Subsystem: "server",
            Name: "ttl_task_status",
            Help: "The tasks count in the specified status",
        },
        &[LblType],
    ));
    TTLPhaseTime = Some(metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "server",
            Name: "ttl_phase_time",
            Help: "The time spent in each phase",
        },
        &[LblType, LblPhase],
    ));
    TTLInsertRowsCount = Some(metricscommon::NewCounter(prometheus::CounterOpts {
        Namespace: "tidb",
        Subsystem: "server",
        Name: "ttl_insert_rows",
        Help: "The count of TTL rows inserted",
    }));
    TTLWatermarkDelay = Some(metricscommon::NewGaugeVec(
        prometheus::GaugeOpts {
            Namespace: "tidb",
            Subsystem: "server",
            Name: "ttl_watermark_delay",
            Help: "Bucketed delay time in seconds for TTL tables.",
        },
        &[LblType, LblName],
    ));
    TTLEventCounter = Some(metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "server",
            Name: "ttl_event_count",
            Help: "Counter of ttl event.",
        },
        &[LblType],
    ));

    // Go 从同一个事件向量缓存两个 label 子句柄；后续递增不会再次解析标签。
    let events = TTLEventCounter.as_ref().unwrap();
    TTLSyncTimerCounter = Some(events.WithLabelValues(&["sync_one_timer"]));
    TTLFullRefreshTimersCounter = Some(events.WithLabelValues(&["full_refresh_timers"]));
}
