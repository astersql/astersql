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

// 基于 Moka TinyLFU 的加权统计缓存（对应 Go Ristretto LFU）。
//
// 两级所有权模型：主缓存持有完整 `Table`；被容量淘汰后在 `keySetShard`
// 中保留“壳表”（直方图/TopN 等可丢弃数据已剥离），供后续读到元信息。
// TinyLFU（Least Frequently Used 近似）按访问频率与权重做准入与淘汰；
// Moka 维护日志异步，`WaitForAsyncUpdates` 是可见性/淘汰屏障。

use cache_internal::StatsCacheInner;
use moka::notification::RemovalCause;
use moka::policy::EvictionPolicy;
use moka::sync::Cache;
use statistics::{AllEvicted, CopyIntent, Table};
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use crate::key_set_shard::keySetShard;

/// Moka 同步缓存：键为表 ID，值为共享的统计表。
type PrimaryCache = Cache<i64, Arc<Table>>;

/// LFU 构造或容量调整失败时的错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LFUError(String);

impl fmt::Display for LFUError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for LFUError {}

/// LFU 运行期计数：累计加入/淘汰成本、淘汰次数与拒绝次数。
#[derive(Default)]
pub struct LFUMetrics {
    costAdded: AtomicU64,
    costEvicted: AtomicU64,
    evictions: AtomicU64,
    rejections: AtomicU64,
}

impl LFUMetrics {
    /// 累计已计入缓存的内存成本（跟踪用量）。
    pub fn CostAdded(&self) -> u64 {
        self.costAdded.load(Ordering::Acquire)
    }

    /// 累计因淘汰/拒绝而扣减的内存成本。
    pub fn CostEvicted(&self) -> u64 {
        self.costEvicted.load(Ordering::Acquire)
    }

    /// 因容量等原因发生的淘汰次数。
    pub fn Evictions(&self) -> u64 {
        self.evictions.load(Ordering::Acquire)
    }

    /// 条目过大被拒绝进入主缓存的次数。
    pub fn Rejections(&self) -> u64 {
        self.rejections.load(Ordering::Acquire)
    }
}

/// 多实例共享的可变状态：二级键集、成本、容量、关闭标志、准入锁与频率表。
struct SharedState {
    resultKeySet: keySetShard,
    cost: AtomicI64,
    maxCost: AtomicI64,
    closed: AtomicBool,
    admission: Mutex<()>,
    frequencies: Mutex<HashMap<i64, u64>>,
    metrics: LFUMetrics,
}

impl SharedState {
    /// 原子更新总成本，并按正负分别累加 CostAdded / CostEvicted 指标。
    fn addCost(&self, delta: i64) {
        let cost = self.cost.fetch_add(delta, Ordering::AcqRel) + delta;
        if delta >= 0 {
            self.metrics
                .costAdded
                .fetch_add(delta as u64, Ordering::Relaxed);
        } else {
            self.metrics
                .costEvicted
                .fetch_add(delta.unsigned_abs(), Ordering::Relaxed);
        }
        setCostGauge(cost);
    }

    /// 记录一次访问并返回更新后的频率，供加权准入比较。
    fn recordFrequency(&self, key: i64) -> u64 {
        let mut frequencies = self
            .frequencies
            .lock()
            .expect("LFU frequency lock poisoned");
        let frequency = frequencies.entry(key).or_default();
        *frequency = frequency.saturating_add(1);
        *frequency
    }

    /// 读取键的当前频率；未见过则视为 0。
    fn frequency(&self, key: i64) -> u64 {
        self.frequencies
            .lock()
            .expect("LFU frequency lock poisoned")
            .get(&key)
            .copied()
            .unwrap_or_default()
    }

