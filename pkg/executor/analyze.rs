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

// ANALYZE 语句执行器：调度列/索引统计收集、落盘与全局统计合并。
//
// ANALYZE 采集直方图、NDV、TopN 等供优化器代价估算；执行前可广播
// `FLUSH STATS_DELTA` 预刷增量。动态分区裁剪开启时还会合并分区全局统计。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

/// 采样/随机相关的全局种子（与 Go 侧测试可复现性对齐）。
pub static RandSeed: AtomicI64 = AtomicI64::new(1);
/// 单个 Region（TiKV 键空间分片）上采样大小上限。
pub static MaxRegionSampleSize: AtomicI64 = AtomicI64::new(1000);

#[derive(Clone, Debug, Eq, PartialEq)]
/// ANALYZE 语句执行错误。
pub struct AnalyzeError(pub String);

impl fmt::Display for AnalyzeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for AnalyzeError {}

const HANDLE_RESULTS_ERROR_SINGLE_THREAD_PANIC: &str =
    "github.com/pingcap/tidb/pkg/executor/handleResultsErrorSingleThreadPanic";
const HANDLE_ANALYZE_WORKER_PANIC: &str =
    "github.com/pingcap/tidb/pkg/executor/handleAnalyzeWorkerPanic";

/// 将生产执行链捕获的 panic payload 转换为本模块的 ANALYZE 错误。
fn getAnalyzePanicErr(value: &(dyn Any + Send)) -> AnalyzeError {
    AnalyzeError(crate::analyze_utils::getAnalyzePanicErr(value).to_string())
}

/// 判断本模块错误是否为 analyze worker panic 或 analyze OOM。
fn isAnalyzeWorkerPanic(error: &AnalyzeError) -> bool {
    error.0 == crate::analyze_utils::errAnalyzeWorkerPanic().to_string()
        || error.0 == crate::analyze_utils::errAnalyzeOOM().to_string()
}

/// Go `handleResultsErrorSingleThreadPanic` 对应的生产 failpoint。
fn handleResultsErrorSingleThreadPanic() {
    let _ = fail::eval(HANDLE_RESULTS_ERROR_SINGLE_THREAD_PANIC, |_| ());
}

/// Go `handleAnalyzeWorkerPanic` 对应的生产 failpoint。
fn handleAnalyzeWorkerPanic() {
    let _ = fail::eval(HANDLE_ANALYZE_WORKER_PANIC, |_| ());
}

/// ANALYZE 操作统一 Result 别名。
pub type AnalyzeResultValue<T = ()> = Result<T, AnalyzeError>;

#[derive(Clone, Debug, Default)]
/// 可取消的分析上下文：支持父子链路与 cause 错误。
pub struct analyzeContext {
    cancelled: Arc<AtomicBool>,
    cause: Arc<Mutex<Option<AnalyzeError>>>,
    parent: Option<Arc<analyzeContext>>,
}

impl analyzeContext {
    /// 记录 cause 并标记已取消。
    pub fn cancel(&self, cause: AnalyzeError) {
        *self.cause.lock().expect("analyze context mutex poisoned") = Some(cause);
        self.cancelled.store(true, Ordering::Release);
    }

    /// 若本层或父层已取消则返回错误。
    pub fn error(&self) -> Option<AnalyzeError> {
        if self.cancelled.load(Ordering::Acquire) {
            self.cause
                .lock()
                .expect("analyze context mutex poisoned")
                .clone()
                .or_else(|| Some(AnalyzeError("context canceled".into())))
        } else {
            self.parent.as_ref().and_then(|parent| parent.error())
        }
    }

    /// 创建挂到父上下文的子上下文。
    pub fn child(parent: analyzeContext) -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            cause: Arc::new(Mutex::new(None)),
            parent: Some(Arc::new(parent)),
        }
    }
}

/// RAII 守卫：丢弃时若上下文仍无错误则取消之。
pub struct analyzeStop {
    context: analyzeContext,
}

impl Drop for analyzeStop {
    fn drop(&mut self) {
        if self.context.error().is_none() {
            self.context.cancel(AnalyzeError("context canceled".into()));
        }
    }
}

/// 会话 kill 信号：等待取消或查询当前错误。
pub trait killSignal: Send + Sync {
    fn wait(&self, context: &analyzeContext) -> Option<AnalyzeError>;
    fn current_error(&self) -> Option<AnalyzeError>;
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
/// 待 FLUSH 的 stats delta 目标（库名.表名）。
pub struct statsObject {
    pub databaseName: String,
    pub tableName: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 分区定义中的物理 ID。
pub struct partitionDefinition {
    pub id: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 表及其分区 ID 列表。
pub struct tableInfo {
    pub id: i64,
    pub partitions: Vec<partitionDefinition>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 计划层列分析任务：库表名与可选 tableInfo。
pub struct analyzeColumnsPlanTask {
    pub databaseName: String,
    pub tableName: String,
    pub tableInfo: Option<tableInfo>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// ANALYZE 计划：列任务列表。
pub struct analyzePlan {
    pub columnTasks: Vec<analyzeColumnsPlanTask>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 集群中某 TiDB 节点的 status RPC 地址信息。
pub struct serverRPCInfo {
    pub ip: String,
    pub statusPort: u16,
    pub unavailable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
/// ANALYZE 选项：样本数、采样率、桶数、TopN。
pub enum analyzeOptionType {
    NumSamples,
    SampleRate,
    NumBuckets,
    NumTopN,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 列 ID（用于持久化 analyze_options）。
pub struct columnInfo {
    pub id: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// v2 分析选项：物理表 ID、原始选项与列选择。
pub struct v2AnalyzeOptions {
    pub physicalTableID: i64,
    pub isPartition: bool,
    pub rawOptions: BTreeMap<analyzeOptionType, u64>,
    pub resetOptions: BTreeSet<analyzeOptionType>,
    pub columnChoice: String,
    pub columnList: Vec<columnInfo>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 逻辑表 ID 与可选分区 ID。
pub struct analyzeTableID {
    pub tableID: i64,
    pub partitionID: i64,
}

impl analyzeTableID {
    /// 是否针对分区（partitionID != 0）。
    pub fn isPartitionTable(self) -> bool {
        self.partitionID != 0
    }

