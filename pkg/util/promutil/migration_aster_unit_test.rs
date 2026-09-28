// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// promutil 迁移单测：默认工厂指标形状、非法 buckets panic、Registry 语义。
//
// 对照 Go：DefaultFactory 可创建全部指标类型；Noop 允许重复注册；Default 拒绝重复。

use super::{NewDefaultFactory, NewDefaultRegistry, NewNoopRegistry};
use prometheus::{HistogramOpts, Opts as CounterOpts, Opts as GaugeOpts};

/// 默认工厂应能创建 Counter/Gauge/Histogram 及其 Vec，并正确读写采样。
#[test]
fn default_factory_creates_all_go_metric_shapes() {
    let factory = NewDefaultFactory();

    let counter = factory.NewCounter(CounterOpts::new("requests_total", "requests"));
    counter.inc_by(2.0);
    assert_eq!(counter.get(), 2.0);

    let counter_vec = factory.NewCounterVec(
        CounterOpts::new("requests_by_method_total", "requests by method"),
        vec!["method".to_owned()],
    );
    counter_vec.with_label_values(&["get"]).inc();
    assert_eq!(counter_vec.with_label_values(&["get"]).get(), 1.0);

    let gauge = factory.NewGauge(GaugeOpts::new("workers", "workers"));
    gauge.set(3.0);
    assert_eq!(gauge.get(), 3.0);

    let gauge_vec = factory.NewGaugeVec(
        GaugeOpts::new("workers_by_pool", "workers by pool"),
        vec!["pool".to_owned()],
    );
    gauge_vec.with_label_values(&["ddl"]).set(4.0);
    assert_eq!(gauge_vec.with_label_values(&["ddl"]).get(), 4.0);

    let histogram = factory.NewHistogram(
        HistogramOpts::new("request_seconds", "request duration").buckets(vec![0.1, 1.0]),
    );
    histogram.observe(0.5);
    assert_eq!(histogram.get_sample_count(), 1);

    let histogram_vec = factory.NewHistogramVec(
        HistogramOpts::new("request_seconds_by_method", "request duration by method"),
        vec!["method".to_owned()],
    );
    histogram_vec.with_label_values(&["get"]).observe(0.2);
    assert_eq!(
        histogram_vec.with_label_values(&["get"]).get_sample_count(),
        1
    );
}

/// buckets 非严格递增时 NewHistogram 应 panic（与 Go 一致）。
#[test]
fn histogram_factory_panics_for_non_increasing_buckets_like_go() {
    let factory = NewDefaultFactory();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        factory.NewHistogram(
            HistogramOpts::new("invalid_bucket_order", "invalid buckets").buckets(vec![1.0, 0.5]),
        )
    }));
    assert!(result.is_err());
}

/// NoopRegistry：重复 Register/MustRegister 成功，Unregister 恒为真。
#[test]
fn noop_registry_accepts_duplicates_and_always_unregisters() {
    let registry = NewNoopRegistry();
    let counter = prometheus::Counter::with_opts(CounterOpts::new("noop_total", "noop"))
        .expect("valid counter");

    assert!(registry.Register(Box::new(counter.clone())).is_ok());
    assert!(registry.Register(Box::new(counter.clone())).is_ok());
    registry.MustRegister(vec![Box::new(counter.clone()), Box::new(counter.clone())]);
    assert!(registry.Unregister(Box::new(counter)));
}

/// DefaultRegistry：第二次 Register 失败；Unregister 后再次 Unregister 为假。
#[test]
fn default_registry_uses_prometheus_duplicate_and_unregister_semantics() {
    let registry = NewDefaultRegistry();
    let counter = prometheus::Counter::with_opts(CounterOpts::new("default_total", "default"))
        .expect("valid counter");

    assert!(registry.Register(Box::new(counter.clone())).is_ok());
    assert!(registry.Register(Box::new(counter.clone())).is_err());
    assert!(registry.Unregister(Box::new(counter.clone())));
    assert!(!registry.Unregister(Box::new(counter)));
}
