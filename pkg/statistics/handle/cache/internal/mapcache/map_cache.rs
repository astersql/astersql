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

// 无淘汰的 HashMap 统计缓存（对应 Go mapCache）。
//
// 仅维护表 ID → 表统计与累计内存用量，不实现容量淘汰；
// 容量相关方法为空操作，便于测试与简单场景。

use cache_internal::StatsCacheInner;
use statistics::Table;
use std::collections::HashMap;
use std::sync::Arc;

/// 缓存条目：共享表统计及其写入时记录的内存成本。
#[derive(Clone)]
struct CacheItem {
    value: Arc<Table>,
    cost: i64,
}

/// A statistics cache backed by a map, without eviction.
///
/// 基于 HashMap 的统计缓存，不做淘汰。
#[derive(Default)]
pub struct MapCache {
    tables: HashMap<i64, CacheItem>,
    memUsage: i64,
}

/// 构造空的 MapCache。
pub fn NewMapCache() -> MapCache {
    MapCache::default()
}

impl MapCache {
    /// 返回当前缓存中的全部表 ID（无序）。
    pub fn Keys(&self) -> Vec<i64> {
        self.tables.keys().copied().collect()
    }
}

/// 实现统一的 `StatsCacheInner`：Put 替换时按新旧成本差值更新 `memUsage`。
impl StatsCacheInner for MapCache {
    fn Get(&self, key: i64) -> Option<Arc<Table>> {
        self.tables.get(&key).map(|item| item.value.clone())
    }

    fn Put(&mut self, key: i64, value: Arc<Table>) -> bool {
        let cost = value.MemoryUsage().TotalMemUsage;
        // 替换时用新成本减旧成本，避免重复累加
        match self.tables.insert(key, CacheItem { value, cost }) {
            Some(previous) => self.memUsage += cost - previous.cost,
            None => self.memUsage += cost,
        }
        true
    }

    fn Del(&mut self, key: i64) {
        if let Some(item) = self.tables.remove(&key) {
            self.memUsage -= item.cost;
        }
    }
    fn Cost(&self) -> i64 {
        self.memUsage
    }
    fn Values(&self) -> Vec<Arc<Table>> {
        self.tables
            .values()
            .map(|item| item.value.clone())
            .collect()
    }
    fn Len(&self) -> usize {
        self.tables.len()
    }
    fn Copy(&self) -> Box<dyn StatsCacheInner> {
        Box::new(Self {
            tables: self.tables.clone(),
            memUsage: self.memUsage,
        })
    }
    fn SetCapacity(&mut self, _capacity: i64) {}
    fn Close(&mut self) {}
    fn TriggerEvict(&mut self) {}
    fn WaitForAsyncUpdates(&mut self) {}
}
