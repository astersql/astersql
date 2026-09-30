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

// TopSQL reporter 子指标句柄：从父向量按标签切出可写 Counter/Histogram。
//
// 对齐 Go 包级全局变量；`InitMetricsVars` 要求父向量已由 `pkg/metrics` 或
// `init_parent_metrics` 初始化。忽略类计数覆盖 SQL/Plan/RU 超额与通道满等场景。

#![allow(non_snake_case, non_upper_case_globals, static_mut_refs)]

use crate::metrics;

// Go 的 prometheus.Counter/Observer 是可重新绑定的全局指标句柄。
/// 忽略：规范化 SQL 条目数超限。
pub static mut IgnoreExceedSQLCounter: Option<prometheus::Counter> = None;
/// 忽略：规范化 Plan 条目数超限。
pub static mut IgnoreExceedPlanCounter: Option<prometheus::Counter> = None;
/// 忽略：RU key 数超限。
pub static mut IgnoreExceedRUKeysCounter: Option<prometheus::Counter> = None;
/// 忽略：RU 总量超限。
pub static mut IgnoreExceedRUTotalCounter: Option<prometheus::Counter> = None;
/// 忽略：迟到的已压缩 RU key。
pub static mut IgnoreLateCompactedRUKeysCounter: Option<prometheus::Counter> = None;
/// 忽略：迟到的已压缩 RU 总量。
pub static mut IgnoreLateCompactedRUTotalCounter: Option<prometheus::Counter> = None;
/// 忽略：采集通道已满。
pub static mut IgnoreCollectChannelFullCounter: Option<prometheus::Counter> = None;
/// 忽略：语句采集通道已满。
pub static mut IgnoreCollectStmtChannelFullCounter: Option<prometheus::Counter> = None;
/// 忽略：RU 采集通道已满。
pub static mut IgnoreCollectRUChannelFullCounter: Option<prometheus::Counter> = None;
/// 忽略：上报通道已满。
pub static mut IgnoreReportChannelFullCounter: Option<prometheus::Counter> = None;
/// 忽略：背压导致整批报告数据丢弃。
pub static mut IgnoreReportDataByBackpressureCounter: Option<prometheus::Counter> = None;
/// 整批上报成功耗时。
pub static mut ReportAllDurationSuccHistogram: Option<prometheus::Histogram> = None;
/// 整批上报失败耗时。
pub static mut ReportAllDurationFailedHistogram: Option<prometheus::Histogram> = None;
/// 记录上报成功耗时。
pub static mut ReportRecordDurationSuccHistogram: Option<prometheus::Histogram> = None;
/// 记录上报失败耗时。
pub static mut ReportRecordDurationFailedHistogram: Option<prometheus::Histogram> = None;
/// SQL 元数据上报成功耗时。
pub static mut ReportSQLDurationSuccHistogram: Option<prometheus::Histogram> = None;
/// SQL 元数据上报失败耗时。
pub static mut ReportSQLDurationFailedHistogram: Option<prometheus::Histogram> = None;
/// Plan 元数据上报成功耗时。
pub static mut ReportPlanDurationSuccHistogram: Option<prometheus::Histogram> = None;
/// Plan 元数据上报失败耗时。
pub static mut ReportPlanDurationFailedHistogram: Option<prometheus::Histogram> = None;
/// 上报记录条数直方图。
pub static mut TopSQLReportRecordCounterHistogram: Option<prometheus::Histogram> = None;
/// 上报 RU 记录条数直方图。
pub static mut TopSQLReportRURecordCounterHistogram: Option<prometheus::Histogram> = None;
/// 上报 SQL 元数据条数直方图。
pub static mut TopSQLReportSQLCountHistogram: Option<prometheus::Histogram> = None;
/// 上报 Plan 元数据条数直方图。
pub static mut TopSQLReportPlanCountHistogram: Option<prometheus::Histogram> = None;
/// RU 记录上报成功耗时。
pub static mut ReportRURecordDurationSuccHistogram: Option<prometheus::Histogram> = None;
/// RU 记录上报失败耗时。
pub static mut ReportRURecordDurationFailedHistogram: Option<prometheus::Histogram> = None;

