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

// 谓词列（predicate column）统计使用率：会话增量落盘与列使用时间查询。
//
// 谓词列指出现在 WHERE/JOIN 等过滤条件中的列；其最近使用/分析时间用于
// 指导 ANALYZE（分析统计信息）优先级。本模块将会话侧 delta / 列使用时间
// 按阈值刷入 KV（键值存储）持久层。

use crate::{ColStatsUsageEntry, SessionStatsItem, SessionStatsList, TableDelta, TableItemId};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Go's default lower bound for dumping a table delta.
pub const DUMP_STATS_DELTA_RATIO: f64 = 1.0 / 10_000.0;
/// Go's default maximum age for an undumped table delta.
pub const DUMP_STATS_MAX_DURATION: Duration = Duration::from_secs(60 * 60);

/// 使用率子系统错误包装。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error(pub String);
/// 列的最近使用时间与最近分析（ANALYZE）时间。
#[derive(Clone, Debug, Default)]
pub struct ColumnTimeInfo {
    /// 该列最近作为谓词被使用的时间。
    pub last_used_at: Option<SystemTime>,
    /// 该列最近完成 ANALYZE 的时间。
    pub last_analyzed_at: Option<SystemTime>,
}

/// 持久化存储抽象：统计元数据、delta、列使用时间读写与索引 GC。
pub trait UsageStore: Send + Sync {
    /// 读取表的 stats meta 行数（用于判断 delta 是否达到刷盘比例）。
    fn stats_meta_count(&self, table_id: i64) -> Result<Option<i64>, Error>;
    /// 将表级增量（delta）写入存储；`locked` 表示表统计是否被锁定。
    fn update_delta(&self, table_id: i64, delta: TableDelta, locked: bool) -> Result<(), Error>;
    /// 批量保存列使用时间条目。
    fn save_column_usage(&self, entries: &[ColStatsUsageEntry]) -> Result<(), Error>;
    /// 加载全部列使用时间映射。
    fn load_column_usage(&self) -> Result<HashMap<TableItemId, ColumnTimeInfo>, Error>;
    /// 查询指定表上有使用记录的谓词列 ID 列表。
    fn predicate_columns(&self, table_id: i64) -> Result<Vec<i64>, Error>;
    /// 垃圾回收已失效的索引使用率记录。
    fn gc_index_usage(&self) -> Result<(), Error>;
}

/// 只读 schema 视图：判断表是否存在、统计是否锁定。
pub trait SchemaState: Send + Sync {
    fn table_exists(&self, id: i64) -> bool;
    fn table_locked(&self, id: i64) -> bool;
}

/// 统计使用率实现：聚合会话列表、存储与 schema，负责刷盘策略。
pub struct StatsUsageImpl {
    pub store: Arc<dyn UsageStore>,
    pub sessions: Arc<SessionStatsList>,
    pub schema: Arc<dyn SchemaState>,
    /// 相对行数的 delta 刷盘比例阈值。
    pub dump_ratio: f64,
    /// delta 超过该年龄则强制刷盘。
    pub max_delta_age: Duration,
    pub index_usage: crate::IndexUsageCollector,
}
impl StatsUsageImpl {
    /// 构造默认实现（dump_ratio=1/10000，max_delta_age=1h）。
    pub fn new(store: Arc<dyn UsageStore>, schema: Arc<dyn SchemaState>) -> Self {
        Self {
            store,
            sessions: Arc::new(SessionStatsList::default()),
            schema,
            dump_ratio: DUMP_STATS_DELTA_RATIO,
            max_delta_age: DUMP_STATS_MAX_DURATION,
            index_usage: crate::IndexUsageCollector::default(),
        }
    }
    /// 从存储加载列统计使用时间。
    pub fn load_column_stats_usage(&self) -> Result<HashMap<TableItemId, ColumnTimeInfo>, Error> {
        self.store.load_column_usage()
    }
    /// 获取表上已记录使用时间的谓词列。
    pub fn get_predicate_columns(&self, table_id: i64) -> Result<Vec<i64>, Error> {
        self.store.predicate_columns(table_id)
    }
    /// 为当前会话注册一项会话统计收集器。
    pub fn new_session_stats_item(&self) -> SessionStatsItem {
        self.sessions.new_item()
    }
    /// 将会话累积的表 delta 按条件刷入 KV；`force` 或超时/比例达标时写入。
    pub fn dump_stats_delta_to_kv(&self, force: bool, targets: &[i64]) -> Result<(), Error> {
        self.sessions.sweep();
        let mut pending = self.sessions.table_delta().take();
        let ids = crate::collect_pending_stats_delta_table_ids(&pending, targets);
        let result = (|| {
            for id in ids {
                let Some(mut delta) = pending.remove(&id) else {
                    continue;
                };
                if !self.schema.table_exists(id) {
                    // Go keeps deltas for dropped/unavailable tables in the map;
                    // a later sweep may observe the table again.
                    pending.insert(id, delta);
                    continue;
                }
                if delta.init_time.is_none() {
                    delta.init_time = Some(SystemTime::now());
                }
                let count = match self.store.stats_meta_count(id) {
                    Ok(count) => count,
                    Err(error) => {
                        pending.insert(id, delta);
                        return Err(error);
                    }
                };
                // Missing, pseudo, or empty stats cause an immediate dump in Go.
                let ratio = count.is_none_or(|count| {
                    count <= 0 || (delta.count as f64) / count as f64 > self.dump_ratio
                });
                let old = delta
                    .init_time
                    .and_then(|time| SystemTime::now().duration_since(time).ok())
                    .is_some_and(|age| age > self.max_delta_age);
                if force || old || ratio {
                    if let Err(error) =
                        self.store
                            .update_delta(id, delta, self.schema.table_locked(id))
                    {
                        pending.insert(id, delta);
                        return Err(error);
                    }
                } else {
                    pending.insert(id, delta);
                }
            }
            Ok(())
        })();
        self.sessions.table_delta().merge(pending);
        result
    }
    /// 将会话侧列使用时间刷入存储；失败时把条目合并回会话以免丢失。
    pub fn dump_column_stats_usage_to_kv(&self) -> Result<(), Error> {
        self.sessions.sweep();
        let usage = self.sessions.stats_usage().take();
        if usage.is_empty() {
            return Ok(());
        }
        let entries = usage
            .into_iter()
            .map(|(item, last_used_at)| ColStatsUsageEntry { item, last_used_at })
            .collect::<Vec<_>>();
        let mut entries = entries;
        entries.sort_by_key(|entry| (entry.item.table_id, entry.item.id));
        if let Err(error) = self.store.save_column_usage(&entries) {
            self.sessions.stats_usage().merge(
                entries
                    .into_iter()
                    .map(|entry| (entry.item, entry.last_used_at))
                    .collect(),
            );
            return Err(error);
        }
        Ok(())
    }
}
