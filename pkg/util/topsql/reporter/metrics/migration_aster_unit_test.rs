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

// AsterSQL 迁移补充：reporter 指标标签绑定与父序列共享性测试。
//
// 用互斥锁串行化，避免静态全局指标在并行测例下互相污染；验证 ignored/
// duration/data 三类句柄的标签与 Go 一致，且写入落到同一父向量 series。

use std::sync::Mutex;
use topsql_reporter_metrics::{metrics, reporter_metrics};

/// 串行化指标相关测例，保护静态全局句柄。
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// 忽略计数句柄标签齐全，且与父 CounterVec 共享 series。
#[test]
fn ignored_counter_handles_match_all_go_labels_and_share_parent_series() {
    let _guard = TEST_LOCK.lock().unwrap();
    metrics::init_parent_metrics();
    reporter_metrics::InitMetricsVars();

    let cases = unsafe {
        [
            (
                "ignore_exceed_sql",
                reporter_metrics::IgnoreExceedSQLCounter.as_ref().unwrap(),
            ),
            (
                "ignore_exceed_plan",
                reporter_metrics::IgnoreExceedPlanCounter.as_ref().unwrap(),
            ),
            (
                "ignore_exceed_ru_keys",
                reporter_metrics::IgnoreExceedRUKeysCounter
                    .as_ref()
                    .unwrap(),
            ),
            (
                "ignore_exceed_ru_total",
                reporter_metrics::IgnoreExceedRUTotalCounter
                    .as_ref()
                    .unwrap(),
            ),
            (
                "ignore_late_compacted_ru_keys",
                reporter_metrics::IgnoreLateCompactedRUKeysCounter
                    .as_ref()
                    .unwrap(),
            ),
            (
                "ignore_late_compacted_ru_total",
                reporter_metrics::IgnoreLateCompactedRUTotalCounter
                    .as_ref()
                    .unwrap(),
            ),
            (
                "ignore_collect_channel_full",
                reporter_metrics::IgnoreCollectChannelFullCounter
                    .as_ref()
                    .unwrap(),
            ),
            (
                "ignore_collect_stmt_channel_full",
                reporter_metrics::IgnoreCollectStmtChannelFullCounter
                    .as_ref()
                    .unwrap(),
            ),
            (
                "ignore_collect_ru_channel_full",
                reporter_metrics::IgnoreCollectRUChannelFullCounter
                    .as_ref()
                    .unwrap(),
            ),
            (
                "ignore_report_channel_full",
                reporter_metrics::IgnoreReportChannelFullCounter
                    .as_ref()
                    .unwrap(),
            ),
        ]
    };

    // 子句柄 inc 应反映到父向量同标签 series。
    for (index, (label, handle)) in cases.into_iter().enumerate() {
        handle.inc_by((index + 1) as f64);
        let parent = unsafe { metrics::TopSQLIgnoredCounter.as_ref().unwrap() };
        assert_eq!(parent.with_label_values(&[label]).get(), (index + 1) as f64);
    }
}

/// 耗时直方图 type/result 标签与 Go 完全对齐。
#[test]
fn duration_handles_match_go_type_and_result_labels() {
    let _guard = TEST_LOCK.lock().unwrap();
    metrics::init_parent_metrics();
    reporter_metrics::init();

    let cases = unsafe {
        [
            (
                ("all", metrics::LblOK),
                reporter_metrics::ReportAllDurationSuccHistogram
                    .as_ref()
                    .unwrap(),
            ),
            (
                ("all", metrics::LblError),
                reporter_metrics::ReportAllDurationFailedHistogram
                    .as_ref()
                    .unwrap(),
            ),
            (
                ("record", metrics::LblOK),
                reporter_metrics::ReportRecordDurationSuccHistogram
                    .as_ref()
                    .unwrap(),
            ),
            (
                ("record", metrics::LblError),
                reporter_metrics::ReportRecordDurationFailedHistogram
                    .as_ref()
                    .unwrap(),
            ),
            (
                ("sql", metrics::LblOK),
                reporter_metrics::ReportSQLDurationSuccHistogram
                    .as_ref()
                    .unwrap(),
            ),
            (
                ("sql", metrics::LblError),
                reporter_metrics::ReportSQLDurationFailedHistogram
                    .as_ref()
                    .unwrap(),
            ),
            (
                ("plan", metrics::LblOK),
                reporter_metrics::ReportPlanDurationSuccHistogram
                    .as_ref()
                    .unwrap(),
            ),
            (
                ("plan", metrics::LblError),
                reporter_metrics::ReportPlanDurationFailedHistogram
                    .as_ref()
                    .unwrap(),
            ),
            (
                ("ru_record", metrics::LblOK),
                reporter_metrics::ReportRURecordDurationSuccHistogram
                    .as_ref()
                    .unwrap(),
            ),
            (
                ("ru_record", metrics::LblError),
                reporter_metrics::ReportRURecordDurationFailedHistogram
                    .as_ref()
                    .unwrap(),
            ),
        ]
    };

    for ((kind, result), handle) in cases {
        handle.observe(0.25);
        let parent = unsafe { metrics::TopSQLReportDurationHistogram.as_ref().unwrap() };
        assert_eq!(
            parent.with_label_values(&[kind, result]).get_sample_count(),
            1
        );
    }
}

/// 数据量直方图覆盖 record / ru_record / sql / plan 四类。
#[test]
fn data_histogram_handles_cover_record_ru_sql_and_plan() {
    let _guard = TEST_LOCK.lock().unwrap();
    metrics::init_parent_metrics();
    reporter_metrics::InitMetricsVars();

    let cases = unsafe {
        [
            (
                "record",
                reporter_metrics::TopSQLReportRecordCounterHistogram
                    .as_ref()
                    .unwrap(),
            ),
            (
                "ru_record",
                reporter_metrics::TopSQLReportRURecordCounterHistogram
                    .as_ref()
                    .unwrap(),
            ),
            (
                "sql",
                reporter_metrics::TopSQLReportSQLCountHistogram
                    .as_ref()
                    .unwrap(),
            ),
            (
                "plan",
                reporter_metrics::TopSQLReportPlanCountHistogram
                    .as_ref()
                    .unwrap(),
            ),
        ]
    };

    for (index, (label, handle)) in cases.into_iter().enumerate() {
        handle.observe((index + 1) as f64);
        let parent = unsafe { metrics::TopSQLReportDataHistogram.as_ref().unwrap() };
        let metric = parent.with_label_values(&[label]);
        assert_eq!(metric.get_sample_count(), 1);
        assert_eq!(metric.get_sample_sum(), (index + 1) as f64);
    }
}