// Rust 模块接入层显式调用该入口，以保持 Go 包初始化顺序。
/// 模块入口：转发到 `InitMetricsVars`。
pub fn init() {
    InitMetricsVars();
}

// InitMetricsVars 对应 Go 的标签绑定逻辑；父向量由 pkg/metrics 初始化。
/// 从父向量 `with_label_values` 绑定全部子句柄。
pub fn InitMetricsVars() {
    unsafe {
        let ignored = metrics::TopSQLIgnoredCounter
            .as_ref()
            .expect("pkg/metrics TopSQLIgnoredCounter must be initialized first");
        // 忽略类：按 type 标签切出独立 Counter。
        IgnoreExceedSQLCounter = Some(ignored.with_label_values(&["ignore_exceed_sql"]));
        IgnoreExceedPlanCounter = Some(ignored.with_label_values(&["ignore_exceed_plan"]));
        IgnoreExceedRUKeysCounter = Some(ignored.with_label_values(&["ignore_exceed_ru_keys"]));
        IgnoreExceedRUTotalCounter = Some(ignored.with_label_values(&["ignore_exceed_ru_total"]));
        IgnoreLateCompactedRUKeysCounter =
            Some(ignored.with_label_values(&["ignore_late_compacted_ru_keys"]));
        IgnoreLateCompactedRUTotalCounter =
            Some(ignored.with_label_values(&["ignore_late_compacted_ru_total"]));
        IgnoreCollectChannelFullCounter =
            Some(ignored.with_label_values(&["ignore_collect_channel_full"]));
        IgnoreCollectStmtChannelFullCounter =
            Some(ignored.with_label_values(&["ignore_collect_stmt_channel_full"]));
        IgnoreCollectRUChannelFullCounter =
            Some(ignored.with_label_values(&["ignore_collect_ru_channel_full"]));
        IgnoreReportChannelFullCounter =
            Some(ignored.with_label_values(&["ignore_report_channel_full"]));
        IgnoreReportDataByBackpressureCounter =
            Some(ignored.with_label_values(&["ignore_report_data_by_backpressure"]));

        let duration = metrics::TopSQLReportDurationHistogram
            .as_ref()
            .expect("pkg/metrics TopSQLReportDurationHistogram must be initialized first");
        // 耗时类：type × result(ok/error)。
        ReportAllDurationSuccHistogram = Some(duration.with_label_values(&["all", metrics::LblOK]));
        ReportAllDurationFailedHistogram =
            Some(duration.with_label_values(&["all", metrics::LblError]));
        ReportRecordDurationSuccHistogram =
            Some(duration.with_label_values(&["record", metrics::LblOK]));
        ReportRecordDurationFailedHistogram =
            Some(duration.with_label_values(&["record", metrics::LblError]));
        ReportSQLDurationSuccHistogram = Some(duration.with_label_values(&["sql", metrics::LblOK]));
        ReportSQLDurationFailedHistogram =
            Some(duration.with_label_values(&["sql", metrics::LblError]));
        ReportPlanDurationSuccHistogram =
            Some(duration.with_label_values(&["plan", metrics::LblOK]));
        ReportPlanDurationFailedHistogram =
            Some(duration.with_label_values(&["plan", metrics::LblError]));

        let data = metrics::TopSQLReportDataHistogram
            .as_ref()
            .expect("pkg/metrics TopSQLReportDataHistogram must be initialized first");
        // 数据量类：按上报对象类型切分。
        TopSQLReportRecordCounterHistogram = Some(data.with_label_values(&["record"]));
        TopSQLReportRURecordCounterHistogram = Some(data.with_label_values(&["ru_record"]));
        TopSQLReportSQLCountHistogram = Some(data.with_label_values(&["sql"]));
        TopSQLReportPlanCountHistogram = Some(data.with_label_values(&["plan"]));

        ReportRURecordDurationSuccHistogram =
            Some(duration.with_label_values(&["ru_record", metrics::LblOK]));
        ReportRURecordDurationFailedHistogram =
            Some(duration.with_label_values(&["ru_record", metrics::LblError]));
    }
}
