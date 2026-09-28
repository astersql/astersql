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

// `SHOW STATS_*` 系列语句执行器。
//
// 统计信息（Statistics）供优化器估算代价：直方图（Histogram）、TopN、修改计数等。
// 本模块通过 `ShowStatsRuntime` 读取表/分区物理统计，并按过滤条件组装 SHOW 结果行。

#![allow(non_snake_case)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};

#[derive(Clone, Debug, PartialEq)]
/// 结果单元格：支持空值、文本、有符号/无符号整数、浮点与时间戳。
pub enum Cell {
    Null,
    Text(String),
    Signed(i64),
    Unsigned(u64),
    Float(f64),
    Timestamp(Timestamp),
}

/// 一行 SHOW STATS 结果。
pub type Row = Vec<Cell>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
/// 毫秒级 Unix 时间戳，用于统计版本时间展示。
pub struct Timestamp {
    pub unix_millis: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// SHOW STATS 路径上的可报告错误（操作名 + 消息）。
pub struct ShowStatsError {
    pub operation: &'static str,
    pub message: String,
}

impl ShowStatsError {
    /// 构造带操作名的错误。
    pub fn new(operation: &'static str, message: impl Into<String>) -> Self {
        Self {
            operation,
            message: message.into(),
        }
    }
}

impl Display for ShowStatsError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.operation, self.message)
    }
}

