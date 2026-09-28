// Copyright 2026 PingCAP, Inc.
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

// Statement Summary（语句摘要）窗口指标。
//
// Statement Summary 聚合同类 SQL 的执行统计；v1 为内存实现，v2 可持久化。
// 本文件维护窗口内记录数、LRU 淘汰数与淘汰日志计数，并缓存 v1/v2 标签句柄。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;

// 这段逻辑不会注册或上报 Prometheus 指标，也不会读写 statement summary；外部指标类型仅保留调用形状。

use std::sync::Mutex;

// 两个实现类型与 Go 标签值完全一致，调用方用它们区分内存版和持久化版。
/// Statement Summary v1（内存版）类型标签。
pub const StmtSummaryTypeV1: &str = "v1";
/// Statement Summary v2（可持久化）类型标签。
pub const StmtSummaryTypeV2: &str = "v2";
/// 淘汰日志已持久化结果标签。
pub const StmtSummaryEvictedLogResultPersisted: &str = "persisted";
/// 淘汰日志被丢弃结果标签。
pub const StmtSummaryEvictedLogResultDropped: &str = "dropped";

/// 当前窗口跟踪的语句摘要记录数。
pub static mut StmtSummaryWindowRecordCount: Option<prometheus::GaugeVec> = None;
/// 当前窗口 LRU 淘汰次数。
pub static mut StmtSummaryWindowEvictedCount: Option<prometheus::GaugeVec> = None;
/// v2 淘汰日志按结果分类的计数。
pub static mut StmtSummaryEvictedLogCounter: Option<prometheus::CounterVec> = None;

// Go 用一把 mutex 保护四个惰性缓存；将它们合并进同一受保护状态，锁粒度不变。
/// v1/v2 窗口 Gauge 的惰性缓存状态。
struct StmtSummaryWindowMetrics {
    record_v1: Option<prometheus::Gauge>,
    record_v2: Option<prometheus::Gauge>,
    evicted_v1: Option<prometheus::Gauge>,
    evicted_v2: Option<prometheus::Gauge>,
}

static stmtSummaryWindowMetricsMu: Mutex<StmtSummaryWindowMetrics> =
    Mutex::new(StmtSummaryWindowMetrics {
        record_v1: None,
        record_v2: None,
        evicted_v1: None,
        evicted_v2: None,
    });

// InitStmtSummaryMetrics 对应 Go 初始化入口，并在重建向量后清空旧标签句柄缓存。
/// 初始化 statement summary 指标并清空 v1/v2 标签缓存。
pub unsafe fn InitStmtSummaryMetrics() {
    StmtSummaryWindowRecordCount = Some(metricscommon::NewGaugeVec(
        prometheus::GaugeOpts {
            Namespace: "tidb",
            Subsystem: "stmt_summary",
            Name: "window_record_count",
            Help: "The number of statement summary records currently tracked by statement summary.",
        },
        &[LblType],
    ));
    StmtSummaryWindowEvictedCount = Some(metricscommon::NewGaugeVec(
        prometheus::GaugeOpts {
            Namespace: "tidb",
            Subsystem: "stmt_summary",
            Name: "window_evicted_count",
            Help: "The number of LRU evictions in the current statement summary window.",
        },
        &[LblType],
    ));
    StmtSummaryEvictedLogCounter = Some(metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "stmt_summary",
            Name: "evicted_log_total",
            Help: "The number of v2 statement summary evicted-log records by result.",
        },
        &[LblType, LblResult],
    ));

    // poison 场景在 Go 中不存在；以 expect 明确表示锁损坏是不可恢复的内部错误。
    let mut cached = stmtSummaryWindowMetricsMu
        .lock()
        .expect("statement summary metrics mutex poisoned");
    cached.record_v1 = None;
    cached.record_v2 = None;
    cached.evicted_v1 = None;
    cached.evicted_v2 = None;
}

// SetStmtSummaryWindowMetrics 对应 Go 的窗口上报函数；v1/v2 复用缓存，未知类型直接按标签取句柄。
/// 更新指定实现类型窗口的记录数与淘汰数。
pub unsafe fn SetStmtSummaryWindowMetrics(typ: &str, recordCount: f64, evictedCount: f64) {
    match typ {
        StmtSummaryTypeV1 | StmtSummaryTypeV2 => {
            let (record, evicted) = getStmtSummaryWindowMetricsLocked(typ);
            record.Set(recordCount);
            evicted.Set(evictedCount);
        }
        _ => {
            StmtSummaryWindowRecordCount
                .as_ref()
                .unwrap()
                .WithLabelValues(&[typ])
                .Set(recordCount);
            StmtSummaryWindowEvictedCount
                .as_ref()
                .unwrap()
                .WithLabelValues(&[typ])
                .Set(evictedCount);
        }
    }
}

// getStmtSummaryWindowMetricsLocked 保留 Go 的惰性初始化与加锁范围。
// 返回 clone 的指标句柄等价于 Go 接口值复制；互斥锁在函数返回前自动释放。
/// 在锁内惰性获取并缓存 v1/v2 的 record/evicted Gauge 句柄。
unsafe fn getStmtSummaryWindowMetricsLocked(typ: &str) -> (prometheus::Gauge, prometheus::Gauge) {
    let mut cached = stmtSummaryWindowMetricsMu
        .lock()
        .expect("statement summary metrics mutex poisoned");
    let records = StmtSummaryWindowRecordCount.as_ref().unwrap();
    let evictions = StmtSummaryWindowEvictedCount.as_ref().unwrap();

    match typ {
        StmtSummaryTypeV1 => {
            if cached.record_v1.is_none() {
                cached.record_v1 = Some(records.WithLabelValues(&[typ]));
                cached.evicted_v1 = Some(evictions.WithLabelValues(&[typ]));
            }
            (
                cached.record_v1.clone().unwrap(),
                cached.evicted_v1.clone().unwrap(),
            )
        }
        StmtSummaryTypeV2 => {
            if cached.record_v2.is_none() {
                cached.record_v2 = Some(records.WithLabelValues(&[typ]));
                cached.evicted_v2 = Some(evictions.WithLabelValues(&[typ]));
            }
            (
                cached.record_v2.clone().unwrap(),
                cached.evicted_v2.clone().unwrap(),
            )
        }
        // Go 默认分支返回 nil；正常调用在进入此函数前已限制为 v1/v2。
        _ => unreachable!("unsupported statement summary implementation type"),
    }
}
