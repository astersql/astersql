// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// ADMIN CHECK TABLE / CHECK INDEX：核对表数据与二级索引一致性。
//
// 慢路径通过 IndexLookUp 并发比对记录与索引；快路径（fast check）用 SQL
// checksum / group-by 在系统会话中抽样比对，并在发现桶差异时报告行不匹配。
#![allow(dead_code, non_camel_case_types, non_snake_case)]

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use astersql_errors as errors;
use astersql_util_chunk as chunk;

/// 检查结果类型别名。
pub type CheckResult<T = ()> = Result<T, errors::SharedError>;

/// 索引列定义（含生成列表达式）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexColumn {
    /// 列名。
    pub name: String,
    /// 生成列表达式；普通列为 None。
    pub generated_expression: Option<String>,
}

/// 待检查的索引元信息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexInfo {
    /// 索引 ID。
    pub id: i64,
    /// 索引名。
    pub name: String,
    /// 是否为多值（multi-valued）索引。
    pub mv_index: bool,
    /// 是否为列存索引。
    pub columnar_index: bool,
    /// 部分索引条件表达式。
    pub condition: Option<String>,
    /// 索引列列表。
    pub columns: Vec<IndexColumn>,
}

/// 表元信息：分区与主键 handle 列。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableMeta {
    /// 表 ID。
    pub id: i64,
    /// 表名。
    pub name: String,
    /// 分区物理表 ID 列表。
    pub partition_ids: Vec<i64>,
    /// common handle（聚簇索引）列名。
    pub common_handle_columns: Vec<String>,
    /// 整型 handle 列名（非聚簇时）。
    pub integer_handle_column: Option<String>,
}

/// 表行数与索引行数比较结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IndexCountComparison {
    /// 两侧计数相等。
    Equal,
    /// 索引侧更多。
    IndexGreater { index_offset: usize },
    /// 表侧更多。
    TableGreater { index_offset: usize },
}

/// IndexLookUp 执行器：按索引回表拉取批次。
pub trait IndexLookUpExecutor: Send {
    fn Open(&mut self) -> CheckResult;
    fn Close(&mut self) -> CheckResult;
    fn NextBatch(&mut self, max_rows: usize) -> CheckResult<usize>;
    fn Index(&self) -> &IndexInfo;
}

/// CHECK TABLE 运行时：计数比较、逐索引核对与失败日志。
pub trait CheckTableRuntime: Send + Sync {
    fn OpenBase(&self) -> CheckResult;
    fn InitCapacity(&self) -> usize;
    fn MaxChunkSize(&self) -> usize;
    fn CheckIndicesCount(
        &self,
        database: &str,
        table: &str,
        indexes: &[String],
    ) -> CheckResult<IndexCountComparison>;
    fn CheckRecordAndIndex(
        &self,
        table: &TableMeta,
        physical_table_id: i64,
        index: &IndexInfo,
    ) -> CheckResult;
    fn LogIndexCheckFailure(&self, index: &IndexInfo, error: &errors::SharedError);
}

/// ADMIN CHECK TABLE 执行器：并发检查各索引与表数据一致性。
pub struct CheckTableExec {
    /// 库名。
    pub dbName: String,
    /// 表元信息。
    pub table: TableMeta,
    /// 待检查索引列表。
    pub indexInfos: Vec<IndexInfo>,
    /// 各索引对应的 IndexLookUp 源。
    pub srcs: Vec<Arc<Mutex<Box<dyn IndexLookUpExecutor>>>>,
    /// 是否已完成检查。
    pub done: bool,
    /// 是否执行索引内容检查（相对仅计数）。
    pub checkIndex: bool,
    /// 协作退出标志。
    exitCh: Arc<AtomicBool>,
    /// 运行时依赖。
    runtime: Arc<dyn CheckTableRuntime>,
}

impl CheckTableExec {
    /// 构造 CheckTableExec。
    pub fn new(
        runtime: Arc<dyn CheckTableRuntime>,
        db_name: String,
        table: TableMeta,
        index_infos: Vec<IndexInfo>,
        sources: Vec<Box<dyn IndexLookUpExecutor>>,
        check_index: bool,
    ) -> Self {
        Self {
            dbName: db_name,
            table,
            indexInfos: index_infos,
            srcs: sources
                .into_iter()
                .map(|source| Arc::new(Mutex::new(source)))
                .collect(),
            done: false,
            checkIndex: check_index,
            exitCh: Arc::new(AtomicBool::new(false)),
            runtime,
        }
    }

    /// 打开：先比较索引计数，再并发核对各索引内容。
    pub fn Open(&mut self) -> CheckResult {
        self.runtime.OpenBase()?;
        for source in &self.srcs {
            source
                .lock()
                .map_err(|_| errors::New("check-table source lock poisoned while opening"))?
                .Open()?;
        }
        self.exitCh.store(false, Ordering::Release);
        self.done = false;
        Ok(())
    }

