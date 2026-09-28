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

// 统计系统表 GC（垃圾回收）逻辑。
//
// 扫描超过多倍 lease 的旧 `stats_meta` 版本，删除已不存在的物理表/列/索引
// 对应的直方图、桶、历史记录；并维护 `tidb_stats_gc_last_ts` 水位。
// GC（Garbage Collection）此处指清理过期统计元数据，而非存储引擎 MVCC GC。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::{Error, SqlStore};

/// 记录上次成功 GC 水位的系统变量名（写入 `mysql.tidb` 一类系统表）。
pub const GC_LAST_TS_VARIABLE: &str = "tidb_stats_gc_last_ts";

/// 向 GC 提供 schema 视角：lease、表/直方图是否仍存在。
pub trait StatsCatalog {
    /// 统计 handle 的 lease（租约）时长。
    fn lease(&self) -> Duration;
    /// 物理表是否仍在当前 schema 中。
    fn table_exists(&self, physical_id: i64) -> bool;
    /// 指定列/索引直方图是否仍存在于 schema。
    fn histogram_exists(&self, physical_id: i64, histogram_id: i64, is_index: bool) -> bool;
}

/// 绑定 `SqlStore` 与 `StatsCatalog` 的 GC 门面。
pub struct StatsGc<'a> {
    store: &'a dyn SqlStore,
    catalog: &'a dyn StatsCatalog,
}

/// 构造 `StatsGc`。
pub fn new_stats_gc<'a>(store: &'a dyn SqlStore, catalog: &'a dyn StatsCatalog) -> StatsGc<'a> {
    StatsGc { store, catalog }
}

impl StatsGc<'_> {
    /// 执行一轮统计 GC（见 `gc_stats`）。
    pub fn gc_stats(&self, ddl_lease: Duration) -> Result<(), Error> {
        gc_stats(self.store, self.catalog, ddl_lease)
    }
    /// 按保留期清理过期历史统计。
    pub fn clear_outdated_history_stats(&self, retention: Duration) -> Result<(), Error> {
        clear_outdated_history_stats(self.store, retention)
    }
    /// 从 KV/系统表删除给定物理表的统计（`soft` 时保留部分 meta）。
    pub fn delete_table_stats_from_kv(&self, ids: &[i64], soft: bool) -> Result<(), Error> {
        delete_table_stats_from_kv(self.store, ids, soft)
    }
}

/// Statistics older than ten leases are safe to collect. The last timestamp is
/// advanced only after the main scan succeeds; historical-retention cleanup is
/// intentionally best-effort like the Go implementation.
///
/// 超过十倍 lease 的统计可安全回收。主扫描成功后才推进水位；
/// 历史保留清理与 Go 一样为尽力而为（best-effort）。
pub fn gc_stats(
    store: &dyn SqlStore,
    catalog: &dyn StatsCatalog,
    ddl_lease: Duration,
) -> Result<(), Error> {
    // 取 catalog lease 与 DDL lease 的较大者，再乘 10 作为安全偏移。
    let lease = catalog.lease().max(ddl_lease);
    let offset = duration_to_ts(lease.saturating_mul(10));
    // The statistics versions come from the backing store's transaction clock.
    // A wall-clock-only timestamp can lag a transaction started in the same
    // millisecond because it lacks the store's logical TSO component, causing a
    // freshly dropped column or index to be skipped until a later GC cycle.
    // `gc_meta_ids` uses an exclusive upper bound. Advance one logical tick so
    // a meta row committed at the store's current version is part of this scan.
    let now = store.start_ts()?.saturating_add(1);
    if now < offset {
        return Ok(());
    }
    let gc_version = now - offset;
    let last_gc = get_last_gc_timestamp(store)?;
    // 仅处理 (last_gc, gc_version) 窗口内的 meta，避免重复全表扫描。
    for id in store.gc_meta_ids(last_gc, gc_version)? {
        gc_table_stats(store, catalog, id)?;
        if !catalog.table_exists(id) {
            gc_history_stats_from_kv(store, id)?;
        }
    }
    // 默认保留 7 天历史；失败不影响主水位推进。
    let _ = clear_outdated_history_stats(store, Duration::from_secs(7 * 24 * 3600));
    write_gc_timestamp(store, gc_version)
}

