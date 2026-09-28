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

// 会话级统计使用量与表增量（delta）收集。
//
// 各 Session 在本地累计表行数变化与列/索引最近使用时间，后台 `sweep` 时
// 合并进全局 `TableDeltaMap` / `StatsUsage`，供统计落盘与自动分析决策使用。

use std::collections::{HashMap, hash_map::Entry};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// 列或索引的使用量键：表 ID + 列/索引 ID，以及是否为索引。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TableItemId {
    pub table_id: i64,
    pub id: i64,
    pub is_index: bool,
}

/// 单表统计增量：行数变化量、修改计数、列大小以及首次观测时间。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TableDelta {
    pub delta: i64,
    pub count: i64,
    pub col_size: i64,
    pub init_time: Option<SystemTime>,
}

/// 全局表增量映射，按物理表 ID 聚合各会话 sweep 上来的 delta。
#[derive(Default)]
pub struct TableDeltaMap {
    inner: Mutex<HashMap<i64, TableDelta>>,
}
impl TableDeltaMap {
    /// 累加指定表的行增量与修改计数；首次写入时记录 `init_time`。
    pub fn update(&self, id: i64, delta: i64, count: i64) {
        assert!(id > 0, "table ID should be greater than 0");
        let mut values = self.inner.lock().expect("delta mutex poisoned");
        let value = values.entry(id).or_insert(TableDelta {
            init_time: Some(SystemTime::now()),
            ..TableDelta::default()
        });
        value.delta += delta;
        value.count += count;
    }
    /// 将另一份增量映射合并进来；已有条目累加字段，并取更早的 `init_time`。
    pub fn merge(&self, other: HashMap<i64, TableDelta>) {
        let mut values = self.inner.lock().expect("delta mutex poisoned");
        for (id, item) in other {
            match values.entry(id) {
                Entry::Vacant(entry) => {
                    entry.insert(item);
                }
                Entry::Occupied(mut entry) => {
                    let value = entry.get_mut();
                    value.delta += item.delta;
                    value.count += item.count;
                    value.col_size += item.col_size;
                    value.init_time = match (value.init_time, item.init_time) {
                        (Some(current), Some(incoming)) => Some(current.min(incoming)),
                        (None, incoming) => incoming,
                        (current, None) => current,
                    };
                }
            }
        }
    }
    /// 取出并清空全部增量，供落盘或进一步处理。
    pub fn take(&self) -> HashMap<i64, TableDelta> {
        std::mem::take(&mut *self.inner.lock().expect("delta mutex poisoned"))
    }
    /// 清空全部增量，不返回内容。
    pub fn reset(&self) {
        self.inner.lock().expect("delta mutex poisoned").clear();
    }
}

/// 全局列/索引最近使用时间表，键为 `TableItemId`。
#[derive(Default)]
pub struct StatsUsage {
    inner: Mutex<HashMap<TableItemId, SystemTime>>,
}
impl StatsUsage {
    /// 合并使用时间：同一条目保留更晚的时间戳。
    pub fn merge(&self, other: HashMap<TableItemId, SystemTime>) {
        let mut values = self.inner.lock().expect("usage mutex poisoned");
        for (item, time) in other {
            values
                .entry(item)
                .and_modify(|current| *current = (*current).max(time))
                .or_insert(time);
        }
    }
    /// 将一批条目以同一时间戳合并进使用量表。
    pub fn merge_raw(&self, items: impl IntoIterator<Item = TableItemId>, time: SystemTime) {
        self.merge(
            items
                .into_iter()
                .map(|item| {
                    assert!(!item.is_index, "predicate column should not be an index");
                    (item, time)
                })
                .collect(),
        );
    }
    /// 取出并清空全部使用时间记录。
    pub fn take(&self) -> HashMap<TableItemId, SystemTime> {
        std::mem::take(&mut *self.inner.lock().expect("usage mutex poisoned"))
    }
    /// 清空使用时间记录。
    pub fn reset(&self) {
        self.inner.lock().expect("usage mutex poisoned").clear();
    }
}

/// 单个会话的本地状态：删除标记、表增量与列使用时间。
#[derive(Default)]
struct SessionState {
    deleted: bool,
    deltas: HashMap<i64, TableDelta>,
    usage: HashMap<TableItemId, SystemTime>,
}

