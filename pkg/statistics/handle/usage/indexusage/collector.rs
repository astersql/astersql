// Copyright 2024 PingCAP, Inc.
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

// 节点级索引使用率（Index Usage）采集器。
//
// 在会话 / 语句执行路径上累加各索引的查询次数、KV 请求数、访问行数，
// 以及按访问比例分桶的直方图；经异步 worker 合并到节点全局视图，
// 供统计子系统判断冷热索引与垃圾回收（GC）已删除对象的使用记录。

#![allow(non_snake_case)]

use crate::meta::model;
use crate::statistics::handle::usage::collector;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// GlobalIndexID is the key type for the index usage map.
/// 全局索引标识：表 ID + 索引 ID，作为使用率映射的键。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct GlobalIndexID {
    /// 表的唯一标识。
    pub TableID: i64,
    /// 索引的唯一标识。
    pub IndexID: i64,
}

/// Sample stores the aggregated usage information for an index.
/// 单个索引的聚合使用样本：时间戳、计数与访问比例分桶。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sample {
    /// 最近一次被记录使用的时间。
    pub LastUsedAt: SystemTime,
    /// 累计查询次数（触及该索引的语句数）。
    pub QueryTotal: u64,
    /// 累计发起的 KV（键值存储）请求次数。
    pub KvReqTotal: u64,
    /// 累计通过该索引访问的行数。
    pub RowAccessTotal: u64,
    /// 访问比例直方图：7 个桶对应不同行访问占比区间。
    pub PercentageAccess: [u64; 7],
}

impl Default for Sample {
    fn default() -> Self {
        Self {
            // Go `time.Time{}` is 0001-01-01T00:00:00Z, not the Unix epoch.
            LastUsedAt: UNIX_EPOCH
                .checked_sub(std::time::Duration::from_secs(62_135_596_800))
                .expect("platform SystemTime must represent Go's zero time"),
            QueryTotal: 0,
            KvReqTotal: 0,
            RowAccessTotal: 0,
            PercentageAccess: [0; 7],
        }
    }
}

/// 访问比例分桶上界：0 / 1% / 10% / 20% / 50% / 100%。
const BUCKET_BOUND: [f64; 6] = [0.0, 0.01, 0.1, 0.2, 0.5, 1.0];

/// 将行访问占比映射到直方图桶下标；0 与 100% 各占专用桶。
fn getIndexUsageAccessBucket(percentage: f64) -> usize {
    if percentage == 0.0 {
        return 0;
    }

    let mut bucket = 0;
    for i in 1..BUCKET_BOUND.len() {
        if percentage >= BUCKET_BOUND[i - 1] && percentage < BUCKET_BOUND[i] {
            bucket = i;
            break;
        }
    }
    // 全表扫描（占比恰好 1.0）落入最后一个桶。
    if percentage == 1.0 {
        bucket = BUCKET_BOUND.len();
    }
    bucket
}

/// NewSample creates a new index usage data point.
/// 根据本次访问行数与表总行数构造一个使用率采样点。
pub fn NewSample(queryTotal: u64, kvReqTotal: u64, rowAccess: u64, tableTotalRows: u64) -> Sample {
    let mut percentage_access = [0; BUCKET_BOUND.len() + 1];
    // 表行数为 0 时无法计算比例，归入满桶（与 Go 行为一致）。
    let bucket = if tableTotalRows == 0 {
        BUCKET_BOUND.len()
    } else {
        getIndexUsageAccessBucket(rowAccess as f64 / tableTotalRows as f64)
    };
    percentage_access[bucket] = 1;

    Sample {
        LastUsedAt: SystemTime::now(),
        QueryTotal: queryTotal,
        KvReqTotal: kvReqTotal,
        RowAccessTotal: rowAccess,
        PercentageAccess: percentage_access,
    }
}

/// 索引 → 使用样本的映射。
type IndexUsageMap = HashMap<GlobalIndexID, Sample>;
/// 会话侧待合并的增量（可跨线程共享）。
type IndexUsageDelta = Arc<Mutex<IndexUsageMap>>;

