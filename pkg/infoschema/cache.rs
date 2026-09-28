// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// InfoSchema 版本缓存。
//
// 按 schema 版本与时间戳降序缓存 `InfoSchema` 快照，支持按版本/快照时间戳查找；
// `Data` 跟踪近期最小时间戳，供 GC/保活；空 schema 版本集合用于填补版本空洞。
// InfoSchema：会话可见的库表元数据快照；schema version 随 DDL 递增。

// 只有 gcOldVersion 保留外部 helper 调用形状，并明确作为后续异步接线点。

/* Mechanical draft retained for migration history.
use std::collections::HashSet;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

type SchemaRef = Arc<dyn InfoSchema + Send + Sync>;

// InfoCacheInner 集中放置受 Go RWMutex 保护的可变字段。
struct InfoCacheInner {
    // cache 同时按版本和时间戳降序排列，源实现假定两者顺序一致。
    cache: Vec<schemaAndTimestamp>,
    /// 最大缓存条数。
    capacity: usize,
    emptySchemaVersions: HashSet<i64>,
    firstKnownSchemaVersion: i64,
    lastCheckVersion: i64,
    lastCheckTime: Option<Instant>,
}

// InfoCache 对应 Go 缓存；共享引用通过 Arc 表达接口对象的跨调用持有语义。
pub struct InfoCache {
    inner: RwLock<InfoCacheInner>,
    store: tidbkv::Storage,
    pub Data: Arc<Data>,
}

#[derive(Clone)]
struct schemaAndTimestamp {
    infoschema: SchemaRef,
    timestamp: i64,
}

// NewCache 对应 Go 构造函数，预分配容量并创建 V2 共用 Data。
pub fn NewCache(store: tidbkv::Storage, capacity: usize) -> InfoCache {
    InfoCache {
        inner: RwLock::new(InfoCacheInner {
            cache: Vec::with_capacity(capacity),
            capacity,
            emptySchemaVersions: HashSet::new(),
            firstKnownSchemaVersion: 0,
            lastCheckVersion: 0,
            lastCheckTime: None,
        }),
        store,
        Data: Arc::new(NewData()),
    }
}

impl InfoCache {
    // GetAndResetRecentInfoSchemaTS 原子交换最近一轮 V2 API 使用的最小 TS，供 GC safepoint 上报。
    /// 取出并重置近期最小时间戳为 `now`。
    pub fn GetAndResetRecentInfoSchemaTS(&self, now: u64) -> u64 {
        self.Data
            .recentMinTS
            .swap(now, std::sync::atomic::Ordering::SeqCst)
    }

    // ReSize 保留最新的 capacity 项；改变容量时不会重排现有降序序列。
    /// 调整容量并截断过长的缓存向量。
    pub fn ReSize(&self, capacity: usize) {
        let mut inner = self.inner.write().unwrap();
        if inner.capacity == capacity {
            return;
        }
        inner.cache.truncate(capacity);
        let mut replacement = Vec::with_capacity(capacity);
        replacement.append(&mut inner.cache);
        inner.cache = replacement;
        inner.capacity = capacity;
    }

    // Size 与 Go 一样在锁内读取，导出供测试观察。
    /// 当前缓存条目数。
    pub fn Size(&self) -> usize {
        self.inner.read().unwrap().cache.len()
    }

    // Reset 清空快照但保留 store、Data 与空版本记录。
    /// 清空缓存并设置新容量。
    pub fn Reset(&self, capacity: usize) {
        let mut inner = self.inner.write().unwrap();
        inner.cache = Vec::with_capacity(capacity);
        inner.capacity = capacity;
    }

    // Upsert 用单个新快照替换缓存，并返回显式失效旧对象的清理闭包。
    pub fn Upsert(&self, schema: SchemaRef, schema_ts: u64) -> Box<dyn FnOnce() + Send> {
        let mut inner = self.inner.write().unwrap();
        let old = std::mem::replace(
            &mut inner.cache,
            vec![schemaAndTimestamp {
                infoschema: schema.clone(),
                timestamp: schema_ts as i64,
            }],
        );
        inner.firstKnownSchemaVersion = schema.SchemaMetaVersion();

        Box::new(move || {
            // Go 会把仍被引用的具体对象清零以暴露陈旧引用；Rust 通过接口失效钩子保留该意图。
            for item in old {
                item.infoschema.invalidate_after_switch();
            }
            logutil::BgLogger().Info(
                "reset the old infoschema after v1 v2 switch, using the stale object will panic",
            );
        })
    }

    // GetLatest 读取降序序列首项，并维护与 Go 相同的命中指标。
    /// 返回最新（下标 0）的 InfoSchema。
    pub fn GetLatest(&self) -> Option<SchemaRef> {
        infoschema_metrics::GetLatestCounter.Inc();
        let result = self.inner.read().unwrap().cache.first().map(|item| item.infoschema.clone());
        if result.is_some() {
            infoschema_metrics::HitLatestCounter.Inc();
        }
        result
    }

    /// `Size` 别名，对齐 Go 命名。
    pub fn Len(&self) -> usize {
        self.inner.read().unwrap().cache.len()
    }

    // GetEmptySchemaVersions 返回快照副本，避免把锁内集合的可变引用泄漏给调用方。
    /// 克隆空版本集合。
    pub fn GetEmptySchemaVersions(&self) -> HashSet<i64> {
        self.inner.read().unwrap().emptySchemaVersions.clone()
    }

    // getSchemaByTimestampNoLock 线性扫描：容量很小且 0 TS 必须跳过，首项命中通常更快。
    fn getSchemaByTimestampNoLock(inner: &InfoCacheInner, ts: u64) -> Option<SchemaRef> {
        logutil::BgLogger().DebugTS("SCHEMA CACHE get schema", ts);
        for (index, item) in inner.cache.iter().enumerate() {
            if item.timestamp == 0 || ts < item.timestamp as u64 {
                continue;
            }
            if index == 0 {
                return Some(item.infoschema.clone());
            }

            if inner.cache[index - 1].timestamp as u64 > ts {
                let newer = inner.cache[index - 1].infoschema.SchemaMetaVersion();
                let current = item.infoschema.SchemaMetaVersion();
                // 相邻版本连续，可安全返回当前条目。
                if newer == current + 1 {
                    return Some(item.infoschema.clone());
                }
                if newer > current
                    && (current + 1..newer)
                        .all(|version| inner.emptySchemaVersions.contains(&version))
                {
                    // 缺口全部是无 schema_diff 的版本时，当前快照仍覆盖目标 TS。
                    return Some(item.infoschema.clone());
                }
            }
            // 第一个时间范围候选不连续时，更旧快照同样不适用，可提前结束。
            break;
        }
        logutil::BgLogger().Debug("SCHEMA CACHE no schema found");
        None
    }

    // GetByVersion 在读锁内调用降序二分查找。
    /// 按 schema 版本二分查找（缓存按版本降序）。
    pub fn GetByVersion(&self, version: i64) -> Option<SchemaRef> {
        let inner = self.inner.read().unwrap();
        Self::getByVersionNoLock(&inner, version)
    }

    fn getByVersionNoLock(inner: &InfoCacheInner, version: i64) -> Option<SchemaRef> {
        infoschema_metrics::GetVersionCounter.Inc();
        // partition_point 对应 Go sort.Search：找第一个 schema version <= 请求值的位置。
        let index = inner
            .cache
            .partition_point(|item| item.infoschema.SchemaMetaVersion() > version);
        let item = inner.cache.get(index)?;
        let found = item.infoschema.SchemaMetaVersion();

        // 请求新于最新版本时必须 miss；Upsert 后只有 firstKnown 以内的历史连续性可信。
        if found == version || (index != 0 && found >= inner.firstKnownSchemaVersion) {
            infoschema_metrics::HitVersionCounter.Inc();
            return Some(item.infoschema.clone());
        }
        None
    }

    // GetBySnapshotTS 返回生效时间不晚于 snapshotTS 的最新且版本连续快照。
    /// 按事务快照时间戳获取可见的 InfoSchema。
    pub fn GetBySnapshotTS(&self, snapshot_ts: u64) -> Option<SchemaRef> {
        infoschema_metrics::GetTSCounter.Inc();
        let inner = self.inner.read().unwrap();
        let result = Self::getSchemaByTimestampNoLock(&inner, snapshot_ts);
        if result.is_some() {
            infoschema_metrics::HitTSCounter.Inc();
        }
        result
    }

    // Insert 仅保证缓存“足够新”的快照，并维持版本降序排列。
    pub fn Insert(self: &Arc<Self>, schema: SchemaRef, schema_ts: u64) -> bool {
        let version = schema.SchemaMetaVersion();
        logutil::BgLogger().DebugSchema("INSERT SCHEMA", schema_ts, version);
        let mut spawn_gc = false;

        {
            let mut inner = self.inner.write().unwrap();
            match inner.lastCheckTime {
                None => {
                    inner.lastCheckVersion = version;
                    inner.lastCheckTime = Some(Instant::now());
                }
                Some(last)
                    if version > inner.lastCheckVersion + gcCheckInterval
                        && last.elapsed() > Duration::from_secs(60) =>
                {
                    inner.lastCheckVersion = version;
                    inner.lastCheckTime = Some(Instant::now());
                    spawn_gc = true;
                }
                _ => {}
            }

            let index = inner
                .cache
                .partition_point(|item| item.infoschema.SchemaMetaVersion() > version);
            if let Some(cached) = inner
                .cache
                .get_mut(index)
                .filter(|item| item.infoschema.SchemaMetaVersion() == version)
            {
                let same_generation = IsV2(cached.infoschema.as_ref()) == IsV2(schema.as_ref());
                if same_generation {
                    if schema_ts > 0 && cached.timestamp == 0 {
                        cached.timestamp = schema_ts as i64;
                    } else if IsV2(schema.as_ref()) {
                        // V2 同版本对象仍可能携带更新后的惰性缓存，需替换接口值。
                        cached.infoschema = schema;
                    }
                    return true;
                }
                cached.infoschema = schema.clone();
                cached.timestamp = schema_ts as i64;
                return true;
            }

            let item = schemaAndTimestamp {
                infoschema: schema.clone(),
                timestamp: schema_ts as i64,
            };
            if inner.cache.len() < inner.capacity {
                inner.cache.insert(index, item);
                if inner.cache.len() == 1 {
                    inner.firstKnownSchemaVersion = version;
                }
            } else if index < inner.cache.len() {
                inner.cache.insert(index, item);
                inner.cache.pop();
            } else {
                // 比所有已缓存快照都旧且容量已满，拒绝插入。
                return false;
            }
        }

        if spawn_gc {
            let cache = Arc::clone(self);
            // Go 使用 goroutine；这里以短生命周期线程保留异步、不阻塞 Insert 的语义。
            std::thread::spawn(move || cache.gcOldVersion());
        }
        true
    }

    // InsertEmptySchemaVersion 记录无 diff 的版本，超容量时按版本升序淘汰最旧项。
    /// 记录空 schema 版本；超过容量时从最小版本开始淘汰。
    pub fn InsertEmptySchemaVersion(&self, version: i64) {
        let mut inner = self.inner.write().unwrap();
        inner.emptySchemaVersions.insert(version);
        if inner.emptySchemaVersions.len() > inner.capacity {
            let mut versions: Vec<i64> = inner.emptySchemaVersions.iter().copied().collect();
            versions.sort_unstable();
            for old in versions {
                inner.emptySchemaVersions.remove(&old);
                if inner.emptySchemaVersions.len() <= inner.capacity {
                    break;
                }
            }
        }
    }

    // gcOldVersion 对应后台压缩：只有支持 helper.Storage 的 store 才查询最老 schema version。
    fn gcOldVersion(&self) {
        let Some(store) = self.store.as_helper_storage() else {
            return;
        };
        let helper = helper::NewHelper(store);
        let version = match meta::GetOldestSchemaVersion(&helper) {
            Ok(version) => version,
            Err(error) => {
                // 外部读取失败只记日志；下一轮版本/时间阈值满足时会再次尝试。
                logutil::BgLogger().WarnError("failed to GC old schema version", error);
                return;
            }
        };
        let started = Instant::now();
        let (deleted, total) = self.Data.GCOldVersion(version);
        let current = self.inner.read().unwrap().lastCheckVersion;
        logutil::BgLogger().InfoGC(
            "GC compact old schema version",
            current,
            version,
            deleted,
            total,
            started.elapsed(),
        );
    }
}

// gcCheckInterval 保留 Go 常量名；每跨过 128 个版本才考虑触发一次后台压缩。
const gcCheckInterval: i64 = 128;
*/

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::infoschema::InfoSchema;