    /// 关闭并等待 worker，清理 IndexLookUp 源。
    pub fn Close(&mut self) -> CheckResult {
        self.exitCh.store(true, Ordering::Release);
        let mut first_error = None;
        for source in &self.srcs {
            let result = source
                .lock()
                .map_err(|_| errors::New("check-table source lock poisoned while closing"))
                .and_then(|mut source| source.Close());
            if first_error.is_none() {
                first_error = result.err();
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// 对单个索引启动 IndexLookUp 并核对记录与索引。
    pub fn checkTableIndexHandle(&self, index: &IndexInfo) -> CheckResult {
        for source in &self.srcs {
            let same_index = source
                .lock()
                .map_err(|_| errors::New("check-table source lock poisoned while matching"))?
                .Index()
                .name
                .eq_ignore_ascii_case(&index.name);
            if same_index {
                self.checkIndexHandle(source)?;
            }
        }
        Ok(())
    }

    /// 在给定物理表上核对索引 handle 与行数据。
    pub fn checkIndexHandle(
        &self,
        source: &Arc<Mutex<Box<dyn IndexLookUpExecutor>>>,
    ) -> CheckResult {
        loop {
            if self.exitCh.load(Ordering::Acquire) {
                return Ok(());
            }
            let rows = source
                .lock()
                .map_err(|_| errors::New("check-table source lock poisoned while reading"))?
                .NextBatch(self.runtime.MaxChunkSize().max(self.runtime.InitCapacity()))?;
            if rows == 0 {
                return Ok(());
            }
        }
    }

    /// 将 worker panic 转为错误并记录索引名。
    pub fn handlePanic(&self, panic: &(dyn std::any::Any + Send)) -> errors::SharedError {
        let message = panic
            .downcast_ref::<&str>()
            .map(|value| (*value).to_owned())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown check-table worker panic".to_owned());
        errors::New(message)
    }

    /// 检查型执行器无结果行；完成后返回空。
    pub fn Next(&mut self, _request: &mut chunk::Chunk) -> CheckResult {
        if self.done || self.srcs.is_empty() {
            return Ok(());
        }
        self.done = true;

        let mut index_names = Vec::with_capacity(self.indexInfos.len());
        for index in &self.indexInfos {
            if index.condition.is_some() {
                return Err(errors::New(
                    "ADMIN CHECK TABLE without fast-check does not support partial indexes",
                ));
            }
            if index.mv_index || index.columnar_index {
                continue;
            }
            index_names.push(index.name.clone());
        }

        match self
            .runtime
            .CheckIndicesCount(&self.dbName, &self.table.name, &index_names)
        {
            Ok(IndexCountComparison::Equal) => {}
            Ok(IndexCountComparison::IndexGreater { index_offset }) => {
                if self.checkIndex {
                    return Err(errors::New("index count is greater than table count"));
                }
                return self.checkTableIndexHandle(self.index_at(index_offset)?);
            }
            Ok(IndexCountComparison::TableGreater { index_offset }) => {
                if self.checkIndex {
                    return Err(errors::New("table count is greater than index count"));
                }
                return self.checkTableRecord(index_offset);
            }
            Err(error) => return Err(error),
        }

        if self.srcs.len() == 1 {
            self.checkIndexHandle(&self.srcs[0])?;
            if self.source_index(&self.srcs[0])?.mv_index {
                self.checkTableRecord(0)?;
            }
        }

        let tasks = Arc::new(Mutex::new((0..self.srcs.len()).collect::<VecDeque<_>>()));
        let failed = Arc::new(AtomicBool::new(false));
        let (error_sender, error_receiver) = mpsc::channel();
        let concurrency = self.srcs.len().min(3);
        let executor: &CheckTableExec = self;
        thread::scope(|scope| {
            for _ in 0..concurrency {
                let tasks = Arc::clone(&tasks);
                let failed = Arc::clone(&failed);
                let error_sender = error_sender.clone();
                scope.spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        loop {
                            if failed.load(Ordering::Acquire)
                                || executor.exitCh.load(Ordering::Acquire)
                            {
                                return Ok(());
                            }
                            let source_offset = tasks
                                .lock()
                                .map_err(|_| errors::New("check-table task queue lock poisoned"))?
                                .pop_front();
                            let Some(source_offset) = source_offset else {
                                return Ok(());
                            };
                            let source = &executor.srcs[source_offset];
                            if let Err(error) = executor.checkIndexHandle(source) {
                                let index = executor.source_index(source).ok();
                                if let Some(index) = index {
                                    executor.runtime.LogIndexCheckFailure(&index, &error);
                                }
                                return Err(error);
                            }
                            let index = executor.source_index(source)?;
                            if index.mv_index {
                                if let Some(index_offset) = executor
                                    .indexInfos
                                    .iter()
                                    .position(|candidate| candidate.id == index.id)
                                {
                                    executor.checkTableRecord(index_offset)?;
                                }
                            }
                        }
                    }));
                    let error = match result {
                        Ok(Ok(())) => return,
                        Ok(Err(error)) => error,
                        Err(panic) => executor.handlePanic(panic.as_ref()),
                    };
                    failed.store(true, Ordering::Release);
                    let _ = error_sender.send(error);
                });
            }
        });
        drop(error_sender);
        error_receiver.try_recv().map_or(Ok(()), Err)
    }

    /// 按索引偏移核对表记录与索引一致性。
    pub fn checkTableRecord(&self, index_offset: usize) -> CheckResult {
        let index = self.index_at(index_offset)?;
        if self.table.partition_ids.is_empty() {
            return self
                .runtime
                .CheckRecordAndIndex(&self.table, self.table.id, index);
        }
        for partition_id in &self.table.partition_ids {
            self.runtime
                .CheckRecordAndIndex(&self.table, *partition_id, index)?;
        }
        Ok(())
    }

    /// 按偏移取索引元信息。
    fn index_at(&self, offset: usize) -> CheckResult<&IndexInfo> {
        self.indexInfos
            .get(offset)
            .ok_or_else(|| errors::New(format!("index offset {offset} is out of range")))
    }

    /// 按偏移取 IndexLookUp 源及其索引。
    fn source_index(
        &self,
        source: &Arc<Mutex<Box<dyn IndexLookUpExecutor>>>,
    ) -> CheckResult<IndexInfo> {
        Ok(source
            .lock()
            .map_err(|_| errors::New("check-table source lock poisoned while reading metadata"))?
            .Index()
            .clone())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]

