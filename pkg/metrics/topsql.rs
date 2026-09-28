// Copyright 2021 PingCAP, Inc.
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

// Top SQL 相关 Prometheus 指标。
//
// Top SQL 将高频/高耗时 SQL 与执行计划上报到 agent，用于热点诊断。
// 本文件只定义忽略计数、上报耗时与上报数据量三类指标句柄，不连接 agent。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;

// 这段逻辑不会连接 Top SQL agent、注册指标或发送 SQL/执行计划数据，外部 Prometheus 类型仅作接口形状参考。

// Top SQL metrics. Option 对应 Go 初始化前的 nil 指标句柄。
/// 被忽略的 Top SQL 指标事件计数（注册 SQL/计划、采集与上报），正常应为 0。
pub static mut TopSQLIgnoredCounter: Option<prometheus::CounterVec> = None;
/// 向 Top SQL agent 上报的耗时分布（秒）。
pub static mut TopSQLReportDurationHistogram: Option<prometheus::HistogramVec> = None;
/// 单次上报的记录/SQL/计划数量分布。
pub static mut TopSQLReportDataHistogram: Option<prometheus::HistogramVec> = None;

// InitTopSQLMetrics 对应 Go 初始化入口，保留忽略计数、上报耗时和上报数据量三个指标。
/// 初始化 Top SQL 三类指标句柄（不注册、不上报）。
pub unsafe fn InitTopSQLMetrics() {
    TopSQLIgnoredCounter = Some(metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "topsql",
            Name: "ignored_total",
            Help: "Counter of ignored top-sql metrics (register-sql, register-plan, collect-data and report-data), normally it should be 0.",
        },
        &[LblType],
    ));

    TopSQLReportDurationHistogram = Some(metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "topsql",
            Name: "report_duration_seconds",
            Help: "Bucket histogram of reporting time (s) to the top-sql agent",
            // 指数桶从 1ms 开始，共 24 桶，覆盖范围约到 2.3 小时。
            Buckets: prometheus::ExponentialBuckets(0.001, 2.0, 24),
        },
        &[LblType, LblResult],
    ));

    TopSQLReportDataHistogram = Some(metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "topsql",
            Name: "report_data_total",
            Help: "Bucket histogram of reporting records/sql/plan count to the top-sql agent.",
            // 数据量桶保留 Go 的 1、2 倍增长和 20 桶配置，最大边界为 524288。
            Buckets: prometheus::ExponentialBuckets(1.0, 2.0, 20),
        },
        &[LblType],
    ));
}
