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

// SQL 绑定缓存（binding cache）模块。
//
// “SQL 绑定”是一种把某条 SQL 语句（以其摘要 SQL Digest 标识）与固定的
// 执行计划提示（hint）关联起来的机制，可在不修改业务 SQL 的情况下
// 强制优化器选择指定的执行计划。本模块提供绑定在内存中的缓存层：
//
// - `BindingCache`：绑定缓存的抽象接口，支持按 SQL Digest 精确查询、
//   跨库（cross-db）模糊匹配、以及基于内存配额的容量管理。
// - `bindingCache`：`BindingCache` 的默认实现，内部用 HashMap 存储
//   绑定，并用插入顺序队列实现简单的 FIFO 淘汰（类似 LRU 缓存的
//   “超出内存配额时逐出最旧条目”策略）。
// - `digestBiMap`：维护 “去库名摘要（noDBDigest）<-> SQL 摘要” 的
//   双向映射，用于跨库绑定匹配。
// - `BindingCacheUpdater` / `bindingCacheUpdater`：在缓存之上增加
//   与持久化存储（系统表）之间的增量/全量同步能力。

use crate::{
    Binding, BindingStore, BindingTime, Result, TableName, crossDBMatchBindings,
    noDBDigestFromBinding, pickCachedBinding, updateBindingUsageInfoToStorage,
};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, RwLock};

/// 测试专用的上下文键，用于在测试中注入或识别绑定缓存相关行为。
pub static bindingCacheTestKey: &str = "binding-cache-test-key";

/// 绑定缓存更新器接口：在 `BindingCache` 基础上补充与持久化存储同步的能力。
pub trait BindingCacheUpdater: BindingCache {
    /// 从存储加载绑定到缓存。`fullLoad` 为 true 时全量加载，
    /// 否则仅增量加载自上次更新时间之后发生变化的绑定。
    fn LoadFromStorageToCache(&self, fullLoad: bool, fromRemote: bool) -> Result<()>;
    /// 把缓存中各绑定的使用信息（如最近使用时间）回写到存储。
    fn UpdateBindingUsageInfoToStorage(&self) -> Result<()>;
    /// 返回缓存中已同步绑定的最新更新时间，作为下次增量加载的起点。
    fn LastUpdateTime(&self) -> BindingTime;
}

/// `BindingCacheUpdater` 的默认实现：组合一个内存缓存与一个绑定存储，
/// 负责在两者之间做增量同步。
pub struct bindingCacheUpdater {
    /// 底层的内存绑定缓存，所有 `BindingCache` 方法直接委托给它。
    pub BindingCache: Arc<dyn BindingCache>,
    /// 绑定的持久化存储（通常对应系统表），同步的数据来源与去向。
    pub store: Arc<dyn BindingStore>,
    /// 最近一次同步到的绑定更新时间，作为增量加载的水位线。
    pub lastUpdateTime: Mutex<BindingTime>,
    /// 缓存的内存配额（字节数），超过配额时缓存会淘汰旧条目。
    pub memQuota: Mutex<i64>,
}

// bindingCacheUpdater 对 BindingCache 接口的实现：
// 全部方法直接转发到内部的 BindingCache 实例，本身不做额外逻辑。
impl BindingCache for bindingCacheUpdater {
    fn MatchingBinding(
        &self,
        current_db: &str,
        noDBDigest: &str,
        tableNames: &[TableName],
    ) -> (Option<Arc<Binding>>, bool) {
        self.BindingCache
            .MatchingBinding(current_db, noDBDigest, tableNames)
    }

    fn GetBinding(&self, sqlDigest: &str) -> Option<Arc<Binding>> {
        self.BindingCache.GetBinding(sqlDigest)
    }

    fn GetAllBindings(&self) -> Vec<Arc<Binding>> {
        self.BindingCache.GetAllBindings()
    }

    fn SetBinding(&self, sqlDigest: String, binding: Arc<Binding>) -> Result<()> {
        self.BindingCache.SetBinding(sqlDigest, binding)
    }

    fn RemoveBinding(&self, sqlDigest: &str) {
        self.BindingCache.RemoveBinding(sqlDigest)
    }

    fn SetMemCapacity(&self, capacity: i64) {
        self.BindingCache.SetMemCapacity(capacity)
    }

    fn GetMemUsage(&self) -> i64 {
        self.BindingCache.GetMemUsage()
    }

    fn GetMemCapacity(&self) -> i64 {
        self.BindingCache.GetMemCapacity()
    }

    fn Size(&self) -> usize {
        self.BindingCache.Size()
    }

