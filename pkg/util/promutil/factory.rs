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

// Prometheus 指标工厂：统一创建 Counter / Gauge / Histogram 及其 Vec 变体。
//
// 对应 Go `promutil.Factory`；默认实现直接委托 `prometheus` crate，
// 非法 opts（如非严格递增的 histogram buckets）会 panic，与 Go 行为一致。

#![allow(non_snake_case)]
#![allow(non_camel_case_types)]

// Factory is the interface to create some native prometheus metric.
/// 创建原生 Prometheus 指标的工厂接口。
pub trait Factory {
    // NewCounter creates a new Counter based on the provided CounterOpts.
    /// 按 CounterOpts 创建 Counter（只增计数器）。
    fn NewCounter(&self, opts: prometheus::Opts) -> prometheus::Counter;

    // NewCounterVec creates a new CounterVec based on the provided CounterOpts and
    // partitioned by the given label names.
    /// 按标签名分区创建 CounterVec。
    fn NewCounterVec(
        &self,
        opts: prometheus::Opts,
        labelNames: Vec<String>,
    ) -> prometheus::CounterVec;

    // NewGauge creates a new Gauge based on the provided GaugeOpts.
    /// 按 GaugeOpts 创建 Gauge（可增可减瞬时值）。
    fn NewGauge(&self, opts: prometheus::Opts) -> prometheus::Gauge;

    // NewGaugeVec creates a new GaugeVec based on the provided GaugeOpts and
    // partitioned by the given label names.
    /// 按标签名分区创建 GaugeVec。
    fn NewGaugeVec(&self, opts: prometheus::Opts, labelNames: Vec<String>) -> prometheus::GaugeVec;

    // NewHistogram creates a new Histogram based on the provided HistogramOpts. It
    // panics if the buckets in HistogramOpts are not in strictly increasing order.
    /// 按 HistogramOpts 创建 Histogram；buckets 非严格递增时 panic。
    fn NewHistogram(&self, opts: prometheus::HistogramOpts) -> prometheus::Histogram;

    // NewHistogramVec creates a new HistogramVec based on the provided HistogramOpts and
    // partitioned by the given label names.
    /// 按标签名分区创建 HistogramVec。
    fn NewHistogramVec(
        &self,
        opts: prometheus::HistogramOpts,
        labelNames: Vec<String>,
    ) -> prometheus::HistogramVec;
}

/// 默认工厂：直接调用 prometheus 构造函数，opts 非法则 expect panic。
struct defaultFactory;

impl Factory for defaultFactory {
    fn NewCounter(&self, opts: prometheus::Opts) -> prometheus::Counter {
        prometheus::Counter::with_opts(opts).expect("invalid CounterOpts")
    }

    fn NewCounterVec(
        &self,
        opts: prometheus::Opts,
        labelNames: Vec<String>,
    ) -> prometheus::CounterVec {
        // prometheus API 需要 &[&str] 标签切片。
        let labels: Vec<&str> = labelNames.iter().map(String::as_str).collect();
        prometheus::CounterVec::new(opts, &labels).expect("invalid CounterOpts or label names")
    }

    fn NewGauge(&self, opts: prometheus::Opts) -> prometheus::Gauge {
        prometheus::Gauge::with_opts(opts).expect("invalid GaugeOpts")
    }

    fn NewGaugeVec(&self, opts: prometheus::Opts, labelNames: Vec<String>) -> prometheus::GaugeVec {
        let labels: Vec<&str> = labelNames.iter().map(String::as_str).collect();
        prometheus::GaugeVec::new(opts, &labels).expect("invalid GaugeOpts or label names")
    }

    fn NewHistogram(&self, opts: prometheus::HistogramOpts) -> prometheus::Histogram {
        prometheus::Histogram::with_opts(opts).expect("invalid HistogramOpts")
    }

    fn NewHistogramVec(
        &self,
        opts: prometheus::HistogramOpts,
        labelNames: Vec<String>,
    ) -> prometheus::HistogramVec {
        let labels: Vec<&str> = labelNames.iter().map(String::as_str).collect();
        prometheus::HistogramVec::new(opts, &labels).expect("invalid HistogramOpts or label names")
    }
}

// NewDefaultFactory returns a default implementation of Factory.
/// 返回默认 `Factory` 实现的堆分配实例。
pub fn NewDefaultFactory() -> Box<dyn Factory> {
    Box::new(defaultFactory)
}
