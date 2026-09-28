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

// 统计信息持久化写入（save）。
//
// 将 ANALYZE 结果与表级元数据写入 `mysql.stats_*` 系统表：
// 直方图桶（Histogram buckets）、TopN、CMSketch、FMSketch 以及 `stats_meta`。
// Histogram：按值域分桶估计基数；TopN：高频值精确计数；CMSketch：近似频率草图。

use crate::{
    AnalyzeResults, ColumnStats, Error, Histogram, SqlStore, TableStats, TopNItem, sql_bytes,
};

/// 单次批量 INSERT 的最大元组数，避免单条 SQL 过大。
const BATCH_INSERT_SIZE: usize = 10;
/// 单条 INSERT SQL 最大字节长度上限（约 1MiB）。
const MAX_INSERT_LENGTH: usize = 1024 * 1024;

/// 将 TopN 条目分批写入 `mysql.stats_top_n`。
/// `is_index` 区分列统计与索引统计；`hist_id` 为直方图对应的列/索引 ID。
fn save_top_n(
    store: &dyn SqlStore,
    table_id: i64,
    is_index: bool,
    hist_id: i64,
    top_n: &[TopNItem],
) -> Result<(), Error> {
    let mut offset = 0;
    // 按 BATCH_INSERT_SIZE 切分，并在接近 MAX_INSERT_LENGTH 时提前截断当前批次。
    while offset < top_n.len() {
        let limit = (offset + BATCH_INSERT_SIZE).min(top_n.len());
        let mut sql =
            "insert into mysql.stats_top_n (table_id,is_index,hist_id,value,count) values "
                .to_owned();
        let mut end = offset;
        for (position, item) in top_n[offset..limit].iter().enumerate() {
            let tuple = format!(
                "({table_id},{},{hist_id},{},{})",
                i32::from(is_index),
                sql_bytes(&item.encoded),
                item.count
            );
            if position > 0 && sql.len() + tuple.len() + 1 > MAX_INSERT_LENGTH {
                break;
            }
            if position > 0 {
                sql.push(',');
            }
            sql.push_str(&tuple);
            end += 1;
        }
        store.execute(&sql)?;
        offset = end;
    }
    Ok(())
}

/// 将直方图各桶分批写入 `mysql.stats_buckets`。
/// 写入的 count 为相对前一桶的增量（delta），与 Go/TiDB 存储格式一致。
fn save_buckets(
    store: &dyn SqlStore,
    table_id: i64,
    is_index: bool,
    histogram: &Histogram,
) -> Result<(), Error> {
    let mut offset = 0;
    // 同样按批插入；桶 count 存增量以便加载时累加还原。
    while offset < histogram.buckets.len() {
        let limit = (offset + BATCH_INSERT_SIZE).min(histogram.buckets.len());
        let mut sql = "insert into mysql.stats_buckets (table_id,is_index,hist_id,bucket_id,count,repeats,lower_bound,upper_bound,ndv) values ".to_owned();
        let mut end = offset;
        for index in offset..limit {
            let bucket = &histogram.buckets[index];
            let previous = index
                .checked_sub(1)
                .map_or(0, |idx| histogram.buckets[idx].count);
            let tuple = format!(
                "({table_id},{},{},{index},{},{},{},{},{})",
                i32::from(is_index),
                histogram.id,
                bucket.count - previous,
                bucket.repeat,
                sql_bytes(&bucket.lower),
                sql_bytes(&bucket.upper),
                bucket.ndv
            );
            if index > offset && sql.len() + tuple.len() + 1 > MAX_INSERT_LENGTH {
                break;
            }
            if index > offset {
                sql.push(',');
            }
            sql.push_str(&tuple);
            end += 1;
        }
        store.execute(&sql)?;
        offset = end;
    }
    Ok(())
}