/// 空映射对象池，减少频繁分配（对齐 Go `sync.Pool`）。
static INDEX_USAGE_POOL: LazyLock<Mutex<Vec<IndexUsageMap>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// 从对象池取出或新建一张空的使用率映射。
fn takeIndexUsageMap() -> IndexUsageMap {
    INDEX_USAGE_POOL.lock().unwrap().pop().unwrap_or_default()
}

/// 包装为可共享的会话增量容器。
fn takeIndexUsageDelta() -> IndexUsageDelta {
    Arc::new(Mutex::new(takeIndexUsageMap()))
}

/// 将一条样本累加到映射中对应索引上（计数 wrapping 相加，时间取较新）。
fn updateByKey(map: &mut IndexUsageMap, id: GlobalIndexID, sample: Sample) {
    let item = map.entry(id).or_default();
    item.QueryTotal = item.QueryTotal.wrapping_add(sample.QueryTotal);
    item.RowAccessTotal = item.RowAccessTotal.wrapping_add(sample.RowAccessTotal);
    item.KvReqTotal = item.KvReqTotal.wrapping_add(sample.KvReqTotal);
    for (current, delta) in item
        .PercentageAccess
        .iter_mut()
        .zip(sample.PercentageAccess)
    {
        *current = current.wrapping_add(delta);
    }
    if item.LastUsedAt < sample.LastUsedAt {
        item.LastUsedAt = sample.LastUsedAt;
    }
}

/// 将会话增量合并进节点全局映射，并把清空后的容器归还对象池。
fn mergeDelta(target: &RwLock<IndexUsageMap>, delta: IndexUsageDelta) {
    let mut target = target.write().unwrap();
    let mut delta = delta.lock().unwrap();
    for (id, sample) in delta.drain() {
        updateByKey(&mut target, id, sample);
    }

    // Match sync.Pool reuse: retain the allocation after clearing the delta.
    // 对齐 Go sync.Pool：清空后保留分配，放回池供下次复用。
    let reusable = std::mem::take(&mut *delta);
    drop(delta);
    INDEX_USAGE_POOL.lock().unwrap().push(reusable);
}

/// Collector records index usage for the whole node.
/// 节点级索引使用率采集器：持有全局映射与异步合并 worker。
pub struct Collector {
    /// 通用全局采集框架（通道 + worker）。
    collector: collector::globalCollector<IndexUsageDelta>,
    /// 节点汇总后的索引使用率视图。
    index_usage: Arc<RwLock<IndexUsageMap>>,
}

/// NewCollector creates a node-level index usage collector.
/// 创建节点级采集器，并注册将增量合并进全局映射的回调。
pub fn NewCollector() -> Collector {
    let index_usage = Arc::new(RwLock::new(takeIndexUsageMap()));
    let merge_target = Arc::clone(&index_usage);
    let global_collector = collector::NewGlobalCollector(move |delta| {
        mergeDelta(&merge_target, delta);
    });
    Collector {
        collector: global_collector,
        index_usage,
    }
}

impl Collector {
    /// GetIndexUsage returns an empty sample when the index is not recorded.
    /// 查询指定表/索引的使用样本；未记录时返回默认空样本。
    pub fn GetIndexUsage(&self, tableID: i64, indexID: i64) -> Sample {
        self.index_usage
            .read()
            .unwrap()
            .get(&GlobalIndexID {
                TableID: tableID,
                IndexID: indexID,
            })
            .cloned()
            .unwrap_or_default()
    }

    /// SpawnSessionCollector creates a session collector attached to this collector.
    /// 派生绑定到本节点的会话级采集器。
    pub fn SpawnSessionCollector(&self) -> SessionIndexUsageCollector {
        SessionIndexUsageCollector {
            state: Arc::new(Mutex::new(SessionIndexUsageState {
                index_usage: takeIndexUsageDelta(),
                collector: self.collector.SpawnSession(),
            })),
        }
    }

    /// 启动后台合并 worker。
    pub fn StartWorker(&self) {
        self.collector.StartWorker();
    }

    /// 关闭采集器并停止 worker。
    pub fn Close(&self) {
        self.collector.Close();
    }

