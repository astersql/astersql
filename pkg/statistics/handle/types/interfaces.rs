// Copyright 2023 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 统计 Handle 子系统接口与 DTO 定义。
//
// 聚合 GC、用量、历史、ANALYZE、缓存、锁表、读写、同步加载、全局合并与 DDL
// 等 trait，作为 Domain/session 与 statistics handle 的稳定边界。

use aster_sql_ddl_notifier::SchemaChangeEvent;
pub use aster_sql_infoschema::InfoSchema;
use aster_sql_meta_model::{PartitionDefinition, StatsLoadItem, TableInfo, TableItemID};
pub use aster_sql_sessionctx_stmtctx::{StatementContext, StatsLoadResult};
use aster_sql_statistics::{AnalyzeJob, AnalyzeResults, CMSketch, Histogram, JobType, Table, TopN};
pub use aster_sql_statistics_handle_usage_indexusage::{
    Sample as IndexUsageSample, SessionIndexUsageCollector,
};
pub use aster_sql_statistics_handle_util::{
    AutoAnalyzeProcIdGenerator as AutoAnalyzeProcIDGenerator, LeaseGetter, Pool, SessionContext,
    TableInfoGetter,
};
pub use aster_sql_statistics_handle_util::{
    ExecOption, ExecutionContext as StatsExecutionContext, GlobalVariableAccessor,
    RecordSet as StatsRecordSet, RestrictedSqlExecutor as StatsRestrictedSqlExecutor,
    ResultField as StatsResultField, Row as StatsRow, SessionVariables as StatsSessionVariables,
    SqlExecutor as StatsSqlExecutor, SqlValue as StatsSqlValue, StatsError as StatsExecError,
    Transaction as StatsTransaction, exec_rows as ExecRows,
};
pub use aster_sql_statistics_util::JSONTable;
pub use aster_sql_types::time::{DefaultFsp, FromGoTime, NewTime, Time, mysql};
pub use aster_sql_util::wait_group_wrapper::WaitGroupEnhancedWrapper;
pub use aster_sql_util_sqlexec::{RestrictedSQLExecutor, context::Context as ExecutionContext};
use chrono_tz::Tz;
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, SystemTime};

#[derive(Clone, Debug, PartialEq, Eq)]
/// 接口层错误包装。
pub struct Error(pub String);

/// Display 输出内部消息。
impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// 本模块 Result 别名。
pub type Result<T> = std::result::Result<T, Error>;