    /// 统计落盘使用的物理 ID（分区优先）。
    pub fn statisticsID(self) -> i64 {
        if self.isPartitionTable() {
            self.partitionID
        } else {
            self.tableID
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 后台可观测的 ANALYZE 作业描述。
pub struct analyzeJob {
    pub id: u64,
    pub databaseName: String,
    pub tableName: String,
    pub partitionName: String,
    pub jobInfo: String,
    pub sampleRateReason: String,
}

/// 列收集器内存追踪：分析结束后 detach。
pub trait memoryTracker: Send + Sync {
    fn detach(&self);
}

#[derive(Clone)]
/// 列分析任务执行句柄。
pub struct analyzeColumnsExec {
    pub tableID: analyzeTableID,
    pub samplingStatsConcurrency: Arc<AtomicUsize>,
    pub memTracker: Option<Arc<dyn memoryTracker>>,
}

#[derive(Clone, Debug)]
/// 索引分析任务执行句柄。
pub struct analyzeIndexExec {
    pub tableID: analyzeTableID,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 任务类型：列或索引。
pub enum taskType {
    colTask,
    idxTask,
}

/// 列任务类型常量别名。
pub const colTask: taskType = taskType::colTask;
/// 索引任务类型常量别名。
pub const idxTask: taskType = taskType::idxTask;

#[derive(Clone)]
/// 待调度的分析任务（列或索引执行器 + 作业）。
pub struct analyzeTask {
    pub taskType: taskType,
    pub idxExec: Option<analyzeIndexExec>,
    pub colExec: Option<analyzeColumnsExec>,
    pub job: Option<analyzeJob>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 结果中的直方图标识（此处仅保留 ID）。
pub struct histogram {
    pub id: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 结果分段：列或索引直方图集合。
pub struct analyzeResultPart {
    pub isIndex: i32,
    pub histograms: Vec<Option<histogram>>,
}

#[derive(Clone, Debug, Default)]
/// 单任务分析结果：错误、作业、表 ID、版本与分段。
pub struct analyzeResults {
    pub error: Option<AnalyzeError>,
    pub job: Option<analyzeJob>,
    pub tableID: analyzeTableID,
    pub statsVersion: i32,
    pub parts: Vec<analyzeResultPart>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
/// 全局统计映射键：表 ID + 索引/列 ID。
pub struct globalStatsKey {
    pub tableID: i64,
    pub indexID: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 待合并全局统计的元信息。
pub struct globalStatsInfo {
    pub isIndex: i32,
    pub histogramIDs: Vec<i64>,
    pub statsVersion: i32,
}

/// 分区表全局统计待办映射。
pub type globalStatsMap = BTreeMap<globalStatsKey, globalStatsInfo>;

/// ANALYZE 执行依赖的运行时边界（广播、落盘、作业、指标等）。
pub trait analyzeRuntime: Send + Sync {
    fn broadcast(&self, context: &analyzeContext, sql: &str) -> AnalyzeResultValue;
    fn append_warning(&self, warning: AnalyzeError);
    fn all_server_rpc_info(
        &self,
        context: &analyzeContext,
    ) -> AnalyzeResultValue<Vec<serverRPCInfo>>;
    fn dump_stats_delta(&self, target_ids: &[i64]) -> AnalyzeResultValue;
    fn locked_table_ids(&self, ids: &[i64]) -> AnalyzeResultValue<BTreeSet<i64>>;
    fn describe_table_or_partition(&self, table_id: analyzeTableID) -> Option<String>;
    fn build_stats_concurrency(&self) -> AnalyzeResultValue<usize>;
    fn sampling_stats_concurrency(&self) -> AnalyzeResultValue<usize>;
    fn save_stats_concurrency(&self) -> usize;
    fn dynamic_partition_prune(&self) -> bool;
    fn in_restricted_sql(&self) -> bool;
    fn persist_analyze_options(&self) -> bool;
    fn analyze_snapshot_enabled(&self) -> bool;
    fn kill_signal(&self) -> Arc<dyn killSignal>;
    fn prepare_columns_job(&self, task: &analyzeTask);
    fn insert_analyze_job(&self, job: &analyzeJob) -> AnalyzeResultValue;
    fn start_analyze_job(&self, job: Option<&analyzeJob>);
    fn finish_analyze_job(&self, job: Option<&analyzeJob>, error: Option<&AnalyzeError>);
    fn analyze_columns(
        &self,
        context: &analyzeContext,
        executor: &analyzeColumnsExec,
    ) -> analyzeResults;
    fn analyze_index(
        &self,
        context: &analyzeContext,
        executor: &analyzeIndexExec,
    ) -> analyzeResults;
    fn check_killed(&self) -> AnalyzeResultValue;
    fn save_analyze_result(
        &self,
        result: &analyzeResults,
        enable_snapshot: bool,
    ) -> AnalyzeResultValue;
    fn historical_stats_enabled(&self) -> AnalyzeResultValue<bool>;
    fn enqueue_historical_stats(&self, table_id: i64);
    fn merge_global_stats(&self, map: &globalStatsMap) -> AnalyzeResultValue;
    fn execute_internal_sql(&self, sql: &str) -> AnalyzeResultValue;
    fn update_stats(&self, table_and_partition_ids: &[i64]) -> AnalyzeResultValue;
    fn manual_analyze_metric(&self, label: &str);
    fn log_warning(&self, message: &str);
}

/// 分析执行器基础配置（并发、选项、快照等）。
pub struct baseAnalyzeExec {
    pub runtime: Arc<dyn analyzeRuntime>,
    pub tableID: analyzeTableID,
    pub concurrency: usize,
    pub analyzeRequest: Vec<u8>,
    pub opts: BTreeMap<analyzeOptionType, u64>,
    pub job: Option<analyzeJob>,
    pub snapshot: u64,
}

/// ANALYZE 语句主执行器：调度 worker、汇总结果并更新统计。
pub struct AnalyzeExec {
    pub tasks: Vec<Arc<analyzeTask>>,
    pub opts: BTreeMap<analyzeOptionType, u64>,
    pub OptionsMap: BTreeMap<i64, v2AnalyzeOptions>,
    pub errExitCh: Arc<AtomicBool>,
    pub runtime: Arc<dyn analyzeRuntime>,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 规范路径下一次提交的统计批次（profiles + jobs）。
pub struct CanonicalAnalyzeBatch {
    pub version: u64,
    pub profiles: Vec<astersql_statistics_handle::TableStats>,
    pub jobs: Vec<astersql_statistics_handle::RuntimeAnalyzeJob>,
}

/// Production boundary shared by SQL sessions and the analyze executor. The
/// executor owns ordering and failure semantics; implementations only provide
/// decoded rows/catalog state and the canonical statistics handle commit.
pub trait CanonicalAnalyzeRuntime: Sync {
    fn check_killed(&self) -> AnalyzeResultValue;
    fn preflush_stats_delta(&self, context: &analyzeContext) -> AnalyzeResultValue;
    fn prepare_batch(&self) -> AnalyzeResultValue<CanonicalAnalyzeBatch>;
    fn save_batch(&self, batch: &CanonicalAnalyzeBatch) -> AnalyzeResultValue;
    fn publish_batch(&self, batch: CanonicalAnalyzeBatch) -> AnalyzeResultValue;
    fn record_failure(&self, error: &AnalyzeError);
}

/// 是否运行在 Rust `cfg!(test)`（对应 Go 测试分支行为）。
pub fn runningUnderGoTest() -> bool {
    cfg!(test)
}

/// 生成 `FLUSH STATS_DELTA ... CLUSTER` SQL。
fn restore_flush_sql(objects: &[statsObject]) -> String {
    let objects = objects
        .iter()
        .map(|object| format!("`{}`.`{}`", object.databaseName, object.tableName))
        .collect::<Vec<_>>()
        .join(",");
    format!("FLUSH STATS_DELTA {objects} CLUSTER")
}

/// ANALYZE 前尝试集群预刷 stats delta；测试环境可走本地 dump。
pub fn flushStatsDeltaForAnalyze(
    context: &analyzeContext,
    runtime: &dyn analyzeRuntime,
    plan: &analyzePlan,
) -> AnalyzeResultValue {
    let objects = collectStatsDeltaFlushObjectsForAnalyze(plan);
    if objects.is_empty() {
        return Ok(());
    }
    if let Some(error) = context.error() {
        return Err(error);
    }
    if runningUnderGoTest() && flushAnalyzeStatsDeltaForTest(context, runtime, plan)? {
        return Ok(());
    }
    tryBroadcast(context, runtime, &restore_flush_sql(&objects))
}

/// 广播 SQL；若对端不支持该 exec 类型则降级为警告并继续。
pub fn tryBroadcast(
    context: &analyzeContext,
    runtime: &dyn analyzeRuntime,
    sql: &str,
) -> AnalyzeResultValue {
    match runtime.broadcast(context, sql) {
        Ok(()) => Ok(()),
        Err(error) if isUnsupportedBroadcastQueryErr(&error) => {
            runtime.log_warning(
                "FLUSH STATS_DELTA CLUSTER broadcast rejected by a peer TiDB during analyze; proceeding without the cluster-wide pre-analyze flush",
            );
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// 识别对端尚不支持某 exec 类型的广播错误。
pub fn isUnsupportedBroadcastQueryErr(error: &AnalyzeError) -> bool {
    error.0.contains("exec type") && error.0.contains("doesn't support yet")
}

/// 从计划列任务收集去重后的库表对象。
pub fn collectStatsDeltaFlushObjectsForAnalyze(plan: &analyzePlan) -> Vec<statsObject> {
    let mut seen = BTreeSet::new();
    let mut objects = Vec::new();
    for task in &plan.columnTasks {
        if task.databaseName.is_empty() || task.tableName.is_empty() {
            continue;
        }
        let object = statsObject {
            databaseName: task.databaseName.clone(),
            tableName: task.tableName.clone(),
        };
        if seen.insert(object.clone()) {
            objects.push(object);
        }
    }
    objects
}

/// 测试路径：不可广播时按表/分区 ID dump stats delta。
pub fn flushAnalyzeStatsDeltaForTest(
    context: &analyzeContext,
    runtime: &dyn analyzeRuntime,
    plan: &analyzePlan,
) -> AnalyzeResultValue<bool> {
    if canBroadcastAnalyzeStatsDeltaForTest(context, runtime)? {
        return Ok(false);
    }
    let target_ids = collectAnalyzeStatsDeltaTargetIDsForTest(plan);
    if target_ids.is_empty() {
        return Ok(false);
    }
    runtime.dump_stats_delta(&target_ids)?;
    Ok(true)
}

/// 测试路径：根据集群 RPC 信息判断是否可广播。
pub fn canBroadcastAnalyzeStatsDeltaForTest(
    context: &analyzeContext,
    runtime: &dyn analyzeRuntime,
) -> AnalyzeResultValue<bool> {
    let addresses = runtime
        .all_server_rpc_info(context)?
        .into_iter()
        .filter(|server| !server.unavailable)
        .map(|server| {
            if server.ip.is_empty() {
                String::new()
            } else {
                format!("{}:{}", server.ip, server.statusPort)
            }
        })
        .collect::<Vec<_>>();
    Ok(canBroadcastToTiDBRPCForTest(context, &addresses))
}

/// 全部地址可连通时才允许广播。
pub fn canBroadcastToTiDBRPCForTest(context: &analyzeContext, addresses: &[String]) -> bool {
    !addresses.is_empty()
        && addresses
            .iter()
            .all(|address| isTiDBRPCReachableForTest(context, address))
}

/// 短超时 TCP 探测 TiDB status 端口是否可达。
pub fn isTiDBRPCReachableForTest(context: &analyzeContext, address: &str) -> bool {
    if address.is_empty() || context.error().is_some() {
        return false;
    }
    let Ok(address) = address.parse::<SocketAddr>() else {
        return false;
    };
    TcpStream::connect_timeout(&address, Duration::from_millis(50)).is_ok()
}

/// 收集计划中表与分区的物理 ID（去重）。
pub fn collectAnalyzeStatsDeltaTargetIDsForTest(plan: &analyzePlan) -> Vec<i64> {
    let mut seen = BTreeSet::new();
    let mut ids = Vec::new();
    for task in &plan.columnTasks {
        let Some(table) = &task.tableInfo else {
            continue;
        };
        if seen.insert(table.id) {
            ids.push(table.id);
        }
        for partition in &table.partitions {
            if seen.insert(partition.id) {
                ids.push(partition.id);
            }
        }
    }
    ids
}

impl AnalyzeExec {
    /// 使用默认上下文运行规范 ANALYZE 批次路径。
    pub fn RunCanonical(runtime: &dyn CanonicalAnalyzeRuntime) -> AnalyzeResultValue {
        Self::RunCanonicalWithContext(analyzeContext::default(), runtime)
    }

    /// 预刷 delta、准备批次、并发保存并发布；失败记入 runtime。
    pub fn RunCanonicalWithContext(
        context: analyzeContext,
        runtime: &dyn CanonicalAnalyzeRuntime,
    ) -> AnalyzeResultValue {
        if let Some(error) = context.error() {
            return Err(error);
        }
        runtime.check_killed()?;
        runtime.preflush_stats_delta(&context)?;
        if let Some(error) = context.error() {
            return Err(error);
        }
        runtime.check_killed()?;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            handleAnalyzeWorkerPanic();
            let batch = runtime.prepare_batch()?;
            runtime.check_killed()?;
            thread::scope(|scope| {
                scope
                    .spawn(|| {
                        handleResultsErrorSingleThreadPanic();
                        runtime.save_batch(&batch)
                    })
                    .join()
            })
            .map_err(|payload| getAnalyzePanicErr(payload.as_ref()))??;
            runtime.check_killed()?;
            runtime.publish_batch(batch)
        }))
        .unwrap_or_else(|payload| Err(getAnalyzePanicErr(payload.as_ref())));
        if let Err(error) = &result {
            runtime.record_failure(error);
        }
        result
    }

    /// 执行全部任务；结束后 detach 内存 tracker 并上报成功/失败指标。
    pub fn Next(&mut self, parent: analyzeContext) -> AnalyzeResultValue {
        let restricted = self.runtime.in_restricted_sql();
        let result = self.next_inner(parent);
        for task in &self.tasks {
            if let Some(tracker) = task
                .colExec
                .as_ref()
                .and_then(|executor| executor.memTracker.as_ref())
            {
                tracker.detach();
            }
        }
        if !restricted {
            self.runtime
                .manual_analyze_metric(if result.is_ok() { "succ" } else { "failed" });
        }
        result
    }

    // 过滤锁表后启动构建 worker 与结果处理，再合并全局统计并持久化选项。
    /// ANALYZE 主循环：并发构建、处理结果、可选全局合并与 update_stats。
    fn next_inner(&mut self, parent: analyzeContext) -> AnalyzeResultValue {
        let (context, _stop) = self.buildAnalyzeKillCtx(parent);
        let (tasks, analyze_count, skipped) =
            filterAndCollectTasks(&self.tasks, self.runtime.as_ref())?;
        warnLockedTableMsg(self.runtime.as_ref(), analyze_count, &skipped);
        if tasks.is_empty() {
            return Ok(());
        }

        let mut table_ids = Vec::with_capacity(tasks.len() * 2);
        for task in &tasks {
            let table_id = getTableIDFromTask(task);
            table_ids.push(table_id.tableID);
            if table_id.isPartitionTable() {
                table_ids.push(table_id.partitionID);
            }
        }
        let build_concurrency = self.runtime.build_stats_concurrency()?.min(tasks.len());
        if build_concurrency == 0 {
            return Err(AnalyzeError(
                "build statistics concurrency must be greater than zero".into(),
            ));
        }
        let sampling_concurrency = self.runtime.sampling_stats_concurrency()?;
        for task in &tasks {
            if let Some(executor) = &task.colExec {
                executor
                    .samplingStatsConcurrency
                    .store(sampling_concurrency, Ordering::Release);
            }
        }

        for task in &tasks {
            self.runtime.prepare_columns_job(task);
            AddNewAnalyzeJob(self.runtime.as_ref(), task.job.as_ref());
        }
        self.errExitCh.store(false, Ordering::Release);
        let need_global_stats = self.runtime.dynamic_partition_prune();
        let global_stats = Arc::new(Mutex::new(globalStatsMap::new()));
        let (task_sender, task_receiver) = mpsc::sync_channel(build_concurrency);
        let task_receiver = Arc::new(Mutex::new(task_receiver));
        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        let mut sent_tasks = 0;
        let task_num = tasks.len();
        let exec: &AnalyzeExec = self;

        let (worker_errors, handler_error) = thread::scope(|scope| {
            let mut workers = Vec::new();
            for _ in 0..build_concurrency {
                let receiver = task_receiver.clone();
                let sender = result_sender.clone();
                let context = context.clone();
                workers.push(scope.spawn(move || exec.analyzeWorker(&context, receiver, sender)));
            }
            drop(result_sender);
            let handler_map = global_stats.clone();
            let handler = scope.spawn(move || {
                exec.handleResultsError(
                    build_concurrency,
                    need_global_stats,
                    handler_map,
                    result_receiver,
                    task_num,
                )
            });

            for task in &tasks {
                if analyzeWorkerExitErr(&context, &exec.errExitCh).is_err() {
                    break;
                }
                let mut pending = task.clone();
                loop {
                    match task_sender.try_send(pending) {
                        Ok(()) => {
                            sent_tasks += 1;
                            break;
                        }
                        Err(mpsc::TrySendError::Full(task)) => {
                            pending = task;
                            if analyzeWorkerExitErr(&context, &exec.errExitCh).is_err() {
                                break;
                            }
                            thread::yield_now();
                        }
                        Err(mpsc::TrySendError::Disconnected(_)) => break,
                    }
                }
            }
            drop(task_sender);
            let worker_errors = workers
                .into_iter()
                .map(|worker| {
                    worker
                        .join()
                        .unwrap_or_else(|payload| Err(getAnalyzePanicErr(payload.as_ref())))
                })
                .collect::<Vec<_>>();
            let handler_error = handler
                .join()
                .unwrap_or_else(|payload| Err(getAnalyzePanicErr(payload.as_ref())));
            (worker_errors, handler_error)
        });

        let completion = self.waitFinish(worker_errors, handler_error);
        if let Err(error) = completion {
            if let Ok(receiver) = task_receiver.lock() {
                for task in receiver.try_iter() {
                    finishJobWithLog(self.runtime.as_ref(), task.job.as_ref(), Some(&error));
                }
            }
            for task in tasks.iter().skip(sent_tasks) {
                finishJobWithLog(self.runtime.as_ref(), task.job.as_ref(), Some(&error));
            }
            return Err(error);
        }
        if let Some(error) = context.error() {
            return Err(error);
        }
        if need_global_stats {
            self.runtime.merge_global_stats(
                &global_stats
                    .lock()
                    .expect("global statistics mutex poisoned"),
            )?;
        }
        if let Err(error) = self.saveAnalyzeOptions() {
            self.runtime.append_warning(error);
        }
        self.runtime.update_stats(&table_ids)
    }

    /// 汇总 handler 与 worker 错误；handler 失败时置 errExitCh。
    pub fn waitFinish(
        &self,
        workerResults: Vec<AnalyzeResultValue>,
        handlerResult: AnalyzeResultValue,
    ) -> AnalyzeResultValue {
        if let Err(error) = handlerResult {
            self.errExitCh.store(true, Ordering::Release);
            return Err(error);
        }
        for result in workerResults {
            result?;
        }
        Ok(())
    }

    /// 将 v2 选项 REPLACE 写入 `mysql.analyze_options`。
    pub fn saveAnalyzeOptions(&self) -> AnalyzeResultValue {
        if !self.runtime.persist_analyze_options() || self.OptionsMap.is_empty() {
            return Ok(());
        }
        let dynamic = self.runtime.dynamic_partition_prune();
        let mut reset_options = BTreeSet::new();
        let mut partition_ids = Vec::new();
        let options = self
            .OptionsMap
            .values()
            .filter(|options| {
                if dynamic && options.isPartition {
                    partition_ids.push(options.physicalTableID);
                    false
                } else {
                    reset_options.extend(options.resetOptions.iter().copied());
                    true
                }
            })
            .collect::<Vec<_>>();
        if options.is_empty() {
            return Ok(());
        }
        let mut values = Vec::with_capacity(options.len());
        for option in options {
            let saved_value = |option_type, transform: fn(u64) -> String| {
                option
                    .rawOptions
                    .get(&option_type)
                    .copied()
                    .map(transform)
                    .unwrap_or_else(|| "DEFAULT".into())
            };
            let sample_num = saved_value(analyzeOptionType::NumSamples, |value| value.to_string());
            let sample_rate = saved_value(analyzeOptionType::SampleRate, |value| {
                f64::from_bits(value).to_string()
            });
            let buckets = saved_value(analyzeOptionType::NumBuckets, |value| value.to_string());
            let topn = saved_value(analyzeOptionType::NumTopN, |value| value.to_string());
            let column_ids = option
                .columnList
                .iter()
                .map(|column| column.id.to_string())
                .collect::<Vec<_>>()
                .join(",");
            values.push(format!(
                "({},{sample_num},{sample_rate},{buckets},{topn},'{}','{column_ids}')",
                option.physicalTableID,
                option.columnChoice.replace('\'', "''")
            ));
        }
        let sql = format!(
            "REPLACE INTO mysql.analyze_options (table_id,sample_num,sample_rate,buckets,topn,column_choice,column_ids) VALUES {}",
            values.join(",")
        );
        self.runtime.execute_internal_sql(&sql)?;
        if dynamic && !reset_options.is_empty() && !partition_ids.is_empty() {
            let assignments = [
                (analyzeOptionType::NumSamples, "sample_num"),
                (analyzeOptionType::SampleRate, "sample_rate"),
                (analyzeOptionType::NumBuckets, "buckets"),
                (analyzeOptionType::NumTopN, "topn"),
            ]
            .into_iter()
            .filter(|(option_type, _)| reset_options.contains(option_type))
            .map(|(_, column)| format!("{column}=DEFAULT"))
            .collect::<Vec<_>>()
            .join(",");
            let ids = partition_ids
                .into_iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",");
            self.runtime.execute_internal_sql(&format!(
                "UPDATE mysql.analyze_options SET {assignments} WHERE table_id IN ({ids})"
            ))?;
        }
        Ok(())
    }

    /// 捕获 panic 地处理结果通道，并按 save 并发落盘。
    pub fn handleResultsError(
        &self,
        buildStatsConcurrency: usize,
        needGlobalStats: bool,
        globalStatsMap: Arc<Mutex<globalStatsMap>>,
        resultsCh: mpsc::Receiver<Arc<analyzeResults>>,
        taskNum: usize,
    ) -> AnalyzeResultValue {
        let save_concurrency = self.runtime.save_stats_concurrency().min(taskNum).max(1);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if save_concurrency == 1 {
                handleResultsErrorSingleThreadPanic();
            }
            self.handleResultsErrorWithConcurrency(
                buildStatsConcurrency,
                save_concurrency,
                needGlobalStats,
                globalStatsMap,
                resultsCh,
            )
        }))
        .unwrap_or_else(|payload| Err(getAnalyzePanicErr(payload.as_ref())));
        if result.is_err() {
            self.errExitCh.store(true, Ordering::Release);
        }
        result
    }

    /// 启动 save worker，合并全局统计键，记录历史统计并汇总错误。
    pub fn handleResultsErrorWithConcurrency(
        &self,
        buildStatsConcurrency: usize,
        saveStatsConcurrency: usize,
        needGlobalStats: bool,
        globalStatsMap: Arc<Mutex<globalStatsMap>>,
        resultsCh: mpsc::Receiver<Arc<analyzeResults>>,
    ) -> AnalyzeResultValue {
        let (save_sender, save_receiver) =
            mpsc::sync_channel::<Arc<analyzeResults>>(saveStatsConcurrency);
        let save_receiver = Arc::new(Mutex::new(save_receiver));
        let (error_sender, error_receiver) = mpsc::channel();
        let table_ids = Arc::new(Mutex::new(BTreeSet::new()));
        let mut analysis_error = None;

        thread::scope(|scope| {
            for _ in 0..saveStatsConcurrency {
                let receiver = save_receiver.clone();
                let errors = error_sender.clone();
                scope.spawn(move || {
                    loop {
                        let result = receiver
                            .lock()
                            .expect("save result receiver mutex poisoned")
                            .recv();
                        let Ok(result) = result else {
                            break;
                        };
                        let save = self.runtime.save_analyze_result(
                            result.as_ref(),
                            self.runtime.analyze_snapshot_enabled(),
                        );
                        match save {
                            Ok(()) => {
                                finishJobWithLog(self.runtime.as_ref(), result.job.as_ref(), None)
                            }
                            Err(error) => {
                                finishJobWithLog(
                                    self.runtime.as_ref(),
                                    result.job.as_ref(),
                                    Some(&error),
                                );
                                let _ = errors.send(error);
                            }
                        }
                    }
                });
            }
            drop(error_sender);
            let mut panic_count = 0;
            while panic_count < buildStatsConcurrency {
                if let Err(error) = self.runtime.check_killed() {
                    analysis_error = Some(error);
                    break;
                }
                let Ok(result) = resultsCh.recv() else {
                    break;
                };
                if let Some(error) = &result.error {
                    analysis_error = Some(error.clone());
                    if isAnalyzeWorkerPanic(error) {
                        panic_count += 1;
                    }
                    finishJobWithLog(self.runtime.as_ref(), result.job.as_ref(), Some(error));
                    continue;
                }
                handleGlobalStats(
                    needGlobalStats,
                    &mut globalStatsMap
                        .lock()
                        .expect("global statistics mutex poisoned"),
                    result.as_ref(),
                );
                table_ids
                    .lock()
                    .expect("historical table IDs mutex poisoned")
                    .insert(result.tableID.statisticsID());
                if save_sender.send(result).is_err() {
                    analysis_error = Some(AnalyzeError("save result workers exited".into()));
                    break;
                }
            }
            drop(save_sender);
        });

        let save_errors = error_receiver
            .try_iter()
            .map(|error| error.0)
            .collect::<BTreeSet<_>>();
        for table_id in table_ids
            .lock()
            .expect("historical table IDs mutex poisoned")
            .iter()
            .copied()
        {
            if let Err(error) = recordHistoricalStats(self.runtime.as_ref(), table_id) {
                self.runtime
                    .log_warning(&format!("record historical stats failed: {error}"));
            }
        }
        if !save_errors.is_empty() {
            return Err(AnalyzeError(
                save_errors.into_iter().collect::<Vec<_>>().join(","),
            ));
        }
        analysis_error.map_or(Ok(()), Err)
    }

    /// 创建子上下文并后台监听 kill 信号。
    pub fn buildAnalyzeKillCtx(&self, parent: analyzeContext) -> (analyzeContext, analyzeStop) {
        let context = analyzeContext::child(parent);
        let watcher_context = context.clone();
        let signal = self.runtime.kill_signal();
        thread::spawn(move || {
            if let Some(error) = signal.wait(&watcher_context) {
                watcher_context.cancel(error);
            }
        });
        (context.clone(), analyzeStop { context })
    }

    /// 非阻塞发送结果；退出时用上下文/kill 错误收尾作业。
    pub fn trySendAnalyzeResult(
        &self,
        context: &analyzeContext,
        resultsCh: &mpsc::SyncSender<Arc<analyzeResults>>,
        result: analyzeResults,
    ) {
        let job = result.job.clone();
        let mut pending = Arc::new(result);
        loop {
            if analyzeWorkerExitErr(context, &self.errExitCh).is_err() {
                break;
            }
            match resultsCh.try_send(pending) {
                Ok(()) => return,
                Err(mpsc::TrySendError::Full(result)) => {
                    pending = result;
                    thread::yield_now();
                }
                Err(mpsc::TrySendError::Disconnected(_)) => break,
            }
        }
        let error = context
            .error()
            .or_else(|| self.runtime.kill_signal().current_error())
            .unwrap_or_else(|| AnalyzeError("query interrupted".into()));
        finishJobWithLog(self.runtime.as_ref(), job.as_ref(), Some(&error));
    }

    /// 从任务通道取任务，执行列/索引分析并发送结果；panic 时上报。
    pub fn analyzeWorker(
        &self,
        context: &analyzeContext,
        taskCh: Arc<Mutex<mpsc::Receiver<Arc<analyzeTask>>>>,
        resultsCh: mpsc::SyncSender<Arc<analyzeResults>>,
    ) -> AnalyzeResultValue {
        let current_job = Mutex::new(None::<analyzeJob>);
        let worker = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            loop {
                let task = taskCh
                    .lock()
                    .expect("analyze task receiver mutex poisoned")
                    .recv();
                let Ok(task) = task else {
                    break;
                };
                *current_job
                    .lock()
                    .expect("current analyze job mutex poisoned") = task.job.clone();
                if let Err(error) = analyzeWorkerExitErr(context, &self.errExitCh) {
                    finishJobWithLog(self.runtime.as_ref(), task.job.as_ref(), Some(&error));
                    break;
                }
                handleAnalyzeWorkerPanic();
                self.runtime.start_analyze_job(task.job.as_ref());
                let mut result = match task.taskType {
                    taskType::colTask => self.runtime.analyze_columns(
                        context,
                        task.colExec
                            .as_ref()
                            .expect("column analyze task requires column executor"),
                    ),
                    taskType::idxTask => self.runtime.analyze_index(
                        context,
                        task.idxExec
                            .as_ref()
                            .expect("index analyze task requires index executor"),
                    ),
                };
                if result.job.is_none() {
                    result.job = task.job.clone();
                }
                self.trySendAnalyzeResult(context, &resultsCh, result);
            }
        }));
        match worker {
            Ok(()) => Ok(()),
            Err(payload) => {
                let error = getAnalyzePanicErr(payload.as_ref());
                let _ = resultsCh.send(Arc::new(analyzeResults {
                    error: Some(error.clone()),
                    job: current_job
                        .lock()
                        .expect("current analyze job mutex poisoned")
                        .clone(),
                    ..Default::default()
                }));
                Err(error)
            }
        }
    }
}

/// 跳过已锁定表/分区，返回可执行任务、需分析计数与跳过名称。
pub fn filterAndCollectTasks(
    tasks: &[Arc<analyzeTask>],
    runtime: &dyn analyzeRuntime,
) -> AnalyzeResultValue<(Vec<Arc<analyzeTask>>, u32, Vec<String>)> {
    let locked = getLockedTableAndPartitionIDs(runtime, tasks)?;
    let mut filtered = Vec::new();
    let mut skipped = Vec::new();
    let mut need_analyze_count = 0;
    let mut visited = BTreeSet::new();
    for task in tasks {
        let table_id = getTableIDFromTask(task);
        let is_locked = locked.contains(&table_id.tableID)
            || (table_id.isPartitionTable() && locked.contains(&table_id.partitionID));
        if !is_locked {
            filtered.push(task.clone());
        }
        let physical_id = table_id.statisticsID();
        if !visited.insert(physical_id) {
            continue;
        }
        if is_locked {
            if let Some(name) = runtime.describe_table_or_partition(table_id) {
                skipped.push(name);
            }
        } else {
            need_analyze_count += 1;
        }
    }
    Ok((filtered, need_analyze_count, skipped))
}

/// 查询任务涉及的已锁定表/分区 ID 集合。
pub fn getLockedTableAndPartitionIDs(
    runtime: &dyn analyzeRuntime,
    tasks: &[Arc<analyzeTask>],
) -> AnalyzeResultValue<BTreeSet<i64>> {
    let mut ids = Vec::with_capacity(tasks.len() * 2);
    for task in tasks {
        let table_id = getTableIDFromTask(task);
        ids.push(table_id.tableID);
        if table_id.isPartitionTable() {
            ids.push(table_id.partitionID);
        }
    }
    runtime.locked_table_ids(&ids)
}

/// 对跳过的锁表追加警告信息。
pub fn warnLockedTableMsg(
    runtime: &dyn analyzeRuntime,
    needAnalyzeTableCnt: u32,
    skippedTables: &[String],
) {
    if skippedTables.is_empty() {
        return;
    }
    let tables = skippedTables.join(", ");
    let message = if skippedTables.len() > 1 && needAnalyzeTableCnt > 0 {
        format!("skip analyze locked tables: {tables}, other tables will be analyzed")
    } else if skippedTables.len() > 1 {
        format!("skip analyze locked tables: {tables}")
    } else {
        format!("skip analyze locked table: {tables}")
    };
    runtime.append_warning(AnalyzeError(message));
}

/// 从列或索引执行器取出 analyzeTableID。
pub fn getTableIDFromTask(task: &analyzeTask) -> analyzeTableID {
    match task.taskType {
        taskType::colTask => {
            task.colExec
                .as_ref()
                .expect("column analyze task requires column executor")
                .tableID
        }
        taskType::idxTask => {
            task.idxExec
                .as_ref()
                .expect("index analyze task requires index executor")
                .tableID
        }
    }
}

/// 若启用历史统计则入队记录。
pub fn recordHistoricalStats(runtime: &dyn analyzeRuntime, tableID: i64) -> AnalyzeResultValue {
    if runtime.historical_stats_enabled()? {
        runtime.enqueue_historical_stats(tableID);
    }
    Ok(())
}

/// 上下文取消或 errExitCh 置位时返回中断错误。
pub fn analyzeWorkerExitErr(
    context: &analyzeContext,
    errExitCh: &AtomicBool,
) -> AnalyzeResultValue {
    if let Some(error) = context.error() {
        return Err(error);
    }
    if errExitCh.load(Ordering::Acquire) {
        return Err(AnalyzeError("query interrupted".into()));
    }
    Ok(())
}

/// 插入分析作业；失败仅记警告。
pub fn AddNewAnalyzeJob(runtime: &dyn analyzeRuntime, job: Option<&analyzeJob>) {
    if let Some(job) = job
        && let Err(error) = runtime.insert_analyze_job(job)
    {
        runtime.log_warning(&format!("failed to insert analyze job: {error}"));
    }
}

/// 结束作业并在失败时打警告日志。
pub fn finishJobWithLog(
    runtime: &dyn analyzeRuntime,
    job: Option<&analyzeJob>,
    analyzeError: Option<&AnalyzeError>,
) {
    runtime.finish_analyze_job(job, analyzeError);
    if let Some(job) = job {
        let state = if analyzeError.is_some() {
            "failed"
        } else {
            "finished"
        };
        if let Some(error) = analyzeError {
            runtime.log_warning(&format!(
                "analyze table `{}.{}` has {state}: {error}",
                job.databaseName, job.tableName
            ));
        }
    }
}

/// 分区表结果写入全局统计映射（列 indexID=-1，索引按直方图 ID）。
pub fn handleGlobalStats(
    needGlobalStats: bool,
    globalStatsMap: &mut globalStatsMap,
    results: &analyzeResults,
) {
    if !needGlobalStats || !results.tableID.isPartitionTable() {
        return;
    }
    for result in &results.parts {
        if result.isIndex == 0 {
            let histogram_ids = result
                .histograms
                .iter()
                .filter_map(|histogram| histogram.as_ref().map(|histogram| histogram.id))
                .collect();
            globalStatsMap.insert(
                globalStatsKey {
                    tableID: results.tableID.tableID,
                    indexID: -1,
                },
                globalStatsInfo {
                    isIndex: result.isIndex,
                    histogramIDs: histogram_ids,
                    statsVersion: results.statsVersion,
                },
            );
        } else {
            for histogram in result.histograms.iter().flatten() {
                globalStatsMap.insert(
                    globalStatsKey {
                        tableID: results.tableID.tableID,
                        indexID: histogram.id,
                    },
                    globalStatsInfo {
                        isIndex: result.isIndex,
                        histogramIDs: vec![histogram.id],
                        statsVersion: results.statsVersion,
                    },
                );
            }
        }
    }
}
