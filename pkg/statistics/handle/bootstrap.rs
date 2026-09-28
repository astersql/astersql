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

// 统计信息 bootstrap（启动加载）逻辑。
//
// 从 `mysql.stats_meta` / `stats_histograms` / `stats_top_n` / `stats_buckets`
// 分页或按表 ID 列表加载统计，写入本地 `StatsCache`，并在内存配额允许时
// 逐步加载完整直方图、TopN 与桶数据。对应 Go `handle` 包的 init stats 路径。

use std::collections::HashSet;

use crate::handle::{
    Bucket, ColumnStats, Error, Handle, HandleBackend, IndexStats, StatsCache, TableInfo,
    TableStats,
};

/// 按 table_id 分页加载时的步长（每批覆盖的 ID 区间宽度）。
pub const INIT_STATS_STEP: i64 = 500;
/// 初始化进度百分比上报的间隔步长（约 33%）。
pub const INIT_STATS_PERCENTAGE_INTERVAL: f64 = 33.0;

/// `mysql.stats_meta` 查询行：表级版本、修改量与快照。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetaRow {
    /// 元数据版本号。
    pub version: u64,
    /// 物理表 / 分区 ID。
    pub table_id: i64,
    /// 自上次 ANALYZE 以来的修改行数估计。
    pub modify_count: i64,
    /// 实时行数估计。
    pub count: i64,
    /// 上次 ANALYZE 快照版本。
    pub snapshot: u64,
    /// 上次直方图版本；缺失时回退到 snapshot。
    pub last_histogram_version: Option<u64>,
}

/// `mysql.stats_histograms` 查询行：列/索引直方图元信息。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistogramRow {
    pub table_id: i64,
    /// 是否为索引统计（否则为列统计）。
    pub is_index: bool,
    /// 列 ID 或索引 ID。
    pub histogram_id: i64,
    /// 不同值个数（NDV）。
    pub ndv: i64,
    pub version: u64,
    pub null_count: i64,
    /// Count-Min Sketch 序列化字节（用于等值估计）。
    pub cm_sketch: Vec<u8>,
    pub total_column_size: i64,
    /// 统计版本；0 表示未真正 ANALYZE。
    pub stats_version: i64,
    pub correlation: f64,
}

/// `mysql.stats_top_n` 查询行：高频值及其出现次数。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TopNRow {
    pub table_id: i64,
    pub histogram_id: i64,
    pub value: Vec<u8>,
    pub count: u64,
}

/// `mysql.stats_buckets` 查询行：直方图单个桶的边界与计数。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BucketRow {
    pub table_id: i64,
    pub histogram_id: i64,
    pub count: i64,
    pub repeats: i64,
    pub lower: Vec<u8>,
    pub upper: Vec<u8>,
    pub ndv: i64,
}

/// 直方图查询的选择范围：全表、指定 ID 列表或半开区间。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuerySelection {
    /// 不加过滤，扫描全部。
    All,
    /// `WHERE table_id IN (...)`。
    TableIds(Vec<i64>),
    /// `WHERE table_id >= lo AND table_id < hi`。
    Range([i64; 2]),
}

/// Bootstrap 所需的后端能力：事务、元数据/直方图查询与内存配额。
pub trait BootstrapBackend: HandleBackend {
    fn begin(&mut self) -> Result<(), Error>;
    fn commit(&mut self) -> Result<(), Error>;
    fn query_meta(&mut self, table_ids: &[i64]) -> Result<Vec<MetaRow>, Error>;
    fn query_histograms(&mut self, selection: &QuerySelection) -> Result<Vec<HistogramRow>, Error>;
    fn query_top_n(&mut self, range: [i64; 2]) -> Result<Vec<TopNRow>, Error>;
    fn query_bucket_table_ids(&mut self, range: [i64; 2]) -> Result<HashSet<i64>, Error>;
    fn query_buckets(&mut self, range: [i64; 2]) -> Result<Vec<BucketRow>, Error>;
    fn physical_id_exists(&self, physical_id: i64) -> bool;
    fn table_info(&self, physical_id: i64) -> Option<TableInfo>;
    fn total_memory(&mut self) -> Result<u64, Error>;
    fn stats_cache_quota(&self) -> i64;
    fn init_concurrency(&self) -> usize;
    fn set_init_percentage(&mut self, percentage: f64);
}