/// 会话变量快照（快路径系统会话覆盖用）。
pub struct SessionVars {
    pub optimizerUseInvisibleIndexes: bool,
    pub memQuotaQuery: i64,
    pub distSQLScanConcurrency: i32,
    pub executorConcurrency: i32,
    pub maxExecutionTime: u64,
    pub tikvClientReadTimeout: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]

/// SQL 查询单元格值。
pub enum SqlValue {
    Null,
    Int64(i64),
    Uint64(u64),
    String(String),
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]

/// 一行查询结果。
pub struct QueryRow {
    pub values: Vec<SqlValue>,
}

/// 从查询行提取类型化字段。
impl QueryRow {
    /// 取单元格原始 SqlValue。
    fn value(&self, offset: usize) -> CheckResult<&SqlValue> {
        self.values
            .get(offset)
            .ok_or_else(|| errors::New(format!("SQL row column {offset} is missing")))
    }

    /// 取字符串列。
    pub fn GetString(&self, offset: usize) -> CheckResult<&str> {
        match self.value(offset)? {
            SqlValue::String(value) => Ok(value),
            _ => Err(errors::New(format!("SQL row column {offset} is not text"))),
        }
    }

    /// 取无符号整数列。
    pub fn GetUint64(&self, offset: usize) -> CheckResult<u64> {
        match self.value(offset)? {
            SqlValue::Uint64(value) => Ok(*value),
            SqlValue::Int64(value) if *value >= 0 => Ok(*value as u64),
            _ => Err(errors::New(format!(
                "SQL row column {offset} is not an unsigned integer"
            ))),
        }
    }

    /// 取有符号整数列。
    pub fn GetInt64(&self, offset: usize) -> CheckResult<i64> {
        match self.value(offset)? {
            SqlValue::Int64(value) => Ok(*value),
            SqlValue::Uint64(value) => i64::try_from(*value)
                .map_err(|_| errors::New(format!("SQL row column {offset} overflows i64"))),
            _ => Err(errors::New(format!(
                "SQL row column {offset} is not an integer"
            ))),
        }
    }