/// 共享的 InfoSchema 引用。
pub type SchemaRef = Arc<dyn InfoSchema>;

#[derive(Default)]
/// 缓存附属数据：原子维护近期观测到的最小时间戳。
pub struct Data {
    /// 近期最小 schema 时间戳（0 表示尚未设置）。
    recent_min_ts: AtomicU64,
}

impl Data {
    /// 创建默认 Data。
    pub fn new() -> Self {
        Self::default()
    }
    /// 读取当前记录的最小时间戳。
    pub fn recent_min_ts(&self) -> u64 {
        self.recent_min_ts.load(Ordering::Acquire)
    }
    /// 若 `ts` 更小（或尚未设置）则 CAS 更新最小时间戳，防止被 GC 过早回收。
    pub fn keep_alive(&self, ts: u64) {
        let mut current = self.recent_min_ts.load(Ordering::Acquire);
        while (current == 0 || ts < current)
            && self
                .recent_min_ts
                .compare_exchange_weak(current, ts, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            current = self.recent_min_ts.load(Ordering::Acquire);
        }
    }
}

#[derive(Clone)]
/// 缓存条目：InfoSchema 与其对应的 schema 时间戳。
struct SchemaAndTimestamp {
    infoschema: SchemaRef,
    timestamp: i64,
}