/// 统计垃圾回收：清理过期/已删对象统计与历史。
pub trait StatsGC: Send + Sync {
    fn GCStats(&self, info_schema: &dyn InfoSchema, ddl_lease: Duration) -> Result<()>;
    fn ClearOutdatedHistoryStats(&self) -> Result<()>;
    fn DeleteTableStatsFromKV(&self, stats_ids: &[i64], soft: bool) -> Result<()>;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 列统计用量时间点：最近使用与最近 ANALYZE 时间。
pub struct ColStatsTimeInfo {
    pub LastUsedAt: Option<Time>,
    pub LastAnalyzedAt: Option<Time>,
}

/// 索引用量收集、查询与后台 worker 控制。
pub trait IndexUsage: Send + Sync {
    fn NewSessionIndexUsageCollector(&self) -> SessionIndexUsageCollector;
    fn GCIndexUsage(&self) -> Result<()>;
    fn StartWorker(&self);
    fn Close(&self);
    fn GetIndexUsage(&self, table_id: i64, index_id: i64) -> IndexUsageSample;
}

/// 列/谓词用量加载，以及 delta/用量落盘到 KV。
/// Delta：相对上次 dump 的行数与修改量增量。
pub trait StatsUsage: IndexUsage {
    fn LoadColumnStatsUsage(&self, location: &Tz)
    -> Result<HashMap<TableItemID, ColStatsTimeInfo>>;
    fn GetPredicateColumns(&self, table_id: i64) -> Result<Vec<i64>>;
    fn NewSessionStatsItem(&self) -> Box<dyn Any + Send>;
    fn ResetSessionStatsList(&self);
    fn DumpStatsDeltaToKV(&self, force_dump: bool, table_ids: &[i64]) -> Result<()>;
    fn DumpColStatsUsageToKV(&self) -> Result<()>;
}

/// 历史统计元数据记录与按快照落盘。
pub trait StatsHistory: Send + Sync {
    fn RecordHistoricalStatsMeta(
        &self,
        version: u64,
        source: &str,
        enforce: bool,
        table_ids: &[i64],
    );
    fn CheckHistoricalStatsEnable(&self) -> Result<bool>;
    fn RecordHistoricalStatsToStorage(
        &self,
        db_name: &str,
        table_info: &TableInfo,
        physical_id: i64,
        is_partition: bool,
    ) -> Result<u64>;
}

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
/// 自动 ANALYZE 优先级队列快照。
pub struct PriorityQueueSnapshot {
    #[serde(rename = "current_jobs")]
    pub CurrentJobs: Vec<AnalysisJobJSON>,
    #[serde(rename = "must_retry_tables")]
    pub MustRetryTables: Vec<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
/// 优先级队列中单个分析作业的 JSON 可序列化视图。
pub struct AnalysisJobJSON {
    #[serde(rename = "type")]
    pub Type: String,
    #[serde(rename = "table_id")]
    pub TableID: i64,
    #[serde(rename = "weight")]
    pub Weight: f64,
    #[serde(rename = "partition_ids")]
    pub PartitionIDs: Vec<i64>,
    #[serde(rename = "index_ids")]
    pub IndexIDs: Vec<i64>,
    #[serde(rename = "partition_index_ids")]
    pub PartitionIndexIDs: HashMap<i64, Vec<i64>>,
    #[serde(rename = "indicators")]
    pub Indicators: IndicatorsJSON,
    #[serde(rename = "has_newly_added_index")]
    pub HasNewlyAddedIndex: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
/// 作业调度指标的字符串化展示（变更比例、表大小、距上次分析时长）。
pub struct IndicatorsJSON {
    #[serde(rename = "change_percentage")]
    pub ChangePercentage: String,
    #[serde(rename = "table_size")]
    pub TableSize: String,
    #[serde(rename = "last_analysis_duration")]
    pub LastAnalysisDuration: String,
}

/// ANALYZE 作业生命周期、自动分析与优先级队列管理。
pub trait StatsAnalyze: Send + Sync {
    fn InsertAnalyzeJob(&self, job: &AnalyzeJob, instance: &str, proc_id: u64) -> Result<()>;
    fn StartAnalyzeJob(&self, job: &AnalyzeJob);
    fn UpdateAnalyzeJobProgress(&self, job: &AnalyzeJob, row_count: i64);
    fn FinishAnalyzeJob(
        &self,
        job: &AnalyzeJob,
        fail_reason: Option<&dyn std::error::Error>,
        analyze_type: JobType,
    );
    fn DeleteAnalyzeJobs(&self, update_time: SystemTime) -> Result<()>;
    fn CleanupCorruptedAnalyzeJobsOnCurrentInstance(
        &self,
        current_running_process_ids: &HashSet<u64>,
    ) -> Result<()>;
    fn CleanupCorruptedAnalyzeJobsOnDeadInstances(&self) -> Result<()>;
    fn HandleAutoAnalyze(&self) -> bool;
    fn AnalyzeVersionMatchesForTable(&self, table_info: &TableInfo, requested_version: i32)
    -> bool;
    fn GetPriorityQueueSnapshot(&self) -> Result<PriorityQueueSnapshot>;
    fn ClosePriorityQueue(&self);
    fn Close(&self);
}

#[derive(Clone, Debug, Default)]
/// 批量更新统计缓存：新增/替换表、删除 ID 与选项。
pub struct CacheUpdate {
    pub Updated: Vec<Arc<Table>>,
    pub Deleted: Vec<i64>,
    pub Options: UpdateOptions,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// 缓存更新选项；SkipMoveForward 为真时不推进版本游标。
pub struct UpdateOptions {
    pub SkipMoveForward: bool,
}

/// 内存统计缓存：查询、替换、容量与驱逐控制。
pub trait StatsCache: Send + Sync + Any {
    fn Close(&self);
    fn Clear(&self);
    fn Update(
        &self,
        context: &ExecutionContext,
        info_schema: &dyn InfoSchema,
        table_and_partition_ids: &[i64],
    ) -> Result<()>;
    fn MemConsumed(&self) -> i64;
    fn Get(&self, table_id: i64) -> Option<Arc<Table>>;
    fn Put(&self, table_id: i64, table: Arc<Table>);
    fn UpdateStatsCache(&self, update: CacheUpdate);
    fn GetNextCheckVersionWithOffset(&self) -> u64;
    fn MaxTableStatsVersion(&self) -> u64;
    fn Values(&self) -> Vec<Arc<Table>>;
    fn Len(&self) -> usize;
    fn SetStatsCacheCapacity(&self, capacity_bytes: i64);
    fn Replace(&self, cache: &dyn StatsCache);
    fn UpdateStatsHealthyMetrics(&self);
    fn TriggerEvict(&self);
    fn WaitForAsyncUpdates(&self);
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 锁统计时描述的表：分区名映射与全限定名。
/// Stats lock：阻止对该表的自动/后台统计更新。
pub struct StatsLockTable {
    pub PartitionInfo: HashMap<i64, String>,
    pub FullName: String,
}

/// 表/分区级统计锁的加锁、解锁与查询。
pub trait StatsLock: Send + Sync {
    fn LockTables(&self, tables: &HashMap<i64, StatsLockTable>) -> Result<String>;
    fn LockPartitions(
        &self,
        table_id: i64,
        table_name: &str,
        partition_names: &HashMap<i64, String>,
    ) -> Result<String>;
    fn RemoveLockedTables(&self, tables: &HashMap<i64, StatsLockTable>) -> Result<String>;
    fn RemoveLockedPartitions(
        &self,
        table_id: i64,
        table_name: &str,
        partition_names: &HashMap<i64, String>,
    ) -> Result<String>;
    fn GetLockedTables(&self, table_ids: &[i64]) -> Result<HashSet<i64>>;
    fn GetTableLockedAndClearForTest(&self) -> Result<HashSet<i64>>;
}

/// 并发从 JSON 加载分区统计时的单任务载荷。
pub struct PartitionStatisticLoadTask {
    pub JSONTable: Option<Box<JSONTable>>,
    pub PhysicalID: i64,
}

/// 按快照持久化时的回调：写入 JSONTable 与物理 ID。
pub type PersistFunc<'a> =
    dyn Fn(&ExecutionContext, Option<&JSONTable>, i64) -> Result<()> + Send + Sync + 'a;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// 批量更新 meta 时的单表 count/modify_count 变更。
pub struct MetaUpdate {
    pub PhysicalID: i64,
    pub Count: i64,
    pub ModifyCount: i64,
}

/// 统计读写：从存储/JSON 加载，保存 ANALYZE/元数据，导出与持久化。
pub trait StatsReadWriter: Send + Sync {
    fn TableStatsFromStorage(
        &self,
        table_info: &TableInfo,
        physical_id: i64,
        load_all: bool,
        snapshot: u64,
    ) -> Result<Option<Arc<Table>>>;
    fn LoadTablePartitionStats(
        &self,
        table_info: &TableInfo,
        partition: &PartitionDefinition,
    ) -> Result<Arc<Table>>;
    fn StatsMetaCountAndModifyCount(&self, table_id: i64) -> Result<(i64, i64)>;
    fn LoadNeededHistograms(&self, info_schema: &dyn InfoSchema) -> Result<()>;
    #[allow(clippy::too_many_arguments)]
    fn SaveColOrIdxStatsToStorage(
        &self,
        table_id: i64,
        count: i64,
        modify_count: i64,
        is_index: i32,
        histogram: &Histogram,
        cms: Option<&CMSketch>,
        top_n: Option<&TopN>,
        stats_version: i32,
        update_analyze_time: bool,
        source: &str,
    ) -> Result<()>;
    fn SaveAnalyzeResultToStorage(
        &self,
        results: &AnalyzeResults,
        analyze_snapshot: bool,
        source: &str,
    ) -> Result<()>;
    fn SaveMetaToStorage(
        &self,
        source: &str,
        refresh_last_histogram_version: bool,
        updates: &[MetaUpdate],
    ) -> Result<()>;
    fn UpdateStatsMetaVersionForGC(&self, physical_id: i64) -> Result<()>;
    fn ChangeGlobalStatsID(&self, from: i64, to: i64) -> Result<()>;
    fn TableStatsToJSON(
        &self,
        db_name: &str,
        table_info: &TableInfo,
        physical_id: i64,
        snapshot: u64,
    ) -> Result<JSONTable>;
    fn DumpStatsToJSON(
        &self,
        db_name: &str,
        table_info: &TableInfo,
        history_executor: &mut dyn RestrictedSQLExecutor,
        dump_partition_stats: bool,
    ) -> Result<JSONTable>;
    fn DumpHistoricalStatsBySnapshot(
        &self,
        db_name: &str,
        table_info: &TableInfo,
        snapshot: u64,
    ) -> Result<(JSONTable, Vec<String>)>;
    fn DumpStatsToJSONBySnapshot(
        &self,
        db_name: &str,
        table_info: &TableInfo,
        snapshot: u64,
        dump_partition_stats: bool,
    ) -> Result<JSONTable>;
    fn PersistStatsBySnapshot(
        &self,
        context: &ExecutionContext,
        db_name: &str,
        table_info: &TableInfo,
        snapshot: u64,
        persist: &PersistFunc<'_>,
    ) -> Result<()>;
    fn LoadStatsFromJSONConcurrently(
        &self,
        context: &ExecutionContext,
        table_info: &TableInfo,
        tasks: Receiver<PartitionStatisticLoadTask>,
        partition_concurrency: usize,
    ) -> Result<()>;
    fn LoadStatsFromJSON(
        &self,
        context: &ExecutionContext,
        info_schema: &dyn InfoSchema,
        json_table: &JSONTable,
        partition_concurrency: usize,
    ) -> Result<()>;
    fn LoadStatsFromJSONNoUpdate(
        &self,
        context: &ExecutionContext,
        info_schema: &dyn InfoSchema,
        json_table: &JSONTable,
        partition_concurrency: usize,
    ) -> Result<()>;
}

/// 同步加载队列任务（接口层形状，含结果发送端）。
pub struct NeededItemTask {
    pub ToTimeout: SystemTime,
    pub ResultCh: std::sync::mpsc::Sender<StatsLoadResult>,
    pub Item: StatsLoadItem,
    pub Retry: i32,
}

/// 同步加载：发送请求、等待完成、追加任务与 worker 处理。
pub trait StatsSyncLoad: Send + Sync {
    fn SendLoadRequests(
        &self,
        statement_context: &mut StatementContext,
        needed_items: &[StatsLoadItem],
        timeout: Duration,
    ) -> Result<()>;
    fn SyncWaitStatsLoad(&self, statement_context: &mut StatementContext) -> Result<()>;
    fn AppendNeededItem(&self, task: NeededItemTask, timeout: Duration) -> Result<()>;
    fn SubLoadWorker(&self, exit: Receiver<()>, exit_wait_group: &WaitGroupEnhancedWrapper);
    fn HandleOneTask(
        &self,
        last_task: Option<NeededItemTask>,
        exit: &Receiver<()>,
    ) -> Result<Option<NeededItemTask>>;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 分区统计合并为全局统计时的输入描述。
pub struct GlobalStatsInfo {
    pub HistIDs: Vec<i64>,
    pub IsIndex: i32,
    pub StatsVersion: i32,
}

/// ANALYZE 选项映射（选项码 -> 值）。
pub type AnalyzeOptions = HashMap<aster_sql_parser_ast::AnalyzeOptionType, u64>;

/// 将分区统计合并为全局（按表 ID）统计。
pub trait StatsGlobal: Send + Sync {
    fn MergePartitionStats2GlobalStatsByTableID(
        &self,
        session_context: &dyn SessionContext,
        options: &AnalyzeOptions,
        info_schema: &dyn InfoSchema,
        info: &GlobalStatsInfo,
        physical_id: i64,
    ) -> Result<()>;
}

/// DDL 变更事件处理与事件通道。
/// DDL：数据定义语言，如建表/改表/删表。
pub trait DDL: Send + Sync {
    fn HandleDDLEvent(
        &self,
        context: &ExecutionContext,
        session_context: &dyn SessionContext,
        event: &SchemaChangeEvent,
    ) -> Result<()>;
    fn DDLEventCh(&self) -> &Receiver<SchemaChangeEvent>;
}

/// 完整统计 Handle：组合上述各子系统能力，并提供物理表统计读取。
pub trait StatsHandle:
    Pool
    + AutoAnalyzeProcIDGenerator
    + LeaseGetter
    + TableInfoGetter
    + StatsGC
    + StatsUsage
    + StatsHistory
    + StatsAnalyze
    + StatsCache
    + StatsLock
    + StatsReadWriter
    + StatsGlobal
    + DDL
{
    fn GetPhysicalTableStats(&self, physical_table_id: i64, table_info: &TableInfo) -> Arc<Table>;
    fn GetNonPseudoPhysicalTableStats(&self, physical_table_id: i64) -> Option<Arc<Table>>;
}
