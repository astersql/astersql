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

// `metric` 单元测试：计数/直方图读取、引擎记录、注册注销与上下文注入。

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

/// 校验 `read_counter` 累加结果。
#[test]
fn test_read_counter() {
    let counter = prometheus::Counter::new("task199_counter", "counter").unwrap();
    counter.inc_by(1256.0);
    counter.inc_by(2214.0);
    assert_eq!(3470.0, metric::read_counter(&counter));
}

/// 校验 `read_histogram_sum` 对多次 observe 求和。
#[test]
fn test_read_histogram_sum() {
    let histogram = prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
        "task199_histogram",
        "histogram",
    ))
    .unwrap();
    histogram.observe(11131.5);
    histogram.observe(15261.0);
    assert_eq!(26392.5, metric::read_histogram_sum(&histogram));
}

/// 无错误记 success、有错误记 failure，各为 1。
#[test]
fn test_record_engine_count() {
    let factory = promutil::NewDefaultFactory();
    let m = metric::new_metrics(factory.as_ref());
    m.record_engine_count("table1", None);
    m.record_engine_count("table1", Some(&std::io::Error::other("mock error")));
    let success_counter = m
        .processed_engine_counter
        .get_metric_with_label_values(&["table1", "success"])
        .unwrap();
    assert_eq!(1.0, metric::read_counter(&success_counter));
    let failure_counter = m
        .processed_engine_counter
        .get_metric_with_label_values(&["table1", "failure"])
        .unwrap();
    assert_eq!(1.0, metric::read_counter(&failure_counter));
}

/// Common 注册 8 个、Metrics 注册 22 个；再逐个 Unregister 清空。
#[test]
fn test_metrics_register() {
    let factory = promutil::NewDefaultFactory();
    let cm = metric::new_common(factory.as_ref(), "test", "", Default::default());
    let r = TestRegistry::new();
    assert_eq!(0, r.metric_count());
    cm.register_to(&r);
    assert_eq!(8, r.metric_count());
    cm.unregister_from(&r);
    assert_eq!(0, r.metric_count());

    let m = metric::new_metrics(factory.as_ref());
    let r = TestRegistry::new();
    m.register_to(&r);
    assert_eq!(22, r.metric_count());
    // 以下逐个注销，确认每个收集器均可独立移除
    assert!(promutil::Registry::Unregister(
        &r,
        Box::new(m.importer_engine_counter.clone())
    ));
    assert!(promutil::Registry::Unregister(
        &r,
        Box::new(m.idle_workers_gauge.clone())
    ));
    assert!(promutil::Registry::Unregister(
        &r,
        Box::new(m.kv_encoder_counter.clone())
    ));
    assert!(promutil::Registry::Unregister(
        &r,
        Box::new(m.table_counter.clone())
    ));
    assert!(promutil::Registry::Unregister(
        &r,
        Box::new(m.processed_engine_counter.clone())
    ));
    assert!(promutil::Registry::Unregister(
        &r,
        Box::new(m.common.chunk_counter.clone())
    ));
    assert!(promutil::Registry::Unregister(
        &r,
        Box::new(m.common.bytes_counter.clone())
    ));
    assert!(promutil::Registry::Unregister(
        &r,
        Box::new(m.common.rows_counter.clone())
    ));
    assert!(promutil::Registry::Unregister(
        &r,
        Box::new(m.import_seconds_histogram.clone())
    ));
    assert!(promutil::Registry::Unregister(
        &r,
        Box::new(m.chunk_parser_read_block_seconds_histogram.clone())
    ));
    assert!(promutil::Registry::Unregister(
        &r,
        Box::new(m.apply_worker_seconds_histogram.clone())
    ));
    assert!(promutil::Registry::Unregister(
        &r,
        Box::new(m.common.row_read_seconds_histogram.clone())
    ));
    assert!(promutil::Registry::Unregister(
        &r,
        Box::new(m.row_read_bytes_histogram.clone())
    ));
    assert!(promutil::Registry::Unregister(
        &r,
        Box::new(m.common.row_encode_seconds_histogram.clone())
    ));
    assert!(promutil::Registry::Unregister(
        &r,
        Box::new(m.row_kv_deliver_seconds_histogram.clone())
    ));
    assert!(promutil::Registry::Unregister(
        &r,
        Box::new(m.common.block_deliver_seconds_histogram.clone())
    ));
    assert!(promutil::Registry::Unregister(
        &r,
        Box::new(m.common.block_deliver_bytes_histogram.clone())
    ));
    assert!(promutil::Registry::Unregister(
        &r,
        Box::new(m.common.block_deliver_kv_pairs_histogram.clone())
    ));
    assert!(promutil::Registry::Unregister(
        &r,
        Box::new(m.checksum_seconds_histogram.clone())
    ));
    assert!(promutil::Registry::Unregister(
        &r,
        Box::new(m.sst_seconds_histogram.clone())
    ));
    assert!(promutil::Registry::Unregister(
        &r,
        Box::new(m.local_storage_usage_bytes_gauge.clone())
    ));
    assert!(promutil::Registry::Unregister(
        &r,
        Box::new(m.progress_gauge.clone())
    ));
    assert_eq!(0, r.metric_count());
}

