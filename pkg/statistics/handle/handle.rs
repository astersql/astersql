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

// 统计信息句柄（Handle）：缓存、持久化增量、历史快照与 ANALYZE 发布。
//
// 对应 TiDB `statistics/handle` 的核心运行时对象：维护物理表级 `TableStats`
// 缓存，协调 delta 刷盘、列使用统计、ANALYZE 作业与历史统计 JSON 编解码。
// 优化器通过此处拿到的直方图/TopN/NDV 等做代价估算与索引选择。

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

pub use crate::runtime_stats::{RuntimeAnalyzeJob, RuntimeColumnUsage, RuntimeHistoricalSnapshot};
pub use astersql_statistics_handle_history::HistoricalStatsMeta;

/// etcd 上统计 owner 竞选使用的 key。
pub const STATS_OWNER_KEY: &str = "/tidb/stats/owner";
/// 统计子系统日志/提示前缀。
pub const STATS_PROMPT: &str = "stats";
/// 伪统计（pseudo stats）分区缓存上限，避免未 ANALYZE 的分区无限膨胀缓存。
pub const PSEUDO_PARTITION_CACHE_LIMIT: usize = 64;
/// 非强制刷盘时，修改行占比达到该阈值才触发 stats delta dump。
pub const DEFAULT_DUMP_STATS_DELTA_RATIO: f64 = 1.0 / 10_000.0;

// 以 f64 位型原子存储进程级 dump 阈值，便于无锁读写。
static DUMP_STATS_DELTA_RATIO_BITS: AtomicU64 =
    AtomicU64::new(DEFAULT_DUMP_STATS_DELTA_RATIO.to_bits());

/// Returns the process-wide threshold used for non-forced delta dumps.
/// 返回进程级非强制 delta 刷盘阈值。
pub fn dump_stats_delta_ratio() -> f64 {
    f64::from_bits(DUMP_STATS_DELTA_RATIO_BITS.load(Ordering::Acquire))
}

/// Sets the process-wide Go-compatible `usage.DumpStatsDeltaRatio`.
/// 设置进程级 Go 兼容的 `usage.DumpStatsDeltaRatio`。
pub fn set_dump_stats_delta_ratio(ratio: f64) {
    assert!(
        ratio.is_finite() && ratio >= 0.0,
        "statistics delta ratio must be finite and non-negative"
    );
    DUMP_STATS_DELTA_RATIO_BITS.store(ratio.to_bits(), Ordering::Release);
}

/// Go-compatible helper for tests which previously assigned
/// `usage.DumpStatsDeltaRatio`.
/// Go 风格导出名：测试中直接赋值 `usage.DumpStatsDeltaRatio` 的兼容入口。
pub fn SetDumpStatsDeltaRatio(ratio: f64) {
    set_dump_stats_delta_ratio(ratio);
}

/// 将 dump 阈值恢复为默认值。
pub fn ResetDumpStatsDeltaRatio() {
    set_dump_stats_delta_ratio(DEFAULT_DUMP_STATS_DELTA_RATIO);
}

#[derive(Default)]
/// 测试用 failpoint：记录历史统计元数据时可注入 panic。
struct HistoricalMetaFailpoints {
    next_id: AtomicU64,
    active: Mutex<HashSet<u64>>,
}

/// 启用历史元数据 panic failpoint 的 RAII 守卫；析构时注销。
pub struct HistoricalMetaPanicGuard {
    failpoints: Arc<HistoricalMetaFailpoints>,
    id: u64,
}