impl std::error::Error for ShowStatsError {}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// SHOW 过滤条件：字段名、LIKE 模式、库名与表名集合。
pub struct ShowFilters {
    pub field: Option<String>,
    pub field_pattern_like: Option<String>,
    pub db_filters: BTreeSet<String>,
    pub table_filters: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 轻量表标识（物理 ID + 名称）。
pub struct SimpleTableInfo {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 分区定义摘要。
pub struct PartitionDefinition {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 列元信息摘要。
pub struct ColumnInfo {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 表元信息：是否分区、分区列表与列列表。
pub struct TableInfo {
    pub id: i64,
    pub name: String,
    pub partitioned: bool,
    pub partitions: Vec<PartitionDefinition>,
    pub columns: Vec<ColumnInfo>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 统计缓存项内存占用拆分（直方图/TopN/CMS 等）。
pub struct CacheItemMemoryUsage {
    pub total: i64,
    pub histogram: i64,
    pub topn: i64,
    pub cms: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 直方图桶：上下界、计数、重复值与桶内 NDV（distinct 近似）。
pub struct Bucket {
    pub count: i64,
    pub repeat: i64,
    pub lower: Vec<u8>,
    pub upper: Vec<u8>,
    pub ndv: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 列/索引直方图：版本、NDV、空值数、相关性与桶列表。
pub struct Histogram {
    pub last_update_version: u64,
    pub ndv: i64,
    pub null_count: i64,
    pub correlation: f64,
    pub field_type: u8,
    pub buckets: Vec<Bucket>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// TopN 高频值项：编码后的值与出现次数。
pub struct TopNItem {
    pub encoded: Vec<u8>,
    pub count: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// TopN 结构：高频值列表。
pub struct TopN {
    pub items: Vec<TopNItem>,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 列级统计：直方图、可选 TopN、平均大小、加载状态与内存。
pub struct ColumnStats {
    pub id: i64,
    pub name: String,
    pub initialized: bool,
    pub histogram: Histogram,
    pub topn: Option<TopN>,
    pub average_size: f64,
    pub load_status: String,
    pub memory: CacheItemMemoryUsage,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 索引级统计，结构与列统计类似并带组成列名。
pub struct IndexStats {
    pub id: i64,
    pub name: String,
    pub column_names: Vec<String>,
    pub initialized: bool,
    pub histogram: Histogram,
    pub topn: Option<TopN>,
    pub load_status: String,
    pub memory: CacheItemMemoryUsage,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 表/分区物理统计快照：伪统计标记、分析版本、修改计数、健康度等。
pub struct TableStats {
    pub pseudo: bool,
    pub analyzed: bool,
    pub version: u64,
    pub last_analyze_version: u64,
    pub modify_count: i64,
    pub realtime_count: i64,
    pub healthy: Option<i64>,
    pub columns: Vec<ColumnStats>,
    pub indexes: Vec<IndexStats>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
/// 表内列或索引项的定位键。
pub struct TableItemId {
    pub table_id: i64,
    pub id: i64,
    pub is_index: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 列统计使用/分析时间戳，供 `SHOW COLUMN_STATS_USAGE`。
pub struct ColumnStatsUsage {
    pub last_used_at: Option<Timestamp>,
    pub last_analyzed_at: Option<Timestamp>,
}

/// Runtime boundary for SHOW STATS. Every operation is mandatory so a
/// production implementation cannot silently manufacture successful results.
/// SHOW STATS 运行时边界：schema 枚举、物理统计读取、锁表集合与值格式化。
pub trait ShowStatsRuntime {
    fn all_schema_names(&self) -> Vec<String>;
    fn partitioned_table_infos(&self) -> Vec<TableInfo>;
    fn schema_simple_table_infos(
        &self,
        database: &str,
    ) -> (Vec<SimpleTableInfo>, Option<ShowStatsError>);
    fn schema_table_infos(&self, database: &str) -> Result<Vec<TableInfo>, ShowStatsError>;
    fn dynamic_partition_prune_enabled(&self) -> bool;
    fn non_pseudo_stats(&self, physical_id: i64) -> Option<TableStats>;
    fn physical_stats(
        &self,
        physical_id: i64,
        logical_table: &TableInfo,
    ) -> Result<TableStats, ShowStatsError>;
    fn locked_table_ids(&self, physical_ids: &[i64]) -> Result<BTreeSet<i64>, ShowStatsError>;
    fn value_to_string(
        &self,
        encoded: &[u8],
        number_of_columns: usize,
        column_types: &[u8],
    ) -> Result<String, ShowStatsError>;
    fn wildcard_match(&self, pattern: &str, value: &str) -> bool;
    fn log_nonfatal(&self, error: &ShowStatsError);
    fn histograms_in_flight(&self) -> Cell;
    fn analyze_status_rows(&self) -> Result<Vec<Row>, ShowStatsError>;
    fn column_stats_usage(&self)
    -> Result<BTreeMap<TableItemId, ColumnStatsUsage>, ShowStatsError>;
}

/// SHOW STATS 执行器：持有过滤条件并累积结果行。
pub struct ShowExec<R: ShowStatsRuntime> {
    pub runtime: R,
    pub filters: ShowFilters,
    pub rows: Vec<Row>,
}

impl<R: ShowStatsRuntime> ShowExec<R> {
    /// 构造空结果的执行器。
    pub fn new(runtime: R, filters: ShowFilters) -> Self {
        Self {
            runtime,
            filters,
            rows: Vec::new(),
        }
    }

    /// 追加一行。
    pub fn appendRow(&mut self, row: Row) {
        self.rows.push(row);
    }

    /// 对已物化结果做分页切片。
    pub fn rows_page(&self, offset: usize, limit: usize) -> &[Row] {
        let start = offset.min(self.rows.len());
        let end = start.saturating_add(limit).min(self.rows.len());
        &self.rows[start..end]
    }

    /// 库名是否通过 db_filters / LIKE 过滤。
    fn schema_allowed(&self, database: &str) -> bool {
        if let Some(field) = &self.filters.field {
            if !field.is_empty() && database != field {
                return false;
            }
        }
        if self.filters.field.as_ref().is_none_or(String::is_empty)
            && let Some(pattern) = &self.filters.field_pattern_like
        {
            if !self.runtime.wildcard_match(pattern, database) {
                return false;
            }
        }
        self.filters.db_filters.is_empty() || self.filters.db_filters.contains(database)
    }

