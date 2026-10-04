// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 运行时统计构建：从行样本生成 `TableStats` 缓存项。
//
// 直方图（histogram）、TopN、FM Sketch 的构造委托给 `astersql_statistics`
// 包；本模块负责按表元信息筛选列/索引、组装 `ColumnStats`/`IndexStats`，
// 并输出与 Go 侧 runtime stats 对齐的缓存结构。

use std::collections::{BTreeSet, HashMap};

use astersql_meta_model::{TableInfo, mysql, types};

use crate::{Bucket, ColumnStats, IndexStats, TableStats};

/// 运行时可见的历史统计快照摘要（表 ID、版本、行数与修改计数）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RuntimeHistoricalSnapshot {
    /// 物理表 ID。
    pub table_id: i64,
    /// 统计版本（通常对应写入时的时间戳）。
    pub version: u64,
    /// 快照中的行数。
    pub row_count: i64,
    /// 相对上次 ANALYZE 的修改行累计。
    pub modify_count: i64,
    /// 是否来自历史统计表（`stats_history` / `stats_meta_history`）。
    pub is_historical: bool,
}

/// 列使用记录：上次被谓词使用与上次被 ANALYZE 的时间。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RuntimeColumnUsage {
    /// 所属物理表 ID。
    pub table_id: i64,
    /// 列 ID。
    pub column_id: i64,
    /// 上次出现在谓词中的时间（文本形式，与系统表一致）。
    pub last_used_at: Option<String>,
    /// 上次 ANALYZE 该列的时间。
    pub last_analyzed_at: Option<String>,
}

/// 运行中的 ANALYZE 作业状态视图（对应 `SHOW ANALYZE STATUS` 一类信息）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RuntimeAnalyzeJob {
    /// 本作业覆盖的物理表/分区 ID 列表。
    pub physical_ids: Vec<i64>,
    /// 请求的并发度。
    pub requested_concurrency: usize,
    /// 允许的最大并发度。
    pub max_concurrency: usize,
    /// 调度后预期活跃 worker 数。
    pub active_workers_after: usize,
    /// 库名。
    pub database: String,
    /// 表名。
    pub table: String,
    /// 分区名（非分区表可为空）。
    pub partition: String,
    /// 作业描述文本。
    pub job_info: String,
    /// 分析涉及的行数估计。
    pub row_count: i64,
    /// 开始时间文本。
    pub start_time: String,
    /// 结束时间文本。
    pub end_time: String,
    /// 作业状态（如 running / finished / failed）。
    pub state: String,
    /// 失败原因（若有）。
    pub fail_reason: Option<String>,
    /// 执行实例标识。
    pub instance: String,
    /// 关联进程 ID（若有）。
    pub process_id: Option<u64>,
    /// 剩余时长估计（若有）。
    pub remaining_duration: Option<String>,
}

/// 将规范统计包中的直方图桶转为 handle 侧 `Bucket`（含编码后的上下界）。
fn convert_buckets(
    builder: &astersql_statistics::RuntimeStatsBuilder,
    histogram: &astersql_statistics::Histogram,
    is_index: bool,
) -> Result<Vec<Bucket>, String> {
    histogram
        .Buckets
        .iter()
        .enumerate()
        .map(|(index, bucket)| {
            Ok(Bucket {
                count: bucket.Count,
                repeats: bucket.Repeat,
                lower: builder
                    .encode_histogram_bound(histogram, index * 2, is_index)
                    .map_err(|error| error.to_string())?,
                upper: builder
                    .encode_histogram_bound(histogram, index * 2 + 1, is_index)
                    .map_err(|error| error.to_string())?,
                ndv: bucket.NDV,
            })
        })
        .collect()
}

fn canonical_histogram(
    builder: &astersql_statistics::RuntimeStatsBuilder,
    id: i64,
    field_type: &types::FieldType,
    ndv: i64,
    null_count: i64,
    total_column_size: i64,
    version: u64,
    buckets: &[Bucket],
    is_index: bool,
) -> Result<astersql_statistics::Histogram, String> {
    let mut histogram = astersql_statistics::NewHistogram(
        id,
        ndv,
        null_count,
        version,
        field_type,
        buckets.len(),
        total_column_size,
    );
    let decode = |encoded: &[u8]| -> Result<datum::Datum, String> {
        if is_index {
            return builder
                .decode_histogram_bound(encoded, true)
                .map_err(|e| e.to_string());
        }
        if field_type.GetType() == mysql::TypeBit {
            let value = std::str::from_utf8(encoded)
                .map_err(|e| e.to_string())?
                .parse::<u64>()
                .map_err(|e| e.to_string())?;
            let mut d = datum::Datum::default();
            d.SetMysqlBit(datum::NewBinaryLiteralFromUint(
                value,
                (field_type.GetFlen() + 7) / 8,
            ));
            return Ok(d);
        }
        astersql_statistics::DecodeColumnTopNValue(encoded, field_type, builder.TimeZone())
            .map_err(|e| e.to_string())
    };
    for bucket in buckets {
        let lower = decode(&bucket.lower)?;
        let upper = decode(&bucket.upper)?;
        histogram.AppendBucketWithNDV(&lower, &upper, bucket.count, bucket.repeats, bucket.ndv);
    }
    Ok(histogram)
}

