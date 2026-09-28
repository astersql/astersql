// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// InfoSchema V2 SIEVE 缓存的状态钩子实现。
//
// 将命中 / 未命中 / 驱逐与内存用量回调写入共享 Prometheus 指标。

#![allow(non_camel_case_types, non_snake_case)]

use crate::sieve::SieveStatusHook;
use prometheus::{Counter, Gauge};

/// SIEVE 缓存状态钩子：共享 evict/hit/miss 与内存/对象数指标句柄。
pub struct sieveStatusHookImpl {
    /// 驱逐次数。
    evict: Counter,
    /// 命中次数。
    hit: Counter,
    /// 未命中次数。
    miss: Counter,
    /// 当前缓存对象数。
    object_count: Gauge,
    /// 当前内存用量（字节）。
    memory_usage: Gauge,
    /// 内存上限（字节）。
    memory_limit: Gauge,
}

impl sieveStatusHookImpl {
    /// 已累计的驱逐次数。
    pub fn evictions(&self) -> u64 {
        self.evict.get() as u64
    }
    /// 已累计的命中次数。
    pub fn hits(&self) -> u64 {
        self.hit.get() as u64
    }
    /// 已累计的未命中次数。
    pub fn misses(&self) -> u64 {
        self.miss.get() as u64
    }
    /// 当前对象数快照。
    pub fn object_count(&self) -> u64 {
        self.object_count.get() as u64
    }
    /// 当前内存用量快照。
    pub fn memory_usage(&self) -> u64 {
        self.memory_usage.get() as u64
    }
    /// 当前内存上限快照。
    pub fn memory_limit(&self) -> u64 {
        self.memory_limit.get() as u64
    }
}

/// 将 SieveStatusHook 回调落到 Prometheus 句柄，语义对齐 Go 钩子。
impl SieveStatusHook for sieveStatusHookImpl {
    fn on_evict(&self) {
        self.evict.inc();
    }
    fn on_hit(&self) {
        self.hit.inc();
    }
    fn on_miss(&self) {
        self.miss.inc();
    }
    fn on_update(&self, size: u64, count: u64) {
        self.memory_usage.set(size as f64);
        self.object_count.set(count as f64);
    }
    fn on_update_limit(&self, limit: u64) {
        self.memory_limit.set(limit as f64);
    }
}

/// 对应 Go `newSieveStatusHookImpl`：绑定三个结果 label 和全局 Gauge。
pub fn newSieveStatusHookImpl() -> sieveStatusHookImpl {
    sieveStatusHookImpl {
        evict: astersql_infoschema_metrics::InfoSchemaV2CacheCounter.with_label_values(&["evict"]),
        hit: astersql_infoschema_metrics::InfoSchemaV2CacheCounter.with_label_values(&["hit"]),
        miss: astersql_infoschema_metrics::InfoSchemaV2CacheCounter.with_label_values(&["miss"]),
        object_count: (*astersql_infoschema_metrics::InfoSchemaV2CacheObjCnt).clone(),
        memory_usage: (*astersql_infoschema_metrics::InfoSchemaV2CacheMemUsage).clone(),
        memory_limit: (*astersql_infoschema_metrics::InfoSchemaV2CacheMemLimit).clone(),
    }
}
