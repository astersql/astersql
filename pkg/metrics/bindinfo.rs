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

// Bindinfo（执行计划绑定）相关 Prometheus 指标与 Go 兼容适配层。
//
// Binding 将特定 SQL 形态绑定到指定执行计划（optimizer hint / plan），缓存命中与内存占用
// 通过本模块指标观测。`compat_prometheus` / `compat_metricscommon` 保留 Go 风格构造器命名，
// 便于迁移单元继续调用。

use std::sync::OnceLock;

use crate::metricscommon;

/// Compatibility adapter for the Go-shaped metric declarations that are still
/// consumed by sibling migration units. Every constructor delegates to the
/// upstream `prometheus` crate; this module owns no collector implementation.
/// 兼容 Go 形态的指标声明：构造器全部委托上游 `prometheus`，本模块不实现采集器。
#[allow(non_snake_case, non_upper_case_globals)]
pub mod compat_prometheus {
    use std::collections::HashMap;

    pub use prometheus::{Counter, CounterVec, Gauge, GaugeVec, Histogram, HistogramVec};
    /// Go SummaryVec 在成熟 Rust prometheus 中不可用，以 HistogramVec 兜底。
    pub type SummaryVec = prometheus::HistogramVec;
    /// 观测器别名，对应直方图样本写入。
    pub type Observer = prometheus::Histogram;
    /// 常量标签映射类型。
    pub type Labels = HashMap<String, String>;

    /// Go `prometheus.CounterOpts` 形状的计数器选项。
    #[derive(Clone, Default)]
    pub struct CounterOpts {
        pub Namespace: &'static str,
        pub Subsystem: &'static str,
        pub Name: &'static str,
        pub Help: &'static str,
    }

    /// Gauge 选项与 Counter 选项字段相同。
    pub type GaugeOpts = CounterOpts;

    /// 直方图选项，含自定义桶边界。
    #[derive(Clone, Default)]
    pub struct HistogramOpts {
        pub Namespace: &'static str,
        pub Subsystem: &'static str,
        pub Name: &'static str,
        pub Help: &'static str,
        pub Buckets: Vec<f64>,
    }

    /// Summary 选项；实际创建时会降级为直方图。
    #[derive(Clone, Default)]
    pub struct SummaryOpts {
        pub Namespace: &'static str,
        pub Subsystem: &'static str,
        pub Name: &'static str,
        pub Help: &'static str,
    }

    /// Go 默认桶占位；空向量表示使用库默认桶。
    pub const DefBuckets: Vec<f64> = Vec::new();

    /// 默认注册表适配器，对接全局 prometheus registry。
    pub struct DefaultRegistry;
    /// 包级默认 Registerer 单例。
    pub static DefaultRegisterer: DefaultRegistry = DefaultRegistry;

    impl crate::promutil::Registry for DefaultRegistry {
        fn Register(
            &self,
            collector: Box<dyn prometheus::core::Collector>,
        ) -> prometheus::Result<()> {
            prometheus::default_registry().register(collector)
        }

        fn MustRegister(&self, collectors: Vec<Box<dyn prometheus::core::Collector>>) {
            for collector in collectors {
                prometheus::default_registry()
                    .register(collector)
                    .expect("metric registration failed");
            }
        }

        fn Unregister(&self, collector: Box<dyn prometheus::core::Collector>) -> bool {
            prometheus::default_registry().unregister(collector).is_ok()
        }
    }

    /// 指数增长直方图桶：`start * factor^i`。
    pub fn ExponentialBuckets(start: f64, factor: f64, count: usize) -> Vec<f64> {
        prometheus::exponential_buckets(start, factor, count)
            .expect("valid exponential histogram buckets")
    }

    /// 按 `[min, max]` 范围生成指定个数的指数桶。
    pub fn ExponentialBucketsRange(min: f64, max: f64, count: usize) -> Vec<f64> {
        assert!(min > 0.0 && max > min && count >= 2);
        ExponentialBuckets(min, (max / min).powf(1.0 / (count - 1) as f64), count)
    }

    /// snake_case 别名，转发至 [`ExponentialBuckets`]。
    pub fn exponential_buckets(start: f64, factor: f64, count: usize) -> Vec<f64> {
        ExponentialBuckets(start, factor, count)
    }