/// Merge partition TopN and histograms while retaining the caller's FM NDV.
/// Kept for callers that already express the TopN budget in the global profile.
pub fn MergeRuntimePartitionHistograms(
    builder: &astersql_statistics::RuntimeStatsBuilder,
    table: &TableInfo,
    global: &mut TableStats,
    partitions: &[TableStats],
    bucket_count: usize,
) -> Result<(), String> {
    let top_n_count = global
        .columns
        .values()
        .map(|stats| stats.top_n.len())
        .chain(global.indexes.values().map(|stats| stats.top_n.len()))
        .max()
        .unwrap_or(0);
    MergeRuntimePartitionStats(
        builder,
        table,
        global,
        partitions,
        top_n_count,
        bucket_count,
        &sqlkiller::sqlkiller::SQLKiller::default(),
    )
}

/// ANALYZE's combined merge boundary. Both streams come from the same
/// partition snapshots; a prior global TopN never controls the selection.
pub fn MergeRuntimePartitionStats(
    builder: &astersql_statistics::RuntimeStatsBuilder,
    table: &TableInfo,
    global: &mut TableStats,
    partitions: &[TableStats],
    top_n_count: usize,
    bucket_count: usize,
    killer: &sqlkiller::sqlkiller::SQLKiller,
) -> Result<(), String> {
    let sc = stmtctx::NewStmtCtxWithTimeZone(builder.TimeZone());
    let top_n_count =
        u32::try_from(top_n_count).map_err(|_| "TopN budget exceeds uint32".to_owned())?;
    let bucket_count =
        i64::try_from(bucket_count).map_err(|_| "bucket budget exceeds int64".to_owned())?;
    for column in table
        .Columns
        .iter()
        .filter(|column| !column.Hidden && !column.IsVirtualGenerated())
    {
        let Some(global_column) = global.columns.get_mut(&column.ID) else {
            continue;
        };
        if !global_column.analyzed_or_synthesized {
            continue;
        }
        let mut hists = Vec::new();
        let mut tops = Vec::new();
        for stats in partitions
            .iter()
            .filter_map(|p| p.columns.get(&column.ID))
            .filter(|stats| stats.analyzed_or_synthesized)
        {
            hists.push(Some(canonical_histogram(
                builder,
                column.ID,
                &column.FieldType,
                stats.ndv,
                stats.null_count,
                stats.total_column_size,
                stats.version,
                &stats.buckets,
                false,
            )?));
            tops.push(Some(canonical_top_n(&stats.top_n)));
        }
        if hists.is_empty() {
            continue;
        }
        let (top, merged) = astersql_statistics::MergePartTopNAndHistToGlobal(
            &sc,
            killer,
            &tops,
            &hists,
            top_n_count,
            bucket_count,
            false,
        )
        .map_err(|e| e.to_string())?;
        global_column.top_n = top.map_or_else(Vec::new, |top| {
            top.TopN.into_iter().map(|e| (e.Encoded, e.Count)).collect()
        });
        global_column.buckets = convert_buckets(builder, &merged, false)?;
        global_column.null_count = merged.NullCount;
        global_column.total_column_size = merged.TotColSize;
        global_column.correlation = merged.Correlation;
    }
    let index_type = types::NewFieldType(mysql::TypeBlob);
    for index in &table.Indices {
        let Some(global_index) = global.indexes.get_mut(&index.ID) else {
            continue;
        };
        if !global_index.analyzed {
            continue;
        }
        let mut hists = Vec::new();
        let mut tops = Vec::new();
        for stats in partitions
            .iter()
            .filter_map(|p| p.indexes.get(&index.ID))
            .filter(|stats| stats.analyzed)
        {
            hists.push(Some(canonical_histogram(
                builder,
                index.ID,
                &index_type,
                stats.ndv,
                stats.null_count,
                stats.total_column_size,
                stats.version,
                &stats.buckets,
                true,
            )?));
            tops.push(Some(canonical_top_n(&stats.top_n)));
        }
        if hists.is_empty() {
            continue;
        }
        let (top, merged) = astersql_statistics::MergePartTopNAndHistToGlobal(
            &sc,
            killer,
            &tops,
            &hists,
            top_n_count,
            bucket_count,
            true,
        )
        .map_err(|e| e.to_string())?;
        global_index.top_n = top.map_or_else(Vec::new, |top| {
            top.TopN.into_iter().map(|e| (e.Encoded, e.Count)).collect()
        });
        global_index.buckets = convert_buckets(builder, &merged, true)?;
        global_index.null_count = merged.NullCount;
        global_index.total_column_size = 0;
        global_index.correlation = merged.Correlation;
    }
    Ok(())
}
fn canonical_top_n(entries: &[(Vec<u8>, u64)]) -> astersql_statistics::TopN {
    let mut top = astersql_statistics::NewTopN(entries.len());
    for (key, count) in entries {
        top.AppendTopN(key.clone(), *count);
    }
    top.Sort();
    top
}