impl Drop for HistoricalMetaPanicGuard {
    fn drop(&mut self) {
        self.failpoints
            .active
            .lock()
            .expect("historical statistics failpoint lock poisoned")
            .remove(&self.id);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 统计句柄统一错误类型。
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Stable table identity shared by Domain, session execution and the canonical handle.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct StatsTableKey {
    pub database: String,
    pub table: String,
    pub table_id: i64,
}

impl StatsTableKey {
    pub fn new(database: &str, table: &str, table_id: i64) -> Self {
        Self {
            database: database.to_lowercase(),
            table: table.to_lowercase(),
            table_id,
        }
    }
}

/// A `stats_meta`-style row exposed by the canonical handle runtime.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StatsMetaRow {
    pub database: String,
    pub table: String,
    pub table_id: i64,
    pub version: u64,
    pub modify_count: i64,
    pub row_count: i64,
}

/// Production ANALYZE/meta boundary implemented by the canonical handle.
pub trait AnalyzeStatsStorage {
    fn register_table_stats(&mut self, table_id: i64) -> Result<(), Error>;
    fn record_table_mutation(
        &mut self,
        table_id: i64,
        row_delta: i64,
        modified_rows: i64,
    ) -> Result<(), Error>;
    fn analyze_table_stats(&mut self, table_id: i64) -> Result<u64, Error>;
}

/// 挂载统计收集器到执行器（当前为透传占位，保持与 Go API 对称）。
pub fn attach_stats_collector<T>(executor: T) -> T {
    executor
}

/// 从执行器卸载统计收集器（当前为透传占位）。
pub fn detach_stats_collector<T>(executor: T) -> T {
    executor
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// 临时表类型：无 / 会话本地 / 全局临时表。
pub enum TemporaryTableType {
    #[default]
    None,
    Local,
    Global,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 表元信息摘要，供伪统计生成与系统库判定使用。
pub struct TableInfo {
    pub id: i64,
    pub database_id: i64,
    pub partitioned: bool,
    pub temporary: TemporaryTableType,
    pub index_ids: Vec<i64>,
    pub column_ids: Vec<i64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 单索引统计：NDV、TopN、直方图桶、CMSketch 加载状态等。
pub struct IndexStats {
    pub analyzed: bool,
    pub stats_version: i64,
    pub version: u64,
    pub ndv: i64,
    pub null_count: i64,
    /// 索引键编码的总字节数，对应 `mysql.stats_histograms.tot_col_size`。
    pub total_column_size: i64,
    pub correlation: f64,
    pub cms_loaded: bool,
    pub top_n: Vec<(Vec<u8>, u64)>,
    pub buckets: Vec<Bucket>,
    pub fully_loaded: bool,
    pub fm_sketch: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 单列统计：NDV、空值数、TopN、直方图与 FMSketch 等。
pub struct ColumnStats {
    pub analyzed_or_synthesized: bool,
    pub stats_version: i64,
    pub ndv: i64,
    pub null_count: i64,
    pub total_column_size: i64,
    pub version: u64,
    pub loaded_or_evicted: bool,
    pub field_type: u8,
    pub correlation: f64,
    pub average_size: f64,
    pub top_n: Vec<(Vec<u8>, u64)>,
    pub buckets: Vec<Bucket>,
    pub fm_sketch: Vec<u8>,
}

impl ColumnStats {
    /// 列统计是否已 ANALYZE 或经合成初始化。
    pub fn IsStatsInitialized(&self) -> bool {
        self.analyzed_or_synthesized
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 直方图桶：累计行数、上界重复次数、上下界编码与桶内 NDV。
pub struct Bucket {
    pub count: i64,
    pub repeats: i64,
    pub lower: Vec<u8>,
    pub upper: Vec<u8>,
    pub ndv: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 物理表（含分区）级统计快照：行数、修改计数、列/索引统计集合。
pub struct TableStats {
    pub physical_id: i64,
    pub pseudo: bool,
    pub initialized: bool,
    pub version: u64,
    pub modify_count: i64,
    pub realtime_count: i64,
    /// Row count captured at the last ANALYZE. Used as the auto-analyze ratio
    /// denominator (`NeedAnalyzeTable` / `GetAnalyzeRowCount` in Go).
    pub analyze_count: i64,
    pub last_analyze_version: u64,
    pub last_stats_hist_version: u64,
    pub stats_version: i64,
    pub indexes: HashMap<i64, IndexStats>,
    pub columns: HashMap<i64, ColumnStats>,
    pub pre_scalar_ready: bool,
}

impl TableStats {
    /// 构造伪统计：未真实 ANALYZE 时供优化器兜底使用。
    pub fn pseudo(table_id: i64) -> Self {
        Self {
            physical_id: table_id,
            pseudo: true,
            initialized: true,
            ..Self::default()
        }
    }

    /// 粗估本表统计占用的内存字节数，供缓存容量控制。
    pub fn estimated_memory(&self) -> i64 {
        let top_n = self
            .indexes
            .values()
            .map(|index| index.top_n.len() as i64 * 32)
            .sum::<i64>();
        let buckets = self
            .indexes
            .values()
            .map(|index| index.buckets.len() as i64 * 64)
            .sum::<i64>();
        let column_top_n = self
            .columns
            .values()
            .map(|column| column.top_n.len() as i64 * 32)
            .sum::<i64>();
        let column_buckets = self
            .columns
            .values()
            .map(|column| column.buckets.len() as i64 * 64)
            .sum::<i64>();
        256 + self.columns.len() as i64 * 128
            + self.indexes.len() as i64 * 192
            + top_n
            + buckets
            + column_top_n
            + column_buckets
    }
}

#[derive(Clone, Debug)]
/// 物理表 ID 到 `TableStats` 的内存缓存。
pub struct StatsCache {
    tables: HashMap<i64, TableStats>,
    closed: bool,
    capacity_bytes: i64,
}

impl Default for StatsCache {
    fn default() -> Self {
        Self {
            tables: HashMap::new(),
            closed: false,
            capacity_bytes: i64::MAX,
        }
    }
}

impl StatsCache {
    /// 按物理表 ID 查询统计。
    pub fn get(&self, physical_id: i64) -> Option<&TableStats> {
        self.tables.get(&physical_id)
    }

    /// 按物理表 ID 可变查询统计。
    pub fn get_mut(&mut self, physical_id: i64) -> Option<&mut TableStats> {
        self.tables.get_mut(&physical_id)
    }

    /// 写入或覆盖一张物理表的统计。
    pub fn put(&mut self, table: TableStats) {
        self.tables.insert(table.physical_id, table);
    }

    /// 缓存中的表数量。
    pub fn len(&self) -> usize {
        self.tables.len()
    }

    /// 缓存是否为空。
    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }

    /// 清空全部缓存条目。
    /// 清空缓存、作业、历史与 DDL 队列，并重置后端会话统计列表。
    pub fn clear(&mut self) {
        self.tables.clear();
    }

    /// 遍历缓存中的统计引用。
    pub fn values(&self) -> impl Iterator<Item = &TableStats> {
        self.tables.values()
    }

    /// 可变遍历缓存中的统计。
    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut TableStats> {
        self.tables.values_mut()
    }

    /// 取出并清空全部缓存条目。
    pub fn drain(&mut self) -> impl Iterator<Item = TableStats> + '_ {
        self.tables.drain().map(|(_, table)| table)
    }

    /// 当前缓存估计内存占用总和。
    pub fn memory_consumed(&self) -> i64 {
        self.tables.values().map(TableStats::estimated_memory).sum()
    }

    /// 设置缓存容量上限（字节）。
    pub fn set_capacity(&mut self, capacity_bytes: i64) {
        self.capacity_bytes = capacity_bytes.max(0);
    }

    /// 返回缓存容量上限。
    pub fn capacity(&self) -> i64 {
        self.capacity_bytes
    }

    /// 等待异步统计更新完成（占位，与 Go API 对齐）。
    pub fn wait_for_async_updates(&self) {}

    /// 标记缓存已关闭。
    /// 关闭连接池、缓存、用量与自动分析子系统。
    pub fn close(&mut self) {
        self.closed = true;
    }

    /// 缓存是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

/// Handle 依赖的后端能力：系统库判定、delta 刷盘、worker 启停等。
pub trait HandleBackend {
    fn memory_schema_id(&self, database_id: i64) -> bool;
    fn system_schema(&mut self, database_id: i64) -> Result<bool, Error>;
    fn reset_session_stats_list(&mut self);
    fn dump_stats_delta(&mut self, dump_all: bool) -> Result<(), Error>;
    fn warn_flush_error(&mut self, _error: &Error) {}
    fn start_usage_worker(&mut self);
    fn close_pool(&mut self);
    fn close_usage(&mut self);
    fn close_auto_analyze(&mut self);
    fn register_ddl_handler(&mut self) {}
}

/// 统计子系统主句柄：缓存、后端、DDL 事件队列、历史快照与 ANALYZE 作业。
pub struct Handle<B> {
    pub(crate) cache: StatsCache,
    pub(crate) backend: B,
    ddl_events: VecDeque<String>,
    system_database_ids: HashSet<i64>,
    pub init_stats_done: bool,
    next_stats_version: u64,
    column_usage: HashMap<(i64, i64), RuntimeColumnUsage>,
    analyze_jobs: Vec<RuntimeAnalyzeJob>,
    historical_snapshots: HashMap<i64, Vec<HistoricalTableStats>>,
    historical_enabled: bool,
    historical_meta_failpoints: Arc<HistoricalMetaFailpoints>,
    lease: Duration,
}

#[derive(Clone, Debug, PartialEq)]
/// 内存中的历史统计快照条目（可延迟编码为 JSON blocks）。
struct HistoricalTableStats {
    stats: TableStats,
    source: String,
    json_blocks: Option<Vec<Vec<u8>>>,
    created_at: SystemTime,
}

/// 将表统计编码为旧版历史 JSON，并按最大列大小切块。
fn encode_legacy_historical_json_blocks(
    stats: &TableStats,
    source: &str,
) -> Result<Vec<Vec<u8>>, Error> {
    // 源字符串须可嵌入 JSON，禁止引号、反斜杠与控制字符。
    if source
        .bytes()
        .any(|byte| byte == b'"' || byte == b'\\' || byte < b' ')
    {
        return Err(Error(
            "historical statistics source is not JSON-safe".to_owned(),
        ));
    }
    fn bytes_json(bytes: &[u8]) -> String {
        format!(
            "[{}]",
            bytes
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        )
    }
    fn topn_json(topn: &[(Vec<u8>, u64)]) -> String {
        format!(
            "[{}]",
            topn.iter()
                .map(|(value, count)| format!(
                    "{{\"value_len\":{},\"value\":{},\"count\":{count}}}",
                    value.len(),
                    bytes_json(value)
                ))
                .collect::<Vec<_>>()
                .join(",")
        )
    }
    fn buckets_json(buckets: &[Bucket]) -> String {
        format!(
            "[{}]",
            buckets.iter()
                .map(|bucket| format!(
                    "{{\"count\":{},\"repeats\":{},\"lower_len\":{},\"lower\":{},\"upper_len\":{},\"upper\":{},\"ndv\":{}}}",
                    bucket.count,
                    bucket.repeats,
                    bucket.lower.len(),
                    bytes_json(&bucket.lower),
                    bucket.upper.len(),
                    bytes_json(&bucket.upper),
                    bucket.ndv
                ))
                .collect::<Vec<_>>()
                .join(",")
        )
    }
    let mut columns = stats.columns.iter().collect::<Vec<_>>();
    columns.sort_by_key(|(id, _)| **id);
    let columns = columns
        .into_iter()
        .map(|(id, column)| format!(
            "{{\"id\":{id},\"analyzed_or_synthesized\":{},\"stats_version\":{},\"ndv\":{},\"null_count\":{},\"total_column_size\":{},\"version\":{},\"loaded_or_evicted\":{},\"field_type\":{},\"correlation_bits\":{},\"average_size_bits\":{},\"top_n_len\":{},\"top_n\":{},\"buckets_len\":{},\"buckets\":{}}}",
            u8::from(column.analyzed_or_synthesized), column.stats_version, column.ndv,
            column.null_count, column.total_column_size, column.version,
            u8::from(column.loaded_or_evicted), column.field_type, column.correlation.to_bits(),
            column.average_size.to_bits(), column.top_n.len(), topn_json(&column.top_n),
            column.buckets.len(), buckets_json(&column.buckets)
        ))
        .collect::<Vec<_>>()
        .join(",");
    let mut indexes = stats.indexes.iter().collect::<Vec<_>>();
    indexes.sort_by_key(|(id, _)| **id);
    let indexes = indexes
        .into_iter()
        .map(|(id, index)| format!(
            "{{\"id\":{id},\"analyzed\":{},\"stats_version\":{},\"version\":{},\"ndv\":{},\"null_count\":{},\"total_column_size\":{},\"correlation_bits\":{},\"cms_loaded\":{},\"fully_loaded\":{},\"top_n_len\":{},\"top_n\":{},\"buckets_len\":{},\"buckets\":{}}}",
            u8::from(index.analyzed), index.stats_version, index.version, index.ndv,
            index.null_count, index.total_column_size, index.correlation.to_bits(),
            u8::from(index.cms_loaded), u8::from(index.fully_loaded), index.top_n.len(),
            topn_json(&index.top_n), index.buckets.len(), buckets_json(&index.buckets)
        ))
        .collect::<Vec<_>>()
        .join(",");
    let encoded = format!(
        "{{\"source\":\"{source}\",\"table\":{{\"physical_id\":{},\"pseudo\":{},\"initialized\":{},\"version\":{},\"modify_count\":{},\"realtime_count\":{},\"last_analyze_version\":{},\"last_stats_hist_version\":{},\"stats_version\":{},\"pre_scalar_ready\":{},\"columns_len\":{},\"columns\":[{columns}],\"indexes_len\":{},\"indexes\":[{indexes}]}}}}",
        stats.physical_id, u8::from(stats.pseudo), u8::from(stats.initialized), stats.version,
        stats.modify_count, stats.realtime_count, stats.last_analyze_version,
        stats.last_stats_hist_version, stats.stats_version, u8::from(stats.pre_scalar_ready),
        stats.columns.len(), stats.indexes.len()
    )
    .into_bytes();
    Ok(encoded
        .chunks(astersql_statistics_handle_history::MAX_COLUMN_SIZE)
        .map(|chunk| chunk.to_vec())
        .collect())
}

/// 解码旧版历史 JSON blocks，并与规范重编码结果比对以校验结构。
fn decode_legacy_historical_json_blocks(blocks: &[Vec<u8>]) -> Result<(String, TableStats), Error> {
    if blocks.is_empty() || blocks.iter().any(Vec::is_empty) {
        return Err(Error(
            "historical statistics JSON blocks must not be empty".to_owned(),
        ));
    }
    let encoded = String::from_utf8(blocks.concat())
        .map_err(|error| Error(format!("historical statistics JSON is not UTF-8: {error}")))?;
    if !encoded.ends_with("}}") {
        return Err(Error(
            "historical statistics JSON has an invalid envelope".to_owned(),
        ));
    }
    let source_start = encoded
        .strip_prefix("{\"source\":\"")
        .ok_or_else(|| Error("historical statistics JSON misses source".to_owned()))?;
    let source_end = source_start
        .find('"')
        .ok_or_else(|| Error("historical statistics JSON has unterminated source".to_owned()))?;
    let source = source_start[..source_end].to_owned();
    let table = &source_start[source_end + 1..];
    let numbers = table
        .split(|character: char| !character.is_ascii_digit() && character != '-')
        .filter(|token| !token.is_empty() && *token != "-")
        .map(|token| {
            token.parse::<i128>().map_err(|error| {
                Error(format!(
                    "invalid historical statistics JSON number {token}: {error}"
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut position = 0;
    let mut take = || -> Result<i128, Error> {
        let value = numbers.get(position).copied().ok_or_else(|| {
            Error("historical statistics JSON ended before the table was complete".to_owned())
        })?;
        position += 1;
        Ok(value)
    };
    fn as_i64(value: i128) -> Result<i64, Error> {
        i64::try_from(value).map_err(|_| Error(format!("historical i64 overflow: {value}")))
    }
    fn as_u64(value: i128) -> Result<u64, Error> {
        u64::try_from(value).map_err(|_| Error(format!("historical u64 overflow: {value}")))
    }
    fn as_usize(value: i128) -> Result<usize, Error> {
        usize::try_from(value).map_err(|_| Error(format!("historical usize overflow: {value}")))
    }
    fn decode_bytes(take: &mut impl FnMut() -> Result<i128, Error>) -> Result<Vec<u8>, Error> {
        let length = as_usize(take()?)?;
        (0..length)
            .map(|_| {
                u8::try_from(take()?).map_err(|_| Error("historical byte overflow".to_owned()))
            })
            .collect()
    }
    fn decode_topn(
        take: &mut impl FnMut() -> Result<i128, Error>,
    ) -> Result<Vec<(Vec<u8>, u64)>, Error> {
        let length = as_usize(take()?)?;
        (0..length)
            .map(|_| Ok((decode_bytes(take)?, as_u64(take()?)?)))
            .collect()
    }
    fn decode_buckets(
        take: &mut impl FnMut() -> Result<i128, Error>,
    ) -> Result<Vec<Bucket>, Error> {
        let length = as_usize(take()?)?;
        (0..length)
            .map(|_| {
                Ok(Bucket {
                    count: as_i64(take()?)?,
                    repeats: as_i64(take()?)?,
                    lower: decode_bytes(take)?,
                    upper: decode_bytes(take)?,
                    ndv: as_i64(take()?)?,
                })
            })
            .collect()
    }
    let mut stats = TableStats {
        physical_id: as_i64(take()?)?,
        pseudo: take()? != 0,
        initialized: take()? != 0,
        version: as_u64(take()?)?,
        modify_count: as_i64(take()?)?,
        realtime_count: as_i64(take()?)?,
        last_analyze_version: as_u64(take()?)?,
        last_stats_hist_version: as_u64(take()?)?,
        stats_version: as_i64(take()?)?,
        pre_scalar_ready: take()? != 0,
        ..TableStats::default()
    };
    let column_count = as_usize(take()?)?;
    for _ in 0..column_count {
        let id = as_i64(take()?)?;
        let column = ColumnStats {
            analyzed_or_synthesized: take()? != 0,
            stats_version: as_i64(take()?)?,
            ndv: as_i64(take()?)?,
            null_count: as_i64(take()?)?,
            total_column_size: as_i64(take()?)?,
            version: as_u64(take()?)?,
            loaded_or_evicted: take()? != 0,
            field_type: u8::try_from(take()?)
                .map_err(|_| Error("historical field type overflow".to_owned()))?,
            correlation: f64::from_bits(as_u64(take()?)?),
            average_size: f64::from_bits(as_u64(take()?)?),
            top_n: decode_topn(&mut take)?,
            buckets: decode_buckets(&mut take)?,
            fm_sketch: Vec::new(),
        };
        stats.columns.insert(id, column);
    }
    let index_count = as_usize(take()?)?;
    for _ in 0..index_count {
        let id = as_i64(take()?)?;
        let index = IndexStats {
            analyzed: take()? != 0,
            stats_version: as_i64(take()?)?,
            version: as_u64(take()?)?,
            ndv: as_i64(take()?)?,
            null_count: as_i64(take()?)?,
            total_column_size: as_i64(take()?)?,
            correlation: f64::from_bits(as_u64(take()?)?),
            cms_loaded: take()? != 0,
            fully_loaded: take()? != 0,
            top_n: decode_topn(&mut take)?,
            buckets: decode_buckets(&mut take)?,
            fm_sketch: Vec::new(),
        };
        stats.indexes.insert(id, index);
    }
    if position != numbers.len() {
        return Err(Error(
            "historical statistics JSON has trailing numeric data".to_owned(),
        ));
    }
    let canonical = encode_legacy_historical_json_blocks(&stats, &source)?.concat();
    if canonical != encoded.as_bytes() {
        return Err(Error(
            "historical statistics JSON is not in canonical structure".to_owned(),
        ));
    }
    Ok((source, stats))
}

/// 校验历史统计来源字符串可安全落库（非空且无可打印控制字符）。
fn validate_historical_source(source: &str) -> Result<(), Error> {
    if source.is_empty() || source.bytes().any(|byte| byte == b'\0' || byte < b' ') {
        return Err(Error(
            "historical statistics source is not storage-safe".to_owned(),
        ));
    }
    Ok(())
}

/// 运行时 `Bucket` 转为 storage 层表示（`repeats`→`repeat`）。
fn storage_bucket(bucket: &Bucket) -> astersql_statistics_handle_storage::Bucket {
    astersql_statistics_handle_storage::Bucket {
        count: bucket.count,
        repeat: bucket.repeats,
        lower: bucket.lower.clone(),
        upper: bucket.upper.clone(),
        ndv: bucket.ndv,
    }
}

/// storage 层桶转回运行时 `Bucket`。
fn runtime_bucket(bucket: &astersql_statistics_handle_storage::Bucket) -> Bucket {
    Bucket {
        count: bucket.count,
        repeats: bucket.repeat,
        lower: bucket.lower.clone(),
        upper: bucket.upper.clone(),
        ndv: bucket.ndv,
    }
}

/// 经 storage `JsonTable` 路径编码历史统计，并切分为可持久化的 gzip blocks。
fn encode_historical_json_blocks(stats: &TableStats, source: &str) -> Result<Vec<Vec<u8>>, Error> {
    validate_historical_source(source)?;
    let mut table = astersql_statistics_handle_storage::JsonTable {
        database_name: source.to_owned(),
        table_name: format!(
            "{}:{}:{}:{}:{}",
            u8::from(stats.pseudo),
            u8::from(stats.initialized),
            stats.last_analyze_version,
            stats.last_stats_hist_version,
            u8::from(stats.pre_scalar_ready)
        ),
        stats: astersql_statistics_handle_storage::TableStats {
            physical_id: stats.physical_id,
            count: stats.realtime_count,
            modify_count: stats.modify_count,
            version: stats.version,
            stats_version: stats.stats_version,
            ..Default::default()
        },
        predicate_columns: Vec::new(),
        is_historical_stats: true,
    };
    // 列/索引元数据编码进 name，便于解码时还原标志位与类型。
    for (id, column) in &stats.columns {
        let name = format!(
            "c:{id}:{}:{}:{}:{}",
            u8::from(column.analyzed_or_synthesized),
            u8::from(column.loaded_or_evicted),
            column.field_type,
            column.average_size.to_bits()
        );
        table.stats.columns.insert(
            name.clone(),
            astersql_statistics_handle_storage::ColumnStats {
                name,
                histogram: astersql_statistics_handle_storage::Histogram {
                    id: *id,
                    ndv: column.ndv,
                    null_count: column.null_count,
                    last_update_version: column.version,
                    total_column_size: column.total_column_size,
                    correlation: column.correlation,
                    buckets: column.buckets.iter().map(storage_bucket).collect(),
                },
                top_n: column
                    .top_n
                    .iter()
                    .map(
                        |(encoded, count)| astersql_statistics_handle_storage::TopNItem {
                            encoded: encoded.clone(),
                            count: *count,
                        },
                    )
                    .collect(),
                stats_version: column.stats_version,
                fm_sketch: (!column.fm_sketch.is_empty()).then(|| column.fm_sketch.clone()),
                ..Default::default()
            },
        );
    }
    for (id, index) in &stats.indexes {
        let name = format!(
            "i:{id}:{}:{}:{}",
            u8::from(index.analyzed),
            u8::from(index.cms_loaded),
            u8::from(index.fully_loaded)
        );
        table.stats.indices.insert(
            name.clone(),
            astersql_statistics_handle_storage::ColumnStats {
                name,
                histogram: astersql_statistics_handle_storage::Histogram {
                    id: *id,
                    ndv: index.ndv,
                    null_count: index.null_count,
                    last_update_version: index.version,
                    total_column_size: index.total_column_size,
                    correlation: index.correlation,
                    buckets: index.buckets.iter().map(storage_bucket).collect(),
                    ..Default::default()
                },
                cmsketch: index.cms_loaded.then(|| vec![1]),
                top_n: index
                    .top_n
                    .iter()
                    .map(
                        |(encoded, count)| astersql_statistics_handle_storage::TopNItem {
                            encoded: encoded.clone(),
                            count: *count,
                        },
                    )
                    .collect(),
                stats_version: index.stats_version,
                fm_sketch: (!index.fm_sketch.is_empty()).then(|| index.fm_sketch.clone()),
                ..Default::default()
            },
        );
    }
    astersql_statistics_handle_storage::json_table_to_blocks(
        &table,
        astersql_statistics_handle_history::MAX_COLUMN_SIZE,
    )
    .map_err(|error| Error(error.to_string()))
}

/// 解析历史元数据中的 `0`/`1` 布尔标志。
fn parse_storage_flag(value: &str) -> Result<bool, Error> {
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(Error(format!("invalid historical boolean {value}"))),
    }
}

/// 校验历史统计 gzip member 头、块长度与结尾校验字段布局。
fn validate_historical_gzip_member(data: &[u8]) -> Result<(), Error> {
    if data.len() < 18 || data[..10] != [0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255] {
        return Err(Error("invalid historical gzip header".to_owned()));
    }
    let mut at = 10usize;
    loop {
        let header = *data
            .get(at)
            .ok_or_else(|| Error("truncated historical gzip block".to_owned()))?;
        at += 1;
        if header & 0x06 != 0 {
            return Err(Error("unsupported historical gzip block".to_owned()));
        }
        let len = u16::from_le_bytes(
            data.get(at..at + 2)
                .ok_or_else(|| Error("truncated historical gzip length".to_owned()))?
                .try_into()
                .unwrap(),
        );
        let nlen = u16::from_le_bytes(
            data.get(at + 2..at + 4)
                .ok_or_else(|| Error("truncated historical gzip length".to_owned()))?
                .try_into()
                .unwrap(),
        );
        if len != !nlen {
            return Err(Error("invalid historical gzip length".to_owned()));
        }
        at = at
            .checked_add(4 + usize::from(len))
            .ok_or_else(|| Error("oversized historical gzip block".to_owned()))?;
        if at > data.len() {
            return Err(Error("truncated historical gzip payload".to_owned()));
        }
        if header & 1 != 0 {
            break;
        }
    }
    if at.checked_add(8) != Some(data.len()) {
        return Err(Error(
            "historical gzip member has trailing or missing bytes".to_owned(),
        ));
    }
    Ok(())
}

/// 解码 gzip 历史 JSON blocks 为来源名与 `TableStats`（Go 风格导出名）。
pub fn DecodeHistoricalJsonBlocks(blocks: &[Vec<u8>]) -> Result<(String, TableStats), Error> {
    let encoded = blocks.concat();
    validate_historical_gzip_member(&encoded)?;
    let table = astersql_statistics_handle_storage::blocks_to_json_table(blocks)
        .map_err(|error| Error(error.to_string()))?;
    validate_historical_source(&table.database_name)?;
    if !table.is_historical_stats || !table.predicate_columns.is_empty() {
        return Err(Error(
            "historical JSONTable contains invalid auxiliary fields".to_owned(),
        ));
    }
    let metadata = table.table_name.split(':').collect::<Vec<_>>();
    if metadata.len() != 5 {
        return Err(Error(
            "historical JSONTable has invalid table metadata".to_owned(),
        ));
    }
    let mut stats = TableStats {
        physical_id: table.stats.physical_id,
        pseudo: parse_storage_flag(metadata[0])?,
        initialized: parse_storage_flag(metadata[1])?,
        version: table.stats.version,
        modify_count: table.stats.modify_count,
        realtime_count: table.stats.count,
        last_analyze_version: metadata[2]
            .parse()
            .map_err(|error| Error(format!("invalid last analyze version: {error}")))?,
        last_stats_hist_version: metadata[3]
            .parse()
            .map_err(|error| Error(format!("invalid histogram version: {error}")))?,
        stats_version: table.stats.stats_version,
        pre_scalar_ready: parse_storage_flag(metadata[4])?,
        // Historical JSON does not store analyze_count separately; after ANALYZE
        // it matches realtime_count on the live profile.
        analyze_count: table.stats.count,
        ..Default::default()
    };
    for (key, item) in table.stats.columns {
        if item.name != key {
            return Err(Error("historical column key/name mismatch".to_owned()));
        }
        let metadata = key.split(':').collect::<Vec<_>>();
        if metadata.len() != 6 || metadata[0] != "c" {
            return Err(Error("historical column metadata is invalid".to_owned()));
        }
        let id = metadata[1]
            .parse::<i64>()
            .map_err(|error| Error(format!("invalid historical column ID: {error}")))?;
        if id != item.histogram.id || stats.columns.contains_key(&id) {
            return Err(Error("historical column ID mismatch".to_owned()));
        }
        stats.columns.insert(
            id,
            ColumnStats {
                analyzed_or_synthesized: parse_storage_flag(metadata[2])?,
                stats_version: item.stats_version,
                ndv: item.histogram.ndv,
                null_count: item.histogram.null_count,
                total_column_size: item.histogram.total_column_size,
                version: item.histogram.last_update_version,
                loaded_or_evicted: parse_storage_flag(metadata[3])?,
                field_type: metadata[4]
                    .parse()
                    .map_err(|error| Error(format!("invalid historical field type: {error}")))?,
                correlation: item.histogram.correlation,
                average_size: f64::from_bits(
                    metadata[5].parse().map_err(|error| {
                        Error(format!("invalid historical average size: {error}"))
                    })?,
                ),
                top_n: item
                    .top_n
                    .into_iter()
                    .map(|item| (item.encoded, item.count))
                    .collect(),
                buckets: item.histogram.buckets.iter().map(runtime_bucket).collect(),
                fm_sketch: item.fm_sketch.unwrap_or_default(),
            },
        );
    }
    for (key, item) in table.stats.indices {
        if item.name != key {
            return Err(Error("historical index key/name mismatch".to_owned()));
        }
        let metadata = key.split(':').collect::<Vec<_>>();
        if metadata.len() != 5 || metadata[0] != "i" {
            return Err(Error("historical index metadata is invalid".to_owned()));
        }
        let id = metadata[1]
            .parse::<i64>()
            .map_err(|error| Error(format!("invalid historical index ID: {error}")))?;
        if id != item.histogram.id || stats.indexes.contains_key(&id) {
            return Err(Error("historical index ID mismatch".to_owned()));
        }
        stats.indexes.insert(
            id,
            IndexStats {
                analyzed: parse_storage_flag(metadata[2])?,
                stats_version: item.stats_version,
                version: item.histogram.last_update_version,
                ndv: item.histogram.ndv,
                null_count: item.histogram.null_count,
                total_column_size: item.histogram.total_column_size,
                correlation: item.histogram.correlation,
                cms_loaded: parse_storage_flag(metadata[3])?,
                top_n: item
                    .top_n
                    .into_iter()
                    .map(|item| (item.encoded, item.count))
                    .collect(),
                buckets: item.histogram.buckets.iter().map(runtime_bucket).collect(),
                fully_loaded: parse_storage_flag(metadata[4])?,
                fm_sketch: item.fm_sketch.unwrap_or_default(),
            },
        );
    }
    Ok((table.database_name, stats))
}

impl<B: HandleBackend> Handle<B> {
    /// 创建统计句柄；非测试且存在 DDL notifier 时注册 DDL 处理器。
    pub fn new(mut backend: B, notifier_present: bool, in_test: bool) -> Result<Self, Error> {
        if notifier_present && !in_test {
            backend.register_ddl_handler();
        }
        Ok(Self {
            cache: StatsCache::default(),
            backend,
            ddl_events: VecDeque::with_capacity(1_000),
            system_database_ids: HashSet::new(),
            init_stats_done: false,
            next_stats_version: 0,
            column_usage: HashMap::new(),
            analyze_jobs: Vec::new(),
            historical_snapshots: HashMap::new(),
            historical_enabled: in_test,
            historical_meta_failpoints: Arc::new(HistoricalMetaFailpoints::default()),
            lease: Duration::ZERO,
        })
    }

    /// 测试专用：下一次记录历史统计元数据时触发 panic。
    pub fn EnablePanicWhenRecordingHistoricalStatsMetaForTest(&self) -> HistoricalMetaPanicGuard {
        let id = self
            .historical_meta_failpoints
            .next_id
            .fetch_add(1, Ordering::Relaxed);
        self.historical_meta_failpoints
            .active
            .lock()
            .expect("historical statistics failpoint lock poisoned")
            .insert(id);
        HistoricalMetaPanicGuard {
            failpoints: Arc::clone(&self.historical_meta_failpoints),
            id,
        }
    }

    /// 若存在活动 failpoint，则 panic 并消费一次。
    fn fail_if_recording_historical_stats_meta(&self) {
        let should_panic = {
            let mut active = self
                .historical_meta_failpoints
                .active
                .lock()
                .expect("historical statistics failpoint lock poisoned");
            let id = active.iter().next().copied();
            id.is_some_and(|id| active.remove(&id))
        };
        if should_panic {
            panic!("record historical statistics meta");
        }
    }

    /// 只读访问统计缓存。
    pub fn cache(&self) -> &StatsCache {
        &self.cache
    }

    /// 可变访问统计缓存。
    pub fn cache_mut(&mut self) -> &mut StatsCache {
        &mut self.cache
    }

    /// 只读访问后端。
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// 可变访问后端。
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Configures the statistics lease used by callers that coordinate cache
    /// refreshes with asynchronous statistics loading.
    /// 设置统计租约（lease），用于与异步加载协调缓存刷新。
    pub fn set_lease(&mut self, lease: Duration) {
        self.lease = lease;
    }

    /// 返回当前统计租约。
    pub fn lease(&self) -> Duration {
        self.lease
    }

    /// Persists pending statistics deltas through the configured backend.
    /// 通过后端将待刷的统计增量持久化；`force` 为真时忽略占比阈值。
    pub fn dump_stats_delta_to_kv(&mut self, force: bool) -> Result<(), Error> {
        self.backend.dump_stats_delta(force)
    }

    /// 按表 ID 取缓存中的统计元数据。
    pub fn stats_meta(&self, table_id: i64) -> Option<&TableStats> {
        self.cache.get(table_id)
    }

    /// 返回缓存中全部表统计的克隆列表。
    pub fn stats_meta_rows(&self) -> Vec<TableStats> {
        self.cache.values().cloned().collect()
    }

    /// 用已落库的 meta 字段回写缓存中对应表的版本与计数。
    pub fn apply_persisted_stats_meta(
        &mut self,
        table_id: i64,
        version: u64,
        count: i64,
        modify_count: i64,
        last_histogram_version: u64,
    ) {
        let Some(stats) = self.cache.get_mut(table_id) else {
            return;
        };
        stats.version = version;
        stats.realtime_count = count;
        stats.modify_count = modify_count;
        stats.last_stats_hist_version = last_histogram_version;
    }

    /// 列出某表内存历史快照的元信息。
    pub fn historical_stats(&self, table_id: i64) -> Vec<HistoricalStatsMeta> {
        self.historical_snapshots
            .get(&table_id)
            .into_iter()
            .flatten()
            .map(|entry| HistoricalStatsMeta {
                table_id,
                version: entry.stats.version,
                modify_count: entry.stats.modify_count,
                row_count: entry.stats.realtime_count,
                source: entry.source.clone(),
            })
            .collect()
    }

    /// 分配下一个单调递增的统计版本号。
    pub fn allocate_stats_version(&mut self) -> u64 {
        self.next_stats_version = self.next_stats_version.saturating_add(1);
        self.next_stats_version
    }

    /// 发布 ANALYZE 结果到缓存（默认来源标记为 `analyze`）。
    pub fn publish_runtime_stats(
        &mut self,
        version: u64,
        profiles: Vec<TableStats>,
        jobs: Vec<RuntimeAnalyzeJob>,
    ) -> Result<(), Error> {
        self.publish_runtime_stats_with_source(version, profiles, jobs, "analyze")
    }

    /// 校验批内表/作业一致性后，原子提交统计配置文件与可选历史快照。
    pub fn publish_runtime_stats_with_source(
        &mut self,
        version: u64,
        profiles: Vec<TableStats>,
        jobs: Vec<RuntimeAnalyzeJob>,
        source: &str,
    ) -> Result<(), Error> {
        if source.is_empty() {
            return Err(Error(
                "historical statistics source must not be empty".to_owned(),
            ));
        }
        // 同一批内物理表 ID 不得重复，且必须已在缓存中注册。
        let mut physical_ids = HashSet::new();
        for profile in &profiles {
            if !physical_ids.insert(profile.physical_id) {
                return Err(Error(format!(
                    "duplicate statistics table {} in analyze batch",
                    profile.physical_id
                )));
            }
            if self.cache.get(profile.physical_id).is_none() {
                return Err(Error(format!(
                    "unknown statistics table {}",
                    profile.physical_id
                )));
            }
        }
        // 若携带 ANALYZE 作业，则作业覆盖的物理表集合须与 profiles 完全一致。
        if !jobs.is_empty() {
            let mut job_physical_ids = HashSet::new();
            for job in &jobs {
                if job.physical_ids.is_empty() {
                    return Err(Error(format!(
                        "analyze job for {}.{} has no physical table IDs",
                        job.database, job.table
                    )));
                }
                for physical_id in &job.physical_ids {
                    if !physical_ids.contains(physical_id) {
                        return Err(Error(format!(
                            "analyze job for {}.{} references unknown batch table {}",
                            job.database, job.table, physical_id
                        )));
                    }
                    // Go records separate global-merge jobs for columns and
                    // indexes, so multiple jobs may legitimately cover the
                    // same physical table in one analyze batch.
                    job_physical_ids.insert(*physical_id);
                }
            }
            if job_physical_ids != physical_ids {
                return Err(Error(
                    "analyze jobs do not cover every statistics table in the batch".to_owned(),
                ));
            }
        }
        // 先在副本上组装，成功后再替换，保证提交原子性。
        let mut next_cache = self.cache.clone();
        let mut next_history = self.historical_snapshots.clone();
        if self.historical_enabled {
            self.fail_if_recording_historical_stats_meta();
            encode_historical_json_blocks(&TableStats::default(), source)?;
        }
        for mut profile in profiles {
            profile.version = version;
            profile.last_analyze_version = version;
            profile.last_stats_hist_version = version;
            profile.modify_count = 0;
            // ANALYZE 成功后以当前行数作为后续自动分析比例的分母。
            profile.analyze_count = profile.realtime_count;
            profile.pseudo = false;
            let physical_id = profile.physical_id;
            if self.historical_enabled {
                next_history
                    .entry(physical_id)
                    .or_default()
                    .push(HistoricalTableStats {
                        stats: profile.clone(),
                        source: source.to_owned(),
                        json_blocks: None,
                        created_at: SystemTime::now(),
                    });
            }
            next_cache.put(profile);
        }
        self.cache = next_cache;
        self.historical_snapshots = next_history;
        self.next_stats_version = self.next_stats_version.max(version);
        self.record_analyze_jobs(jobs);
        Ok(())
    }

    /// 批量应用行增量/修改计数并推进版本，可选写入历史快照。
    pub fn flush_runtime_stats_deltas(
        &mut self,
        deltas: &[(i64, (i64, i64))],
        source: &str,
    ) -> Result<u64, Error> {
        if source.is_empty() {
            return Err(Error(
                "historical statistics source must not be empty".to_owned(),
            ));
        }
        for (physical_id, (_, modified_rows)) in deltas {
            if *modified_rows < 0 {
                return Err(Error("modified row count must not be negative".to_owned()));
            }
            if self.cache.get(*physical_id).is_none() {
                return Err(Error(format!("unknown statistics table {physical_id}")));
            }
        }
        let version = self.next_stats_version.saturating_add(1);
        let mut next_cache = self.cache.clone();
        let mut next_history = self.historical_snapshots.clone();
        if self.historical_enabled {
            self.fail_if_recording_historical_stats_meta();
            encode_historical_json_blocks(&TableStats::default(), source)?;
        }
        for (physical_id, (row_delta, modified_rows)) in deltas {
            let stats = next_cache
                .tables
                .get_mut(physical_id)
                .expect("validated statistics table disappeared");
            stats.realtime_count = stats.realtime_count.saturating_add(*row_delta).max(0);
            stats.modify_count = stats.modify_count.saturating_add(*modified_rows);
            stats.version = version;
            if self.historical_enabled {
                next_history
                    .entry(*physical_id)
                    .or_default()
                    .push(HistoricalTableStats {
                        stats: stats.clone(),
                        source: source.to_owned(),
                        json_blocks: None,
                        created_at: SystemTime::now(),
                    });
            }
        }
        self.cache = next_cache;
        self.historical_snapshots = next_history;
        self.next_stats_version = version;
        Ok(version)
    }

    /// 将单表已持久化的 delta 应用到缓存并分配新版本。
    pub fn apply_persisted_stats_delta(
        &mut self,
        physical_id: i64,
        row_delta: i64,
        modified_rows: i64,
    ) -> Result<u64, Error> {
        let version = self.next_stats_version.saturating_add(1);
        let stats = self
            .cache
            .get_mut(physical_id)
            .ok_or_else(|| Error(format!("unknown statistics table {physical_id}")))?;
        stats.realtime_count = stats.realtime_count.saturating_add(row_delta).max(0);
        stats.modify_count = stats.modify_count.saturating_add(modified_rows);
        stats.version = version;
        self.next_stats_version = version;
        Ok(version)
    }

    /// 仅推进某表的统计版本号（不改行数）。
    pub fn touch_stats_version(&mut self, physical_id: i64) -> Result<u64, Error> {
        let version = self.next_stats_version.saturating_add(1);
        let stats = self
            .cache
            .get_mut(physical_id)
            .ok_or_else(|| Error(format!("unknown statistics table {physical_id}")))?;
        stats.version = version;
        self.next_stats_version = version;
        Ok(version)
    }

    /// 开关内存历史统计快照记录。
    pub fn set_historical_enabled(&mut self, enabled: bool) {
        self.historical_enabled = enabled;
    }

    /// 是否启用历史统计快照。
    pub fn historical_enabled(&self) -> bool {
        self.historical_enabled
    }

    /// 回收超过保留期的历史快照，返回删除条数。
    pub fn gc_historical_stats_older_than(&mut self, retention: Duration) -> usize {
        let now = SystemTime::now();
        let before = self
            .historical_snapshots
            .values()
            .map(Vec::len)
            .sum::<usize>();
        self.historical_snapshots.retain(|_, snapshots| {
            snapshots.retain(|snapshot| {
                now.duration_since(snapshot.created_at).unwrap_or_default() <= retention
            });
            !snapshots.is_empty()
        });
        before
            - self
                .historical_snapshots
                .values()
                .map(Vec::len)
                .sum::<usize>()
    }

    /// 按版本查找历史快照；若无匹配则回退为当前缓存表的非历史视图。
    pub fn historical_snapshot(
        &self,
        table_id: i64,
        version: u64,
    ) -> Option<RuntimeHistoricalSnapshot> {
        if let Some(snapshot) = self
            .historical_snapshots
            .get(&table_id)
            .and_then(|snapshots| {
                snapshots
                    .iter()
                    .rev()
                    .find(|entry| entry.stats.version <= version)
            })
        {
            return Some(RuntimeHistoricalSnapshot {
                table_id,
                version: snapshot.stats.version,
                row_count: snapshot.stats.realtime_count,
                modify_count: snapshot.stats.modify_count,
                is_historical: true,
            });
        }
        let table = self.cache.get(table_id)?;
        Some(RuntimeHistoricalSnapshot {
            table_id,
            version: table.version,
            row_count: table.realtime_count,
            modify_count: table.modify_count,
            is_historical: false,
        })
    }

    /// 取已编码的历史 JSON blocks（若该快照尚未 dump 则为 `None`）。
    pub fn historical_json_blocks(&self, table_id: i64, version: u64) -> Option<Vec<Vec<u8>>> {
        self.historical_snapshots
            .get(&table_id)?
            .iter()
            .rev()
            .find(|entry| entry.stats.version <= version)
            .and_then(|entry| entry.json_blocks.clone())
    }

    /// 按需将匹配版本的历史快照编码为 JSON blocks。
    pub fn dump_historical_stats(&mut self, table_id: i64, version: u64) -> Result<bool, Error> {
        let Some(entry) = self
            .historical_snapshots
            .get_mut(&table_id)
            .and_then(|snapshots| {
                snapshots
                    .iter_mut()
                    .rev()
                    .find(|entry| entry.stats.version <= version)
            })
        else {
            return Ok(false);
        };
        if entry.json_blocks.is_none() {
            entry.json_blocks = Some(encode_historical_json_blocks(&entry.stats, &entry.source)?);
        }
        Ok(true)
    }

    /// 记录列统计使用情况（供谓词列/自动分析决策）。
    pub fn record_column_usage(&mut self, usage: RuntimeColumnUsage) {
        self.column_usage
            .insert((usage.table_id, usage.column_id), usage);
    }

    /// 返回当前记录的全部列使用信息。
    pub fn column_usage(&self) -> Vec<RuntimeColumnUsage> {
        self.column_usage.values().cloned().collect()
    }

    /// `DELETE FROM mysql.column_stats_usage`.
    /// 清空列使用统计（对应 `DELETE FROM mysql.column_stats_usage`）。
    pub fn clear_column_usage(&mut self) {
        self.column_usage.clear();
    }

    /// 返回已记录的 ANALYZE 作业列表。
    pub fn analyze_jobs(&self) -> Vec<RuntimeAnalyzeJob> {
        self.analyze_jobs.clone()
    }

    /// 清空 ANALYZE 作业列表。
    pub fn clear_analyze_jobs(&mut self) {
        self.analyze_jobs.clear();
    }

    /// 追加 ANALYZE 作业记录。
    pub fn record_analyze_jobs(&mut self, jobs: Vec<RuntimeAnalyzeJob>) {
        for job in jobs {
            self.analyze_jobs.retain(|existing| {
                existing.database != job.database
                    || existing.table != job.table
                    || existing.partition != job.partition
                    || existing.state != "running"
            });
            self.analyze_jobs.push(job);
        }
    }

    /// 从缓存与列使用中移除物理表；历史快照留给 GC 路径处理。
    pub fn remove_tables(&mut self, physical_ids: &[i64]) {
        for physical_id in physical_ids {
            self.cache.tables.remove(physical_id);
            self.column_usage
                .retain(|(table_id, _), _| table_id != physical_id);
            // Keep historical_snapshots until GCStats / gc_dropped_stats runs.
            // Dropping a table must not erase mysql.stats_*_history rows early.
        }
    }

    /// 显式删除指定物理表的历史快照。
    pub fn remove_historical_snapshots(&mut self, physical_ids: &[i64]) {
        for physical_id in physical_ids {
            self.historical_snapshots.remove(physical_id);
        }
    }

    /// Reconciles cache membership with committed metadata without wiping
    /// historical snapshots or unrelated runtime state for retained tables.
    /// 按提交后的元数据集合对齐缓存：淘汰多余表并合并保留表的列/索引集合。
    pub fn reconcile_cache_profiles(&mut self, profiles: Vec<TableStats>) {
        let retained = profiles
            .iter()
            .map(|profile| profile.physical_id)
            .collect::<HashSet<_>>();
        let stale = self
            .cache
            .tables
            .keys()
            .copied()
            .filter(|physical_id| !retained.contains(physical_id))
            .collect::<Vec<_>>();
        self.remove_tables(&stale);
        for profile in profiles {
            if let Some(existing) = self.cache.tables.remove(&profile.physical_id) {
                let mut merged = existing;
                merged.pseudo = profile.pseudo;
                merged.initialized = profile.initialized;
                merged.version = profile.version;
                merged.modify_count = profile.modify_count;
                merged.realtime_count = profile.realtime_count;
                merged.last_analyze_version = profile.last_analyze_version;
                merged.last_stats_hist_version = profile.last_stats_hist_version;
                merged
                    .columns
                    .retain(|column_id, _| profile.columns.contains_key(column_id));
                merged
                    .indexes
                    .retain(|index_id, _| profile.indexes.contains_key(index_id));
                for (column_id, column) in &profile.columns {
                    if let Some(existing) = merged.columns.get_mut(column_id) {
                        existing.analyzed_or_synthesized = column.analyzed_or_synthesized;
                        if !self.lease.is_zero() && existing.stats_version != 0 {
                            existing.loaded_or_evicted = false;
                            existing.buckets.clear();
                            existing.top_n.clear();
                        }
                    } else {
                        merged.columns.insert(*column_id, column.clone());
                    }
                }
                for (index_id, index) in &profile.indexes {
                    if let Some(existing) = merged.indexes.get_mut(index_id) {
                        existing.analyzed = index.analyzed;
                        if !self.lease.is_zero() && existing.stats_version != 0 {
                            existing.fully_loaded = false;
                            existing.buckets.clear();
                            existing.top_n.clear();
                        }
                    } else {
                        merged.indexes.insert(*index_id, index.clone());
                    }
                }
                self.cache.put(merged);
            } else {
                self.cache.put(profile);
            }
        }
    }

    /// Merges persisted profiles for a selected set of physical tables.
    ///
    /// Unlike [`Self::reconcile_cache_profiles`], this preserves unrelated
    /// cache entries.  Bootstrap's lite-init path uses that property when it
    /// is asked to load only a caller-provided table-ID set.
    /// 合并指定物理表的持久化配置文件，保留缓存中无关表项（lite-init 用）。
    pub fn merge_cache_profiles(&mut self, profiles: Vec<TableStats>) {
        for profile in profiles {
            if let Some(existing) = self.cache.tables.get_mut(&profile.physical_id) {
                existing.pseudo = profile.pseudo;
                existing.initialized = profile.initialized;
                existing.version = profile.version;
                existing.modify_count = profile.modify_count;
                existing.realtime_count = profile.realtime_count;
                existing.last_analyze_version = profile.last_analyze_version;
                existing.last_stats_hist_version = profile.last_stats_hist_version;
                existing
                    .columns
                    .retain(|column_id, _| profile.columns.contains_key(column_id));
                existing
                    .indexes
                    .retain(|index_id, _| profile.indexes.contains_key(index_id));
                for (column_id, column) in &profile.columns {
                    existing
                        .columns
                        .entry(*column_id)
                        .or_insert_with(|| column.clone());
                }
                for (index_id, index) in &profile.indexes {
                    existing
                        .indexes
                        .entry(*index_id)
                        .or_insert_with(|| index.clone());
                }
            } else {
                self.cache.put(profile);
            }
        }
    }

    /// 从指定物理表缓存中删除给定列/索引统计项。
    pub fn remove_stats_items(
        &mut self,
        physical_ids: &[i64],
        column_ids: &HashSet<i64>,
        index_ids: &HashSet<i64>,
    ) {
        for physical_id in physical_ids {
            if let Some(stats) = self.cache.tables.get_mut(physical_id) {
                stats.columns.retain(|id, _| !column_ids.contains(id));
                stats.indexes.retain(|id, _| !index_ids.contains(id));
            }
        }
    }

    /// 将 DDL 事件入队，供统计侧异步处理。
    pub fn enqueue_ddl_event(&mut self, event: String) {
        self.ddl_events.push_back(event);
    }

    pub fn clear(&mut self) {
        self.cache.clear();
        self.ddl_events.clear();
        self.backend.reset_session_stats_list();
        self.reset_system_database_id_cache();
    }

    /// 清空系统库 ID 缓存。
    pub fn reset_system_database_id_cache(&mut self) {
        self.system_database_ids.clear();
    }

    /// 测试用：返回系统库 ID 缓存大小。
    pub fn system_database_id_cache_len_for_test(&self) -> usize {
        self.system_database_ids.len()
    }

    /// 取物理表统计；缺失时按表信息生成（并可能缓存）伪统计。
    pub fn physical_table_stats(&mut self, physical_id: i64, table: &TableInfo) -> TableStats {
        self.stats_by_physical_id(physical_id, Some(table))
            .expect("table info always produces pseudo statistics")
    }

    /// 仅当缓存中存在非伪统计时返回。
    pub fn non_pseudo_physical_table_stats(&self, physical_id: i64) -> Option<&TableStats> {
        self.cache.get(physical_id).filter(|table| !table.pseudo)
    }

    /// 按物理 ID 取统计；未命中则构造伪统计并按分区/临时表/系统库规则决定是否入缓存。
    fn stats_by_physical_id(
        &mut self,
        physical_id: i64,
        info: Option<&TableInfo>,
    ) -> Option<TableStats> {
        if let Some(table) = self.cache.get(physical_id) {
            return Some(table.clone());
        }
        let info = info?;
        let pseudo = TableStats::pseudo(physical_id);
        // 分区表伪统计受数量上限约束；临时表不写入缓存。
        let should_cache = !info.partitioned || self.cache.len() < PSEUDO_PARTITION_CACHE_LIMIT;
        if !should_cache || info.temporary != TemporaryTableType::None {
            return Some(pseudo);
        }
        match self.is_system_table(physical_id, info) {
            Ok(true) | Err(_) => Some(pseudo),
            Ok(false) => {
                self.cache.put(pseudo.clone());
                Some(pseudo)
            }
        }
    }

    /// 判定是否系统库表，命中后缓存 database_id 以免重复查询。
    fn is_system_table(&mut self, physical_id: i64, info: &TableInfo) -> Result<bool, Error> {
        if info.database_id <= 0 {
            return Err(Error(format!(
                "invalid database ID {} for table {physical_id}",
                info.database_id
            )));
        }
        if self.backend.memory_schema_id(info.database_id)
            || self.system_database_ids.contains(&info.database_id)
        {
            return Ok(true);
        }
        let system = self.backend.system_schema(info.database_id)?;
        if system {
            self.system_database_ids.insert(info.database_id);
        }
        Ok(system)
    }

    /// 强制刷盘统计增量；失败时仅告警不向上返回。
    pub fn flush_stats(&mut self) {
        if let Err(error) = self.dump_stats_delta_to_kv(true) {
            self.backend.warn_flush_error(&error);
        }
    }

    /// Flushes the local statistics delta collector before ANALYZE. Unlike the
    /// background flush wrapper, this boundary preserves the backend error so
    /// statement execution can stop before building or publishing statistics.
    /// ANALYZE 前强制刷增量，错误向上传播以中止语句。
    pub fn preflush_stats_delta(&mut self) -> Result<(), Error> {
        self.dump_stats_delta_to_kv(true)
    }

    /// 启动用量统计相关后台 worker。
    pub fn start_worker(&mut self) {
        self.backend.start_usage_worker();
    }

    pub fn close(&mut self) {
        self.backend.close_pool();
        self.cache.close();
        self.backend.close_usage();
        self.backend.close_auto_analyze();
        self.reset_system_database_id_cache();
    }

    /// 用给定缓存整体替换当前缓存（内部测试/初始化用）。
    pub(crate) fn replace_cache(&mut self, cache: StatsCache) {
        self.cache = cache;
    }
}

/// 将 Handle 适配为 ANALYZE 存储接口，供会话/执行器提交统计。
impl<B: HandleBackend> AnalyzeStatsStorage for Handle<B> {
    /// 注册一张新的物理表统计槽位（ID 必须为正且尚未存在）。
    fn register_table_stats(&mut self, table_id: i64) -> Result<(), Error> {
        if table_id <= 0 {
            return Err(Error("statistics table ID must be positive".to_owned()));
        }
        if self.cache.get(table_id).is_some() {
            return Err(Error(format!("statistics table {table_id} already exists")));
        }
        self.cache.put(TableStats {
            physical_id: table_id,
            pseudo: false,
            initialized: true,
            ..TableStats::default()
        });
        Ok(())
    }

    /// 记录 DML 引起的行增量与修改行数。
    fn record_table_mutation(
        &mut self,
        table_id: i64,
        row_delta: i64,
        modified_rows: i64,
    ) -> Result<(), Error> {
        if modified_rows < 0 {
            return Err(Error("modified row count must not be negative".to_owned()));
        }
        let table = self
            .cache
            .get_mut(table_id)
            .ok_or_else(|| Error(format!("unknown statistics table {table_id}")))?;
        table.realtime_count = table.realtime_count.saturating_add(row_delta).max(0);
        table.modify_count = table.modify_count.saturating_add(modified_rows);
        Ok(())
    }

    /// 兼容入口：分配版本并发布该表的 ANALYZE 结果。
    fn analyze_table_stats(&mut self, table_id: i64) -> Result<u64, Error> {
        // This compatibility entry point shares the exact atomic Handle commit
        // used by SQL ANALYZE. Session/executor code remains responsible for
        // kill checks and job orchestration before calling the commit boundary.
        let mut profile = self
            .cache
            .get(table_id)
            .cloned()
            .ok_or_else(|| Error(format!("unknown statistics table {table_id}")))?;
        let version = self.allocate_stats_version();
        profile.modify_count = 0;
        profile.analyze_count = profile.realtime_count;
        self.publish_runtime_stats(version, vec![profile], Vec::new())
            .map_err(|error| Error(error.to_string()))?;
        Ok(version)
    }
}