/// `unregister_from` 后再次 Unregister 同一收集器应返回 false。
#[test]
fn test_metrics_unregister() {
    let factory = promutil::NewDefaultFactory();
    let m = metric::new_metrics(factory.as_ref());
    let r = TestRegistry::new();
    m.register_to(&r);
    m.unregister_from(&r);
    assert!(!promutil::Registry::Unregister(
        &r,
        Box::new(m.importer_engine_counter.clone())
    ));
    assert!(!promutil::Registry::Unregister(
        &r,
        Box::new(m.idle_workers_gauge.clone())
    ));
    assert!(!promutil::Registry::Unregister(
        &r,
        Box::new(m.kv_encoder_counter.clone())
    ));
    assert!(!promutil::Registry::Unregister(
        &r,
        Box::new(m.table_counter.clone())
    ));
    assert!(!promutil::Registry::Unregister(
        &r,
        Box::new(m.processed_engine_counter.clone())
    ));
    assert!(!promutil::Registry::Unregister(
        &r,
        Box::new(m.common.chunk_counter.clone())
    ));
    assert!(!promutil::Registry::Unregister(
        &r,
        Box::new(m.common.bytes_counter.clone())
    ));
    assert!(!promutil::Registry::Unregister(
        &r,
        Box::new(m.common.rows_counter.clone())
    ));
    assert!(!promutil::Registry::Unregister(
        &r,
        Box::new(m.import_seconds_histogram.clone())
    ));
    assert!(!promutil::Registry::Unregister(
        &r,
        Box::new(m.chunk_parser_read_block_seconds_histogram.clone())
    ));
    assert!(!promutil::Registry::Unregister(
        &r,
        Box::new(m.apply_worker_seconds_histogram.clone())
    ));
    assert!(!promutil::Registry::Unregister(
        &r,
        Box::new(m.common.row_read_seconds_histogram.clone())
    ));
    assert!(!promutil::Registry::Unregister(
        &r,
        Box::new(m.row_read_bytes_histogram.clone())
    ));
    assert!(!promutil::Registry::Unregister(
        &r,
        Box::new(m.common.row_encode_seconds_histogram.clone())
    ));
    assert!(!promutil::Registry::Unregister(
        &r,
        Box::new(m.row_kv_deliver_seconds_histogram.clone())
    ));
    assert!(!promutil::Registry::Unregister(
        &r,
        Box::new(m.common.block_deliver_seconds_histogram.clone())
    ));
    assert!(!promutil::Registry::Unregister(
        &r,
        Box::new(m.common.block_deliver_bytes_histogram.clone())
    ));
    assert!(!promutil::Registry::Unregister(
        &r,
        Box::new(m.common.block_deliver_kv_pairs_histogram.clone())
    ));
    assert!(!promutil::Registry::Unregister(
        &r,
        Box::new(m.checksum_seconds_histogram.clone())
    ));
    assert!(!promutil::Registry::Unregister(
        &r,
        Box::new(m.sst_seconds_histogram.clone())
    ));
    assert!(!promutil::Registry::Unregister(
        &r,
        Box::new(m.local_storage_usage_bytes_gauge.clone())
    ));
    assert!(!promutil::Registry::Unregister(
        &r,
        Box::new(m.progress_gauge.clone())
    ));
}

/// `with_metric` 同时暴露 Metrics 与 Common；`with_common_metric` 仅有 Common。
#[test]
fn test_context() {
    let factory = promutil::NewDefaultFactory();
    let metrics = Arc::new(metric::new_metrics(factory.as_ref()));
    let ctx = metric::with_metric(metric::MetricContext::background(), metrics.clone());
    assert!(Arc::ptr_eq(metric::from_context(&ctx).unwrap(), &metrics));
    assert!(Arc::ptr_eq(
        metric::get_common_metric(&ctx).unwrap(),
        &metrics.common
    ));

    let ctx =
        metric::with_common_metric(metric::MetricContext::background(), metrics.common.clone());
    assert!(metric::from_context(&ctx).is_none());
    assert!(metric::get_common_metric(&ctx).is_some());
}