/// Produces the existing `TableStats` cache value from decoded KV rows.
/// Histogram construction and datum encoding are delegated to the canonical
/// statistics package; this module does not retain the intermediate objects.
///
/// 从解码后的 KV 行样本生成已有形态的 `TableStats` 缓存值。
/// 直方图构建与 datum 编码委托给规范统计包；本模块不保留中间对象。
pub fn BuildRuntimeTableStats(
    physical_id: i64,
    table: &TableInfo,
    rows: &[HashMap<String, Option<String>>],
    version: u64,
    topn: usize,
) -> Result<TableStats, String> {
    let builder = astersql_statistics::RuntimeStatsBuilder::default();
    BuildRuntimeTableStatsWithBuilder(&builder, physical_id, table, rows, version, topn)
}

/// 使用指定 builder、默认桶数、不限制列/索引集合地构建 `TableStats`。
pub fn BuildRuntimeTableStatsWithBuilder(
    builder: &astersql_statistics::RuntimeStatsBuilder,
    physical_id: i64,
    table: &TableInfo,
    rows: &[HashMap<String, Option<String>>],
    version: u64,
    topn: usize,
) -> Result<TableStats, String> {
    BuildRuntimeTableStatsSelectionWithBuilder(
        builder,
        physical_id,
        table,
        rows,
        version,
        topn,
        astersql_statistics::DefaultHistogramBuckets,
        None,
        None,
    )
}

