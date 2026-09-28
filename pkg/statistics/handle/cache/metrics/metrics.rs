// Copyright 2023 PingCAP, Inc.
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

// 统计缓存操作的 Prometheus 指标句柄绑定。
//
// 从父级 `StatsCacheCounter`/`StatsCacheGauge` 按标签切分出 miss/hit/update 等
// Counter，以及 track/capacity 等 Gauge，语义对齐 Go 侧全局可重绑定指标句柄。

#![allow(non_snake_case, non_upper_case_globals, static_mut_refs)]

use crate::metrics;

// Go 的 prometheus.Counter/Gauge 是可重新绑定的全局指标句柄。
/// 缓存未命中计数。
pub static mut MissCounter: Option<prometheus::Counter> = None;
/// 缓存命中计数。
pub static mut HitCounter: Option<prometheus::Counter> = None;
/// 缓存更新计数。
pub static mut UpdateCounter: Option<prometheus::Counter> = None;
/// 缓存删除计数。
pub static mut DelCounter: Option<prometheus::Counter> = None;
/// 缓存驱逐计数。
pub static mut EvictCounter: Option<prometheus::Counter> = None;
/// 缓存拒绝写入计数。
pub static mut RejectCounter: Option<prometheus::Counter> = None;
/// 当前已追踪的缓存内存开销（cost）仪表。
pub static mut CostGauge: Option<prometheus::Gauge> = None;
/// 缓存容量上限仪表。
pub static mut CapacityGauge: Option<prometheus::Gauge> = None;

// Rust 模块接入层显式调用该入口，以保持 Go 包初始化顺序。
/// 模块初始化入口：绑定全部标签化指标句柄。
pub fn init() {
    InitMetricsVars();
}

// InitMetricsVars 对应 Go 的标签绑定逻辑；父向量由 pkg/metrics 初始化。
/// 从父级 CounterVec/GaugeVec 按固定标签值切分并写入各全局句柄。
pub fn InitMetricsVars() {
    unsafe {
        let counters = metrics::StatsCacheCounter
            .as_ref()
            .expect("pkg/metrics StatsCacheCounter must be initialized first");
        MissCounter = Some(counters.with_label_values(&["miss"]));
        HitCounter = Some(counters.with_label_values(&["hit"]));
        UpdateCounter = Some(counters.with_label_values(&["update"]));
        DelCounter = Some(counters.with_label_values(&["del"]));
        EvictCounter = Some(counters.with_label_values(&["evict"]));
        RejectCounter = Some(counters.with_label_values(&["reject"]));

        let gauges = metrics::StatsCacheGauge
            .as_ref()
            .expect("pkg/metrics StatsCacheGauge must be initialized first");
        CostGauge = Some(gauges.with_label_values(&["track"]));
        CapacityGauge = Some(gauges.with_label_values(&["capacity"]));
    }
}