    /// 判断单元格是否为 SQL NULL。
    pub fn IsNull(&self, offset: usize) -> CheckResult<bool> {
        Ok(matches!(self.value(offset)?, SqlValue::Null))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]

/// 不匹配时报告的行数据。
pub struct RecordData {
    pub handle: Vec<u8>,
    pub values: Vec<SqlValue>,
    pub checksum: u64,
}

/// 快路径会话：执行 SQL、取变量、写警告。
pub trait FastCheckSession: Send {
    fn Variables(&self) -> SessionVars;
    fn SetVariables(&mut self, variables: &SessionVars);
    fn Execute(&mut self, sql: &str) -> CheckResult;
    fn Query(&mut self, sql: &str, row_limit: usize) -> CheckResult<Vec<QueryRow>>;
}

/// 快路径运行时：系统会话、日志与错误构造。
pub trait FastCheckRuntime: Send + Sync {
    fn OpenBase(&self) -> CheckResult;
    fn SetInvisibleIndexes(&self, enabled: bool);
    fn SnapshotTS(&self) -> u64;
    fn UserSessionVars(&self) -> SessionVars;
    fn BucketSize(&self) -> usize;
    fn AcquireSystemSession(&self) -> CheckResult<Box<dyn FastCheckSession>>;
    fn ReleaseSystemSession(&self, session: Box<dyn FastCheckSession>);
    // 强制走分桶比对，跳过全局 checksum 快路径。
    fn ForceBucketedCheck(&self) -> bool;
    fn DecodeRecord(
        &self,
        row: &QueryRow,
        table: &TableMeta,
        index: &IndexInfo,
        primary_key_columns: usize,
    ) -> CheckResult<RecordData>;
    fn ReportInconsistency(
        &self,
        table: &TableMeta,
        index: &IndexInfo,
        handle: &[u8],
        index_record: Option<&RecordData>,
        table_record: Option<&RecordData>,
    ) -> CheckResult;
    fn LogBucketDifference(
        &self,
        table: &TableMeta,
        index: &IndexInfo,
        table_checksum: Option<&groupByChecksum>,
        index_checksum: Option<&groupByChecksum>,
    );
}

/// ADMIN CHECK TABLE 快路径执行器。
pub struct FastCheckTableExec {
    pub dbName: String,
    pub table: TableMeta,
    pub indexInfos: Vec<IndexInfo>,
    pub done: bool,
    runtime: Arc<dyn FastCheckRuntime>,
}

/// 离开作用域时恢复不可见索引相关会话变量。
struct InvisibleIndexGuard {
    runtime: Arc<dyn FastCheckRuntime>,
}

/// Drop 时恢复不可见索引会话设置。
impl Drop for InvisibleIndexGuard {
    fn drop(&mut self) {
        self.runtime.SetInvisibleIndexes(false);
    }
}

impl FastCheckTableExec {
    pub fn new(
        runtime: Arc<dyn FastCheckRuntime>,
        db_name: String,
        table: TableMeta,
        index_infos: Vec<IndexInfo>,
    ) -> Self {
        Self {
            dbName: db_name,
            table,
            indexInfos: index_infos,
            done: false,
            runtime,
        }
    }

    /// 打开快路径：获取系统会话并执行检查。
    pub fn Open(&mut self) -> CheckResult {
        self.runtime.OpenBase()?;
        self.done = false;
        Ok(())
    }

    /// 快路径无结果行；检查在 Open 中完成。
    pub fn Next(&mut self, _request: &mut chunk::Chunk) -> CheckResult {
        if self.done || self.indexInfos.is_empty() {
            return Ok(());
        }
        self.done = true;
        self.runtime.SetInvisibleIndexes(true);
        let _guard = InvisibleIndexGuard {
            runtime: Arc::clone(&self.runtime),
        };

        let tasks = Arc::new(Mutex::new(
            (0..self.indexInfos.len()).collect::<VecDeque<_>>(),
        ));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (error_sender, error_receiver) = mpsc::channel();
        let concurrency = self.indexInfos.len().min(3);
        thread::scope(|scope| {
            for _ in 0..concurrency {
                let tasks = Arc::clone(&tasks);
                let cancelled = Arc::clone(&cancelled);
                let error_sender = error_sender.clone();
                let worker = self.createWorker();
                scope.spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        loop {
                            if cancelled.load(Ordering::Acquire) {
                                return Ok(());
                            }
                            let index_offset = tasks
                                .lock()
                                .map_err(|_| errors::New("fast-check task queue lock poisoned"))?
                                .pop_front();
                            let Some(indexOffset) = index_offset else {
                                return worker.Close();
                            };
                            worker.HandleTask(checkIndexTask { indexOffset }, |_| {})?;
                        }
                    }));
                    let error = match result {
                        Ok(Ok(())) => return,
                        Ok(Err(error)) => error,
                        Err(panic) => panicError(panic.as_ref(), "fast_check_table/checkIndexTask"),
                    };
                    cancelled.store(true, Ordering::Release);
                    if error_sender.send(error).is_err() {
                        cancelled.store(true, Ordering::Release);
                    }
                });
            }
        });
        drop(error_sender);
        error_receiver.try_recv().map_or(Ok(()), Err)
    }

    /// 构造快路径索引检查 worker。
    pub fn createWorker(&self) -> checkIndexWorker {
        checkIndexWorker {
            sctx: self.runtime.UserSessionVars(),
            dbName: self.dbName.clone(),
            table: self.table.clone(),
            indexInfos: self.indexInfos.clone(),
            runtime: Arc::clone(&self.runtime),
        }
    }
}

#[derive(Clone)]