    fn Close(&self) {
        self.BindingCache.Close()
    }
}

impl BindingCacheUpdater for bindingCacheUpdater {
    fn LoadFromStorageToCache(&self, fullLoad: bool, _fromRemote: bool) -> Result<()> {
        // 确定加载起点：全量加载时从时间零点开始，
        // 增量加载时只读取上次同步水位之后更新过的绑定。
        let boundary = if fullLoad {
            BindingTime::default()
        } else {
            *self
                .lastUpdateTime
                .lock()
                .expect("update-time lock poisoned")
        };
        let mut latest = boundary;
        let bindings = self.store.read_bindings_since(boundary)?;
        for binding in bindings {
            // 记录本批数据中最大的更新时间，循环结束后推进水位线。
            latest = latest.max(binding.UpdateTime);
            let old = self.GetBinding(&binding.SQLDigest);
            let digest = binding.SQLDigest.clone();
            // 合并新旧绑定：pickCachedBinding 会根据状态挑选应缓存的版本，
            // 返回 None 表示该绑定已被删除/失效，需要从缓存中移除。
            match pickCachedBinding(old, [binding]) {
                Some(binding) => self.SetBinding(digest, binding)?,
                None => self.RemoveBinding(&digest),
            }
        }
        *self
            .lastUpdateTime
            .lock()
            .expect("update-time lock poisoned") = latest;
        Ok(())
    }

    fn UpdateBindingUsageInfoToStorage(&self) -> Result<()> {
        updateBindingUsageInfoToStorage(self.store.as_ref(), &self.GetAllBindings())
    }

    fn LastUpdateTime(&self) -> BindingTime {
        *self
            .lastUpdateTime
            .lock()
            .expect("update-time lock poisoned")
    }
}

/// 构造一个绑定缓存更新器，`max_cost` 为缓存的内存配额（字节数）。
pub fn NewBindingCacheUpdater(
    store: Arc<dyn BindingStore>,
    max_cost: i64,
) -> Arc<dyn BindingCacheUpdater> {
    Arc::new(bindingCacheUpdater {
        BindingCache: newBindingCache(max_cost),
        store,
        lastUpdateTime: Mutex::new(BindingTime::default()),
        memQuota: Mutex::new(max_cost),
    })
}

/// 摘要双向映射接口：维护 “去库名摘要（noDBDigest）<-> SQL 摘要（sqlDigest）”
/// 之间的对应关系。
///
/// noDBDigest 是把 SQL 中的库名去掉后再计算的摘要，同一条 SQL 在不同库下
/// 会得到相同的 noDBDigest，因而可用于跨库（cross-db）绑定匹配；
/// 一个 noDBDigest 可能对应多个 sqlDigest（一对多），反向则是一对一。
pub trait digestBiMap: Send + Sync {
    /// 建立 noDBDigest 与 sqlDigest 的映射（幂等，重复添加不产生重复项）。
    fn Add(&self, noDBDigest: String, sqlDigest: String);
    /// 按 sqlDigest 删除映射，并在必要时清理空的反向桶。
    fn Del(&self, sqlDigest: &str);
    /// 返回所有已登记的 sqlDigest（排序后），主要用于遍历清理。
    fn All(&self) -> Vec<String>;
    /// 由 noDBDigest 查出对应的所有 sqlDigest。
    fn NoDBDigest2SQLDigest(&self, noDBDigest: &str) -> Vec<String>;
    /// 由 sqlDigest 反查其 noDBDigest。
    fn SQLDigest2NoDBDigest(&self, sqlDigest: &str) -> Option<String>;
}

/// digestBiMap 的内部数据：两个方向的哈希表，由同一把读写锁保护。
#[derive(Default)]
struct DigestMaps {
    /// noDBDigest -> 该摘要下所有 sqlDigest 的列表（一对多）。
    no_db_to_sql: HashMap<String, Vec<String>>,
    /// sqlDigest -> noDBDigest（一对一）。
    sql_to_no_db: HashMap<String, String>,
}

/// `digestBiMap` 的默认实现，用读写锁保证并发安全。
pub struct digestBiMapImpl {
    maps: RwLock<DigestMaps>,
}

/// 构造一个空的摘要双向映射。
pub fn newDigestBiMap() -> Arc<dyn digestBiMap> {
    Arc::new(digestBiMapImpl {
        maps: RwLock::new(DigestMaps::default()),
    })
}