/// 单个会话的统计收集句柄，可在会话生命周期内并发更新。
#[derive(Clone, Default)]
pub struct SessionStatsItem {
    state: Arc<Mutex<SessionState>>,
}
impl SessionStatsItem {
    /// 标记会话已结束，下次 `sweep` 时会先合并数据再移除该条目。
    pub fn delete(&self) {
        self.state
            .lock()
            .expect("session stats mutex poisoned")
            .deleted = true;
    }
    /// 在本会话累计指定表的行增量与修改计数。
    pub fn update(&self, id: i64, delta: i64, count: i64) {
        let mut state = self.state.lock().expect("session stats mutex poisoned");
        let value = state.deltas.entry(id).or_insert(TableDelta {
            init_time: Some(SystemTime::now()),
            ..TableDelta::default()
        });
        value.delta += delta;
        value.count += count;
    }
    /// 记录列/索引在给定时刻被使用；重复写入取更晚时间。
    pub fn update_column_usage(
        &self,
        items: impl IntoIterator<Item = TableItemId>,
        time: SystemTime,
    ) {
        let mut state = self.state.lock().expect("session stats mutex poisoned");
        for item in items {
            state
                .usage
                .entry(item)
                .and_modify(|current| *current = (*current).max(time))
                .or_insert(time);
        }
    }
    /// 测试用：清空本会话本地增量与使用量，不标记删除。
    pub fn clear_for_test(&self) {
        let mut state = self.state.lock().expect("session stats mutex poisoned");
        state.deltas.clear();
        state.usage.clear();
        state.deleted = false;
    }
}

/// 会话统计列表：持有各会话条目，并维护全局增量与使用量汇总。
#[derive(Default)]
pub struct SessionStatsList {
    items: Mutex<Vec<SessionStatsItem>>,
    table_delta: TableDeltaMap,
    stats_usage: StatsUsage,
}
impl SessionStatsList {
    /// 注册新会话收集项并返回可交给该会话使用的句柄。
    pub fn new_item(&self) -> SessionStatsItem {
        let item = SessionStatsItem::default();
        self.items
            .lock()
            .expect("stats list mutex poisoned")
            .push(item.clone());
        item
    }
    /// 扫一遍会话：把本地 delta/usage 合并进全局，并丢弃已 `delete` 的会话。
    pub fn sweep(&self) {
        let mut items = self.items.lock().expect("stats list mutex poisoned");
        items.retain(|item| {
            let mut state = item.state.lock().expect("session stats mutex poisoned");
            self.table_delta.merge(std::mem::take(&mut state.deltas));
            self.stats_usage.merge(std::mem::take(&mut state.usage));
            !state.deleted
        });
    }
    /// 访问全局表增量映射。
    pub fn table_delta(&self) -> &TableDeltaMap {
        &self.table_delta
    }
    /// 访问全局列/索引使用量表。
    pub fn stats_usage(&self) -> &StatsUsage {
        &self.stats_usage
    }
    /// 清空所有会话条目及全局汇总。
    pub fn reset(&self) {
        self.items
            .lock()
            .expect("stats list mutex poisoned")
            .clear();
        self.table_delta.reset();
        self.stats_usage.reset();
    }
}

/// 列/索引使用量的扁平条目，便于序列化或批量写出。
#[derive(Clone, Debug)]
pub struct ColStatsUsageEntry {
    pub item: TableItemId,
    pub last_used_at: SystemTime,
}

/// 从待处理增量映射中选出需要落盘的表 ID。
///
/// `targets` 为空时返回映射中全部键；否则只返回仍存在于映射中的目标 ID。
pub fn collect_pending_stats_delta_table_ids(
    delta_map: &HashMap<i64, TableDelta>,
    targets: &[i64],
) -> Vec<i64> {
    let mut ids: Vec<i64> = if targets.is_empty() {
        delta_map.keys().copied().collect()
    } else {
        let mut seen = std::collections::HashSet::new();
        targets
            .iter()
            .copied()
            .filter(|id| delta_map.contains_key(id) && seen.insert(*id))
            .collect()
    };
    ids.sort_unstable();
    ids
}
