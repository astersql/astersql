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

// Prometheus 指标构造包装：包级常量标签的读写、合并与注入。
//
// 所有 New* 工厂在创建指标前用包级常量标签替换调用方 `const_labels`，
// 保证集群/节点等维度在注册前统一注入。SummaryVec 降级为 HistogramVec。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

/// 标签键值映射别名。
pub type Labels = HashMap<String, String>;

static CONST_LABELS: OnceLock<RwLock<Labels>> = OnceLock::new();

/// 惰性初始化包级常量标签读写锁。
fn const_labels() -> &'static RwLock<Labels> {
    CONST_LABELS.get_or_init(|| RwLock::new(Labels::new()))
}

/// Returns a snapshot of the package-level constant labels.
/// 返回包级常量标签快照。
pub fn GetConstLabels() -> Labels {
    const_labels()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Merges input labels with package-level labels, which have higher priority.
/// 合并输入与包级标签；包级同名键覆盖输入。
pub fn GetMergedConstLabels(mut input: Labels) -> Labels {
    let globals = GetConstLabels();
    if input.is_empty() {
        return globals;
    }

    // extend：后者（globals）覆盖同名键。
    input.extend(globals);
    input
}

/// Replaces the package-level labels. Keys are normalized to lowercase.
/// 替换包级标签；键统一转小写；参数须为偶数个（k,v 成对）。
pub fn SetConstLabels(kv: &[String]) {
    if kv.len() % 2 == 1 {
        panic!(
            "got the odd number of inputs for const labels: {}",
            kv.len()
        );
    }

    let labels = kv
        .chunks_exact(2)
        .map(|pair| (pair[0].to_lowercase(), pair[1].clone()))
        .collect();
    *const_labels()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = labels;
}

/// Creates a counter after replacing caller-supplied constant labels.
/// 创建 Counter，常量标签替换为包级值。
pub fn NewCounter(mut opts: prometheus::Opts) -> prometheus::Counter {
    opts.const_labels = GetConstLabels();
    prometheus::Counter::with_opts(opts).expect("invalid counter options")
}

/// Creates a counter vector after replacing caller-supplied constant labels.
/// 创建 CounterVec，常量标签替换为包级值。
pub fn NewCounterVec(mut opts: prometheus::Opts, labelNames: &[String]) -> prometheus::CounterVec {
    opts.const_labels = GetConstLabels();
    let label_names: Vec<&str> = labelNames.iter().map(String::as_str).collect();
    prometheus::CounterVec::new(opts, &label_names).expect("invalid counter vector options")
}

/// Creates a gauge after replacing caller-supplied constant labels.
/// 创建 Gauge，常量标签替换为包级值。
pub fn NewGauge(mut opts: prometheus::Opts) -> prometheus::Gauge {
    opts.const_labels = GetConstLabels();
    prometheus::Gauge::with_opts(opts).expect("invalid gauge options")
}

/// Creates a gauge vector after replacing caller-supplied constant labels.
/// 创建 GaugeVec，常量标签替换为包级值。
pub fn NewGaugeVec(mut opts: prometheus::Opts, labelNames: &[String]) -> prometheus::GaugeVec {
    opts.const_labels = GetConstLabels();
    let label_names: Vec<&str> = labelNames.iter().map(String::as_str).collect();
    prometheus::GaugeVec::new(opts, &label_names).expect("invalid gauge vector options")
}

/// Creates a histogram after replacing caller-supplied constant labels.
/// 创建 Histogram，常量标签替换为包级值。
pub fn NewHistogram(mut opts: prometheus::HistogramOpts) -> prometheus::Histogram {
    opts.common_opts.const_labels = GetConstLabels();
    prometheus::Histogram::with_opts(opts).expect("invalid histogram options")
}

/// Creates a histogram vector after replacing caller-supplied constant labels.
/// 创建 HistogramVec，常量标签替换为包级值。
pub fn NewHistogramVec(
    mut opts: prometheus::HistogramOpts,
    labelNames: &[String],
) -> prometheus::HistogramVec {
    opts.common_opts.const_labels = GetConstLabels();
    let label_names: Vec<&str> = labelNames.iter().map(String::as_str).collect();
    prometheus::HistogramVec::new(opts, &label_names).expect("invalid histogram vector options")
}

/// Compatibility fallback for Go's SummaryVec.
///
/// The mature Rust `prometheus` crate has no Summary collector. This preserves
/// observations, count, sum, labels, and registry compatibility as a HistogramVec;
/// client-side streaming quantiles are intentionally unavailable.
/// Go SummaryVec 兼容：以 HistogramVec 保留观测/计数/求和与标签；无客户端流式分位数。
pub fn NewSummaryVec(
    mut opts: prometheus::HistogramOpts,
    labelNames: &[String],
) -> prometheus::HistogramVec {
    opts.common_opts.const_labels = GetConstLabels();
    let label_names: Vec<&str> = labelNames.iter().map(String::as_str).collect();
    prometheus::HistogramVec::new(opts, &label_names)
        .expect("invalid summary compatibility options")
}

/// Creates a descriptor after merging caller and package-level constant labels.
/// 创建 Desc：合并调用方与包级常量标签（包级优先）。
pub fn NewDesc(
    fqName: &str,
    help: &str,
    variableLabels: &[String],
    inConstLbls: Labels,
) -> prometheus::Result<prometheus::core::Desc> {
    prometheus::core::Desc::new(
        fqName.to_owned(),
        help.to_owned(),
        variableLabels.to_vec(),
        GetMergedConstLabels(inConstLbls),
    )
}

/// Serializes tests that mutate package-level const labels (Go tests are serial by default).
/// 串行化会修改包级常量标签的测试（对齐 Go 默认串行测试）。
#[cfg(test)]
pub(crate) static CONST_LABELS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