    /// 表名是否通过 table_filters。
    fn table_allowed(&self, table: &str) -> bool {
        self.filters.table_filters.is_empty() || self.filters.table_filters.contains(table)
    }

    /// 返回排序后的可见库名列表。
    fn sorted_schema_names(&self) -> Vec<String> {
        let mut databases = self.runtime.all_schema_names();
        databases.sort();
        databases
    }

    /// Go `metadef.IsMemOrSysDB`: SHOW STATS never walks these schemas.
    fn is_mem_or_sys_schema(database: &str) -> bool {
        matches!(
            database.to_ascii_lowercase().as_str(),
            "mysql" | "information_schema" | "performance_schema" | "metrics_schema" | "sys"
        ) || database
            .to_ascii_lowercase()
            .starts_with("__tidb_br_temporary_")
    }

    /// 按动态/静态裁剪模式展开物理表（全局 + 分区）列表。
    fn visible_physical_tables(&self, table: &TableInfo) -> Vec<(i64, String)> {
        let mut physical = Vec::new();
        if !table.partitioned || self.runtime.dynamic_partition_prune_enabled() {
            physical.push((
                table.id,
                if table.partitioned {
                    "global".to_owned()
                } else {
                    String::new()
                },
            ));
        }
        let mut partitions = table.partitions.clone();
        partitions.sort_by_key(|partition| partition.id);
        physical.extend(
            partitions
                .into_iter()
                .map(|partition| (partition.id, partition.name)),
        );
        physical
    }

    /// 填充 `SHOW STATS_META`：修改计数、行数等元信息。
    pub fn fetchShowStatsMeta(&mut self) -> Result<(), ShowStatsError> {
        let partitioned: BTreeMap<i64, TableInfo> = self
            .runtime
            .partitioned_table_infos()
            .into_iter()
            .map(|table| (table.id, table))
            .collect();
        for database in self.sorted_schema_names() {
            if Self::is_mem_or_sys_schema(&database) {
                continue;
            }
            if !self.schema_allowed(&database) {
                continue;
            }
            let (mut tables, error) = self.runtime.schema_simple_table_infos(&database);
            if let Some(error) = error {
                self.runtime.log_nonfatal(&error);
            }
            tables.sort_by_key(|table| table.id);
            for table in tables {
                if !self.table_allowed(&table.name) {
                    continue;
                }
                if let Some(partitioned_table) = partitioned.get(&table.id) {
                    if self.runtime.dynamic_partition_prune_enabled() {
                        if let Some(stats) = self.runtime.non_pseudo_stats(partitioned_table.id) {
                            self.appendTableForStatsMeta(
                                &database,
                                &partitioned_table.name,
                                "global",
                                &stats,
                            );
                        }
                    }
                    let mut definitions = partitioned_table.partitions.clone();
                    definitions.sort_by_key(|definition| definition.id);
                    for definition in definitions {
                        if let Some(stats) = self.runtime.non_pseudo_stats(definition.id) {
                            self.appendTableForStatsMeta(
                                &database,
                                &partitioned_table.name,
                                &definition.name,
                                &stats,
                            );
                        }
                    }
                } else if let Some(stats) = self.runtime.non_pseudo_stats(table.id) {
                    self.appendTableForStatsMeta(&database, &table.name, "", &stats);
                }
            }
        }
        Ok(())
    }

    /// 为单张物理表追加 STATS_META 行。
    pub fn appendTableForStatsMeta(
        &mut self,
        database: &str,
        table: &str,
        partition: &str,
        stats: &TableStats,
    ) {
        // 伪统计（未真实分析）不展示桶/TopN 细节。
        if stats.pseudo {
            return;
        }
        self.appendRow(vec![
            Cell::Text(database.to_owned()),
            Cell::Text(table.to_owned()),
            Cell::Text(partition.to_owned()),
            Cell::Timestamp(self.versionToTime(stats.version)),
            Cell::Signed(stats.modify_count),
            Cell::Signed(stats.realtime_count),
            if stats.analyzed {
                Cell::Timestamp(self.versionToTime(stats.last_analyze_version))
            } else {
                Cell::Null
            },
        ]);
    }