/// 快路径单索引检查 worker 状态。
pub struct checkIndexWorker {
    pub sctx: SessionVars,
    pub dbName: String,
    pub table: TableMeta,
    pub indexInfos: Vec<IndexInfo>,
    runtime: Arc<dyn FastCheckRuntime>,
}

#[derive(Clone, Debug, Eq, PartialEq)]

/// 系统会话变量备份，便于恢复。
pub struct fastCheckSysSessionVarsBackup {
    pub optimizerUseInvisibleIndexes: bool,
    pub memQuotaQuery: i64,
    pub distSQLScanConcurrency: i32,
    pub executorConcurrency: i32,
    pub maxExecutionTime: u64,
    pub tikvClientReadTimeout: u64,
}

/// 备份快路径将覆盖的系统会话变量。
pub fn backupFastCheckSysSessionVars(vars: &SessionVars) -> fastCheckSysSessionVarsBackup {
    fastCheckSysSessionVarsBackup {
        optimizerUseInvisibleIndexes: vars.optimizerUseInvisibleIndexes,
        memQuotaQuery: vars.memQuotaQuery,
        distSQLScanConcurrency: vars.distSQLScanConcurrency,
        executorConcurrency: vars.executorConcurrency,
        maxExecutionTime: vars.maxExecutionTime,
        tikvClientReadTimeout: vars.tikvClientReadTimeout,
    }
}

/// 将备份写回系统会话变量。
impl fastCheckSysSessionVarsBackup {
    /// 将备份的变量写回会话。
    pub fn restoreTo(&self, vars: &mut SessionVars) {
        vars.optimizerUseInvisibleIndexes = self.optimizerUseInvisibleIndexes;
        vars.memQuotaQuery = self.memQuotaQuery;
        vars.distSQLScanConcurrency = self.distSQLScanConcurrency;
        vars.executorConcurrency = self.executorConcurrency;
        vars.maxExecutionTime = self.maxExecutionTime;
        vars.tikvClientReadTimeout = self.tikvClientReadTimeout;
    }
}

/// 将用户会话相关变量应用到系统会话。
pub fn applyFastCheckSysSessionVars(system: &mut SessionVars, user: &SessionVars) {
    system.optimizerUseInvisibleIndexes = true;
    system.memQuotaQuery = user.memQuotaQuery;
    system.distSQLScanConcurrency = user.distSQLScanConcurrency;
    system.executorConcurrency = user.executorConcurrency;
    system.maxExecutionTime = user.maxExecutionTime;
    system.tikvClientReadTimeout = user.tikvClientReadTimeout;
}

impl checkIndexWorker {
    /// 初始化 worker 使用的系统会话上下文。
    pub fn initSessCtx(
        &self,
        session: &mut dyn FastCheckSession,
    ) -> CheckResult<fastCheckSysSessionVarsBackup> {
        let mut system_vars = session.Variables();
        let backup = backupFastCheckSysSessionVars(&system_vars);
        applyFastCheckSysSessionVars(&mut system_vars, &self.sctx);
        session.SetVariables(&system_vars);
        let snapshot = self.runtime.SnapshotTS();
        if snapshot != 0 {
            // Go treats snapshot setup as best-effort: failure is logged but must not
            // abort ADMIN CHECK TABLE or prevent the session variables being restored.
            let _ = session.Execute(&format!("set session tidb_snapshot = {snapshot}"));
        }
        Ok(backup)
    }

    /// 全局 checksum 快速路径：两侧一致则跳过细查。
    pub fn quickPassGlobalChecksum(
        &self,
        session: &mut dyn FastCheckSession,
        table_name: &str,
        index: &IndexInfo,
        row_checksum_expression: &str,
        index_condition: &str,
    ) -> CheckResult<bool> {
        let table_query = format!(
            "select /*+ read_from_storage(tikv[{table_name}]), AGG_TO_COP() */ bit_xor({row_checksum_expression}), count(*) from {table_name} use index() where {index_condition}(0 = 0)"
        );
        let index_query = format!(
            "select /*+ AGG_TO_COP() */ bit_xor({row_checksum_expression}), count(*) from {table_name} use index(`{}`) where {index_condition}(0 = 0)",
            escapeName(&index.name)
        );
        let table_checksum = getGlobalCheckSum(session, &table_query)?;
        if !verifyIndexSideQuery(session, &index_query) {
            return Err(errors::New(format!(
                "index side query plan is not correct: {index_query}"
            )));
        }
        let index_checksum = getGlobalCheckSum(session, &index_query)?;
        Ok(table_checksum == index_checksum)
    }

