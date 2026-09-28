// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// metric 迁移对齐测试：对照 Go 快照校验读取、标签、注册与上下文语义。

use astersql_lightning_metric::{metric, promutil};
use prometheus::core::Collector;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 测试用 Registry：包装 prometheus::Registry 并统计已注册收集器数量。
struct TestRegistry {
    inner: prometheus::Registry,
    registered: AtomicUsize,
}

impl TestRegistry {
    /// 创建空注册表。
    fn new() -> Self {
        Self {
            inner: prometheus::Registry::new(),
            registered: AtomicUsize::new(0),
        }
    }

    /// 返回当前已注册收集器个数。
    fn metric_count(&self) -> usize {
        self.registered.load(Ordering::SeqCst)
    }
}

impl promutil::Registry for TestRegistry {
    fn Register(&self, collector: Box<dyn Collector>) -> prometheus::Result<()> {
        self.inner.register(collector)?;
        self.registered.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn MustRegister(&self, collectors: Vec<Box<dyn Collector>>) {
        for collector in collectors {
            self.inner
                .register(collector)
                .expect("metric registration failed");
            self.registered.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn Unregister(&self, collector: Box<dyn Collector>) -> bool {
        if self.inner.unregister(collector).is_ok() {
            self.registered.fetch_sub(1, Ordering::SeqCst);
            true
        } else {
            false
        }
    }
}

/// 计数与直方图求和/样本数对齐 Go 测试快照。
#[test]
fn read_counter_and_histogram_match_go_snapshots() {
    let counter = prometheus::Counter::new("task170_counter", "counter").unwrap();
    counter.inc_by(1256.0);
    counter.inc_by(2214.0);
    assert_eq!(metric::read_counter(&counter), 3470.0);

    let histogram = prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
        "task170_histogram",
        "histogram",
    ))
    .unwrap();
    histogram.observe(11131.5);
    histogram.observe(15261.0);
    assert_eq!(metric::read_histogram_sum(&histogram), 26392.5);
    let snapshot = metric::read_histogram(&histogram).unwrap();
    assert_eq!(snapshot.histogram.as_ref().unwrap().sample_count(), 2);
}

/// `record_*` 按 error 有无选择 success/failure 标签，并用 `read_all_counters` 聚合。
#[test]
fn record_counts_select_success_and_failure_labels() {
    let factory = promutil::NewDefaultFactory();
    let metrics = metric::new_metrics(factory.as_ref());
    metrics.record_engine_count("table1", None);
    metrics.record_engine_count("table1", Some(&std::io::Error::other("mock error")));
    metrics.record_table_count(metric::TABLE_STATE_COMPLETED, None);

    let success = metrics
        .processed_engine_counter
        .get_metric_with_label_values(&["table1", metric::TABLE_RESULT_SUCCESS])
        .unwrap();
    let failure = metrics
        .processed_engine_counter
        .get_metric_with_label_values(&["table1", metric::TABLE_RESULT_FAILURE])
        .unwrap();
    assert_eq!(metric::read_counter(&success), 1.0);
    assert_eq!(metric::read_counter(&failure), 1.0);
    assert_eq!(
        metric::read_all_counters(
            &metrics.table_counter,
            &[("state".to_owned(), metric::TABLE_STATE_COMPLETED.to_owned())]
                .into_iter()
                .collect(),
        ),
        1.0
    );
}

/// Common 8 个、Metrics 22 个收集器的注册与整批注销对齐 Go。
#[test]
fn registration_and_unregistration_cover_all_go_metrics() {
    let factory = promutil::NewDefaultFactory();
    let common = metric::new_common(factory.as_ref(), "test", "", Default::default());
    let registry = TestRegistry::new();
    assert_eq!(registry.metric_count(), 0);
    common.register_to(&registry);
    assert_eq!(registry.metric_count(), 8);
    common.unregister_from(&registry);
    assert_eq!(registry.metric_count(), 0);

    let metrics = metric::new_metrics(factory.as_ref());
    metrics.register_to(&registry);
    assert_eq!(registry.metric_count(), 22);
    metrics.unregister_from(&registry);
    assert_eq!(registry.metric_count(), 0);
}

/// `read_all_counters` 为「任意标签命中」语义：匹配 state 或 table 任一即计入。
#[test]
fn read_all_counters_preserves_go_any_label_match_semantics() {
    let factory = promutil::NewDefaultFactory();
    let common = metric::new_common(factory.as_ref(), "test", "", Default::default());
    common
        .rows_counter
        .with_label_values(&["finished", "a"])
        .inc_by(2.0);
    common
        .rows_counter
        .with_label_values(&["failed", "a"])
        .inc_by(3.0);
    common
        .rows_counter
        .with_label_values(&["finished", "b"])
        .inc_by(5.0);

    // 过滤同时含 finished 与 a：按 Go any-match，finished/a(2)+failed/a(3)+finished/b(5)=10
    let labels = [
        ("state".to_owned(), "finished".to_owned()),
        ("table".to_owned(), "a".to_owned()),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        metric::read_all_counters(&common.rows_counter, &labels),
        10.0
    );
}

/// 完整 Metrics 与仅 Common 两种上下文注入方式对齐 Go。
#[test]
fn metric_context_exposes_full_and_common_metrics_like_go_context() {
    let factory = promutil::NewDefaultFactory();
    let metrics = Arc::new(metric::new_metrics(factory.as_ref()));
    let ctx = metric::with_metric(metric::MetricContext::background(), metrics.clone());
    assert!(Arc::ptr_eq(metric::from_context(&ctx).unwrap(), &metrics));
    assert!(Arc::ptr_eq(
        metric::get_common_metric(&ctx).unwrap(),
        &metrics.common
    ));

    let common_only =
        metric::with_common_metric(metric::MetricContext::background(), metrics.common.clone());
    assert!(metric::from_context(&common_only).is_none());
    assert!(metric::get_common_metric(&common_only).is_some());
}
