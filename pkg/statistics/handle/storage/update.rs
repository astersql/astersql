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

// 统计元数据增量更新与全局 ID 变更。
//
// 负责批量推进 `stats_meta`/`stats_histograms` 版本、按表增量（delta）
// 合并 modify_count/count，以及分区合并时改写全局统计的 table_id。

use crate::{Error, SqlStore};

/// 用当前 start_ts 批量更新所有 stats_meta 与 histograms 的 version。
pub fn update_stats_version(store: &dyn SqlStore) -> Result<(), Error> {
    let version = store.start_ts()?;
    store.execute(&format!("update mysql.stats_meta set version = {version}"))?;
    store.execute(&format!(
        "update mysql.stats_histograms set version = {version}"
    ))?;
    Ok(())
}

#[derive(Clone, Copy, Debug, Default)]
/// 单表行数变化：`count` 为绝对增量，`delta` 为带符号变更量。
pub struct TableDelta {
    pub count: i64,
    pub delta: i64,
}

#[derive(Clone, Copy, Debug)]
/// 一次待写入的表增量，含是否处于 stats 锁（写入 locked 表而非 meta）。
/// Stats lock：锁定后 ANALYZE/自动更新跳过该表，变更记入 stats_table_locked。
pub struct DeltaUpdate {
    pub delta: TableDelta,
    pub table_id: i64,
    pub is_locked: bool,
}

/// 构造 DeltaUpdate。
pub fn new_delta_update(table_id: i64, delta: TableDelta, is_locked: bool) -> DeltaUpdate {
    DeltaUpdate {
        delta,
        table_id,
        is_locked,
    }
}

/// 批量应用表增量：先 FOR UPDATE 锁定相关行，再按锁定/正负 delta 分组 UPSERT。
pub fn update_stats_meta(
    store: &dyn SqlStore,
    start_ts: u64,
    updates: &[DeltaUpdate],
) -> Result<(), Error> {
    if updates.is_empty() {
        return Ok(());
    }
    // 拆成 locked / unlocked，unlocked 再按 delta 正负分流（减法用不同 count 表达式）。
    let mut locked_ids = Vec::new();
    let mut unlocked_ids = Vec::new();
    let mut locked = Vec::new();
    let mut positive = Vec::new();
    let mut negative = Vec::new();
    for update in updates {
        if update.is_locked {
            locked_ids.push(update.table_id);
            locked.push(format!(
                "({start_ts},{},{},{})",
                update.table_id, update.delta.count, update.delta.delta
            ));
        } else {
            unlocked_ids.push(update.table_id);
            let value = format!(
                "({start_ts},{},{},{})",
                update.table_id,
                update.delta.count,
                update.delta.delta.unsigned_abs()
            );
            if update.delta.delta < 0 {
                negative.push(value);
            } else {
                positive.push(value);
            }
        }
    }
    // 对将要更新的行加行锁，避免并发 dump 冲突。
    if !locked_ids.is_empty() {
        store.execute(&format!(
            "select * from mysql.stats_table_locked where table_id in ({}) for update",
            join_ids(&locked_ids)
        ))?;
    }
    if !unlocked_ids.is_empty() {
        store.execute(&format!(
            "select * from mysql.stats_meta where table_id in ({}) for update",
            join_ids(&unlocked_ids)
        ))?;
    }
    exec_delta(store, "stats_table_locked", &locked, false)?;
    exec_delta(store, "stats_meta", &positive, false)?;
    exec_delta(store, "stats_meta", &negative, true)
}

/// 对指定系统表执行 INSERT ... ON DUPLICATE KEY UPDATE 增量合并。
/// `negative` 为真时 count 按减法且下限为 0。
fn exec_delta(
    store: &dyn SqlStore,
    table: &str,
    values: &[String],
    negative: bool,
) -> Result<(), Error> {
    if values.is_empty() {
        return Ok(());
    }
    let count = if negative {
        "if(count > values(count), count - values(count), 0)"
    } else {
        "count + values(count)"
    };
    store.execute(&format!("insert into mysql.{table} (version,table_id,modify_count,count) values {} on duplicate key update version=values(version),modify_count=modify_count+values(modify_count),count={count}", values.join(",")))?;
    Ok(())
}

/// 变更全局统计 table_id 时需要同步改写的系统表列表。
pub const CHANGE_GLOBAL_STATS_TABLES: &[&str] = &[
    "stats_meta",
    "stats_top_n",
    "stats_fm_sketch",
    "stats_buckets",
    "stats_histograms",
    "column_stats_usage",
];

/// 将各统计系统表中 table_id 从 from 更新为 to。
pub fn change_global_stats_id(store: &dyn SqlStore, from: i64, to: i64) -> Result<(), Error> {
    for table in CHANGE_GLOBAL_STATS_TABLES {
        store.execute(&format!(
            "update mysql.{table} set table_id = {to} where table_id = {from}"
        ))?;
    }
    Ok(())
}

/// 刷新单表 meta 的 version 与 last_stats_histograms_version，返回新版本。
pub fn update_stats_meta_version_and_last_histogram_version(
    store: &dyn SqlStore,
    physical_id: i64,
) -> Result<u64, Error> {
    let version = store.start_ts()?;
    store.execute(&format!("update mysql.stats_meta set version={version}, last_stats_histograms_version={version} where table_id={physical_id}"))?;
    Ok(version)
}

/// 将表 ID 列表拼成 SQL IN 子句内容。
fn join_ids(ids: &[i64]) -> String {
    ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
}