impl digestBiMap for digestBiMapImpl {
    fn Add(&self, noDBDigest: String, sqlDigest: String) {
        let mut maps = self.maps.write().expect("digest map poisoned");
        // 若该 sqlDigest 原先映射到不同的 noDBDigest，
        // 需要先把它从旧的反向桶中移除，保持双向一致。
        if let Some(old_no_db) = maps
            .sql_to_no_db
            .insert(sqlDigest.clone(), noDBDigest.clone())
        {
            if old_no_db != noDBDigest {
                if let Some(values) = maps.no_db_to_sql.get_mut(&old_no_db) {
                    values.retain(|value| value != &sqlDigest);
                }
            }
        }
        let values = maps.no_db_to_sql.entry(noDBDigest).or_default();
        if !values.contains(&sqlDigest) {
            values.push(sqlDigest);
        }
    }

    fn Del(&self, sqlDigest: &str) {
        let mut maps = self.maps.write().expect("digest map poisoned");
        if let Some(no_db_digest) = maps.sql_to_no_db.remove(sqlDigest) {
            // 从反向桶中剔除该 sqlDigest；若桶因此变空则整个删除，避免残留空桶。
            let remove_bucket = if let Some(values) = maps.no_db_to_sql.get_mut(&no_db_digest) {
                values.retain(|value| value != sqlDigest);
                values.is_empty()
            } else {
                false
            };
            if remove_bucket {
                maps.no_db_to_sql.remove(&no_db_digest);
            }
        }
    }

    fn All(&self) -> Vec<String> {
        let maps = self.maps.read().expect("digest map poisoned");
        let mut values: Vec<_> = maps.sql_to_no_db.keys().cloned().collect();
        values.sort();
        values
    }

    fn NoDBDigest2SQLDigest(&self, noDBDigest: &str) -> Vec<String> {
        self.maps
            .read()
            .expect("digest map poisoned")
            .no_db_to_sql
            .get(noDBDigest)
            .cloned()
            .unwrap_or_default()
    }

    fn SQLDigest2NoDBDigest(&self, sqlDigest: &str) -> Option<String> {
        self.maps
            .read()
            .expect("digest map poisoned")
            .sql_to_no_db
            .get(sqlDigest)
            .cloned()
    }
}

/// 绑定缓存接口：以 SQL 摘要为键缓存绑定，并支持跨库匹配与内存配额管理。
pub trait BindingCache: Send + Sync {
    /// 按去库名摘要做跨库匹配：先由 noDBDigest 找出候选绑定，
    /// 再结合当前库名与语句涉及的表名挑选最合适的绑定。
    /// 返回值第二项表示是否存在（即便未命中）相关的跨库绑定。
    fn MatchingBinding(
        &self,
        current_db: &str,
        noDBDigest: &str,
        tableNames: &[TableName],
    ) -> (Option<Arc<Binding>>, bool);
    /// 按 SQL 摘要精确获取绑定。
    fn GetBinding(&self, sqlDigest: &str) -> Option<Arc<Binding>>;
    /// 返回缓存中的全部绑定（按 SQL 摘要排序）。
    fn GetAllBindings(&self) -> Vec<Arc<Binding>>;
    /// 写入或覆盖一条绑定，可能触发容量淘汰。
    fn SetBinding(&self, sqlDigest: String, binding: Arc<Binding>) -> Result<()>;
    /// 按 SQL 摘要删除绑定。
    fn RemoveBinding(&self, sqlDigest: &str);
    /// 调整内存配额（字节数），缩小配额会立即淘汰超额条目。
    fn SetMemCapacity(&self, capacity: i64);
    /// 返回当前缓存的内存占用估算值（字节数）。
    fn GetMemUsage(&self) -> i64;
    /// 返回当前内存配额（字节数）。
    fn GetMemCapacity(&self) -> i64;
    /// 返回缓存中的绑定条数。
    fn Size(&self) -> usize;
    /// 清空缓存并释放相关索引。
    fn Close(&self);
}

/// 绑定缓存的内部可变状态，由读写锁整体保护。
#[derive(Default)]
struct CacheState {
    /// sqlDigest -> 绑定 的主存储。
    bindings: HashMap<String, Arc<Binding>>,
    /// 按插入先后记录的摘要队列，用于超额时按 FIFO 淘汰最旧条目
    /// （近似 LRU：LRU 淘汰“最久未使用”，此处淘汰“最早插入”）。
    insertion_order: VecDeque<String>,
    /// 当前内存占用估算值（字节数）。
    usage: i64,
    /// 内存配额上限（字节数）。
    capacity: i64,
}

/// `BindingCache` 的默认实现：HashMap 存储 + 插入顺序淘汰 + 摘要双向索引。
pub struct bindingCache {
    state: RwLock<CacheState>,
    /// 摘要双向映射，为跨库匹配提供 noDBDigest -> sqlDigest 的索引。
    digestMap: Arc<dyn digestBiMap>,
}

