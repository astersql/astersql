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

// 从 `mysql.stats_*` 系统表读取统计信息。
//
// 提供 meta 行计数、直方图（histogram）桶重建、TopN / CMSketch / FM Sketch
// 加载，以及按快照组装完整 `TableStats` 的路径。

use crate::{Bucket, ColumnStats, Error, Histogram, SqlStore, TableStats, TopNItem};

/// 读取 `stats_meta` 的 count / modify_count；无行时第三元为 true。
///
/// `for_update` 为真时附加 `FOR UPDATE` 行锁（事务内防止并发改写）。
pub fn stats_meta_count_and_modify_count(
    store: &dyn SqlStore,
    table_id: i64,
    for_update: bool,
) -> Result<(i64, i64, bool), Error> {
    let suffix = if for_update { " for update" } else { "" };
    let rows = store.execute(&format!(
        "select count,modify_count from mysql.stats_meta where table_id={table_id}{suffix}"
    ))?;
    match rows.first() {
        Some(row) => Ok((row.int(0), row.int(1), false)),
        None => Ok((0, 0, true)),
    }
}

/// 从系统表重建单个列/索引直方图；`priority` 控制 SQL 优先级提示。
pub fn histogram_from_storage(
    store: &dyn SqlStore,
    table_id: i64,
    is_index: bool,
    hist_id: i64,
    priority: i32,
) -> Result<Option<Histogram>, Error> {
    let meta = store.execute(&format!("select high_priority distinct_count,version,null_count,tot_col_size,stats_ver,correlation from mysql.stats_histograms where table_id={table_id} and hist_id={hist_id} and is_index={}", i32::from(is_index)))?;
    let Some(row) = meta.first() else {
        return Ok(None);
    };
    let prefix = match priority {
        1 => "high_priority ",
        -1 => "low_priority ",
        _ => "",
    };
    let rows = store.execute(&format!("select {prefix}count,repeats,lower_bound,upper_bound,ndv from mysql.stats_buckets where table_id={table_id} and is_index={} and hist_id={hist_id} order by bucket_id", i32::from(is_index)))?;
    // 桶 count 在系统表中是增量；累计为直方图 CumulativeCount。
    let mut total = 0;
    let mut buckets = Vec::with_capacity(rows.len());
    for row in rows {
        total += row.int(0);
        buckets.push(Bucket {
            count: total,
            repeat: row.int(1),
            lower: row.bytes(2),
            upper: row.bytes(3),
            ndv: row.int(4),
        });
    }
    Ok(Some(Histogram {
        id: hist_id,
        ndv: row.int(0),
        last_update_version: row.uint(1),
        null_count: row.int(2),
        total_column_size: row.int(3),
        correlation: match row.0.get(5) {
            Some(crate::Value::Float(value)) => *value,
            _ => 0.0,
        },
        buckets,
    }))
}

/// 读取 TopN；统计版本 ≤1 时附带 CMSketch（Count-Min Sketch，频次估计结构）。
pub fn cmsketch_and_top_n_from_storage(
    store: &dyn SqlStore,
    table_id: i64,
    is_index: bool,
    hist_id: i64,
    stats_version: i64,
) -> Result<(Option<Vec<u8>>, Vec<TopNItem>), Error> {
    let rows = store.execute(&format!("select high_priority value,count from mysql.stats_top_n where table_id={table_id} and is_index={} and hist_id={hist_id}", i32::from(is_index)))?;
    let top_n = rows
        .into_iter()
        .map(|row| TopNItem {
            encoded: row.bytes(0),
            count: row.uint(1),
        })
        .collect();
    // 版本 2+ 不再使用 CMSketch，改由直方图 NDV / TopN 承担。
    let cms = if stats_version > 1 {
        None
    } else {
        store.execute(&format!("select cm_sketch from mysql.stats_histograms where table_id={table_id} and is_index={} and hist_id={hist_id}", i32::from(is_index)))?.first().map(|row| row.bytes(0)).filter(|value| !value.is_empty())
    };
    Ok((cms, top_n))
}

/// 读取 FM Sketch（Flajolet–Martin，基数估计）原始字节。
pub fn fm_sketch_from_storage(
    store: &dyn SqlStore,
    table_id: i64,
    is_index: bool,
    hist_id: i64,
) -> Result<Option<Vec<u8>>, Error> {
    Ok(store.execute(&format!("select hex(value) from mysql.stats_fm_sketch where table_id={table_id} and is_index={} and hist_id={hist_id}", i32::from(is_index)))?.first().map(|row| decode_hex(&row.bytes(0))))
}