    /// 追加被统计锁锁定的表行。
    pub fn appendTableForStatsLocked(&mut self, database: &str, table: &str, partition: &str) {
        self.appendRow(vec![
            Cell::Text(database.to_owned()),
            Cell::Text(table.to_owned()),
            Cell::Text(partition.to_owned()),
            Cell::Text("locked".to_owned()),
        ]);
    }

    /// 填充 `SHOW STATS_LOCKED`。
    pub fn fetchShowStatsLocked(&mut self) -> Result<(), ShowStatsError> {
        let mut table_info = BTreeMap::new();
        for database in self.sorted_schema_names() {
            if Self::is_mem_or_sys_schema(&database) {
                continue;
            }
            let mut tables = self.runtime.schema_table_infos(&database)?;
            tables.sort_by_key(|table| table.id);
            for table in tables {
                for (physical_id, partition) in self.visible_physical_tables(&table) {
                    table_info.insert(
                        physical_id,
                        (database.clone(), table.name.clone(), partition),
                    );
                }
            }
        }
        let physical_ids: Vec<_> = table_info.keys().copied().collect();
        let locked = self.runtime.locked_table_ids(&physical_ids)?;
        for physical_id in physical_ids {
            if locked.contains(&physical_id) {
                let (database, table, partition) = &table_info[&physical_id];
                self.appendTableForStatsLocked(database, table, partition);
            }
        }
        Ok(())
    }

    /// 填充 `SHOW STATS_HISTOGRAMS`。
    pub fn fetchShowStatsHistogram(&mut self) -> Result<(), ShowStatsError> {
        for database in self.sorted_schema_names() {
            if Self::is_mem_or_sys_schema(&database) {
                continue;
            }
            let mut tables = self.runtime.schema_table_infos(&database)?;
            tables.sort_by_key(|table| table.id);
            for table in tables {
                for (physical_id, partition) in self.visible_physical_tables(&table) {
                    let stats = self.runtime.physical_stats(physical_id, &table)?;
                    self.appendTableForStatsHistograms(&database, &table.name, &partition, &stats);
                }
            }
        }
        Ok(())
    }