/// 构造一个绑定缓存，`maxCost` 为内存配额（字节数，负值按 0 处理）。
pub fn newBindingCache(maxCost: i64) -> Arc<dyn BindingCache> {
    Arc::new(bindingCache {
        state: RwLock::new(CacheState {
            capacity: maxCost.max(0),
            ..CacheState::default()
        }),
        digestMap: newDigestBiMap(),
    })
}

impl bindingCache {
    /// 内存占用超过配额时循环淘汰：每次弹出插入最早的摘要，
    /// 删除对应绑定并同步清理摘要索引，直到降回配额以内。
    fn evict_to_capacity(&self, state: &mut CacheState) {
        while state.usage > state.capacity {
            let Some(oldest) = state.insertion_order.pop_front() else {
                break;
            };
            if let Some(binding) = state.bindings.remove(&oldest) {
                state.usage -= binding.size().ceil() as i64;
                self.digestMap.Del(&oldest);
            }
        }
    }
}

impl BindingCache for bindingCache {
    fn MatchingBinding(
        &self,
        current_db: &str,
        noDBDigest: &str,
        tableNames: &[TableName],
    ) -> (Option<Arc<Binding>>, bool) {
        // 先用去库名摘要在索引中找到所有候选 sqlDigest，
        // 再取出对应绑定，交给 crossDBMatchBindings 做库名/表名层面的匹配。
        let digests = self.digestMap.NoDBDigest2SQLDigest(noDBDigest);
        let state = self.state.read().expect("binding cache poisoned");
        let bindings: Vec<_> = digests
            .iter()
            .filter_map(|digest| state.bindings.get(digest).cloned())
            .collect();
        crossDBMatchBindings(current_db, tableNames, &bindings)
    }

    fn GetBinding(&self, sqlDigest: &str) -> Option<Arc<Binding>> {
        self.state
            .read()
            .expect("binding cache poisoned")
            .bindings
            .get(sqlDigest)
            .cloned()
    }

    fn GetAllBindings(&self) -> Vec<Arc<Binding>> {
        let state = self.state.read().expect("binding cache poisoned");
        let mut bindings: Vec<_> = state.bindings.values().cloned().collect();
        bindings.sort_by(|a, b| a.SQLDigest.cmp(&b.SQLDigest));
        bindings
    }

    fn SetBinding(&self, sqlDigest: String, binding: Arc<Binding>) -> Result<()> {
        let no_db_digest = noDBDigestFromBinding(&binding)?;
        let mut state = self.state.write().expect("binding cache poisoned");
        // 覆盖旧值时先扣除旧绑定的内存占用，并把摘要从淘汰队列中移除，
        // 随后统一按“新插入”重新入队。
        if let Some(old) = state
            .bindings
            .insert(sqlDigest.clone(), Arc::clone(&binding))
        {
            state.usage -= old.size().ceil() as i64;
            state.insertion_order.retain(|value| value != &sqlDigest);
        }
        state.usage += binding.size().ceil() as i64;
        state.insertion_order.push_back(sqlDigest.clone());
        self.digestMap.Add(no_db_digest, sqlDigest);
        // 写入后立即检查配额，必要时淘汰最旧条目。
        self.evict_to_capacity(&mut state);
        Ok(())
    }

    fn RemoveBinding(&self, sqlDigest: &str) {
        let mut state = self.state.write().expect("binding cache poisoned");
        if let Some(binding) = state.bindings.remove(sqlDigest) {
            state.usage -= binding.size().ceil() as i64;
        }
        state.insertion_order.retain(|value| value != sqlDigest);
        self.digestMap.Del(sqlDigest);
    }

    fn SetMemCapacity(&self, capacity: i64) {
        let mut state = self.state.write().expect("binding cache poisoned");
        state.capacity = capacity.max(0);
        self.evict_to_capacity(&mut state);
    }

    fn GetMemUsage(&self) -> i64 {
        self.state.read().expect("binding cache poisoned").usage
    }

    fn GetMemCapacity(&self) -> i64 {
        self.state.read().expect("binding cache poisoned").capacity
    }

    fn Size(&self) -> usize {
        self.state
            .read()
            .expect("binding cache poisoned")
            .bindings
            .len()
    }

    fn Close(&self) {
        let mut state = self.state.write().expect("binding cache poisoned");
        state.bindings.clear();
        state.insertion_order.clear();
        state.usage = 0;
        for digest in self.digestMap.All() {
            self.digestMap.Del(&digest);
        }
    }
}