/// The restricted statistics SQL surface renders blob columns as hexadecimal
/// text; recover the original bytes. Non-hex payloads are returned verbatim so
/// stores that already hand back raw blobs keep working.
///
/// 受限统计 SQL 将 blob 列渲染为十六进制文本；此处还原原始字节。
/// 非 hex 载荷原样返回，兼容已直接返回原始 blob 的存储实现。
fn decode_hex(value: &[u8]) -> Vec<u8> {
    if value.is_empty() || value.len() % 2 != 0 || !value.iter().all(u8::is_ascii_hexdigit) {
        return value.to_vec();
    }
    value
        .chunks_exact(2)
        .map(|pair| {
            let digit = |byte: u8| match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                _ => byte - b'A' + 10,
            };
            digit(pair[0]) * 16 + digit(pair[1])
        })
        .collect()
}

/// 检查分区统计是否已写入 `stats_histograms`；缺失则报错。
pub fn check_partition_stats(
    store: &dyn SqlStore,
    table_id: i64,
    is_index: bool,
    hist_id: Option<i64>,
) -> Result<(), Error> {
    let hist = hist_id
        .map(|id| format!(" and hist_id={id}"))
        .unwrap_or_default();
    if store.execute(&format!("select distinct_count from mysql.stats_histograms where table_id={table_id} and is_index={}{}", i32::from(is_index), hist))?.is_empty() { return Err(Error(if hist_id.is_some() { "partition column stats missing".into() } else { "partition stats missing".into() })); }
    Ok(())
}

/// 从系统表组装完整 `TableStats`；`snapshot!=0` 时只取 `version <= snapshot` 的 meta。
pub fn table_stats_from_storage(
    store: &dyn SqlStore,
    table_id: i64,
    snapshot: u64,
    existing: Option<TableStats>,
) -> Result<TableStats, Error> {
    let suffix = if snapshot == 0 {
        String::new()
    } else {
        format!(" and version <= {snapshot}")
    };
    let meta = store.execute(&format!(
        "select version,modify_count,count from mysql.stats_meta where table_id={table_id}{suffix}"
    ))?;
    let Some(row) = meta.first() else {
        return Ok(existing.unwrap_or_else(|| TableStats {
            physical_id: table_id,
            ..TableStats::default()
        }));
    };
    let mut table = existing.unwrap_or_else(|| TableStats {
        physical_id: table_id,
        ..TableStats::default()
    });
    // The histogram query is a complete snapshot of this table's stats.  Do
    // not retain entries from the cache baseline when a DDL has removed them.
    table.columns.clear();
    table.indices.clear();
    table.stats_version = 0;
    table.version = row.uint(0);
    table.modify_count = row.int(1);
    table.count = row.int(2);
    let histograms = store.execute(&format!(
        "select hist_id,is_index,stats_ver from mysql.stats_histograms where table_id={table_id}"
    ))?;
    for item in histograms {
        let id = item.int(0);
        let is_index = item.int(1) == 1;
        let stats_version = item.int(2);
        let Some(histogram) = histogram_from_storage(store, table_id, is_index, id, 0)? else {
            continue;
        };
        let (cmsketch, top_n) =
            cmsketch_and_top_n_from_storage(store, table_id, is_index, id, stats_version)?;
        let stats = ColumnStats {
            name: id.to_string(),
            histogram,
            cmsketch,
            top_n,
            fm_sketch: fm_sketch_from_storage(store, table_id, is_index, id)?,
            stats_version,
        };
        table.stats_version = table.stats_version.max(stats_version);
        if is_index {
            table.indices.insert(id.to_string(), stats);
        } else {
            table.columns.insert(id.to_string(), stats);
        }
    }
    Ok(table)
}

/// 按快照读取 `stats_meta` 的 version / modify_count / count；缺失返回全 0。
pub fn stats_meta_by_table_id(
    store: &dyn SqlStore,
    table_id: i64,
    snapshot: u64,
) -> Result<(u64, i64, i64), Error> {
    let suffix = if snapshot == 0 {
        String::new()
    } else {
        format!(" and version <= {snapshot}")
    };
    let rows = store.execute(&format!(
        "select version,modify_count,count from mysql.stats_meta where table_id={table_id}{suffix}"
    ))?;
    Ok(rows
        .first()
        .map_or((0, 0, 0), |row| (row.uint(0), row.int(1), row.int(2))))
}