/// 生成加载 `stats_meta` 的 SQL；`table_ids` 非空时附加 IN 过滤。
pub fn gen_init_stats_meta_sql(table_ids: &[i64]) -> String {
    let prefix = "select HIGH_PRIORITY version, table_id, modify_count, count, snapshot, last_stats_histograms_version from mysql.stats_meta";
    if table_ids.is_empty() {
        prefix.into()
    } else {
        format!("{prefix} where table_id in ({})", join_ids(table_ids))
    }
}

/// 生成直方图 SQL 的选项：分页区间或显式表 ID 列表。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenHistogramSqlOptions {
    pub paging: bool,
    pub table_range: [i64; 2],
    pub table_ids: Vec<i64>,
}

impl GenHistogramSqlOptions {
    /// 构造分页模式选项（半开区间 `[lo, hi)`）。
    pub fn paging(table_range: [i64; 2]) -> Self {
        assert!(
            table_range[0] < table_range[1],
            "paging requires a valid range"
        );
        Self {
            paging: true,
            table_range,
            table_ids: Vec::new(),
        }
    }

    /// 构造按表 ID 列表过滤的选项；空列表表示全表。
    pub fn table_ids(table_ids: &[i64]) -> Self {
        assert!(
            table_ids.iter().all(|id| *id >= 0),
            "table IDs must be non-negative"
        );
        Self {
            paging: false,
            table_range: [0, 0],
            table_ids: table_ids.to_vec(),
        }
    }

    /// 转换为后端查询选择枚举。
    fn selection(&self) -> QuerySelection {
        if self.paging {
            QuerySelection::Range(self.table_range)
        } else if self.table_ids.is_empty() {
            QuerySelection::All
        } else {
            QuerySelection::TableIds(self.table_ids.clone())
        }
    }
}

/// 生成加载 `stats_histograms` 的 SQL（含 ORDER_INDEX 提示）。
pub fn gen_init_stats_histograms_sql(options: &GenHistogramSqlOptions) -> String {
    let prefix = "select /*+ ORDER_INDEX(mysql.stats_histograms,tbl) */ HIGH_PRIORITY table_id, is_index, hist_id, distinct_count, version, null_count, cm_sketch, tot_col_size, stats_ver, correlation from mysql.stats_histograms";
    let suffix = " order by table_id";
    if options.paging {
        format!(
            "{prefix} where table_id >= {} and table_id < {}{suffix}",
            options.table_range[0], options.table_range[1]
        )
    } else if options.table_ids.is_empty() {
        format!("{prefix}{suffix}")
    } else {
        format!(
            "{prefix} where table_id in ({}){suffix}",
            join_ids(&options.table_ids)
        )
    }
}

/// 生成加载索引 TopN 的 SQL；`paging` 时附加 table_id 区间过滤。
pub fn gen_init_stats_top_n_sql_for_indexes(paging: bool, table_range: [i64; 2]) -> String {
    let prefix = "select /*+ ORDER_INDEX(mysql.stats_top_n,tbl) */ HIGH_PRIORITY table_id, hist_id, value, count from mysql.stats_top_n where is_index = 1";
    if !paging {
        return format!("{prefix} order by table_id");
    }
    assert!(table_range[0] < table_range[1], "invalid table range");
    format!(
        "{prefix} and table_id >= {} and table_id < {} order by table_id",
        table_range[0], table_range[1]
    )
}

/// 生成加载索引直方图桶的 SQL；`paging` 时附加 table_id 区间过滤。
pub fn gen_init_stats_buckets_sql_for_indexes(paging: bool, table_range: [i64; 2]) -> String {
    let prefix = "select /*+ ORDER_INDEX(mysql.stats_buckets,tbl) */ HIGH_PRIORITY table_id, hist_id, count, repeats, lower_bound, upper_bound, ndv from mysql.stats_buckets where is_index=1";
    if !paging {
        return format!("{prefix} order by table_id");
    }
    assert!(table_range[0] < table_range[1], "invalid table range");
    format!(
        "{prefix} and table_id >= {} and table_id < {} order by table_id",
        table_range[0], table_range[1]
    )
}

