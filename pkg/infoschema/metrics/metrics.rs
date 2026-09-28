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

// InfoSchema 指标句柄：按固定 label 绑定的 Counter / Histogram 包装。
//
// 对应 Go 包变量（GetLatestCounter、LoadSchemaDurationTotal 等）。
// `LazyLock` 延迟创建底层 Prometheus 子序列，`InitMetricsVars` 可提前 force。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use crate::metrics;
use std::ops::Deref;
use std::sync::LazyLock;

/// 对 `prometheus::Counter` 的惰性包装，提供 Go 风格 `Inc` / `Get`。
pub struct Counter(LazyLock<prometheus::Counter>);

impl Counter {
    /// 用初始化闭包构造常量级 LazyLock 包装。
    const fn new(init: fn() -> prometheus::Counter) -> Self {
        Self(LazyLock::new(init))
    }

    /// 计数器加一。
    pub fn Inc(&self) {
        self.0.inc();
    }

    /// 读取当前计数值。
    pub fn Get(&self) -> f64 {
        self.0.get()
    }
}

impl Deref for Counter {
    type Target = prometheus::Counter;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// 对 `prometheus::Histogram` 的惰性包装，提供 Go 风格 `Observe`。
pub struct Observer(LazyLock<prometheus::Histogram>);

impl Observer {
    /// 用初始化闭包构造常量级 LazyLock 包装。
    const fn new(init: fn() -> prometheus::Histogram) -> Self {
        Self(LazyLock::new(init))
    }

    /// 记录一次观测值（秒）。
    pub fn Observe(&self, value: f64) {
        self.0.observe(value);
    }

    /// 已观测样本数。
    pub fn GetSampleCount(&self) -> u64 {
        self.0.get_sample_count()
    }
}

impl Deref for Observer {
    type Target = prometheus::Histogram;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// —— InfoCache get/hit × latest/ts/version 六个固定子序列 ——

fn get_latest_counter() -> prometheus::Counter {
    metrics::InfoCacheCounters.with_label_values(&["get", "latest"])
}
fn get_ts_counter() -> prometheus::Counter {
    metrics::InfoCacheCounters.with_label_values(&["get", "ts"])
}
fn get_version_counter() -> prometheus::Counter {
    metrics::InfoCacheCounters.with_label_values(&["get", "version"])
}
fn hit_latest_counter() -> prometheus::Counter {
    metrics::InfoCacheCounters.with_label_values(&["hit", "latest"])
}
fn hit_ts_counter() -> prometheus::Counter {
    metrics::InfoCacheCounters.with_label_values(&["hit", "ts"])
}
fn hit_version_counter() -> prometheus::Counter {
    metrics::InfoCacheCounters.with_label_values(&["hit", "version"])
}
fn load_schema_counter_snapshot() -> prometheus::Counter {
    metrics::LoadSchemaCounter.with_label_values(&["snapshot"])
}
fn load_schema_duration_total() -> prometheus::Histogram {
    metrics::LoadSchemaDuration.with_label_values(&["total"])
}
fn load_schema_duration_load_diff() -> prometheus::Histogram {
    metrics::LoadSchemaDuration.with_label_values(&["load-diff"])
}
fn load_schema_duration_load_all() -> prometheus::Histogram {
    metrics::LoadSchemaDuration.with_label_values(&["load-all"])
}

/// 按 latest 取 InfoCache 的 get 次数。
pub static GetLatestCounter: Counter = Counter::new(get_latest_counter);
/// 按时间戳取 InfoCache 的 get 次数。
pub static GetTSCounter: Counter = Counter::new(get_ts_counter);
/// 按 schema 版本取 InfoCache 的 get 次数。
pub static GetVersionCounter: Counter = Counter::new(get_version_counter);
/// latest 路径命中次数。
pub static HitLatestCounter: Counter = Counter::new(hit_latest_counter);
/// 时间戳路径命中次数。
pub static HitTSCounter: Counter = Counter::new(hit_ts_counter);
/// 版本路径命中次数。
pub static HitVersionCounter: Counter = Counter::new(hit_version_counter);
/// 以 snapshot 方式加载 schema 的次数。
pub static LoadSchemaCounterSnapshot: Counter = Counter::new(load_schema_counter_snapshot);
/// 加载 schema 总耗时直方图。
pub static LoadSchemaDurationTotal: Observer = Observer::new(load_schema_duration_total);
/// 增量 load-diff 耗时直方图。
pub static LoadSchemaDurationLoadDiff: Observer = Observer::new(load_schema_duration_load_diff);
/// 全量 load-all 耗时直方图。
pub static LoadSchemaDurationLoadAll: Observer = Observer::new(load_schema_duration_load_all);

/// 包初始化入口，对应 Go `init`：强制绑定全部子序列。
pub fn init() {
    InitMetricsVars();
}

// InitMetricsVars eagerly binds the same children that Go initializes at package load.
// LazyLock also makes repeated explicit initialization safe and preserves each series.
/// 提前 force 各 LazyLock，与 Go 包加载时注册子指标一致；重复调用安全。
pub fn InitMetricsVars() {
    LazyLock::force(&GetLatestCounter.0);
    LazyLock::force(&GetTSCounter.0);
    LazyLock::force(&GetVersionCounter.0);
    LazyLock::force(&HitLatestCounter.0);
    LazyLock::force(&HitTSCounter.0);
    LazyLock::force(&HitVersionCounter.0);
    LazyLock::force(&LoadSchemaCounterSnapshot.0);
    LazyLock::force(&LoadSchemaDurationTotal.0);
    LazyLock::force(&LoadSchemaDurationLoadDiff.0);
    LazyLock::force(&LoadSchemaDurationLoadAll.0);
}