    /// 处理单个索引检查任务。
    pub fn HandleTask(&self, task: checkIndexTask, _emit: impl Fn(())) -> CheckResult {
        let index = self.indexInfos.get(task.indexOffset).ok_or_else(|| {
            errors::New(format!("index offset {} is out of range", task.indexOffset))
        })?;
        let bucket_size = self.runtime.BucketSize();
        if bucket_size < 2 {
            return Err(errors::New("fast-check bucket size must be at least 2"));
        }
        let mut session = self.runtime.AcquireSystemSession()?;
        let backup = match self.initSessCtx(session.as_mut()) {
            Ok(backup) => backup,
            Err(error) => {
                self.runtime.ReleaseSystemSession(session);
                return Err(error);
            }
        };
        let result = self.checkIndex(session.as_mut(), index, bucket_size);
        let mut variables = session.Variables();
        backup.restoreTo(&mut variables);
        session.SetVariables(&variables);
        if self.runtime.SnapshotTS() != 0 {
            // Match Go's deferred cleanup: a reset failure is diagnostic only and
            // never replaces the index-check result.
            let _ = session.Execute("set session tidb_snapshot = 0");
        }
        self.runtime.ReleaseSystemSession(session);
        result
    }

    /// 对单个索引执行 checksum / 分桶比对。
    fn checkIndex(
        &self,
        session: &mut dyn FastCheckSession,
        index: &IndexInfo,
        bucket_size: usize,
    ) -> CheckResult {
        let table_name = TableName(&self.dbName, &self.table.name);
        let primary_keys = if !self.table.common_handle_columns.is_empty() {
            self.table
                .common_handle_columns
                .iter()
                .map(|column| ColumnName(column))
                .collect::<Vec<_>>()
        } else if let Some(column) = self.table.integer_handle_column.as_deref() {
            vec![ColumnName(column)]
        } else {
            vec![ColumnName("_tidb_rowid")]
        };
        let handle_columns = primary_keys.join(",");
        let index_columns = index
            .columns
            .iter()
            .map(|column| {
                column
                    .generated_expression
                    .clone()
                    .unwrap_or_else(|| ColumnName(&column.name))
            })
            .collect::<Vec<_>>()
            .join(",");
        let row_checksum_expression =
            format!("crc32(md5(concat_ws(0x2, {handle_columns}, {index_columns})))");
        let handle_checksum_expression = format!("crc32(md5(concat_ws(0x2, {handle_columns})))");
        let index_condition = index
            .condition
            .as_ref()
            .map_or_else(String::new, |condition| format!("({condition}) AND "));

        session.Execute("begin")?;
        let mut matched = self.quickPassGlobalChecksum(
            session,
            &table_name,
            index,
            &row_checksum_expression,
            &index_condition,
        )?;
        if self.runtime.ForceBucketedCheck() {
            matched = false;
        }
        if matched {
            return Ok(());
        }

        let mut rows_to_check = 0_i64;
        let mut offset = 0_u64;
        let mut modulus = 1_u64;
        let mut checked_once = false;
        let mut mismatch = false;
        for _round in 1..10 {
            if checked_once && rows_to_check <= 100 {
                break;
            }
            let where_key = if checked_once {
                format!("((cast({handle_checksum_expression} as signed) - {offset}) % {modulus})")
            } else {
                "0".to_owned()
            };
            checked_once = true;
            let group_key = format!(
                "((cast({handle_checksum_expression} as signed) - {offset}) div {modulus} % {bucket_size})"
            );
            let table_query = format!(
                "select /*+ read_from_storage(tikv[{table_name}]), AGG_TO_COP() */ bit_xor({row_checksum_expression}), {group_key}, count(*) from {table_name} use index() where {index_condition}({where_key} = 0) group by {group_key}"
            );
            let index_query = format!(
                "select /*+ AGG_TO_COP() */ bit_xor({row_checksum_expression}), {group_key}, count(*) from {table_name} use index(`{}`) where {index_condition}({where_key} = 0) group by {group_key}",
                escapeName(&index.name)
            );
            let table_checksums = getCheckSum(session, &table_query)?;
            if !verifyIndexSideQuery(session, &index_query) {
                return Err(errors::New(format!(
                    "index side query plan is not correct: {index_query}"
                )));
            }
            let index_checksums = getCheckSum(session, &index_query)?;
            logBucketDifferences(
                self.runtime.as_ref(),
                &self.table,
                index,
                &table_checksums,
                &index_checksums,
            );
            // 定位首个差异桶以便报告不一致行。
            let different = firstDifferentBucket(&table_checksums, &index_checksums);
            let Some(different) = different else {
                mismatch = false;
                break;
            };
            mismatch = true;
            rows_to_check = different.count;
            offset = offset
                .checked_add(different.bucket.saturating_mul(modulus))
                .ok_or_else(|| errors::New("fast-check bucket offset overflow"))?;
            modulus = modulus
                .checked_mul(bucket_size as u64)
                .ok_or_else(|| errors::New("fast-check bucket modulus overflow"))?;
        }

        if mismatch {
            let group_key =
                format!("((cast({handle_checksum_expression} as signed) - {offset}) % {modulus})");
            let index_sql = format!(
                "select /*+ AGG_TO_COP() */ {handle_columns}, {index_columns}, {row_checksum_expression} from {table_name} use index(`{}`) where {index_condition}({group_key} = 0) order by {handle_columns}",
                escapeName(&index.name)
            );
            let table_sql = format!(
                "select /*+ read_from_storage(tikv[{table_name}]), AGG_TO_COP() */ {handle_columns}, {index_columns}, {row_checksum_expression} from {table_name} use index() where {index_condition}({group_key} = 0) order by {handle_columns}"
            );
            if !verifyIndexSideQuery(session, &index_sql) {
                return Err(errors::New(format!(
                    "index side query plan is not correct: {index_sql}"
                )));
            }
            let index_rows = queryToRow(session, &index_sql)?;
            let table_rows = queryToRow(session, &table_sql)?;
            report_row_mismatch(
                self.runtime.as_ref(),
                &self.table,
                index,
                primary_keys.len(),
                &table_rows,
                &index_rows,
            )?;
        }
        Ok(())
    }