/// 按当前 StartTS 删除一批物理表的统计记录。
pub fn delete_table_stats_from_kv(
    store: &dyn SqlStore,
    stats_ids: &[i64],
    soft: bool,
) -> Result<(), Error> {
    let version = store.start_ts()?;
    for id in stats_ids {
        store.gc_delete_table_stats(*id, soft, version)?;
    }
    Ok(())
}

/// 计算批次数：`ceil(total / batch)`；非法输入返回 0。
pub fn batch_count(total: i64, batch: i64) -> i64 {
    if total <= 0 || batch <= 0 {
        0
    } else {
        total / batch + i64::from(total % batch != 0)
    }
}

/// 清理超过 `retention` 的 `stats_meta_history` / `stats_history` 行。
pub fn clear_outdated_history_stats(
    store: &dyn SqlStore,
    retention: Duration,
) -> Result<(), Error> {
    store.gc_clear_expired_history(retention.as_secs())
}

/// 删除指定物理表的全部历史统计。
fn gc_history_stats_from_kv(store: &dyn SqlStore, physical_id: i64) -> Result<(), Error> {
    store.gc_delete_history(physical_id)
}

/// 删除单个列/索引的直方图及相关桶、FM Sketch。
fn delete_histogram_stats_from_kv(
    store: &dyn SqlStore,
    physical_id: i64,
    histogram_id: i64,
    is_index: bool,
) -> Result<(), Error> {
    let version = store.start_ts()?;
    store.gc_delete_histogram(physical_id, histogram_id, is_index, version)
}

/// 对单表：若表已不存在则硬删全部统计或只删 meta；否则清理孤儿直方图。
fn gc_table_stats(
    store: &dyn SqlStore,
    catalog: &dyn StatsCatalog,
    physical_id: i64,
) -> Result<(), Error> {
    let rows = store.gc_histogram_identities(physical_id)?;
    if !catalog.table_exists(physical_id) {
        if !rows.is_empty() {
            return delete_table_stats_from_kv(store, &[physical_id], false);
        }
        return store.gc_delete_meta(physical_id);
    }
    for (is_index, id) in rows {
        if !catalog.histogram_exists(physical_id, id, is_index) {
            delete_histogram_stats_from_kv(store, physical_id, id, is_index)?;
        }
    }
    Ok(())
}

/// 读取上次 GC 水位；缺失时视为 0。
fn get_last_gc_timestamp(store: &dyn SqlStore) -> Result<u64, Error> {
    match store.gc_timestamp(GC_LAST_TS_VARIABLE)? {
        None => Ok(0),
        Some(value) => value
            .parse()
            .map_err(|error| Error(format!("invalid stats GC timestamp: {error}"))),
    }
}

/// 持久化本次 GC 水位。
fn write_gc_timestamp(store: &dyn SqlStore, timestamp: u64) -> Result<(), Error> {
    store.set_gc_timestamp(GC_LAST_TS_VARIABLE, timestamp)
}

/// 生成单调递增的统计时间戳（物理毫秒左移 18 位，模拟 TiDB TSO 物理部分）。
pub fn current_ts() -> u64 {
    static LAST_TIMESTAMP: AtomicU64 = AtomicU64::new(0);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let physical = millis << 18;
    let previous = LAST_TIMESTAMP
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |last| {
            Some(physical.max(last.saturating_add(1)))
        })
        .unwrap_or_default();
    physical.max(previous.saturating_add(1))
}
/// 将 `Duration` 转为与 `current_ts` 同刻度的时间戳偏移。
fn duration_to_ts(duration: Duration) -> u64 {
    (duration.as_millis().min((u64::MAX >> 18) as u128) as u64) << 18
}
