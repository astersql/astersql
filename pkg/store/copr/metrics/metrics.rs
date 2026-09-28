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

// Coprocessor 缓存 Prometheus 计数器（evict / hit / miss）。
//
// 用 `LazyLock` 模拟 Go 包级全局变量的惰性初始化，避免未同步的可变 static。
// 依赖上层先初始化 `DistSQLCoprCacheCounter`（DistSQL 分布式 SQL 指标命名空间）。

use std::sync::LazyLock;

use prometheus::Counter;

// copr metrics vars. LazyLock provides the package-initialized Go globals
// without unsynchronized mutable Rust statics.
/// 缓存驱逐（evict）次数计数器。
pub static CoprCacheCounterEvict: LazyLock<Counter> = LazyLock::new(|| copr_cache_counter("evict"));
/// 缓存命中（hit）次数计数器。
pub static CoprCacheCounterHit: LazyLock<Counter> = LazyLock::new(|| copr_cache_counter("hit"));
/// 缓存未命中（miss）次数计数器。
pub static CoprCacheCounterMiss: LazyLock<Counter> = LazyLock::new(|| copr_cache_counter("miss"));

/// 初始化全部 copr 缓存指标变量（强制求值 LazyLock）。
pub fn init() {
    InitMetricsVars();
}

// InitMetricsVars init copr metrics vars.
/// Go 风格入口：强制初始化 evict/hit/miss 三个计数器。
pub fn InitMetricsVars() {
    LazyLock::force(&CoprCacheCounterEvict);
    LazyLock::force(&CoprCacheCounterHit);
    LazyLock::force(&CoprCacheCounterMiss);
}

/// 从全局 `CounterVec` 按 label 取出具体 `Counter`。
fn copr_cache_counter(label: &str) -> Counter {
    let counter = unsafe {
        crate::metrics::DistSQLCoprCacheCounter
            .as_ref()
            .expect("DistSQL coprocessor cache counter must be initialized first")
    };
    counter.with_label_values(&[label])
}