/// Write the histogram row and all of its auxiliary rows in the same order as
/// TiDB's storage path.  Keeping this in one helper is important: replacing a
/// histogram without first removing its old FM sketch or usage row leaves
/// stale data visible to a later load.
fn save_column_or_index_stats_at_version(
    store: &dyn SqlStore,
    table_id: i64,
    is_index: bool,
    stats: &ColumnStats,
    version: u64,
    stats_version: i64,
    save_cmsketch: bool,
    save_fm_sketch: bool,
    update_analyze_time: bool,
) -> Result<(), Error> {
    let hist_id = stats.histogram.id;
    store.execute(&format!(
        "delete from mysql.stats_top_n where table_id={table_id} and is_index={} and hist_id={hist_id}",
        i32::from(is_index)
    ))?;
    save_top_n(store, table_id, is_index, hist_id, &stats.top_n)?;

    store.execute(&format!(
        "delete from mysql.stats_fm_sketch where table_id={table_id} and is_index={} and hist_id={hist_id}",
        i32::from(is_index)
    ))?;
    if save_fm_sketch && let Some(fm) = &stats.fm_sketch {
        store.execute(&format!(
            "insert into mysql.stats_fm_sketch (table_id,is_index,hist_id,value) values ({table_id},{},{hist_id},{})",
            i32::from(is_index),
            sql_bytes(fm)
        ))?;
    }

    let cm_sketch = if save_cmsketch {
        stats
            .cmsketch
            .as_deref()
            .map(sql_bytes)
            .unwrap_or_else(|| "NULL".to_owned())
    } else {
        "NULL".to_owned()
    };
    let total_column_size = stats.histogram.total_column_size.max(0);
    store.execute(&format!(
        "replace into mysql.stats_histograms (table_id,is_index,hist_id,distinct_count,version,null_count,cm_sketch,tot_col_size,stats_ver,correlation) values ({table_id},{},{hist_id},{},{version},{},{cm_sketch},{total_column_size},{},{})",
        i32::from(is_index),
        stats.histogram.ndv,
        stats.histogram.null_count,
        stats_version,
        stats.histogram.correlation
    ))?;

    store.execute(&format!(
        "delete from mysql.stats_buckets where table_id={table_id} and is_index={} and hist_id={hist_id}",
        i32::from(is_index)
    ))?;
    save_buckets(store, table_id, is_index, &stats.histogram)?;

    if update_analyze_time && !is_index {
        store.execute(&format!(
            "insert into mysql.column_stats_usage (table_id,column_id,last_analyzed_at) values ({table_id},{hist_id},current_timestamp()) on duplicate key update last_analyzed_at=current_timestamp()"
        ))?;
    }
    Ok(())
}

/// 将一次 ANALYZE 结果持久化到存储，并返回本次写入使用的版本号（start_ts）。
/// `analyze_snapshot` 为真时按快照增量合并行数；否则直接采用结果中的 count。
/// 若已有更新快照且为 v2 统计且非 MV/全局索引，则跳过写入并返回 0。
pub fn save_analyze_result_to_storage(
    store: &dyn SqlStore,
    results: &AnalyzeResults,
    analyze_snapshot: bool,
) -> Result<u64, Error> {
    // start_ts：事务开始时间戳，用作 stats 版本号以支持后续增量与 GC。
    let version = store.start_ts()?;
    // 负 table_id 用于锁定/探测伪元行；与正 ID 一并 FOR UPDATE。
    let fake_id = -results.table_id;
    let rows = store.execute(&format!("select snapshot,count,modify_count from mysql.stats_meta where table_id in ({fake_id},{}) for update", results.table_id))?;
    let current_count = rows.first().map_or(0, |row| row.int(1));
    let current_modify = rows.first().map_or(0, |row| row.int(2));
    let mut saved_version = version;
    // 已有相等或更新的 snapshot：跳过过期 ANALYZE，避免回退元数据。
    if rows
        .first()
        .is_some_and(|row| row.uint(0) >= results.snapshot)
        && results.stats_version == 2
        && !results.for_mv_or_global_index
    {
        return Ok(0);
    }
    // 无元数据或非 v2：REPLACE 整行；MV/全局索引的 snapshot/count 置 0。
    if rows.is_empty() || results.stats_version != 2 {
        let snapshot = if results.for_mv_or_global_index {
            0
        } else {
            results.snapshot
        };
        let count = if results.for_mv_or_global_index {
            0
        } else {
            results.count
        };
        store.execute(&format!("replace into mysql.stats_meta (version,table_id,count,snapshot,last_stats_histograms_version) values ({version},{},{count},{snapshot},{version})", results.table_id))?;
    // MV/全局索引：只刷新版本字段，不改动 count/snapshot。
    } else if results.for_mv_or_global_index {
        store.execute(&format!("update mysql.stats_meta set version={version},last_stats_histograms_version={version} where table_id={}", results.table_id))?;
        // Go intentionally leaves the named return value at zero for this
        // branch: the auxiliary MV/global-index analyze must not cause the
        // caller to record another table-level historical meta snapshot.
        saved_version = 0;
    } else {
        // 普通表：合并 modify_count，并按 analyze_snapshot 决定 count 增量或覆盖。
        let modify = (current_modify - results.base_modify_count).max(0);
        let count = if analyze_snapshot {
            current_count + results.count - results.base_count
        } else {
            results.count
        }
        .max(0);
        store.execute(&format!("update mysql.stats_meta set version={version},modify_count={modify},count={count},snapshot={},last_stats_histograms_version={version} where table_id={}", results.snapshot, results.table_id))?;
    }
    for (is_index, column) in &results.columns {
        save_column_or_index_stats_at_version(
            store,
            results.table_id,
            *is_index,
            column,
            version,
            results.stats_version,
            results.stats_version != 2,
            true,
            true,
        )?;
    }
    Ok(saved_version)
}