/// 受读写锁保护的可变缓存状态。
struct CacheState {
    /// 按版本降序排列的条目（最新在前）。
    cache: Vec<SchemaAndTimestamp>,
    capacity: usize,
    /// 已知为空的中间 schema 版本，用于跨版本空洞查找。
    empty_schema_versions: HashSet<i64>,
    /// 缓存中已知的最早 schema 版本。
    first_known_schema_version: i64,
    /// 上次考虑触发 GC 时的 schema 版本。
    last_check_version: i64,
    /// 上次 GC 检查墙钟时间。
    last_check_time: Option<Instant>,
}

/// 按版本与时间戳排序的 InfoSchema 缓存。
/// Version- and timestamp-ordered InfoSchema cache.
pub struct InfoCache {
    state: RwLock<CacheState>,
    pub Data: Arc<Data>,
}

/// 以给定容量创建空缓存。
pub fn NewCache(capacity: usize) -> InfoCache {
    InfoCache {
        state: RwLock::new(CacheState {
            cache: Vec::with_capacity(capacity),
            capacity,
            empty_schema_versions: HashSet::new(),
            first_known_schema_version: 0,
            last_check_version: 0,
            last_check_time: None,
        }),
        Data: Arc::new(Data::new()),
    }
}

impl InfoCache {
    pub fn GetAndResetRecentInfoSchemaTS(&self, now: u64) -> u64 {
        self.Data.recent_min_ts.swap(now, Ordering::AcqRel)
    }