/// `analyzed_indexes` restricts which indexes are collected (Go statistics
/// version 1 `ANALYZE TABLE ... INDEX`), and `selected_columns` restricts the
/// collected columns to Go's `AnalyzeColumnsExec.colsInfo`.
///
/// `analyzed_indexes` 限制收集哪些索引（对应 Go 统计版本 1 的
/// `ANALYZE TABLE ... INDEX`）；`selected_columns` 对应 Go
/// `AnalyzeColumnsExec.colsInfo`，只收集指定列。
pub fn BuildRuntimeTableStatsSelectionWithBuilder(
    builder: &astersql_statistics::RuntimeStatsBuilder,
    physical_id: i64,
    table: &TableInfo,
    rows: &[HashMap<String, Option<String>>],
    version: u64,
    topn: usize,
    buckets: usize,
    analyzed_indexes: Option<&BTreeSet<String>>,
    selected_columns: Option<&BTreeSet<String>>,
) -> Result<TableStats, String> {
    // 若指定了索引集合，则只收集这些索引覆盖的列；否则沿用 selected_columns。
    let analyzed_columns = analyzed_indexes
        .map(|names| {
            table
                .Indices
                .iter()
                .filter(|index| names.is_empty() || names.contains(&index.Name.L))
                .flat_map(|index| index.Columns.iter().map(|column| column.Name.L.clone()))
                .collect::<BTreeSet<_>>()
        })
        .or_else(|| selected_columns.cloned());
    let mut columns = HashMap::new();
    // 跳过 Hidden 列，并按 analyzed_columns 过滤后为每列建直方图与 FM Sketch。
    for column in table.Columns.iter().filter(|column| {
        !column.Hidden
            && analyzed_columns
                .as_ref()
                .is_none_or(|names| names.contains(&column.Name.L))
    }) {
        // Go disables TopN for a column covered by a single-column unique
        // index; otherwise a one-row unique table would incorrectly expose
        // its only value as a frequent value.
        let unique_column = table.Indices.iter().any(|index| {
            index.Unique
                && !index.MVIndex
                && index.Columns.len() == 1
                && index.Columns[0].Length == -1
                && index.Columns[0].Name.L == column.Name.L
        });
        let column_topn = if unique_column { 0 } else { topn };
        let input = rows
            .iter()
            .map(|row| vec![row.get(&column.Name.L).cloned().unwrap_or(None)])
            .collect::<Vec<_>>();
        let (mut histogram, top_n) = builder
            .build_histogram_with_buckets(
                column.ID,
                std::slice::from_ref(&column.FieldType),
                &input,
                false,
                column_topn,
                buckets,
            )
            .map_err(|error| error.to_string())?;
        histogram.LastUpdateVersion = version;
        columns.insert(
            column.ID,
            ColumnStats {
                analyzed_or_synthesized: true,
                stats_version: 2,
                ndv: histogram.NDV,
                null_count: histogram.NullCount,
                total_column_size: histogram.TotColSize,
                version,
                loaded_or_evicted: true,
                field_type: column.GetType(),
                correlation: histogram.Correlation,
                average_size: if rows.is_empty() {
                    0.0
                } else {
                    histogram.TotColSize as f64 / rows.len() as f64
                },
                top_n: top_n
                    .TopN
                    .into_iter()
                    .map(|item| (item.Encoded, item.Count))
                    .collect(),
                buckets: convert_buckets(builder, &histogram, false)?,
                fm_sketch: builder
                    .encode_fm_sketch(std::slice::from_ref(&column.FieldType), &input, false)
                    .map_err(|error| error.to_string())?,
            },
        );
    }
    // Go keeps one zero-valued stats_histograms row for every schema column,
    // including columns omitted by ANALYZE COLUMNS and virtual generated
    // columns.  These rows retain stats_ver=0 until they are analyzed.
    for column in table.Columns.iter().filter(|column| !column.Hidden) {
        columns.entry(column.ID).or_insert_with(|| ColumnStats {
            field_type: column.GetType(),
            ..ColumnStats::default()
        });
    }

    let mut indexes = HashMap::new();
    // 按索引列偏移从 TableInfo 取列定义，再对（可能多列）键值建索引直方图。
    for index in table.Indices.iter().filter(|index| {
        analyzed_indexes.is_none_or(|names| names.is_empty() || names.contains(&index.Name.L))
    }) {
        if index.Columns.is_empty() {
            return Err(format!("index {} contains no columns", index.Name.O));
        }
        let index_columns = index
            .Columns
            .iter()
            .map(|column| {
                let offset = usize::try_from(column.Offset).map_err(|_| {
                    format!("index {} contains negative column offset", index.Name.O)
                })?;
                let info = table.Columns.get(offset).ok_or_else(|| {
                    format!(
                        "index {} column offset {} exceeds table column count {}",
                        index.Name.O,
                        offset,
                        table.Columns.len()
                    )
                })?;
                if info.Name.L != column.Name.L {
                    return Err(format!(
                        "index {} column {} does not match table offset {}",
                        index.Name.O, column.Name.O, offset
                    ));
                }
                Ok(info)
            })
            .collect::<Result<Vec<_>, String>>()?;
        let column_names = index_columns
            .iter()
            .map(|column| column.Name.L.clone())
            .collect::<Vec<_>>();
        let mut input = rows
            .iter()
            .map(|row| {
                column_names
                    .iter()
                    .map(|column| row.get(column).cloned().unwrap_or(None))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        // 单列索引：NULL 行不参与直方图，但 NullCount 需单独保留。
        let single_column_nulls = if column_names.len() == 1 {
            let null_count = input.iter().filter(|row| row[0].is_none()).count() as i64;
            input.retain(|row| row[0].is_some());
            null_count
        } else {
            0
        };
        let field_types = index_columns
            .iter()
            .map(|column| column.FieldType.clone())
            .collect::<Vec<_>>();
        let index_topn = if index.Unique
            && !index.MVIndex
            && index.Columns.len() == 1
            && index.Columns[0].Length == -1
        {
            0
        } else {
            topn
        };
        let (mut histogram, top_n) = builder
            .build_histogram_with_buckets(index.ID, &field_types, &input, true, index_topn, buckets)
            .map_err(|error| error.to_string())?;
        histogram.NullCount = single_column_nulls;
        histogram.LastUpdateVersion = version;
        indexes.insert(
            index.ID,
            IndexStats {
                analyzed: true,
                stats_version: 2,
                version,
                ndv: histogram.NDV,
                null_count: histogram.NullCount,
                total_column_size: histogram.TotColSize,
                correlation: histogram.Correlation,
                cms_loaded: true,
                top_n: top_n
                    .TopN
                    .into_iter()
                    .map(|item| (item.Encoded, item.Count))
                    .collect(),
                buckets: convert_buckets(builder, &histogram, true)?,
                fully_loaded: true,
                fm_sketch: builder
                    .encode_fm_sketch(&field_types, &input, true)
                    .map_err(|error| error.to_string())?,
            },
        );
    }
    for index in &table.Indices {
        indexes.entry(index.ID).or_default();
    }
    // realtime_count 取样本行数；ANALYZE 场景下即为全表扫描行数。
    let realtime_count = rows.len() as i64;
    Ok(TableStats {
        physical_id,
        pseudo: false,
        initialized: true,
        version,
        modify_count: 0,
        realtime_count,
        analyze_count: realtime_count,
        last_analyze_version: version,
        last_stats_hist_version: version,
        stats_version: 2,
        indexes,
        columns,
        pre_scalar_ready: true,
    })
}