    /// 关闭 worker 并释放系统会话。
    pub fn Close(&self) -> CheckResult {
        Ok(())
    }
}

/// 执行 SQL 并解析为 QueryRow 列表。
pub fn queryToRow(session: &mut dyn FastCheckSession, sql: &str) -> CheckResult<Vec<QueryRow>> {
    session.Query(sql, 4096)
}

/// 验证索引侧查询是否可执行（探测用）。
pub fn verifyIndexSideQuery(session: &mut dyn FastCheckSession, sql: &str) -> bool {
    let Ok(rows) = session.Query(&format!("explain {sql}"), 4096) else {
        return false;
    };
    let mut table_scan = false;
    let mut index_scan = false;
    for row in rows {
        let Ok(operator) = row.GetString(0) else {
            return false;
        };
        if operator.contains("TableFullScan") {
            table_scan = true;
        } else if operator.contains("IndexFullScan") || operator.contains("IndexRangeScan") {
            index_scan = true;
        } else if (operator.contains("PointGet") || operator.contains("BatchPointGet"))
            && row
                .values
                .get(3)
                .and_then(|value| match value {
                    SqlValue::String(value) => Some(value),
                    _ => None,
                })
                .is_some_and(|access| access.contains(", index:"))
        {
            index_scan = true;
        }
    }
    !table_scan && index_scan
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]

/// 单个索引检查任务。
pub struct checkIndexTask {
    pub indexOffset: usize,
}

/// 索引任务的辅助方法。
impl checkIndexTask {
    /// panic 恢复时需要的上下文参数。
    pub fn RecoverArgs(&self) -> (&'static str, &'static str, Option<errors::SharedError>) {
        ("fast_check_table", "checkIndexTask", None)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]

/// GROUP BY 桶的 checksum 与计数。
pub struct groupByChecksum {
    pub bucket: u64,
    pub checksum: u64,
    pub count: i64,
}

/// 分组 checksum 的相等比较与展示。
impl groupByChecksum {
    /// 格式化为可读字符串。
    pub fn String(&self) -> String {
        format!(
            "{{bkt:{},sum:{},cnt:{}}}",
            self.bucket, self.checksum, self.count
        )
    }
}

impl fmt::Display for groupByChecksum {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.String())
    }
}

/// 按 SQL 拉取各分组的 checksum。
pub fn getCheckSum(
    session: &mut dyn FastCheckSession,
    sql: &str,
) -> CheckResult<Vec<groupByChecksum>> {
    let rows = session.Query(sql, 256)?;
    let mut checksums = Vec::with_capacity(rows.len());
    for row in rows {
        checksums.push(groupByChecksum {
            bucket: row.GetUint64(1)?,
            checksum: if row.IsNull(0)? { 0 } else { row.GetUint64(0)? },
            count: row.GetInt64(2)?,
        });
    }
    checksums.sort_unstable_by_key(|checksum| checksum.bucket);
    Ok(checksums)
}

/// 拉取全局 checksum 与行数。
pub fn getGlobalCheckSum(session: &mut dyn FastCheckSession, sql: &str) -> CheckResult<(u64, i64)> {
    let rows = session.Query(sql, 1)?;
    let Some(row) = rows.first() else {
        return Ok((0, 0));
    };
    let checksum = if row.IsNull(0)? { 0 } else { row.GetUint64(0)? };
    Ok((checksum, row.GetInt64(1)?))
}

/// 拼接转义后的 schema.table 名。
pub fn TableName(schema: &str, table: &str) -> String {
    format!("`{}`.`{}`", escapeName(schema), escapeName(table))
}

/// 转义列名。
pub fn ColumnName(column: &str) -> String {
    format!("`{}`", escapeName(column))
}

/// 对标识符加反引号转义。
pub fn escapeName(name: &str) -> String {
    name.replace('`', "``")
}