    /// Go 风格按标签取值 / 删除标签的兼容 trait。
    pub trait MetricCompat {
        type Child;
        fn WithLabelValues<'a, L: AsRef<[&'a str]>>(&self, labels: L) -> Self::Child;
        fn DeleteLabelValues<'a, L: AsRef<[&'a str]>>(&self, _labels: L) -> bool {
            false
        }
    }

    impl MetricCompat for CounterVec {
        type Child = Counter;
        fn WithLabelValues<'a, L: AsRef<[&'a str]>>(&self, labels: L) -> Counter {
            self.with_label_values(labels.as_ref())
        }
        fn DeleteLabelValues<'a, L: AsRef<[&'a str]>>(&self, labels: L) -> bool {
            self.remove_label_values(labels.as_ref()).is_ok()
        }
    }

    impl MetricCompat for GaugeVec {
        type Child = Gauge;
        fn WithLabelValues<'a, L: AsRef<[&'a str]>>(&self, labels: L) -> Gauge {
            self.with_label_values(labels.as_ref())
        }
        fn DeleteLabelValues<'a, L: AsRef<[&'a str]>>(&self, labels: L) -> bool {
            self.remove_label_values(labels.as_ref()).is_ok()
        }
    }

    impl MetricCompat for HistogramVec {
        type Child = Histogram;
        fn WithLabelValues<'a, L: AsRef<[&'a str]>>(&self, labels: L) -> Histogram {
            self.with_label_values(labels.as_ref())
        }
    }

    /// Go `Counter.Add` 兼容。
    pub trait CounterCompat {
        fn Add(&self, value: f64);
    }
    impl CounterCompat for Counter {
        fn Add(&self, value: f64) {
            self.inc_by(value);
        }
    }

    /// Go `Gauge.Set` 兼容。
    pub trait GaugeCompat {
        fn Set(&self, value: f64);
    }
    impl GaugeCompat for Gauge {
        fn Set(&self, value: f64) {
            self.set(value);
        }
    }

    /// Go `Observer.Observe` 兼容。
    pub trait ObserverCompat {
        fn Observe(&self, value: f64);
    }
    impl ObserverCompat for Histogram {
        fn Observe(&self, value: f64) {
            self.observe(value);
        }
    }

    /// 按 Go CounterOpts 创建 CounterVec。
    pub fn NewCounterVec(opts: CounterOpts, labels: &[&str]) -> CounterVec {
        prometheus::CounterVec::new(super::compat_metricscommon::counter_opts(opts), labels)
            .expect("valid counter vector options")
    }

    /// 按 Go GaugeOpts 创建 Gauge。
    pub fn NewGauge(opts: GaugeOpts) -> Gauge {
        prometheus::Gauge::with_opts(super::compat_metricscommon::counter_opts(opts))
            .expect("valid gauge options")
    }
}

/// 将 Go 风格 Opts 转为真实 prometheus 构造，并注入 metricscommon 工厂。
#[allow(non_snake_case)]
pub mod compat_metricscommon {
    use super::compat_prometheus as prometheus;

    /// 将标签名列表转为 `Vec<String>`，供向量指标使用。
    pub trait LabelNames {
        fn strings(&self) -> Vec<String>;
    }
    impl LabelNames for Vec<&'static str> {
        fn strings(&self) -> Vec<String> {
            self.iter().map(|label| (*label).to_owned()).collect()
        }
    }
    impl<const N: usize> LabelNames for &[&'static str; N] {
        fn strings(&self) -> Vec<String> {
            self.iter().map(|label| (*label).to_owned()).collect()
        }
    }

    /// 将 Go CounterOpts 转为 `prometheus::Opts`。
    pub fn counter_opts(opts: prometheus::CounterOpts) -> ::prometheus::Opts {
        ::prometheus::Opts::new(opts.Name, opts.Help)
            .namespace(opts.Namespace)
            .subsystem(opts.Subsystem)
            .const_labels(prometheus::Labels::new())
    }

    /// 将 Go HistogramOpts 转为库直方图选项（非空时覆盖桶）。
    fn histogram_opts(opts: prometheus::HistogramOpts) -> ::prometheus::HistogramOpts {
        let mut built = ::prometheus::HistogramOpts::new(opts.Name, opts.Help)
            .namespace(opts.Namespace)
            .subsystem(opts.Subsystem)
            .const_labels(prometheus::Labels::new());
        if !opts.Buckets.is_empty() {
            built = built.buckets(opts.Buckets);
        }
        built
    }

    /// 创建 Counter。
    pub fn NewCounter(opts: prometheus::CounterOpts) -> prometheus::Counter {
        crate::metricscommon::NewCounter(counter_opts(opts))
    }
    /// 创建 Gauge。
    pub fn NewGauge(opts: prometheus::GaugeOpts) -> prometheus::Gauge {
        crate::metricscommon::NewGauge(counter_opts(opts))
    }
    /// 创建 Histogram。
    pub fn NewHistogram(opts: prometheus::HistogramOpts) -> prometheus::Histogram {
        crate::metricscommon::NewHistogram(histogram_opts(opts))
    }
    /// 创建 CounterVec。
    pub fn NewCounterVec<L: LabelNames>(
        opts: prometheus::CounterOpts,
        labels: L,
    ) -> prometheus::CounterVec {
        crate::metricscommon::NewCounterVec(counter_opts(opts), &labels.strings())
    }
    /// 创建 GaugeVec。
    pub fn NewGaugeVec<L: LabelNames>(
        opts: prometheus::GaugeOpts,
        labels: L,
    ) -> prometheus::GaugeVec {
        crate::metricscommon::NewGaugeVec(counter_opts(opts), &labels.strings())
    }
    /// 创建 HistogramVec。
    pub fn NewHistogramVec<L: LabelNames>(
        opts: prometheus::HistogramOpts,
        labels: L,
    ) -> prometheus::HistogramVec {
        crate::metricscommon::NewHistogramVec(histogram_opts(opts), &labels.strings())
    }
    /// SummaryVec 兼容：降级为 HistogramVec。
    pub fn NewSummaryVec<L: LabelNames>(
        opts: prometheus::SummaryOpts,
        labels: L,
    ) -> prometheus::SummaryVec {
        let histogram = prometheus::HistogramOpts {
            Namespace: opts.Namespace,
            Subsystem: opts.Subsystem,
            Name: opts.Name,
            Help: opts.Help,
            Buckets: Vec::new(),
        };
        NewHistogramVec(histogram, labels)
    }

    pub use crate::metricscommon::GetMergedConstLabels;
    pub use NewCounter as new_counter;
    pub use NewHistogramVec as new_histogram_vec;
}