    pub fn ReSize(&self, capacity: usize) {
        let mut state = self.state.write().expect("infoschema cache lock poisoned");
        if state.capacity == capacity {
            return;
        }
        state.cache.truncate(capacity);
        let mut resized = Vec::with_capacity(capacity);
        resized.append(&mut state.cache);
        state.cache = resized;
        state.capacity = capacity;
    }

    pub fn Size(&self) -> usize {
        self.state
            .read()
            .expect("infoschema cache lock poisoned")
            .cache
            .len()
    }
    pub fn Len(&self) -> usize {
        self.Size()
    }

    pub fn Reset(&self, capacity: usize) {
        let mut state = self.state.write().expect("infoschema cache lock poisoned");
        state.cache = Vec::with_capacity(capacity);
        state.capacity = capacity;
    }

    /// 用单一最新条目替换整个缓存；返回延迟 drop 旧条目的闭包。
    pub fn Upsert(&self, schema: SchemaRef, schema_ts: u64) -> impl FnOnce() + Send + 'static {
        let mut state = self.state.write().expect("infoschema cache lock poisoned");
        let old = std::mem::replace(
            &mut state.cache,
            vec![SchemaAndTimestamp {
                infoschema: schema.clone(),
                timestamp: schema_ts as i64,
            }],
        );
        state.first_known_schema_version = schema.SchemaMetaVersion();
        move || drop(old)
    }

    pub fn GetLatest(&self) -> Option<SchemaRef> {
        self.state
            .read()
            .expect("infoschema cache lock poisoned")
            .cache
            .first()
            .map(|entry| entry.infoschema.clone())
    }

    pub fn GetEmptySchemaVersions(&self) -> HashSet<i64> {
        self.state
            .read()
            .expect("infoschema cache lock poisoned")
            .empty_schema_versions
            .clone()
    }

    /// 无锁按快照时间戳查找：要求相邻版本连续或中间均为空版本。
    fn get_schema_by_timestamp_no_lock(state: &CacheState, ts: u64) -> Option<SchemaRef> {
        for (index, entry) in state.cache.iter().enumerate() {
            // 时间戳未填或快照早于该条目，跳过。
            if entry.timestamp == 0 || ts < entry.timestamp as u64 {
                continue;
            }
            if index == 0 {
                return Some(entry.infoschema.clone());
            }
            if state.cache[index - 1].timestamp as u64 > ts {
                let newer = state.cache[index - 1].infoschema.SchemaMetaVersion();
                let current = entry.infoschema.SchemaMetaVersion();
                if newer == current + 1 {
                    return Some(entry.infoschema.clone());
                }
                if newer > current
                    && (current + 1..newer)
                        .all(|version| state.empty_schema_versions.contains(&version))
                {
                    return Some(entry.infoschema.clone());
                }
            }
            break;
        }
        None
    }

    pub fn GetByVersion(&self, version: i64) -> Option<SchemaRef> {
        let state = self.state.read().expect("infoschema cache lock poisoned");
        // 在降序序列中找插入点：partition_point 谓词为 version 更大。
        let index = state
            .cache
            .partition_point(|entry| entry.infoschema.SchemaMetaVersion() > version);
        let entry = state.cache.get(index)?;
        let found = entry.infoschema.SchemaMetaVersion();
        if found == version || (index != 0 && found >= state.first_known_schema_version) {
            Some(entry.infoschema.clone())
        } else {
            None
        }
    }

    pub fn GetBySnapshotTS(&self, snapshot_ts: u64) -> Option<SchemaRef> {
        Self::get_schema_by_timestamp_no_lock(
            &self.state.read().expect("infoschema cache lock poisoned"),
            snapshot_ts,
        )
    }

    /// 按版本插入或更新条目；满容量时淘汰最旧；可能触发 GC 检查窗口更新。
    pub fn Insert(&self, schema: SchemaRef, schema_ts: u64) -> bool {
        let version = schema.SchemaMetaVersion();
        let mut state = self.state.write().expect("infoschema cache lock poisoned");
        match state.last_check_time {
            None => {
                state.last_check_version = version;
                state.last_check_time = Some(Instant::now());
            }
            Some(last)
                if version > state.last_check_version + gcCheckInterval
                    && last.elapsed() > Duration::from_secs(60) =>
            {
                // Go 此处会异步 GC 旧版本；本 crate 仅维护排序缓存，存储侧 GC 在 Data。
                // The Go implementation starts an asynchronous old-version GC here.
                // This crate owns only the ordering cache; storage-backed GC lives in Data.
                state.last_check_version = version;
                state.last_check_time = Some(Instant::now());
            }
            _ => {}
        }

        // 定位同版本条目：命中则就地更新，否则按容量插入/淘汰。
        let index = state
            .cache
            .partition_point(|entry| entry.infoschema.SchemaMetaVersion() > version);
        if index < state.cache.len() && state.cache[index].infoschema.SchemaMetaVersion() == version
        {
            // 同代（同 v1/v2）则尽量补时间戳或替换 v2 对象。
            let same_generation = state.cache[index].infoschema.IsV2() == schema.IsV2();
            if same_generation {
                if schema_ts > 0 && state.cache[index].timestamp == 0 {
                    state.cache[index].timestamp = schema_ts as i64;
                } else if schema.IsV2() {
                    state.cache[index].infoschema = schema;
                }
                return true;
            }
            state.cache[index] = SchemaAndTimestamp {
                infoschema: schema,
                timestamp: schema_ts as i64,
            };
            return true;
        }

        let item = SchemaAndTimestamp {
            infoschema: schema,
            timestamp: schema_ts as i64,
        };
        // 未满则插入；恰好第一条时记录 first_known。
        if state.cache.len() < state.capacity {
            state.cache.insert(index, item);
            if state.cache.len() == 1 {
                state.first_known_schema_version = version;
            }
            // 已满但插入点不在末尾：插入后弹出最旧。
        } else if index < state.cache.len() {
            state.cache.insert(index, item);
            state.cache.pop();
        } else {
            return false;
        }
        true
    }

    pub fn InsertEmptySchemaVersion(&self, version: i64) {
        let mut state = self.state.write().expect("infoschema cache lock poisoned");
        state.empty_schema_versions.insert(version);
        if state.empty_schema_versions.len() > state.capacity {
            let mut versions: Vec<i64> = state.empty_schema_versions.iter().copied().collect();
            versions.sort_unstable();
            for version in versions {
                state.empty_schema_versions.remove(&version);
                if state.empty_schema_versions.len() <= state.capacity {
                    break;
                }
            }
        }
    }
}

/// 版本推进超过该间隔且距上次检查超过 60s 时，才更新 GC 检查窗口。
pub const gcCheckInterval: i64 = 128;