/// 将表 ID 列表拼成 SQL IN 子句中的逗号分隔串。
fn join_ids(ids: &[i64]) -> String {
    ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
}

/// 启动加载策略：按最大 table_id 分页，或按显式表列表逐表加载。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadStrategy {
    /// 扫描 `[0, max_table_id]`，按 `INIT_STATS_STEP` 切分任务。
    MaxTableId(i64),
    /// 每个表 ID 对应一个长度为 1 的区间任务。
    TableList(Vec<i64>),
}

impl LoadStrategy {
    /// 表列表非空时用 TableList，否则用 MaxTableId。
    pub fn new(max_table_id: i64, table_ids: &[i64]) -> Self {
        if table_ids.is_empty() {
            assert!(max_table_id >= 0, "max table ID must be non-negative");
            Self::MaxTableId(max_table_id)
        } else {
            Self::TableList(table_ids.to_vec())
        }
    }

    /// 估算总任务数（用于进度上报）。
    pub fn total_task_count(&self) -> u64 {
        match self {
            Self::MaxTableId(maximum) if *maximum > INIT_STATS_STEP * 2 => {
                (*maximum / INIT_STATS_STEP) as u64
            }
            Self::MaxTableId(_) => 1,
            Self::TableList(ids) => {
                assert!(!ids.is_empty(), "table list must not be empty");
                ids.len() as u64
            }
        }
    }

    /// 生成半开区间任务列表 `[start, end)`。
    pub fn tasks(&self) -> Vec<[i64; 2]> {
        match self {
            Self::MaxTableId(maximum) => (0..=*maximum)
                .step_by(INIT_STATS_STEP as usize)
                .map(|start| [start, start + INIT_STATS_STEP])
                .collect(),
            Self::TableList(ids) => ids.iter().map(|id| [*id, *id + 1]).collect(),
        }
    }
}

impl<B: BootstrapBackend> Handle<B> {
    /// 轻量初始化：仅加载 meta + 直方图摘要，不加载 TopN/桶。
    pub fn init_stats_lite(&mut self, table_ids: &[i64]) -> Result<(), Error> {
        if let Err(error) = self.backend.begin() {
            return finish_transaction(Err(error), self.backend.commit());
        }
        let result = (|| {
            let (mut local, _) = self.load_meta(table_ids)?;
            local.wait_for_async_updates();
            let options = GenHistogramSqlOptions::table_ids(table_ids);
            let rows = self.backend.query_histograms(&options.selection())?;
            load_histograms_lite(&mut local, rows);
            local.wait_for_async_updates();
            self.publish_cache(local, table_ids.is_empty());
            Ok(())
        })();
        finish_transaction(result, self.backend.commit())
    }