/// 覆盖写入单列或单索引的桶与 TopN，并可选更新 CMSketch / FMSketch。
/// 先删旧桶与 TopN，再插入新数据，保证与当前直方图一致。
pub fn save_column_or_index_stats(
    store: &dyn SqlStore,
    table_id: i64,
    is_index: bool,
    stats: &ColumnStats,
) -> Result<(), Error> {
    let version = store.start_ts()?;
    save_column_or_index_stats_at_version(
        store,
        table_id,
        is_index,
        stats,
        version,
        stats.stats_version,
        stats.stats_version != 2,
        false,
        false,
    )
}

/// 更新 `mysql.stats_meta` 中的 version、行数与修改计数，并同步 last_stats_histograms_version。
pub fn save_meta_to_storage(
    store: &dyn SqlStore,
    table_id: i64,
    version: u64,
    count: i64,
    modify_count: i64,
) -> Result<(), Error> {
    store.execute(&format!("update mysql.stats_meta set version={version},count={count},modify_count={modify_count},last_stats_histograms_version={version} where table_id={table_id}"))?;
    Ok(())
}

/// 插入/替换列直方图元行后，再写入桶、TopN 等明细。
pub fn insert_column_stats_to_kv(
    store: &dyn SqlStore,
    table_id: i64,
    column: &ColumnStats,
) -> Result<(), Error> {
    let version = store.start_ts()?;
    save_column_or_index_stats_at_version(
        store,
        table_id,
        false,
        column,
        version,
        column.stats_version,
        column.stats_version != 2,
        false,
        false,
    )
}

/// 将整张表的元数据、列统计与索引统计一次性写入 KV/系统表。
pub fn insert_table_stats_to_kv(store: &dyn SqlStore, table: &TableStats) -> Result<(), Error> {
    save_meta_to_storage(
        store,
        table.physical_id,
        table.version,
        table.count,
        table.modify_count,
    )?;
    for column in table.columns.values() {
        save_column_or_index_stats_at_version(
            store,
            table.physical_id,
            false,
            column,
            table.version,
            column.stats_version,
            column.stats_version != 2,
            false,
            false,
        )?;
    }
    for index in table.indices.values() {
        let version = table.version.max(index.histogram.last_update_version);
        save_column_or_index_stats_at_version(
            store,
            table.physical_id,
            true,
            index,
            version,
            index.stats_version,
            index.stats_version != 2,
            false,
            false,
        )?;
    }
    Ok(())
}