    /// 将表的列/索引直方图摘要写入结果。
    pub fn appendTableForStatsHistograms(
        &mut self,
        database: &str,
        table: &str,
        partition: &str,
        stats: &TableStats,
    ) {
        if stats.pseudo {
            return;
        }
        let mut columns = stats.columns.clone();
        columns.sort_by_key(|column| column.id);
        for column in columns {
            if column.initialized {
                self.histogramToRow(
                    database,
                    table,
                    partition,
                    &column.name,
                    false,
                    &column.histogram,
                    column.average_size,
                    &column.load_status,
                    &column.memory,
                );
            }
        }
        let mut indexes = stats.indexes.clone();
        indexes.sort_by_key(|index| index.id);
        for index in indexes {
            if index.initialized {
                self.histogramToRow(
                    database,
                    table,
                    partition,
                    &index.name,
                    true,
                    &index.histogram,
                    0.0,
                    &index.load_status,
                    &index.memory,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    /// 把单个直方图对象格式化为一行 SHOW 输出。
    pub fn histogramToRow(
        &mut self,
        database: &str,
        table: &str,
        partition: &str,
        column: &str,
        is_index: bool,
        histogram: &Histogram,
        average_column_size: f64,
        load_status: &str,
        memory: &CacheItemMemoryUsage,
    ) {
        self.appendRow(vec![
            Cell::Text(database.to_owned()),
            Cell::Text(table.to_owned()),
            Cell::Text(partition.to_owned()),
            Cell::Text(column.to_owned()),
            Cell::Signed(i64::from(is_index)),
            Cell::Timestamp(self.versionToTime(histogram.last_update_version)),
            Cell::Signed(histogram.ndv),
            Cell::Signed(histogram.null_count),
            Cell::Float(average_column_size),
            Cell::Float(histogram.correlation),
            Cell::Text(load_status.to_owned()),
            Cell::Signed(memory.total),
            Cell::Signed(memory.histogram),
            Cell::Signed(memory.topn),
            Cell::Signed(memory.cms),
        ]);
    }

    /// 将统计版本号映射为可读时间戳。
    pub fn versionToTime(&self, version: u64) -> Timestamp {
        // TiDB 版本号高位编码物理时间（毫秒），右移 18 位还原。
        let physical_millis = version >> 18;
        Timestamp {
            unix_millis: i64::try_from(physical_millis).unwrap_or(i64::MAX),
        }
    }

    /// 填充 `SHOW STATS_BUCKETS`（直方图各桶细节）。
    pub fn fetchShowStatsBuckets(&mut self) -> Result<(), ShowStatsError> {
        for database in self.sorted_schema_names() {
            if Self::is_mem_or_sys_schema(&database) {
                continue;
            }
            let mut tables = self.runtime.schema_table_infos(&database)?;
            tables.sort_by_key(|table| table.id);
            for table in tables {
                for (physical_id, partition) in self.visible_physical_tables(&table) {
                    let stats = self.runtime.physical_stats(physical_id, &table)?;
                    self.appendTableForStatsBuckets(&database, &table.name, &partition, &stats)?;
                }
            }
        }
        Ok(())
    }

    /// 将表的列/索引直方图桶展开为多行。
    pub fn appendTableForStatsBuckets(
        &mut self,
        database: &str,
        table: &str,
        partition: &str,
        stats: &TableStats,
    ) -> Result<(), ShowStatsError> {
        if stats.pseudo {
            return Ok(());
        }
        let mut column_types = BTreeMap::new();
        let mut columns = stats.columns.clone();
        columns.sort_by_key(|column| column.id);
        for column in columns {
            self.bucketsToRows(
                database,
                table,
                partition,
                &column.name,
                0,
                &column.histogram,
                &[],
            )?;
            column_types.insert(column.name, column.histogram.field_type);
        }
        let mut indexes = stats.indexes.clone();
        indexes.sort_by_key(|index| index.id);
        for index in indexes {
            let types: Vec<_> = index
                .column_names
                .iter()
                .map(|name| column_types.get(name).copied().unwrap_or(0))
                .collect();
            self.bucketsToRows(
                database,
                table,
                partition,
                &index.name,
                index.column_names.len(),
                &index.histogram,
                &types,
            )?;
        }
        Ok(())
    }

    /// 填充 `SHOW STATS_TOPN`。
    pub fn fetchShowStatsTopN(&mut self) -> Result<(), ShowStatsError> {
        for database in self.sorted_schema_names() {
            if Self::is_mem_or_sys_schema(&database) {
                continue;
            }
            let mut tables = self.runtime.schema_table_infos(&database)?;
            tables.sort_by_key(|table| table.id);
            for table in tables {
                for (physical_id, partition) in self.visible_physical_tables(&table) {
                    let stats = self.runtime.physical_stats(physical_id, &table)?;
                    self.appendTableForStatsTopN(&database, &table.name, &partition, &stats)?;
                }
            }
        }
        Ok(())
    }

    /// 输出列与索引的 TopN 高频值行。
    pub fn appendTableForStatsTopN(
        &mut self,
        database: &str,
        table: &str,
        partition: &str,
        stats: &TableStats,
    ) -> Result<(), ShowStatsError> {
        if stats.pseudo {
            return Ok(());
        }
        let mut column_types = BTreeMap::new();
        let mut columns = stats.columns.clone();
        columns.sort_by_key(|column| column.id);
        for column in columns {
            self.topNToRows(
                database,
                table,
                partition,
                &column.name,
                1,
                false,
                column.topn.as_ref(),
                &[column.histogram.field_type],
            )?;
            column_types.insert(column.name, column.histogram.field_type);
        }
        let mut indexes = stats.indexes.clone();
        indexes.sort_by_key(|index| index.id);
        for index in indexes {
            let types: Vec<_> = index
                .column_names
                .iter()
                .map(|name| column_types.get(name).copied().unwrap_or(0))
                .collect();
            self.topNToRows(
                database,
                table,
                partition,
                &index.name,
                index.column_names.len(),
                true,
                index.topn.as_ref(),
                &types,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    /// 将 TopN 各项解码为可读字符串并追加结果行。
    pub fn topNToRows(
        &mut self,
        database: &str,
        table: &str,
        partition: &str,
        column: &str,
        number_of_columns: usize,
        is_index: bool,
        topn: Option<&TopN>,
        column_types: &[u8],
    ) -> Result<(), ShowStatsError> {
        let Some(topn) = topn else {
            return Ok(());
        };
        for item in &topn.items {
            let value =
                self.runtime
                    .value_to_string(&item.encoded, number_of_columns, column_types)?;
            self.appendRow(vec![
                Cell::Text(database.to_owned()),
                Cell::Text(table.to_owned()),
                Cell::Text(partition.to_owned()),
                Cell::Text(column.to_owned()),
                Cell::Signed(i64::from(is_index)),
                Cell::Text(value),
                Cell::Unsigned(item.count),
            ]);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    /// 遍历直方图桶，解码上下界后追加 `STATS_BUCKETS` 行。
    pub fn bucketsToRows(
        &mut self,
        database: &str,
        table: &str,
        partition: &str,
        column: &str,
        number_of_columns: usize,
        histogram: &Histogram,
        index_column_types: &[u8],
    ) -> Result<(), ShowStatsError> {
        let is_index = number_of_columns > 0;
        for (number, bucket) in histogram.buckets.iter().enumerate() {
            let lower = self.runtime.value_to_string(
                &bucket.lower,
                number_of_columns,
                index_column_types,
            )?;
            let upper = self.runtime.value_to_string(
                &bucket.upper,
                number_of_columns,
                index_column_types,
            )?;
            self.appendRow(vec![
                Cell::Text(database.to_owned()),
                Cell::Text(table.to_owned()),
                Cell::Text(partition.to_owned()),
                Cell::Text(column.to_owned()),
                Cell::Signed(i64::from(is_index)),
                Cell::Unsigned(number as u64),
                Cell::Signed(bucket.count),
                Cell::Signed(bucket.repeat),
                Cell::Text(lower),
                Cell::Text(upper),
                Cell::Signed(bucket.ndv),
            ]);
        }
        Ok(())
    }

    /// 填充 `SHOW STATS_HEALTHY`：统计新鲜度/健康度百分比。
    pub fn fetchShowStatsHealthy(&mut self) {
        let partitioned: BTreeMap<i64, TableInfo> = self
            .runtime
            .partitioned_table_infos()
            .into_iter()
            .map(|table| (table.id, table))
            .collect();
        for database in self.sorted_schema_names() {
            if Self::is_mem_or_sys_schema(&database) {
                continue;
            }
            if !self.schema_allowed(&database) {
                continue;
            }
            let (mut tables, error) = self.runtime.schema_simple_table_infos(&database);
            if let Some(error) = error {
                self.runtime.log_nonfatal(&error);
            }
            tables.sort_by_key(|table| table.id);
            for table in tables {
                if let Some(partitioned_table) = partitioned.get(&table.id) {
                    if self.runtime.dynamic_partition_prune_enabled() {
                        if let Some(stats) = self.runtime.non_pseudo_stats(partitioned_table.id) {
                            self.appendTableForStatsHealthy(
                                &database,
                                &partitioned_table.name,
                                "global",
                                &stats,
                            );
                        }
                    }
                    let mut definitions = partitioned_table.partitions.clone();
                    definitions.sort_by_key(|definition| definition.id);
                    for definition in definitions {
                        if let Some(stats) = self.runtime.non_pseudo_stats(definition.id) {
                            self.appendTableForStatsHealthy(
                                &database,
                                &partitioned_table.name,
                                &definition.name,
                                &stats,
                            );
                        }
                    }
                } else if let Some(stats) = self.runtime.non_pseudo_stats(table.id) {
                    self.appendTableForStatsHealthy(&database, &table.name, "", &stats);
                }
            }
        }
    }

    /// 若存在健康度则追加一行。
    pub fn appendTableForStatsHealthy(
        &mut self,
        database: &str,
        table: &str,
        partition: &str,
        stats: &TableStats,
    ) {
        let Some(healthy) = stats.healthy else {
            return;
        };
        self.appendRow(vec![
            Cell::Text(database.to_owned()),
            Cell::Text(table.to_owned()),
            Cell::Text(partition.to_owned()),
            Cell::Signed(healthy),
        ]);
    }

    /// 填充飞行中（正在加载）的直方图计数。
    pub fn fetchShowHistogramsInFlight(&mut self) {
        self.appendRow(vec![self.runtime.histograms_in_flight()]);
    }

    /// 填充 `SHOW ANALYZE STATUS` 任务列表。
    pub fn fetchShowAnalyzeStatus(&mut self) -> Result<(), ShowStatsError> {
        for row in self.runtime.analyze_status_rows()? {
            self.appendRow(row);
        }
        Ok(())
    }

    /// 填充 `SHOW COLUMN_STATS_USAGE`。
    pub fn fetchShowColumnStatsUsage(&mut self) -> Result<(), ShowStatsError> {
        let usage = self.runtime.column_stats_usage()?;
        for database in self.sorted_schema_names() {
            if Self::is_mem_or_sys_schema(&database) {
                continue;
            }
            let mut tables = self.runtime.schema_table_infos(&database)?;
            tables.sort_by_key(|table| table.id);
            for table in tables {
                self.appendTableForColumnStatsUsage(
                    &usage,
                    &database,
                    &table,
                    table.id,
                    if table.partitioned { "global" } else { "" },
                );
                let mut definitions = table.partitions.clone();
                definitions.sort_by_key(|definition| definition.id);
                for definition in definitions {
                    self.appendTableForColumnStatsUsage(
                        &usage,
                        &database,
                        &table,
                        definition.id,
                        &definition.name,
                    );
                }
            }
        }
        Ok(())
    }

    /// 按物理 ID 匹配列使用记录并输出行。
    fn appendTableForColumnStatsUsage(
        &mut self,
        usage: &BTreeMap<TableItemId, ColumnStatsUsage>,
        database: &str,
        table: &TableInfo,
        physical_id: i64,
        partition: &str,
    ) {
        let mut columns = table.columns.clone();
        columns.sort_by_key(|column| column.id);
        for column in columns {
            let key = TableItemId {
                table_id: physical_id,
                id: column.id,
                is_index: false,
            };
            let Some(column_usage) = usage.get(&key) else {
                continue;
            };
            self.appendRow(vec![
                Cell::Text(database.to_owned()),
                Cell::Text(table.name.clone()),
                Cell::Text(partition.to_owned()),
                Cell::Text(column.name),
                column_usage
                    .last_used_at
                    .map(Cell::Timestamp)
                    .unwrap_or(Cell::Null),
                column_usage
                    .last_analyzed_at
                    .map(Cell::Timestamp)
                    .unwrap_or(Cell::Null),
            ]);
        }
    }
}
