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

// 统计信息异步加载请求队列。
//
// 优化器按需把列/索引直方图（Histogram）加载请求放入分片映射；后台任务从 KV
// （键值存储）拉取完整或增量统计。索引与主键直方图常自动加载，普通列按需排队。

#![allow(non_snake_case, non_upper_case_globals)]

pub use astersql_meta_model::{StatsLoadItem, TableItemID};
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

/// Stores the columns and indices whose histograms must be loaded from KV.
///
/// As in Go, index and primary-key histograms are loaded automatically while
/// ordinary columns are queued on demand.
/// 记录需从 KV 加载直方图的列与索引集合（与 Go 一致：索引/主键常自动加载，列按需排队）。

pub static AsyncLoadHistogramNeededItems: LazyLock<NeededStatsMap> =
    LazyLock::new(newNeededStatsMap);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// 表内统计项键：表 ID、列/索引 ID、是否索引、同步加载是否失败。
struct TableItemKey {
    /// 表 ID。
    table_id: i64,
    /// 列 ID 或索引 ID。
    id: i64,
    /// 是否为索引统计项。
    is_index: bool,
    /// 同步加载是否曾失败（影响后续异步重试策略）。
    is_sync_load_failed: bool,
}

impl TableItemKey {
    /// 从对外 `TableItemID` 构造内部键。
    fn from_item(item: &TableItemID) -> Self {
        Self {
            table_id: item.TableID,
            id: item.ID,
            is_index: item.IsIndex,
            is_sync_load_failed: item.IsSyncLoadFailed,
        }
    }

    /// 还原为对外 `TableItemID`。
    fn into_item(self) -> TableItemID {
        TableItemID {
            TableID: self.table_id,
            ID: self.id,
            IsIndex: self.is_index,
            IsSyncLoadFailed: self.is_sync_load_failed,
        }
    }
}

/// 单分片内的待加载集合；值为是否 full load（完整加载）。
struct NeededStatsInternalMap {
    // bool 表示是否完整加载（相对仅加载部分桶/摘要）。
    // The bool value indicates whether this is a full load.
    items: RwLock<HashMap<TableItemKey, bool>>,
}

impl NeededStatsInternalMap {
    /// 创建空分片映射。
    fn new() -> Self {
        Self {
            items: RwLock::new(HashMap::new()),
        }
    }

    /// 导出本分片全部待加载项（含 FullLoad 标记）。
    fn AllItems(&self) -> Vec<StatsLoadItem> {
        let items = self
            .items
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        items
            .iter()
            .map(|(key, full_load)| StatsLoadItem {
                TableItemID: key.into_item(),
                FullLoad: *full_load,
            })
            .collect()
    }

    /// 插入或升级请求：full load 一经设置不可被后续部分加载降级。
    fn Insert(&self, item: TableItemID, full_load: bool) {
        let mut items = self
            .items
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let current = items.entry(TableItemKey::from_item(&item)).or_insert(false);
        // A full-load request dominates an existing partial-load request. Once
        // set, it must not be downgraded by later partial-load requests.
        // 完整加载请求覆盖部分加载；已置位后不可降级。

        *current |= full_load;
    }

    /// 移除已完成或取消的加载请求。
    fn Delete(&self, item: TableItemID) {
        let mut items = self
            .items
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        items.remove(&TableItemKey::from_item(&item));
    }

    /// 本分片待加载项数量。
    fn Length(&self) -> usize {
        self.items
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

/// 分片数量，降低并发写锁竞争。
const shardCnt: usize = 128;

/// A sharded concurrent set of pending statistics load requests.
/// 分片并发的待加载统计请求集合。

pub struct NeededStatsMap {
    items: [NeededStatsInternalMap; shardCnt],
}

/// 按统计项 ID 绝对值映射到分片下标。
pub fn getIdx(item: &TableItemID) -> usize {
    item.ID.unsigned_abs() as usize % shardCnt
}

/// 构造空的分片待加载映射。
pub fn newNeededStatsMap() -> NeededStatsMap {
    NeededStatsMap {
        items: std::array::from_fn(|_| NeededStatsInternalMap::new()),
    }
}

impl NeededStatsMap {
    /// 汇总全部分片的待加载项。
    pub fn AllItems(&self) -> Vec<StatsLoadItem> {
        let mut result = Vec::with_capacity(shardCnt);
        for shard in &self.items {
            result.extend(shard.AllItems());
        }
        result
    }

    /// 将请求路由到对应分片并插入/升级。
    pub fn Insert(&self, item: TableItemID, full_load: bool) {
        self.items[getIdx(&item)].Insert(item, full_load);
    }

    /// 从对应分片删除请求。
    pub fn Delete(&self, item: TableItemID) {
        self.items[getIdx(&item)].Delete(item);
    }

    /// 全部分片待加载项总数。
    pub fn Length(&self) -> usize {
        self.items.iter().map(NeededStatsInternalMap::Length).sum()
    }
}