    /// 淘汰后保留壳表：复制可写副本并丢弃列/索引上非必要的直方图与 TopN 数据。
    fn retainEvictedShell(&self, key: i64, table: &Table) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        let mut evicted = table.CopyAs(CopyIntent::AllDataWritable);
        // Keep the Go HistColl.DropEvicted condition exactly. This is written
        // locally because the current canonical Table method still has an old
        // condition that does not match the Go eviction predicate.
        // 与 Go HistColl.DropEvicted 条件一致：已初始化且未全部淘汰时再丢数据。
        for column in evicted.HistColl.Columns.values_mut() {
            if column.IsStatsInitialized() && column.GetEvictedStatus() != AllEvicted {
                column.DropUnnecessaryData();
            }
        }
        for index in evicted.HistColl.Indices.values_mut() {
            if index.IsStatsInitialized() && index.GetEvictedStatus() != AllEvicted {
                index.DropUnnecessaryData();
            }
        }
        let evicted = Arc::new(evicted);
        let after = evicted.MemoryUsage().TotalTrackingMemUsage();
        self.resultKeySet.AddKeyValue(key, evicted);
        self.addCost(after);
    }

    /// 先写入壳表再扣减完整表成本，完成从主缓存到二级集合的内存交接。
    fn dropMemory(&self, key: i64, table: Arc<Table>) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        let before = table.MemoryUsage().TotalTrackingMemUsage();
        self.retainEvictedShell(key, &table);
        self.addCost(-before);
    }

    /// Moka 淘汰监听器：按容量/过期做拒绝或淘汰统计，替换/显式删除仅扣成本。
    fn onRemoval(&self, key: i64, table: Arc<Table>, cause: RemovalCause) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        match cause {
            RemovalCause::Size | RemovalCause::Expired => {
                // 单表跟踪内存超过 maxCost 视为拒绝，否则计为淘汰
                if table.MemoryUsage().TotalTrackingMemUsage()
                    > self.maxCost.load(Ordering::Acquire)
                {
                    self.metrics.rejections.fetch_add(1, Ordering::Relaxed);
                    incrementRejectCounter();
                } else {
                    self.metrics.evictions.fetch_add(1, Ordering::Relaxed);
                    incrementEvictCounter();
                }
                self.dropMemory(key, table);
            }
            RemovalCause::Replaced | RemovalCause::Explicit => {
                self.addCost(-table.MemoryUsage().TotalTrackingMemUsage());
            }
        }
    }
}

/// A weighted TinyLFU cache with the same two-level ownership model as the Go
/// Ristretto implementation. Moka's maintenance log is asynchronous;
/// `WaitForAsyncUpdates` is the visibility/eviction barrier.
///
/// 加权 TinyLFU 缓存，两级所有权与 Go Ristretto 一致；
/// Moka 维护日志异步，`WaitForAsyncUpdates` 为可见性/淘汰屏障。
#[derive(Clone)]
pub struct LFU {
    cache: Arc<RwLock<PrimaryCache>>,
    state: Arc<SharedState>,
}

/// 将表跟踪内存映射为 Moka weigher 所需的 `u32` 权重。
fn cacheWeight(table: &Arc<Table>) -> u32 {
    table
        .MemoryUsage()
        .TotalTrackingMemUsage()
        .max(0)
        .min(i64::from(u32::MAX)) as u32
}

/// 构建带 TinyLFU 策略与淘汰监听器的主缓存。
fn buildPrimary(maxCost: i64, state: &Arc<SharedState>) -> PrimaryCache {
    let listenerState = Arc::clone(state);
    Cache::builder()
        .max_capacity(maxCost.max(0) as u64)
        .eviction_policy(EvictionPolicy::tiny_lfu())
        .weigher(|_, table: &Arc<Table>| cacheWeight(table))
        .eviction_listener(move |key, table, cause| {
            listenerState.onRemoval(*key, table, cause);
        })
        .build()
}

/// Creates a weighted TinyLFU cache. A zero capacity follows TiDB and becomes
/// twenty percent of total system memory.
///
/// 创建加权 TinyLFU；容量为 0 时与 TiDB 一致，取系统总内存的 20%。
pub fn NewLFU(totalMemCost: i64) -> Result<LFU, LFUError> {
    let maxCost = adjustMemCost(totalMemCost)?;
    setCapacityGauge(maxCost);
    let state = Arc::new(SharedState {
        resultKeySet: keySetShard::newKeySetShard(),
        cost: AtomicI64::new(0),
        maxCost: AtomicI64::new(maxCost),
        closed: AtomicBool::new(false),
        admission: Mutex::new(()),
        frequencies: Mutex::new(HashMap::new()),
        metrics: LFUMetrics::default(),
    });
    Ok(LFU {
        cache: Arc::new(RwLock::new(buildPrimary(maxCost, &state))),
        state,
    })
}

/// 规范化容量：负数报错；0 转为系统内存 20%；其余原样返回。
fn adjustMemCost(totalMemCost: i64) -> Result<i64, LFUError> {
    if totalMemCost < 0 {
        return Err(LFUError("LFU capacity cannot be negative".into()));
    }
    if totalMemCost != 0 {
        return Ok(totalMemCost);
    }
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    let total = system.total_memory();
    if total == 0 {
        return Err(LFUError("cannot read total memory".into()));
    }
    i64::try_from(total.saturating_mul(20) / 100)
        .map_err(|_| LFUError("total memory exceeds i64 capacity".into()))
}

impl LFU {
    /// 先记频率，再查主缓存，未命中则回落到二级 `resultKeySet`。
    pub fn Get(&self, tid: i64) -> Option<Arc<Table>> {
        self.state.recordFrequency(tid);
        self.cache
            .read()
            .expect("LFU primary cache lock poisoned")
            .get(&tid)
            .or_else(|| self.state.resultKeySet.Get(tid))
    }