    /// 完整初始化：meta → 直方图 →（配额允许）TopN → 桶，并上报进度百分比。
    pub fn init_stats(&mut self, table_ids: &[i64]) -> Result<(), Error> {
        self.backend.set_init_percentage(0.0);
        let result = (|| {
            let total_memory = self.backend.total_memory()?;
            if let Err(error) = self.backend.begin() {
                return finish_transaction(Err(error), self.backend.commit());
            }
            let result = (|| {
                let (mut local, max_table_id) = self.load_meta(table_ids)?;
                local.wait_for_async_updates();
                self.backend
                    .set_init_percentage(INIT_STATS_PERCENTAGE_INTERVAL);
                let strategy = LoadStrategy::new(max_table_id, table_ids);
                let _concurrency = self.backend.init_concurrency().max(1);
                // 第一阶段：按区间加载完整直方图元数据（CMS 是否装入取决于缓存是否已满）。
                for range in strategy.tasks() {
                    let rows = self
                        .backend
                        .query_histograms(&QuerySelection::Range(range))?;
                    let full =
                        is_full_cache(&local, total_memory, self.backend.stats_cache_quota());
                    self.load_histograms_full(&mut local, rows, full);
                    local.wait_for_async_updates();
                }
                // 第二阶段：缓存未满时加载 TopN。
                if !is_full_cache(&local, total_memory, self.backend.stats_cache_quota()) {
                    for range in strategy.tasks() {
                        if is_full_cache(&local, total_memory, self.backend.stats_cache_quota()) {
                            break;
                        }
                        let with_buckets = self.backend.query_bucket_table_ids(range)?;
                        let rows = self.backend.query_top_n(range)?;
                        load_top_n(&mut local, rows, &with_buckets);
                        local.wait_for_async_updates();
                    }
                }
                self.backend
                    .set_init_percentage(INIT_STATS_PERCENTAGE_INTERVAL * 2.0);
                // 第三阶段：缓存未满时加载直方图桶，并标记标量统计就绪。
                if !is_full_cache(&local, total_memory, self.backend.stats_cache_quota()) {
                    for range in strategy.tasks() {
                        if is_full_cache(&local, total_memory, self.backend.stats_cache_quota()) {
                            break;
                        }
                        let rows = self.backend.query_buckets(range)?;
                        load_buckets(&mut local, rows);
                        local.wait_for_async_updates();
                    }
                    for table in local.values_mut() {
                        table.pre_scalar_ready = true;
                    }
                }
                local.wait_for_async_updates();
                self.publish_cache(local, table_ids.is_empty());
                Ok(())
            })();
            finish_transaction(result, self.backend.commit())
        })();
        self.backend.set_init_percentage(100.0);
        result
    }

    /// 加载 stats_meta 到本地缓存，跳过 schema 中已不存在的 physical_id。
    fn load_meta(&mut self, table_ids: &[i64]) -> Result<(StatsCache, i64), Error> {
        let rows = self.backend.query_meta(table_ids)?;
        let mut cache = StatsCache::default();
        let mut max_physical_id = 0;
        for row in rows {
            if !self.backend.physical_id_exists(row.table_id) {
                continue;
            }
            max_physical_id = max_physical_id.max(row.table_id);
            // last_hist 至少不小于 snapshot，避免直方图版本回退。
            let last_hist = row
                .last_histogram_version
                .unwrap_or(row.snapshot)
                .max(row.snapshot);
            cache.put(TableStats {
                physical_id: row.table_id,
                version: row.version,
                modify_count: row.modify_count,
                realtime_count: row.count,
                last_analyze_version: row.snapshot,
                last_stats_hist_version: last_hist,
                ..TableStats::default()
            });
        }
        Ok((cache, max_physical_id))
    }

    /// 将直方图行写入缓存；校验列/索引 ID 属于当前表元信息。
    fn load_histograms_full(
        &self,
        cache: &mut StatsCache,
        rows: Vec<HistogramRow>,
        cache_full: bool,
    ) {
        for row in rows {
            let Some(info) = self.backend.table_info(row.table_id) else {
                continue;
            };
            let Some(table) = cache.get_mut(row.table_id) else {
                continue;
            };
            if row.stats_version != 0 {
                table.stats_version = row.stats_version;
                table.last_analyze_version = table.last_analyze_version.max(row.version);
            }
            if row.is_index {
                if !info.index_ids.contains(&row.histogram_id) {
                    continue;
                }
                table.indexes.insert(
                    row.histogram_id,
                    IndexStats {
                        analyzed: row.stats_version != 0,
                        stats_version: row.stats_version,
                        version: row.version,
                        ndv: row.ndv,
                        null_count: row.null_count,
                        total_column_size: row.total_column_size,
                        correlation: row.correlation,
                        // 缓存已满时跳过 CMS 装载以节省内存。
                        cms_loaded: !cache_full && !row.cm_sketch.is_empty(),
                        fully_loaded: false,
                        ..IndexStats::default()
                    },
                );
            } else {
                if !info.column_ids.contains(&row.histogram_id) {
                    continue;
                }
                table.columns.insert(
                    row.histogram_id,
                    ColumnStats {
                        analyzed_or_synthesized: row.stats_version != 0
                            || row.ndv > 0
                            || row.null_count > 0,
                        stats_version: row.stats_version,
                        ndv: row.ndv,
                        null_count: row.null_count,
                        total_column_size: row.total_column_size,
                        version: row.version,
                        loaded_or_evicted: row.stats_version != 0
                            || row.ndv > 0
                            || row.null_count > 0,
                        ..ColumnStats::default()
                    },
                );
            }
        }
    }