    /// GCIndexUsage deletes usage information for missing tables and indexes.
    /// 垃圾回收：按元数据查找回调删除已不存在的表或索引的使用记录。
    pub fn GCIndexUsage<F>(&self, mut tableMetaLookup: F)
    where
        F: FnMut(i64) -> (Option<model::TableInfo>, bool),
    {
        self.index_usage.write().unwrap().retain(|key, _| {
            let (table, exists) = tableMetaLookup(key.TableID);
            if !exists {
                return false;
            }
            // 表仍存在时，仅保留元数据中仍列出的索引。
            table
                .expect("table lookup returned exists=true without table metadata")
                .Indices
                .iter()
                .any(|index| index.ID == key.IndexID)
        });
    }
}

/// 会话采集器内部状态：待上报增量 + 会话通道句柄。
struct SessionIndexUsageState {
    index_usage: IndexUsageDelta,
    collector: collector::sessionCollector<IndexUsageDelta>,
}

/// SessionIndexUsageCollector collects index usage per session.
/// 会话级索引使用率采集器：本地累加后 Report/Flush 到节点。
#[derive(Clone)]
pub struct SessionIndexUsageCollector {
    state: Arc<Mutex<SessionIndexUsageState>>,
}

impl SessionIndexUsageCollector {
    /// 将一次索引使用样本累加到本会话的待上报增量中。
    pub fn Update(&self, tableID: i64, indexID: i64, sample: Sample) {
        let state = self.state.lock().unwrap();
        let mut usage = state.index_usage.lock().unwrap();
        updateByKey(
            &mut usage,
            GlobalIndexID {
                TableID: tableID,
                IndexID: indexID,
            },
            sample,
        );
    }

    /// Report sends a delta without blocking. A rejected delta remains pending.
    /// 非阻塞上报增量；通道满被拒绝时增量仍留在会话侧。
    pub fn Report(&self) {
        let mut state = self.state.lock().unwrap();
        if state.index_usage.lock().unwrap().is_empty() {
            return;
        }

        let delta = Arc::clone(&state.index_usage);
        if state.collector.SendDelta(delta) {
            state.index_usage = takeIndexUsageDelta();
        }
    }

    /// Flush synchronously sends the pending delta to the global collector.
    /// 同步刷新：阻塞直到待上报增量送入全局采集器。
    pub fn Flush(&self) {
        let mut state = self.state.lock().unwrap();
        if state.index_usage.lock().unwrap().is_empty() {
            return;
        }

        let delta = Arc::clone(&state.index_usage);
        state.collector.SendDeltaSync(delta);
        state.index_usage = takeIndexUsageDelta();
    }
}

/// StmtIndexUsageCollector avoids duplicate QueryTotal increments per statement.
/// 语句级采集器：同一语句内同一索引只计一次 QueryTotal。
pub struct StmtIndexUsageCollector {
    /// 本语句已计入 QueryTotal 的索引集合。
    recorded_index: Mutex<HashSet<GlobalIndexID>>,
    session_collector: SessionIndexUsageCollector,
}

/// 基于会话采集器构造语句级去重包装。
pub fn NewStmtIndexUsageCollector(
    sessionCollector: SessionIndexUsageCollector,
) -> StmtIndexUsageCollector {
    StmtIndexUsageCollector {
        recorded_index: Mutex::new(HashSet::new()),
        session_collector: sessionCollector,
    }
}

impl StmtIndexUsageCollector {
    /// 更新使用样本；首次见到该索引时 QueryTotal=1，之后置 0 避免重复计数。
    pub fn Update(&self, tableID: i64, indexID: i64, mut sample: Sample) {
        let mut recorded = self.recorded_index.lock().unwrap();
        let id = GlobalIndexID {
            TableID: tableID,
            IndexID: indexID,
        };
        // insert 返回 true 表示首次出现，转为 1；否则 0。
        sample.QueryTotal = u64::from(recorded.insert(id));
        self.session_collector.Update(tableID, indexID, sample);
    }

    /// 清空已记录集合，供下一条语句重新计数。
    pub fn Reset(&self) {
        self.recorded_index.lock().unwrap().clear();
    }
}