    /// 写入统计表：先发布到二级集合，再按容量做拒绝或加权准入后插入主缓存。
    pub fn Put(&self, tblID: i64, tbl: Arc<Table>) -> bool {
        if self.state.closed.load(Ordering::Acquire) {
            return false;
        }
        let cost = tbl.MemoryUsage().TotalTrackingMemUsage();
        let candidate_frequency = self.state.recordFrequency(tblID);
        // 先写入二级集合，保证读侧在主缓存准入完成前也能 Get 到
        self.state.resultKeySet.AddKeyValue(tblID, tbl.clone());
        self.state.addCost(cost);
        let _admission = self
            .state
            .admission
            .lock()
            .expect("LFU admission lock poisoned");
        let cache = self.cache.read().expect("LFU primary cache lock poisoned");
        if cost > self.state.maxCost.load(Ordering::Acquire) {
            // Moka intentionally drops over-capacity insertions before they
            // enter the cache and does not invoke the eviction listener. The
            // Go Ristretto implementation invokes OnReject, so bridge that
            // semantic difference explicitly.
            // 超容量条目：显式失效旧键并走拒绝路径，桥接 Go OnReject 语义。
            if cache.contains_key(&tblID) {
                cache.invalidate(&tblID);
                cache.run_pending_tasks();
            }
            self.state
                .metrics
                .rejections
                .fetch_add(1, Ordering::Relaxed);
            incrementRejectCounter();
            self.state.dropMemory(tblID, tbl);
            return true;
        }
        let resident = cache.contains_key(&tblID);
        if !resident {
            cache.run_pending_tasks();
            let max_cost = self.state.maxCost.load(Ordering::Acquire) as u64;
            let mut weighted_size = cache.weighted_size();
            if weighted_size.saturating_add(cost as u64) > max_cost {
                // Ristretto's weighted admission makes room when a cheaper
                // candidate can replace a larger resident. Moka's TinyLFU
                // compares frequency but otherwise retains the first entry on
                // a tie, so perform the weighted tie-break explicitly.
                // 为候选腾出空间：优先淘汰频率不高且权重更大的驻留项。
                let mut victims = cache
                    .iter()
                    .map(|(key, table)| {
                        (
                            *key,
                            table.clone(),
                            u64::from(cacheWeight(&table)),
                            self.state.frequency(*key),
                        )
                    })
                    .filter(|(_, _, _, frequency)| *frequency <= candidate_frequency)
                    .collect::<Vec<_>>();
                victims.sort_unstable_by_key(|(_, _, weight, frequency)| {
                    (*frequency, std::cmp::Reverse(*weight))
                });
                for (key, victim, weight, _) in victims {
                    if weighted_size.saturating_add(cost as u64) <= max_cost {
                        break;
                    }
                    cache.invalidate(&key);
                    cache.run_pending_tasks();
                    self.state.retainEvictedShell(key, &victim);
                    self.state.metrics.evictions.fetch_add(1, Ordering::Relaxed);
                    incrementEvictCounter();
                    weighted_size = weighted_size.saturating_sub(weight);
                }
            }
        }
        cache.insert(tblID, tbl);
        // Ristretto applies resident-key replacements synchronously. Moka keeps
        // their listener in its maintenance log, so drain it before returning.
        // 替换已存在键时排空异步监听，对齐 Ristretto 同步语义。
        if resident {
            cache.run_pending_tasks();
        }
        true
    }

    /// 从主缓存与二级集合删除，并清除频率记录。
    pub fn Del(&self, tblID: i64) {
        self.cache
            .read()
            .expect("LFU primary cache lock poisoned")
            .invalidate(&tblID);
        self.state.resultKeySet.Remove(tblID);
        self.state
            .frequencies
            .lock()
            .expect("LFU frequency lock poisoned")
            .remove(&tblID);
    }

    /// 当前跟踪的总内存成本。
    pub fn Cost(&self) -> i64 {
        self.state.cost.load(Ordering::Acquire)
    }

    /// 枚举二级集合中仍保留的全部表（含壳表）。
    pub fn Values(&self) -> Vec<Arc<Table>> {
        self.state
            .resultKeySet
            .Keys()
            .into_iter()
            .filter_map(|key| self.state.resultKeySet.Get(key))
            .collect()
    }

    /// 二级集合条目数（主缓存与壳表合计键数）。
    pub fn Len(&self) -> usize {
        self.state.resultKeySet.Len()
    }

    /// 浅拷贝为 `StatsCacheInner` 装箱对象（共享底层状态）。
    pub fn Copy(&self) -> Box<dyn StatsCacheInner> {
        Box::new(self.clone())
    }