static BINDING_CACHE_HIT_COUNTER: OnceLock<prometheus::Counter> = OnceLock::new();
static BINDING_CACHE_MISS_COUNTER: OnceLock<prometheus::Counter> = OnceLock::new();
static BINDING_CACHE_MEM_USAGE: OnceLock<prometheus::Gauge> = OnceLock::new();
static BINDING_CACHE_MEM_LIMIT: OnceLock<prometheus::Gauge> = OnceLock::new();
static BINDING_CACHE_NUM_BINDINGS: OnceLock<prometheus::Gauge> = OnceLock::new();

/// 在 `tidb_server` 子系统下创建 Counter。
fn counter(name: &str, help: &str) -> prometheus::Counter {
    metricscommon::NewCounter(
        prometheus::Opts::new(name, help)
            .namespace("tidb")
            .subsystem("server"),
    )
}

/// 在 `tidb_server` 子系统下创建 Gauge。
fn gauge(name: &str, help: &str) -> prometheus::Gauge {
    metricscommon::NewGauge(
        prometheus::Opts::new(name, help)
            .namespace("tidb")
            .subsystem("server"),
    )
}

/// Initializes bindinfo metrics. Calling this function repeatedly is harmless.
/// 初始化 bindinfo 指标；重复调用无害（OnceLock 幂等）。
pub fn init_bind_info_metrics() {
    BINDING_CACHE_HIT_COUNTER
        .get_or_init(|| counter("binding_cache_hit_total", "Counter of binding cache hit."));
    BINDING_CACHE_MISS_COUNTER
        .get_or_init(|| counter("binding_cache_miss_total", "Counter of binding cache miss."));
    BINDING_CACHE_MEM_USAGE
        .get_or_init(|| gauge("binding_cache_mem_usage", "Memory usage of binding cache."));
    BINDING_CACHE_MEM_LIMIT
        .get_or_init(|| gauge("binding_cache_mem_limit", "Memory limit of binding cache."));
    BINDING_CACHE_NUM_BINDINGS.get_or_init(|| {
        gauge(
            "binding_cache_num_bindings",
            "Number of bindings in binding cache.",
        )
    });
}

/// Binding 缓存命中次数计数器。
pub fn binding_cache_hit_counter() -> &'static prometheus::Counter {
    BINDING_CACHE_HIT_COUNTER
        .get_or_init(|| counter("binding_cache_hit_total", "Counter of binding cache hit."))
}

/// Binding 缓存未命中次数计数器。
pub fn binding_cache_miss_counter() -> &'static prometheus::Counter {
    BINDING_CACHE_MISS_COUNTER
        .get_or_init(|| counter("binding_cache_miss_total", "Counter of binding cache miss."))
}

/// Binding 缓存当前内存占用（字节）仪表。
pub fn binding_cache_mem_usage() -> &'static prometheus::Gauge {
    BINDING_CACHE_MEM_USAGE
        .get_or_init(|| gauge("binding_cache_mem_usage", "Memory usage of binding cache."))
}

/// Binding 缓存内存上限仪表。
pub fn binding_cache_mem_limit() -> &'static prometheus::Gauge {
    BINDING_CACHE_MEM_LIMIT
        .get_or_init(|| gauge("binding_cache_mem_limit", "Memory limit of binding cache."))
}

/// Binding 缓存中绑定条目数量仪表。
pub fn binding_cache_num_bindings() -> &'static prometheus::Gauge {
    BINDING_CACHE_NUM_BINDINGS.get_or_init(|| {
        gauge(
            "binding_cache_num_bindings",
            "Number of bindings in binding cache.",
        )
    })
}

/// Go 风格导出：转发至 [`init_bind_info_metrics`]。
#[allow(non_snake_case)]
pub fn InitBindInfoMetrics() {
    init_bind_info_metrics();
}