    /// 发布本地缓存：全量替换或按表合并进现有 cache。
    fn publish_cache(&mut self, mut cache: StatsCache, full: bool) {
        if full {
            self.replace_cache(cache);
        } else {
            for table in cache.drain() {
                self.cache.put(table);
            }
            self.cache.wait_for_async_updates();
            cache.close();
        }
    }
}

/// 轻量模式：仅更新 analyzed / analyzed_or_synthesized 标记。
fn load_histograms_lite(cache: &mut StatsCache, rows: Vec<HistogramRow>) {
    for row in rows {
        let Some(table) = cache.get_mut(row.table_id) else {
            continue;
        };
        if row.stats_version != 0 {
            table.stats_version = row.stats_version;
            table.last_analyze_version = table.last_analyze_version.max(row.version);
        }
        if row.is_index {
            table.indexes.entry(row.histogram_id).or_default().analyzed = row.stats_version != 0;
        } else {
            table
                .columns
                .entry(row.histogram_id)
                .or_default()
                .analyzed_or_synthesized =
                row.stats_version != 0 || row.ndv > 0 || row.null_count > 0;
        }
    }
}

/// 加载 TopN；若该表没有桶数据则可将索引标为 fully_loaded。
fn load_top_n(cache: &mut StatsCache, rows: Vec<TopNRow>, tables_with_buckets: &HashSet<i64>) {
    let touched = rows.iter().map(|row| row.table_id).collect::<HashSet<_>>();
    for row in rows {
        let Some(index) = cache
            .get_mut(row.table_id)
            .and_then(|table| table.indexes.get_mut(&row.histogram_id))
        else {
            continue;
        };
        // 旧版本且未装 CMS 时跳过 TopN，避免不完整索引统计。
        if !index.cms_loaded && index.stats_version <= 1 {
            continue;
        }
        index.top_n.push((row.value, row.count));
    }
    for table_id in touched {
        if let Some(table) = cache.get_mut(table_id) {
            for index in table.indexes.values_mut() {
                index.top_n.sort_by(|left, right| left.0.cmp(&right.0));
                if !tables_with_buckets.contains(&table_id) {
                    index.fully_loaded = true;
                }
            }
        }
    }
}

/// 加载直方图桶，并将触及的索引标为 fully_loaded。
fn load_buckets(cache: &mut StatsCache, rows: Vec<BucketRow>) {
    let touched = rows.iter().map(|row| row.table_id).collect::<HashSet<_>>();
    for row in rows {
        let Some(index) = cache
            .get_mut(row.table_id)
            .and_then(|table| table.indexes.get_mut(&row.histogram_id))
        else {
            continue;
        };
        index.buckets.push(Bucket {
            count: row.count,
            repeats: row.repeats,
            lower: row.lower,
            upper: row.upper,
            ndv: row.ndv,
        });
    }
    for table_id in touched {
        if let Some(table) = cache.get_mut(table_id) {
            for index in table.indexes.values_mut() {
                index.fully_loaded = true;
            }
        }
    }
}

/// 合并业务结果与事务 commit：任一失败则返回错误。
fn finish_transaction(result: Result<(), Error>, commit: Result<(), Error>) -> Result<(), Error> {
    match (result, commit) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

/// 判断统计缓存是否已满：占用 ≥ 总内存 1/4，或达到配置配额。
pub fn is_full_cache(cache: &StatsCache, total_memory: u64, quota: i64) -> bool {
    let consumed = cache.memory_consumed();
    consumed as u64 >= total_memory / 4 || (quota != 0 && consumed >= quota)
}