    /// 调整最大容量：重建主缓存并按新上限重新准入或淘汰驻留项。
    pub fn SetCapacity(&self, maxCost: i64) {
        let Ok(maxCost) = adjustMemCost(maxCost) else {
            return;
        };
        let mut cache = self.cache.write().expect("LFU primary cache lock poisoned");
        cache.run_pending_tasks();
        let resident = cache
            .iter()
            .map(|(key, table)| (*key, table))
            .collect::<Vec<_>>();
        self.state.maxCost.store(maxCost, Ordering::Release);
        let replacement = buildPrimary(maxCost, &self.state);
        for (key, table) in resident {
            if table.MemoryUsage().TotalTrackingMemUsage() > maxCost {
                self.state.metrics.evictions.fetch_add(1, Ordering::Relaxed);
                incrementEvictCounter();
                self.state.dropMemory(key, table);
            } else {
                replacement.insert(key, table);
            }
        }
        replacement.run_pending_tasks();
        *cache = replacement;
        setCapacityGauge(maxCost);
        setCostGauge(self.Cost());
    }

    /// 排空 Moka 异步维护任务，使淘汰/替换对读侧立即可见。
    pub fn WaitForAsyncUpdates(&self) {
        self.cache
            .read()
            .expect("LFU primary cache lock poisoned")
            .run_pending_tasks();
    }

    /// 返回运行期指标引用。
    pub fn metrics(&self) -> &LFUMetrics {
        &self.state.metrics
    }

    /// 关闭缓存：置 closed、清空主缓存与二级集合及频率表；重复调用幂等。
    pub fn Close(&self) {
        if self.state.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let cache = self.cache.read().expect("LFU primary cache lock poisoned");
        cache.invalidate_all();
        cache.run_pending_tasks();
        self.state.resultKeySet.Clear();
        self.state
            .frequencies
            .lock()
            .expect("LFU frequency lock poisoned")
            .clear();
    }

    /// 清空内容但不关闭，后续仍可 Put。
    pub fn Clear(&self) {
        let cache = self.cache.read().expect("LFU primary cache lock poisoned");
        cache.invalidate_all();
        cache.run_pending_tasks();
        self.state.resultKeySet.Clear();
        self.state
            .frequencies
            .lock()
            .expect("LFU frequency lock poisoned")
            .clear();
    }

    /// 若当前成本超过上限则等待异步淘汰完成。
    fn triggerEvict(&self) {
        if self.Cost() > self.state.maxCost.load(Ordering::Acquire) {
            self.WaitForAsyncUpdates();
        }
    }

    /// 对外触发一次可能的异步淘汰等待。
    pub fn TriggerEvict(&self) {
        self.triggerEvict();
    }
}

/// 将 `LFU` 适配为 `StatsCacheInner` 统一接口。
impl StatsCacheInner for LFU {
    fn Get(&self, tid: i64) -> Option<Arc<Table>> {
        LFU::Get(self, tid)
    }
    fn Put(&mut self, tid: i64, table: Arc<Table>) -> bool {
        LFU::Put(self, tid, table)
    }
    fn Del(&mut self, tid: i64) {
        LFU::Del(self, tid)
    }
    fn Cost(&self) -> i64 {
        LFU::Cost(self)
    }
    fn Values(&self) -> Vec<Arc<Table>> {
        LFU::Values(self)
    }
    fn Len(&self) -> usize {
        LFU::Len(self)
    }
    fn Copy(&self) -> Box<dyn StatsCacheInner> {
        LFU::Copy(self)
    }
    fn SetCapacity(&mut self, capacity: i64) {
        LFU::SetCapacity(self, capacity)
    }
    fn Close(&mut self) {
        LFU::Close(self)
    }
    fn TriggerEvict(&mut self) {
        LFU::TriggerEvict(self)
    }
    fn WaitForAsyncUpdates(&mut self) {
        LFU::WaitForAsyncUpdates(self)
    }
}

/// 更新缓存当前成本 Prometheus gauge（若已初始化）。
fn setCostGauge(value: i64) {
    unsafe {
        if let Some(gauge) = cache_metrics::cache_metrics::CostGauge.as_ref() {
            gauge.set(value as f64);
        }
    }
}

/// 更新缓存容量 Prometheus gauge（若已初始化）。
fn setCapacityGauge(value: i64) {
    unsafe {
        if let Some(gauge) = cache_metrics::cache_metrics::CapacityGauge.as_ref() {
            gauge.set(value as f64);
        }
    }
}

/// 递增淘汰计数器指标。
fn incrementEvictCounter() {
    unsafe {
        if let Some(counter) = cache_metrics::cache_metrics::EvictCounter.as_ref() {
            counter.inc();
        }
    }
}

/// 递增拒绝计数器指标。
fn incrementRejectCounter() {
    unsafe {
        if let Some(counter) = cache_metrics::cache_metrics::RejectCounter.as_ref() {
            counter.inc();
        }
    }
}
