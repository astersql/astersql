// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// metricscommon 迁移对照测试：常量标签大小写、合并优先级与各构造器注入行为。

use std::collections::HashMap;

use super::{
    CONST_LABELS_TEST_LOCK, GetConstLabels, GetMergedConstLabels, NewCounter, NewCounterVec,
    NewDesc, NewGauge, NewGaugeVec, NewHistogram, NewHistogramVec, NewSummaryVec, SetConstLabels,
};
use prometheus::core::Collector;
use prometheus::{HistogramOpts, Opts};

/// 由键值对切片构造标签映射。
fn labels(entries: &[(&str, &str)]) -> HashMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

/// 从 Collector 首个样本提取标签映射。
fn collected_labels(collector: &dyn Collector) -> HashMap<String, String> {
    collector.collect()[0].get_metric()[0]
        .get_label()
        .iter()
        .map(|pair| (pair.name().to_owned(), pair.value().to_owned()))
        .collect()
}

/// 测试结束时恢复进入前的包级常量标签，对齐 Go 测试的 `t.Cleanup`。
struct ConstLabelsGuard(HashMap<String, String>);

impl Drop for ConstLabelsGuard {
    fn drop(&mut self) {
        let kv = self
            .0
            .iter()
            .flat_map(|(key, value)| [key.clone(), value.clone()])
            .collect::<Vec<_>>();
        SetConstLabels(&kv);
    }
}

/// 常量标签键转小写，且全局值覆盖调用方同名标签。
#[test]
fn const_labels_are_lowercased_and_override_input_labels() {
    let _guard = CONST_LABELS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _restore = ConstLabelsGuard(GetConstLabels());
    SetConstLabels(&["Cluster".into(), "east".into(), "NODE".into(), "n1".into()]);

    assert_eq!(
        GetConstLabels(),
        labels(&[("cluster", "east"), ("node", "n1")])
    );
    assert_eq!(
        GetMergedConstLabels(labels(&[("cluster", "west"), ("tenant", "t1")])),
        labels(&[("cluster", "east"), ("node", "n1"), ("tenant", "t1")])
    );
}

/// 奇数个参数时 `SetConstLabels` 应 panic，对齐 Go。
#[test]
fn odd_const_label_arguments_panic_like_go() {
    let _guard = CONST_LABELS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _restore = ConstLabelsGuard(GetConstLabels());
    let panic = std::panic::catch_unwind(|| SetConstLabels(&["key".into()])).unwrap_err();
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied());
    assert_eq!(
        message,
        Some("got the odd number of inputs for const labels: 1")
    );
}

/// 标量与向量 Counter/Gauge 构造后，采集到的常量标签应为包级全局值。
#[test]
fn scalar_and_vector_constructors_inject_const_labels() {
    let _guard = CONST_LABELS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _restore = ConstLabelsGuard(GetConstLabels());
    SetConstLabels(&["cluster".into(), "east".into()]);

    let counter =
        NewCounter(Opts::new("requests_total", "requests").const_label("cluster", "caller-value"));
    counter.inc();
    assert_eq!(collected_labels(&counter), labels(&[("cluster", "east")]));

    let counter_vec = NewCounterVec(
        Opts::new("requests_by_method_total", "requests by method"),
        &["method".into()],
    );
    let counter_child = counter_vec.with_label_values(&["get"]);
    counter_child.inc();
    assert_eq!(
        collected_labels(&counter_vec),
        labels(&[("cluster", "east"), ("method", "get")])
    );

    let gauge = NewGauge(Opts::new("workers", "workers"));
    gauge.set(2.0);
    assert_eq!(collected_labels(&gauge), labels(&[("cluster", "east")]));

    let gauge_vec = NewGaugeVec(
        Opts::new("workers_by_pool", "workers by pool"),
        &["pool".into()],
    );
    gauge_vec.with_label_values(&["ddl"]).set(3.0);
    assert_eq!(
        collected_labels(&gauge_vec),
        labels(&[("cluster", "east"), ("pool", "ddl")])
    );
}

/// Histogram / SummaryVec（降级为 HistogramVec）可记录观测值。
#[test]
fn histogram_and_summary_fallback_collect_observations() {
    let _guard = CONST_LABELS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _restore = ConstLabelsGuard(GetConstLabels());
    SetConstLabels(&["cluster".into(), "east".into()]);

    let histogram = NewHistogram(HistogramOpts::new("latency_seconds", "latency"));
    histogram.observe(0.5);
    assert_eq!(histogram.get_sample_count(), 1);
    assert_eq!(histogram.get_sample_sum(), 0.5);
    assert_eq!(collected_labels(&histogram), labels(&[("cluster", "east")]));

    let histogram_vec = NewHistogramVec(
        HistogramOpts::new("latency_by_kind_seconds", "latency by kind"),
        &["kind".into()],
    );
    let child = histogram_vec.with_label_values(&["read"]);
    child.observe(0.25);
    assert_eq!(child.get_sample_count(), 1);
    assert_eq!(child.get_sample_sum(), 0.25);
    assert_eq!(
        collected_labels(&histogram_vec),
        labels(&[("cluster", "east"), ("kind", "read")])
    );

    // prometheus 0.14 has no Summary collector. The compatibility wrapper keeps
    // observations and dimensions as a HistogramVec instead of blocking migration.
    // Rust prometheus 无 Summary：兼容包装用 HistogramVec 保留观测与维度。
    let summary = NewSummaryVec(
        HistogramOpts::new("phase_duration_seconds", "phase duration"),
        &["phase".into()],
    );
    let summary_child = summary.with_label_values(&["build"]);
    summary_child.observe(0.75);
    assert_eq!(summary_child.get_sample_count(), 1);
    assert_eq!(summary_child.get_sample_sum(), 0.75);
    assert_eq!(
        collected_labels(&summary),
        labels(&[("cluster", "east"), ("phase", "build")])
    );
}

/// `NewDesc` 合并全局与调用方常量标签，全局键优先。
#[test]
fn descriptor_merges_global_labels_with_global_priority() {
    let _guard = CONST_LABELS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _restore = ConstLabelsGuard(GetConstLabels());
    SetConstLabels(&["cluster".into(), "east".into()]);

    let desc = NewDesc(
        "tidb_jobs_total",
        "jobs",
        &["state".into()],
        labels(&[("cluster", "west"), ("tenant", "t1")]),
    )
    .unwrap();

    assert_eq!(desc.fq_name, "tidb_jobs_total");
    assert_eq!(desc.help, "jobs");
    assert_eq!(desc.variable_labels, vec!["state"]);
    let actual: HashMap<_, _> = desc
        .const_label_pairs
        .iter()
        .map(|pair| (pair.name().to_owned(), pair.value().to_owned()))
        .collect();
    assert_eq!(actual, labels(&[("cluster", "east"), ("tenant", "t1")]));
}
