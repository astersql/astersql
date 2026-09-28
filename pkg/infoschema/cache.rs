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