/// 找到表侧与索引侧第一个差异桶。
fn firstDifferentBucket(
    table: &[groupByChecksum],
    index: &[groupByChecksum],
) -> Option<groupByChecksum> {
    let mut table_offset = 0;
    let mut index_offset = 0;
    while table_offset < table.len() || index_offset < index.len() {
        match (table.get(table_offset), index.get(index_offset)) {
            (Some(table), Some(index)) if table.bucket == index.bucket => {
                table_offset += 1;
                index_offset += 1;
                if table.checksum != index.checksum || table.count != index.count {
                    return Some(groupByChecksum {
                        bucket: table.bucket,
                        checksum: table.checksum,
                        count: table.count.max(index.count),
                    });
                }
            }
            (Some(table), Some(index)) if table.bucket < index.bucket => {
                return Some(table.clone());
            }
            (Some(_), Some(index)) => return Some(index.clone()),
            (Some(table), None) => return Some(table.clone()),
            (None, Some(index)) => return Some(index.clone()),
            (None, None) => break,
        }
    }
    None
}

/// 记录桶级差异详情。
fn logBucketDifferences(
    runtime: &dyn FastCheckRuntime,
    table: &TableMeta,
    index: &IndexInfo,
    table_checksums: &[groupByChecksum],
    index_checksums: &[groupByChecksum],
) {
    let table_map = table_checksums
        .iter()
        .map(|checksum| (checksum.bucket, checksum))
        .collect::<BTreeMap<_, _>>();
    let index_map = index_checksums
        .iter()
        .map(|checksum| (checksum.bucket, checksum))
        .collect::<BTreeMap<_, _>>();
    for bucket in table_map.keys().chain(index_map.keys()) {
        let table_checksum = table_map.get(bucket).copied();
        let index_checksum = index_map.get(bucket).copied();
        if table_checksum != index_checksum {
            runtime.LogBucketDifference(table, index, table_checksum, index_checksum);
        }
    }
}

/// 报告具体行不匹配错误。
fn report_row_mismatch(
    runtime: &dyn FastCheckRuntime,
    table: &TableMeta,
    index: &IndexInfo,
    primary_key_columns: usize,
    table_rows: &[QueryRow],
    index_rows: &[QueryRow],
) -> CheckResult {
    let mut table_records = table_rows
        .iter()
        .map(|row| runtime.DecodeRecord(row, table, index, primary_key_columns))
        .collect::<CheckResult<Vec<_>>>()?;
    let mut index_records = index_rows
        .iter()
        .map(|row| runtime.DecodeRecord(row, table, index, primary_key_columns))
        .collect::<CheckResult<Vec<_>>>()?;
    table_records.sort_unstable_by(|left, right| left.handle.cmp(&right.handle));
    index_records.sort_unstable_by(|left, right| left.handle.cmp(&right.handle));

    let mut table_offset = 0;
    let mut index_offset = 0;
    while table_offset < table_records.len() || index_offset < index_records.len() {
        match (
            table_records.get(table_offset),
            index_records.get(index_offset),
        ) {
            (Some(table_record), Some(index_record)) => {
                match table_record.handle.cmp(&index_record.handle) {
                    std::cmp::Ordering::Equal => {
                        if table_record.checksum != index_record.checksum
                            || table_record.values != index_record.values
                        {
                            runtime.ReportInconsistency(
                                table,
                                index,
                                &table_record.handle,
                                Some(index_record),
                                Some(table_record),
                            )?;
                        }
                        table_offset += 1;
                        index_offset += 1;
                    }
                    std::cmp::Ordering::Less => {
                        runtime.ReportInconsistency(
                            table,
                            index,
                            &table_record.handle,
                            None,
                            Some(table_record),
                        )?;
                        table_offset += 1;
                    }
                    std::cmp::Ordering::Greater => {
                        runtime.ReportInconsistency(
                            table,
                            index,
                            &index_record.handle,
                            Some(index_record),
                            None,
                        )?;
                        index_offset += 1;
                    }
                }
            }
            (Some(table_record), None) => {
                runtime.ReportInconsistency(
                    table,
                    index,
                    &table_record.handle,
                    None,
                    Some(table_record),
                )?;
                table_offset += 1;
            }
            (None, Some(index_record)) => {
                runtime.ReportInconsistency(
                    table,
                    index,
                    &index_record.handle,
                    Some(index_record),
                    None,
                )?;
                index_offset += 1;
            }
            (None, None) => break,
        }
    }
    Ok(())
}

/// 将 worker panic 转为 SharedError。
fn panicError(panic: &(dyn std::any::Any + Send), worker: &str) -> errors::SharedError {
    let message = panic
        .downcast_ref::<&str>()
        .map(|value| (*value).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned());
    errors::New(format!("{worker}: {message}"))
}
