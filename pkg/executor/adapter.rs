// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 语句执行适配层（statement adapter）。
//
// 将解析后的语句与物理执行计划（plan）接到执行器（executor）上：
// 构建/打开执行器、拉取结果集（RecordSet）、处理悲观事务（pessimistic）
// 下的 DML 与 SELECT FOR UPDATE 加锁重试、外键级联、慢查询与 RU 计量、
// Plan Replayer 捕获等。是会话层与执行引擎之间的枢纽。

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use astersql_errors as errors;
use astersql_planner_core_base as base;
use astersql_sessionctx_vardef::QueryLogMaxLen;
use astersql_util_chunk as chunk;
pub use astersql_util_execdetails::ruv2_metrics::RUV2Metrics;
use astersql_util_execdetails::ruv2_metrics::{SyncRUV2MetricsFromRUDetails, tikvutil};

/// 适配层统一结果类型。
pub type AdapterResult<T = ()> = Result<T, errors::SharedError>;
/// 编码后的键（TiKV key）。
pub type Key = Vec<u8>;

/// 外键级联（cascade）最大递归深度，防止环状引用导致无限递归。
pub const MAX_FOREIGN_KEY_CASCADE_DEPTH: usize = 15;
/// 结果集列别名标识符最大长度（超出则截断）。
pub const MAX_ALIAS_IDENTIFIER_LEN: usize = 256;

/// 是否启用日志脱敏（redact）的全局开关。
static REDACT_LOG_ENABLED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 语句调度优先级。
pub enum Priority {
    Unspecified,
    Low,
    Normal,
    High,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// SQL 语句种类（用于指标与只读判定）。
pub enum StatementKind {
    Select,
    Insert,
    Replace,
    Update,
    Delete,
    DDL,
    Execute,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 物理执行计划节点种类。
pub enum PlanKind {
    LoadData(astersql_parser_ast::FileLocRef),
    PointGet,
    TableDual,
    Set,
    Insert,
    Update,
    Delete,
    DDL,
    Projection,
    Analyze,
    Query,
}

/// Real parser field type carried from physical schema into client metadata.
pub use astersql_parser_types::FieldType;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 计划 schema 中的一列。
pub struct SchemaColumn {
    pub field_type: FieldType,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 结果列的库/表/列名信息。
pub struct FieldName {
    pub db_name: String,
    pub table_name: String,
    pub original_table_name: String,
    pub column_name: String,
    pub original_column_name: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 返回给客户端的结果列元数据。
pub struct ResultField {
    pub column_name: String,
    pub column_alias: String,
    pub empty_original_name: bool,
    pub table_name: String,
    pub table_alias: String,
    pub database_name: String,
    pub field_type: FieldType,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 执行计划摘要：种类、schema、编码文本与 hint。
pub struct PlanInfo {
    /// Physical plan node ID used for executor runtime stats registration.
    pub id: i32,
    pub kind: PlanKind,
    pub schema: Vec<SchemaColumn>,
    pub calculate_no_delay: bool,
    pub projection_child: Option<Box<PlanInfo>>,
    pub encoded: String,
    pub binary: String,
    pub hints: String,
}

impl PlanInfo {
    /// 是否为插入/更新/删除类 DML 计划。
    pub fn IsDML(&self) -> bool {
        matches!(
            self.kind,
            PlanKind::Insert | PlanKind::Update | PlanKind::Delete
        )
    }
}

/// Optimizer result used when a statement is rebuilt after preparation or retry.
/// Keep the physical tree alongside the compatibility summary for RU traversal.
pub struct RebuiltPlan {
    pub summary: PlanInfo,
    pub typed: Arc<dyn base::Plan>,
    pub output_names: Vec<FieldName>,
    pub schema_version: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 语句 AST 侧摘要：种类与各形态 SQL 文本。
pub struct StatementNode {
    pub kind: StatementKind,
    pub original_text: String,
    pub text: String,
    pub secure_text: String,
    pub prepared_text: Option<String>,
}

#[derive(Clone, Default)]
/// 执行上下文：链路追踪 ID 与继承的 RU 上下文。
pub struct ExecutionContext {
    pub trace_id: Vec<u8>,
    pub inherited_ru_context: Vec<u8>,
    pub ru_details: Option<Arc<tikvutil::RUDetails>>,
    /// Canonical kill signal shared with lazy executor loops.
    pub sql_killer: Option<Arc<astersql_util_sqlkiller::sqlkiller::SQLKiller>>,
}

/// TiKV/TiFlash RU details share the canonical execdetails contract.
pub type RUDetails = tikvutil::RUDetails;

/// Select the RU values exposed by slow logs and statement summaries without
/// mutating the shared execution accounting.
pub fn SelectRUDetailsForStatementLog(
    raw: Option<astersql_util_execdetails::execdetails::util::RUDetails>,
    version: u8,
    total_ru_v2: Option<f64>,
    is_write: bool,
) -> Option<astersql_util_execdetails::execdetails::util::RUDetails> {
    if version != 2 {
        return raw;
    }
    let Some(total) = total_ru_v2 else {
        return raw;
    };
    let wait = raw
        .as_ref()
        .map_or(Duration::ZERO, |details| details.RUWaitDuration());
    Some(astersql_util_execdetails::execdetails::util::RUDetails {
        read_ru: if is_write { 0.0 } else { total },
        write_ru: if is_write { total } else { 0.0 },
        ru_wait_duration: wait,
        ..Default::default()
    })
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// SQL/计划摘要：文本与二进制 digest。
pub struct Digest {
    pub text: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone)]
/// 单次语句执行期上下文：计划、RU、网络流量、缓存命中等。
pub struct StatementContext {
    pub priority: Priority,
    pub found_rows: u64,
    pub affected_rows: u64,
    pub statement_type: String,
    pub sql_normalized: String,
    pub sql_digest: Digest,
    pub plan: Option<PlanInfo>,
    pub flat_plan: Option<PlanInfo>,
    pub plan_digest: Option<(String, Digest)>,
    pub binding_sql: String,
    pub binding_sql_digest: String,
    pub plan_encode_error: Option<String>,
    pub use_plan_cache: bool,
    pub plan_cache_unqualified: Option<String>,
    pub ru_metrics: Option<Arc<RUV2Metrics>>,
    pub commit_details: Option<Arc<tikvutil::CommitDetails>>,
    pub statement_ru_evidence:
        Option<Arc<crate::statement_ru_plan_walk::StatementRURuntimeEvidence>>,
    pub statement_ru_owner: Option<Arc<crate::statement_ru_plan_walk::StatementRUOwner>>,
    pub statement_ru_finalized:
        Option<Arc<crate::statement_ru_result::StatementRUFinalizedSnapshot>>,
    pub total_ru: f64,
    pub network_sent_bytes: u64,
    pub network_received_bytes: u64,
    pub mpp_network_bytes: u64,
}

impl Default for StatementContext {
    fn default() -> Self {
        Self {
            priority: Priority::Unspecified,
            found_rows: 0,
            affected_rows: 0,
            statement_type: String::new(),
            sql_normalized: String::new(),
            sql_digest: Digest::default(),
            plan: None,
            flat_plan: None,
            plan_digest: None,
            binding_sql: String::new(),
            binding_sql_digest: String::new(),
            plan_encode_error: None,
            use_plan_cache: false,
            plan_cache_unqualified: None,
            ru_metrics: None,
            commit_details: None,
            statement_ru_evidence: None,
            statement_ru_owner: None,
            statement_ru_finalized: None,
            total_ru: 0.0,
            network_sent_bytes: 0,
            network_received_bytes: 0,
            mpp_network_bytes: 0,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Chunk（列式批处理块）容量与字段配置。
pub struct ChunkConfig {
    pub fields: Vec<FieldType>,
    pub initial_capacity: usize,
    pub maximum_chunk_size: usize,
}

/// 进程信息（SHOW PROCESSLIST）设置接口。
pub trait processinfoSetter {
    fn SetProcessInfo(
        &self,
        sql: &str,
        started: SystemTime,
        command: u8,
        maximum_execution_time: u64,
    );
    fn UpdateProcessInfo(&self);
}

/// 外键级联批：有待处理行时构建子执行器并标记完成。
pub trait CascadeBatch {
    fn HasPendingRows(&self) -> bool;
    fn BuildExecutor(&mut self) -> AdapterResult<Option<Box<dyn ExecExecutor>>>;
    fn MarkBatchComplete(&mut self);
}

/// 执行器接口：Open/Next/Close、外键检查与 Detach。
/// Canonical session expressions and transactions are thread bound. Individual
/// detached scan implementations can still be Send when they own their source.
pub trait ExecExecutor {
    fn Open(&mut self) -> AdapterResult;
    fn Close(&mut self) -> AdapterResult;
    fn Next(&mut self, output: &mut chunk::Chunk) -> AdapterResult;
    fn NextWithContext(
        &mut self,
        _context: &ExecutionContext,
        output: &mut chunk::Chunk,
    ) -> AdapterResult {
        self.Next(output)
    }
    fn ChunkConfig(&self) -> ChunkConfig;
    fn NewChunk(&self) -> chunk::Chunk;
    fn Schema(&self) -> &[SchemaColumn];
    fn CalculateNoDelay(&self) -> bool;
    fn IsWriteExecutor(&self) -> bool;
    fn CheckForeignKeys(&mut self) -> AdapterResult;
    fn TakeForeignKeyCascades(&mut self) -> Vec<Box<dyn CascadeBatch>>;
    fn HasForeignKeyCascades(&self) -> bool;
    fn PrepareFKCascadeContext(&mut self);
    fn AddFKCheckLockDuration(&mut self, duration: Duration);
    /// Record keys produced by this page for a locking plan. The select-lock
    /// executor consumes these before exposing rows to the client.
    fn TakeLockKeys(&mut self) -> Vec<Key> {
        Vec::new()
    }
    fn ScannedRows(&self) -> usize {
        0
    }
    fn Detach(&mut self) -> Option<Box<dyn ExecExecutor>>;
}

/// 结果集接口：向客户端分批返回行。原结果集绑定会话；Detach 后的执行器独立持有快照。
pub trait RecordSet {
    /// Optional executor cleanup before the session publishes its final outcome.
    /// Buffered result sets without an active executor need no separate cleanup.
    fn Finish(&mut self) -> AdapterResult {
        Ok(())
    }
    fn Fields(&mut self) -> &[ResultField];
    fn Next(&mut self, output: &mut chunk::Chunk) -> AdapterResult;
    fn NewChunk(&mut self) -> chunk::Chunk;
    fn Close(&mut self) -> AdapterResult;
    /// Close after a session-side terminal failure and preserve that error for
    /// statement finalization. Buffered/detached results have no statement to
    /// finalize, so their default behavior is the ordinary close path.
    fn CloseWithError(&mut self, _last_error: errors::SharedError) -> AdapterResult {
        self.Close()
    }
    /// Optional detach hook; a result set without an owned executor is not detachable.
    fn TryDetach(&mut self) -> AdapterResult<(Option<Box<dyn RecordSet>>, bool)> {
        Ok((None, false))
    }
    /// Optional COM_FETCH notification for result sets that own a statement.
    fn OnFetchReturned(&mut self) {}
}

/// A detached executor owns its result set without retaining the session or statement.
pub struct detachedRecordSet {
    fields: Vec<ResultField>,
    executor: Option<Box<dyn ExecExecutor>>,
    sql_text: String,
    source_context: Option<ExecutionContext>,
}

impl detachedRecordSet {
    pub(crate) fn new(
        fields: Vec<ResultField>,
        executor: Box<dyn ExecExecutor>,
        sql_text: String,
        source_context: Option<ExecutionContext>,
    ) -> Self {
        Self {
            fields,
            executor: Some(executor),
            sql_text,
            source_context,
        }
    }
}

impl RecordSet for detachedRecordSet {
    fn Finish(&mut self) -> AdapterResult {
        self.Close()
    }

    fn Fields(&mut self) -> &[ResultField] {
        &self.fields
    }

    fn Next(&mut self, output: &mut chunk::Chunk) -> AdapterResult {
        let executor = self
            .executor
            .as_mut()
            .ok_or_else(|| errors::New("record set is closed"))?;
        let context = self.source_context.clone().unwrap_or_default();
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            executor.NextWithContext(&context, output)
        }))
        .unwrap_or_else(|panic| {
            Err(panicError(
                panic.as_ref(),
                &format!("detachedRecordSet.Next ({})", self.sql_text),
            ))
        })
    }

    fn NewChunk(&mut self) -> chunk::Chunk {
        self.executor
            .as_ref()
            .map_or_else(chunk::Chunk::default, |executor| executor.NewChunk())
    }

    fn Close(&mut self) -> AdapterResult {
        self.executor
            .take()
            .map_or(Ok(()), |mut executor| executor.Close())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 悲观锁错误后的动作：可重试或直接返回错误。
pub enum PessimisticErrorAction {
    RetryReady,
    ReturnError,
}

/// 悲观事务接口：待加锁键与是否已写判定。
pub trait pessimisticTxn {
    fn KeysNeedToLock(&mut self) -> AdapterResult<Vec<Key>>;
    fn IsValid(&self) -> bool;
    fn IsKeyWritten(&self, key: &[u8]) -> AdapterResult<bool>;
}

/// 适配层运行时：构建执行器、事务/锁、指标、慢查询与 Plan Replayer。
/// canonical 会话含线程本地事务和计划缓存，因此运行时不强制跨线程共享。
pub trait AdapterRuntime {
    fn BuildExecutor(
        &self,
        plan: &PlanInfo,
        telemetry: Option<&TelemetryInfo>,
    ) -> AdapterResult<Box<dyn ExecExecutor>>;
    fn BuildExecutorForSelectLock(
        &self,
        plan: &PlanInfo,
        telemetry: Option<&TelemetryInfo>,
    ) -> AdapterResult<Box<dyn ExecExecutor>> {
        self.BuildExecutor(plan, telemetry)
    }
    fn BuildPointGetExecutor(
        &self,
        plan: &PlanInfo,
        start_ts: u64,
        prepared_key: Option<&str>,
    ) -> AdapterResult<Box<dyn ExecExecutor>>;
    fn RebuildPlan(
        &self,
        statement: &StatementNode,
        previous_summary: &PlanInfo,
        previous_names: &[FieldName],
    ) -> AdapterResult<RebuiltPlan>;
    fn NewChunk(&self, config: &ChunkConfig) -> chunk::Chunk;
    fn StatementReadTS(&self) -> AdapterResult<u64>;
    fn TransactionStartTS(&self) -> u64;
    fn SnapshotTS(&self) -> u64;
    fn LowResolutionTSO(&self) -> bool;
    fn IsPessimistic(&self) -> bool;
    fn SupportsSelectForUpdate(&self) -> bool;
    fn SupportsPreparedExecution(&self) -> bool;
    fn RunawayBeforeExecutor(
        &self,
        _statement: &StatementNode,
        _sql_digest: &str,
        _plan_digest: &str,
    ) -> AdapterResult {
        Ok(())
    }
    fn ForeignKeyChecks(&self) -> bool;
    fn StmtCommit(&self) -> AdapterResult;
    fn ForeignKeySavepointName(&self) -> String;
    fn ReleaseForeignKeySavepoint(&self, savepoint: &str);
    fn SetInHandleForeignKeyTrigger(&self, active: bool);
    fn ForeignKeyCheckInSharedLock(&self) -> bool;
    fn KillSignal(&self) -> AdapterResult;
    fn SQLKillerHandle(&self) -> Option<Arc<astersql_util_sqlkiller::sqlkiller::SQLKiller>> {
        None
    }
    fn CurrentDatabase(&self) -> String;
    fn InitialChunkSize(&self) -> usize;
    fn MaximumChunkSize(&self) -> usize;
    fn Command(&self) -> u8;
    fn MaximumExecutionTime(&self) -> u64;
    fn DMLMaximumExecutionTime(&self) -> u64 {
        0
    }
    fn SetProcessInfo(&self, sql: &str, started: SystemTime, command: u8, maximum_time: u64);
    fn CancelMaximumExecutionTime(&self);
    fn FinalizePreparedExecution(&self, _scanned_rows: usize, _success: bool) {}
    fn SetPriority(&self, priority: Priority);
    fn SetLastFoundRows(&self, rows: u64);
    fn AddFoundRows(&self, rows: u64);
    fn ResetStatementForRetry(&self);
    fn InheritExecuteStatement(&self, statement: &mut StatementNode) -> AdapterResult;
    fn PreparedStatementSQL(&self) -> String;
    fn IsReadOnly(&self, statement: &StatementNode) -> bool;
    fn PrepareFKCascadeContext(&self);
    fn HandleFKTriggerError(&self) -> AdapterResult;
    fn DetachTrackers(&self);
    fn ResetCTEStorage(&self) -> AdapterResult;
    fn OnPessimisticStmtStart(&self) -> AdapterResult;
    fn OnPessimisticStmtEnd(&self, success: bool) -> AdapterResult;
    fn AbortLazyUniquenessOnPessimisticDMLFailure(
        &self,
        _error: &errors::SharedError,
    ) -> AdapterResult<Option<errors::SharedError>> {
        Ok(None)
    }
    fn PessimisticTransaction(&self) -> AdapterResult<Box<dyn pessimisticTxn>>;
    fn ResetUnchangedKeysForLock(&self);
    fn CollectUnchangedKeysForXLock(&self, keys: Vec<Key>) -> Vec<Key>;
    fn CollectUnchangedKeysForSLock(&self, keys: Vec<Key>) -> Vec<Key>;
    fn LockKeys(&self, keys: &[Key], shared: bool) -> AdapterResult;
    fn OnPessimisticLockError(
        &self,
        error: &errors::SharedError,
    ) -> AdapterResult<PessimisticErrorAction>;
    fn OnPessimisticStmtRetry(&self) -> AdapterResult;
    fn RollbackStatementForRetry(&self) -> AdapterResult;
    fn MaximumPessimisticRetries(&self) -> usize;
    fn StatementContext(&self) -> StatementContext;
    fn SetStatementContext(&self, context: &StatementContext);
    fn Digest(&self, text: &str) -> Digest;
    fn Audit(&self, sql: &str);
    fn ObservePhase(&self, phase: &str, internal: bool, duration: Duration);
    fn RecordDMLMetric(&self, statement_type: &str, value: i64);
    fn RUVersion(&self) -> u8 {
        1
    }
    fn RUV2ReporterAvailable(&self) -> bool;
    fn ResourceGroupName(&self) -> String;
    fn ReportRUV2Consumption(&self, resource_group: &str, tikv: f64, tidb: f64, tiflash: f64);
    fn RecordLastQuery(&self, error: Option<&str>, total_ru_v2: f64);
    fn PlanReplayerCapture(&self, statement: &StatementNode, start_ts: u64, continuous: bool);
    fn SlowQuery(&self, transaction_ts: u64, sql: &str, success: bool, has_more_results: bool);
    fn Summary(&self, summary: &StatementSummary);
    fn UpdatePreviousStatement(&self, sql: &str, digest: &str);
    fn RecordNetworkTraffic(&self, sent: u64, received: u64, mpp: u64);
    fn RecordPlanCache(&self, hit: bool, reason: Option<&str>);
    fn TopSQLStart(&self, sql_digest: &[u8], plan_digest: &[u8]);
    fn TopSQLFinish(&self, total_ru_v2: f64);
    fn OnExecComplete(&self, _success: bool) {}
    fn ExecLockMetrics(&self) -> ExecLockMetrics {
        ExecLockMetrics::default()
    }
    fn FairLockingFinishMetrics(&self) -> FairLockingFinishMetrics {
        FairLockingFinishMetrics::default()
    }
    fn SupplementaryFinishMetrics(&self) -> SupplementaryFinishMetrics {
        SupplementaryFinishMetrics::default()
    }
    fn ExecuteRunDurationForFinish(&self) -> Option<Duration> {
        None
    }
    fn ObserveStatementDuration(&self, _statement_type: &str) {}
    fn OnFinishStatement(&self, _retries: usize, _success: bool, _affected_rows: u64) {}
    fn CommitDetailsForFinish(&self) -> Option<CommitDetails> {
        None
    }
    fn AttachFinishRuntimeStats(&self, _plan_id: i32) {}
    fn StatementRURuntimeEvidence(
        &self,
        _plan_ids: &[i32],
    ) -> crate::statement_ru_plan_walk::StatementRURuntimeEvidence {
        Default::default()
    }
    fn StatementRUScalarSubqueries(&self) -> Vec<std::rc::Rc<dyn std::any::Any>> {
        Vec::new()
    }
    fn StatementRUFrontendCompileBytes(&self, statement: &StatementNode) -> f64 {
        crate::statement_ru_result::statement_ru_frontend_compile_bytes(statement, false, "", "")
    }

    /// None represents a missing session or statement context; never guess eligibility.
    fn StatementRUInstallState(
        &self,
        _statement: &StatementNode,
    ) -> Option<crate::statement_ru_result::StatementRUInstallState> {
        None
    }
    /// Full-mode calibration boundary, dormant unless a consumer is installed.
    fn StatementRUCalibration(
        &self,
        _state: crate::statement_ru_result::StatementRUCalibrationState,
        _units: astersql_resourcegroup::ruv2::model::StmtUnits,
    ) {
    }
    fn StatementRUIneligible(&self) {
        let _ = std::panic::catch_unwind(|| {
            let counter = {
                let _guard = astersql_metrics::metrics::PACKAGE_INIT_LOCK
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                // The package lock serializes metric initialization and this read.
                unsafe { (&*std::ptr::addr_of!(astersql_metrics::ru_v2::RUV2Statements)).clone() }
            };
            if let Some(counter) = counter {
                counter.with_label_values(&["skipped", "ineligible"]).inc();
            }
        });
    }
    fn CleanupAfterFinish(&self) {}
    fn RestrictedSQL(&self) -> bool;
    fn RedactLog(&self) -> bool;
}

struct ForeignKeyTriggerGuard(Arc<dyn AdapterRuntime>);

impl Drop for ForeignKeyTriggerGuard {
    fn drop(&mut self) {
        self.0.SetInHandleForeignKeyTrigger(false);
    }
}

/// 流式结果集：持有执行器，按 chunk 拉取并在关闭时收尾。
pub struct recordSet {
    fields: Vec<ResultField>,
    executor: Option<Box<dyn ExecExecutor>>,
    schema: Vec<SchemaColumn>,
    stmt: ExecStmt,
    lastErrs: Vec<errors::SharedError>,
    txnStartTS: u64,
    finished: bool,
    traceID: Vec<u8>,
    chunk_config: ChunkConfig,
}

impl recordSet {
    /// 懒构建并返回结果列元数据。
    pub fn Fields(&mut self) -> &[ResultField] {
        if self.fields.is_empty() {
            self.fields = colNames2ResultFields(
                &self.schema,
                &self.stmt.OutputNames,
                &self.stmt.Ctx.CurrentDatabase(),
            );
        }
        &self.fields
    }

    /// 拉取下一批行；捕获 panic 并累计 lastErrs。
    pub fn Next(&mut self, request: &mut chunk::Chunk) -> AdapterResult {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> AdapterResult {
            if let Err(error) = self.stmt.Ctx.KillSignal() {
                return Err(error);
            }
            let executor = self
                .executor
                .as_mut()
                .ok_or_else(|| errors::New("query execution was interrupted"));
            let executor = match executor {
                Ok(executor) => executor,
                Err(error) => {
                    self.lastErrs.push(error.clone());
                    return Err(error);
                }
            };
            let mut context = self.stmt.GoCtx.clone().unwrap_or_default();
            if !self.traceID.is_empty() {
                context.trace_id = self.traceID.clone();
            }
            context = inheritStmtRUV2Context(context, Some(&self.stmt));
            if let Err(error) = self
                .stmt
                .nextWithContext(&context, executor.as_mut(), request)
            {
                self.lastErrs.push(error.clone());
                return Err(error);
            }
            let rows = request.NumRows();
            if rows == 0 {
                self.stmt.recordStatementRURootEOF();
                self.stmt
                    .Ctx
                    .SetLastFoundRows(self.stmt.StatementCtx.found_rows);
            } else {
                self.stmt.StatementCtx.found_rows += rows as u64;
                self.stmt.Ctx.AddFoundRows(rows as u64);
            }
            Ok(())
        }));
        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                self.stmt.abortStatementRU();
                Err(error)
            }
            Err(panic) => {
                self.stmt.abortStatementRU();
                Err(panicError(panic.as_ref(), "recordSet.Next"))
            }
        }
    }

    /// 当前执行器或缓存的 chunk 配置。
    pub fn chunkConfig(&self) -> ChunkConfig {
        self.executor
            .as_ref()
            .map(|executor| executor.ChunkConfig())
            .unwrap_or_else(|| self.chunk_config.clone())
    }

    /// 按配置分配新 chunk。
    pub fn NewChunk(&mut self) -> chunk::Chunk {
        self.executor.as_ref().map_or_else(
            || self.stmt.Ctx.NewChunk(&self.chunkConfig()),
            |executor| executor.NewChunk(),
        )
    }

    /// 关闭执行器并重置 CTE 存储，幂等。
    pub fn Finish(&mut self) -> AdapterResult {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        let scanned_rows = self
            .executor
            .as_ref()
            .map_or(0, |executor| executor.ScannedRows());
        let close_error = self
            .executor
            .as_mut()
            .and_then(|executor| executor.Close().err());
        self.stmt.Ctx.FinalizePreparedExecution(
            scanned_rows,
            close_error.is_none() && self.lastErrs.is_empty(),
        );
        self.executor = None;
        let cte_error = resetCTEStorageMap(self.stmt.Ctx.as_ref()).err();
        let error = close_error.or(cte_error);
        if let Some(error) = error {
            self.lastErrs.push(error.clone());
            return Err(error);
        }
        Ok(())
    }

    fn close_with_error(&mut self, last_error: Option<errors::SharedError>) -> AdapterResult {
        let result = self.Finish();
        let mut final_errors = self.lastErrs.clone();
        if let Some(last_error) = last_error {
            final_errors.push(last_error);
        }
        let final_error = joinRecordSetErrors(&final_errors);
        self.stmt.CloseRecordSet(self.txnStartTS, final_error);
        result
    }

    /// Finish 后调用 CloseRecordSet 完成语句收尾。
    pub fn Close(&mut self) -> AdapterResult {
        self.close_with_error(None)
    }

    /// 客户端已取回结果时写慢查询（可能还有更多结果）。
    pub fn OnFetchReturned(&mut self) {
        self.stmt
            .LogSlowQuery(self.txnStartTS, self.lastErrs.is_empty(), true);
    }

    /// 尝试 Detach 执行器，得到可独立消费的结果集副本。
    pub fn TryDetach(&mut self) -> AdapterResult<(Option<Box<dyn RecordSet>>, bool)> {
        let Some(executor) = self.executor.as_mut() else {
            return Ok((None, false));
        };
        let Some(detached) = executor.Detach() else {
            return Ok((None, false));
        };
        let fields = self.Fields().to_vec();
        let result = detachedRecordSet::new(
            fields,
            detached,
            self.stmt.GetTextToLog(false),
            self.stmt.GoCtx.clone(),
        );
        Ok((Some(Box::new(result)), true))
    }

    /// 测试用：暴露内部执行器。
    pub fn GetExecutor4Test(&mut self) -> Option<&mut (dyn ExecExecutor + '_)> {
        match self.executor.as_mut() {
            Some(executor) => Some(executor.as_mut()),
            None => None,
        }
    }
}

pub(crate) fn joinRecordSetErrors(
    record_errors: &[errors::SharedError],
) -> Option<errors::SharedError> {
    errors::Join(&record_errors.iter().cloned().map(Some).collect::<Vec<_>>())
}

impl RecordSet for recordSet {
    fn Finish(&mut self) -> AdapterResult {
        recordSet::Finish(self)
    }

    fn Fields(&mut self) -> &[ResultField] {
        recordSet::Fields(self)
    }

    fn Next(&mut self, output: &mut chunk::Chunk) -> AdapterResult {
        recordSet::Next(self, output)
    }

    fn NewChunk(&mut self) -> chunk::Chunk {
        recordSet::NewChunk(self)
    }

    fn Close(&mut self) -> AdapterResult {
        recordSet::Close(self)
    }

    fn CloseWithError(&mut self, last_error: errors::SharedError) -> AdapterResult {
        self.close_with_error(Some(last_error))
    }

    fn TryDetach(&mut self) -> AdapterResult<(Option<Box<dyn RecordSet>>, bool)> {
        recordSet::TryDetach(self)
    }

    fn OnFetchReturned(&mut self) {
        recordSet::OnFetchReturned(self)
    }
}

/// 将 schema 与 FieldName 转为客户端 ResultField（补默认库名、截断别名）。
pub fn colNames2ResultFields(
    schema: &[SchemaColumn],
    names: &[FieldName],
    default_database: &str,
) -> Vec<ResultField> {
    schema
        .iter()
        .enumerate()
        .map(|(offset, column)| {
            let name = names.get(offset).cloned().unwrap_or_default();
            let database = if name.db_name.is_empty() && !name.table_name.is_empty() {
                default_database.to_owned()
            } else {
                name.db_name.clone()
            };
            let empty_original_name = name.original_column_name.is_empty();
            let original_name = if empty_original_name {
                name.column_name.clone()
            } else {
                name.original_column_name.clone()
            };
            ResultField {
                column_name: original_name,
                column_alias: truncateIdentifier(&name.column_name),
                empty_original_name,
                table_name: name.original_table_name,
                table_alias: name.table_name,
                database_name: database,
                field_type: column.field_type.clone(),
            }
        })
        .collect()
}

/// 从父语句继承 RU V2 上下文到当前 ExecutionContext。
pub fn inheritStmtRUV2Context(
    mut context: ExecutionContext,
    statement: Option<&ExecStmt>,
) -> ExecutionContext {
    if let Some(statement_context) = statement.and_then(|statement| statement.GoCtx.as_ref()) {
        context.inherited_ru_context = statement_context.inherited_ru_context.clone();
        context.ru_details = statement_context.ru_details.clone();
    }
    context
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// DDL/查询特性遥测标志（分区、索引合并等）。
pub struct TelemetryInfo {
    pub UseNonRecursive: bool,
    pub UseRecursive: bool,
    pub UseMultiSchemaChange: bool,
    pub UseExchangePartition: bool,
    pub UseFlashbackToCluster: bool,
    pub PartitionTelemetry: Option<PartitionTelemetryInfo>,
    pub AccountLockTelemetry: Option<AccountLockTelemetryInfo>,
    pub UseIndexMerge: bool,
    pub UseTableLookUp: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 分区相关遥测细节。
pub struct PartitionTelemetryInfo {
    pub UseTablePartition: bool,
    pub UseTablePartitionList: bool,
    pub UseTablePartitionRange: bool,
    pub UseTablePartitionHash: bool,
    pub UseTablePartitionRangeColumns: bool,
    pub UseTablePartitionRangeColumnsGt1: bool,
    pub UseTablePartitionRangeColumnsGt2: bool,
    pub UseTablePartitionRangeColumnsGt3: bool,
    pub UseTablePartitionListColumns: bool,
    pub TablePartitionMaxPartitionsNum: u64,
    pub UseCreateIntervalPartition: bool,
    pub UseAddIntervalPartition: bool,
    pub UseDropIntervalPartition: bool,
    pub UseCompactTablePartition: bool,
    pub UseReorganizePartition: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 账户锁定相关遥测计数。
pub struct AccountLockTelemetryInfo {
    pub LockUser: i64,
    pub UnlockUser: i64,
    pub CreateOrAlterUser: i64,
}

#[derive(Clone)]
/// 可执行语句：持有计划、运行时、阶段耗时与语句上下文。
pub struct ExecStmt {
    pub GoCtx: Option<ExecutionContext>,
    pub InfoSchema: i64,
    pub Plan: PlanInfo,
    /// Original typed planner tree retained for operator-level RU traversal.
    pub TypedPlan: Option<Arc<dyn base::Plan>>,
    pub StmtNode: StatementNode,
    pub Ctx: Arc<dyn AdapterRuntime>,
    pub LowerPriority: bool,
    pub isPreparedStmt: bool,
    pub isSelectForUpdate: bool,
    pub retryCount: usize,
    pub retryStartTime: Option<Instant>,
    pub phaseBuildDurations: [Duration; 2],
    pub phaseOpenDurations: [Duration; 2],
    pub phaseNextDurations: [Duration; 2],
    pub phaseLockDurations: [Duration; 2],
    pub OutputNames: Vec<FieldName>,
    pub PsStmt: Option<String>,
    pub Ti: Option<TelemetryInfo>,
    pub StatementCtx: StatementContext,
}

/// Keeps the published statement's RU owner alive through session cleanup.
/// Early errors and unwinding consume an unknown outcome without running an
/// executor terminal; a previously recorded result retains first-record priority.
#[must_use = "keep the guard alive until session cleanup completes"]
pub struct StatementRUFailureGuard {
    owner: Option<Arc<crate::statement_ru_plan_walk::StatementRUOwner>>,
    context: Arc<dyn AdapterRuntime>,
}

impl Drop for StatementRUFailureGuard {
    fn drop(&mut self) {
        if let Some(owner) = &self.owner {
            let (_, setup) = owner.record_final_outcome_with_setup(false);
            if setup.is_some_and(|setup| setup.full_report) {
                crate::statement_ru_result::publish_statement_ru_failure_safely(
                    &crate::statement_ru_result::StatementRUContextSink {
                        context: self.context.as_ref(),
                        ttl_job: false,
                    },
                    crate::statement_ru_reporting::StatementRUFailureReason::StatementError,
                );
            }
        }
    }
}

impl ExecStmt {
    /// Install immediately after compilation, before any fallible session cleanup.
    pub fn StatementRUFailureGuard(&self) -> StatementRUFailureGuard {
        StatementRUFailureGuard {
            owner: self.StatementCtx.statement_ru_owner.clone(),
            context: self.Ctx.clone(),
        }
    }

    /// Session completion is independent from executor EOF; the first outcome wins.
    pub fn RecordStatementRUFinalOutcome(&self, success: bool) {
        if let Some(owner) = &self.StatementCtx.statement_ru_owner {
            let (_, setup) = owner.record_final_outcome_with_setup(success);
            if let Some(setup) = setup {
                self.publishStatementRUAbort(setup.full_report);
            }
        }
    }

    pub fn abortStatementRU(&self) {
        if let Some(owner) = &self.StatementCtx.statement_ru_owner {
            if let Some(setup) = owner.take_terminal_setup() {
                self.publishStatementRUAbort(setup.full_report);
            }
        }
    }

    fn publishStatementRUAbort(&self, full_report: bool) {
        if full_report {
            crate::statement_ru_result::publish_statement_ru_failure_safely(
                &crate::statement_ru_result::StatementRUContextSink {
                    context: self.Ctx.as_ref(),
                    ttl_job: false,
                },
                crate::statement_ru_reporting::StatementRUFailureReason::StatementError,
            );
        }
    }

    pub fn recordStatementRURootEOF(&self) {
        if let Some(owner) = &self.StatementCtx.statement_ru_owner {
            owner.record_root_eof();
        }
    }

    /// Consume before inspecting live state so errors, panic and reentry cannot retry.
    /// The returned value is private to terminal bookkeeping; publication is separate.
    pub fn finishStatementRU(
        &mut self,
        terminal_error: Option<&errors::SharedError>,
    ) -> Option<crate::statement_ru_result::StatementRUFinalizedSnapshot> {
        use crate::statement_ru_plan_walk::*;
        use crate::statement_ru_result::{StatementRUCalculator, StatementRUPlanKind};
        let owner = self.StatementCtx.statement_ru_owner.clone()?;
        let setup = owner.take_terminal_setup()?;
        use crate::statement_ru_reporting::StatementRUFailureReason;
        let mut failure = StatementRUFailureReason::NotFinished;
        let terminal = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if owner.final_outcome() != StatementRUFinalOutcome::Success || terminal_error.is_some()
            {
                if terminal_error.is_some() {
                    failure = StatementRUFailureReason::StatementError;
                }
                return None;
            }
            failure = StatementRUFailureReason::Invalid;
            let live = self.Ctx.StatementRUInstallState(&self.StmtNode)?;
            if !live.statement_context_present
                || owner.cursor_at_install
                || live.cursor_exists
                || ((owner.restricted_sql_at_install || live.restricted_sql)
                    && !owner.ttl_job_at_install)
            {
                failure = StatementRUFailureReason::Ineligible;
                return None;
            }
            if !owner.root_eof() {
                failure = StatementRUFailureReason::NotFinished;
                return None;
            }
            self.SnapshotStatementRUEvidence();
            let plan = self.ClassifiedTypedPlan()?;
            let evidence = self.StatementCtx.statement_ru_evidence.as_deref()?;
            if plan.kind == StatementRUPlanKind::Commit {
                let mut calculator = StatementRUCalculator::new(setup);
                let writes = evidence.writes.unwrap_or_default();
                calculator.units.write_keys = writes.keys as f64;
                calculator.units.write_bytes = writes.bytes as f64;
                let mut finalized = calculator.finalize()?;
                finalized.sql_type = "commit".into();
                return Some(finalized);
            }
            if plan.kind == StatementRUPlanKind::PointLookup && plan.plan.id() <= 0 {
                return None;
            }
            let subqueries = self.Ctx.StatementRUScalarSubqueries();
            let forest = astersql_planner_core::FlattenTypedPhysicalPlanForest(
                self.TypedPlan.as_deref()?,
                &subqueries,
            )?;
            if !forest
                .Main
                .first()
                .is_some_and(|root| std::ptr::eq(root.Origin, plan.plan))
            {
                return None;
            }
            match calculate_statement_ru_forest(&forest, evidence, setup, owner.root_eof()) {
                Ok(snapshot) => Some(snapshot),
                Err(state) => {
                    failure = statement_ru_failed(state);
                    None
                }
            }
        }));
        if terminal.is_err() {
            failure = StatementRUFailureReason::Panic;
        }
        let finalized = terminal.ok().flatten();
        if finalized.is_none() && setup.full_report {
            crate::statement_ru_result::publish_statement_ru_failure_safely(
                &crate::statement_ru_result::StatementRUContextSink {
                    context: self.Ctx.as_ref(),
                    ttl_job: owner.ttl_job_at_install,
                },
                failure,
            );
        }
        finalized
    }

    /// Freeze the live sources before cleanup can release statement statistics.
    pub fn SnapshotStatementRUEvidence(&mut self) {
        if self.StatementCtx.statement_ru_evidence.is_some() {
            return;
        }
        let mut ids = Vec::new();
        let scalar_subqueries = self.Ctx.StatementRUScalarSubqueries();
        if let Some(plan) = self.TypedPlan.as_deref() {
            if let Some(forest) =
                astersql_planner_core::FlattenTypedPhysicalPlanForest(plan, &scalar_subqueries)
            {
                for tree in std::iter::once(&forest.Main)
                    .chain(forest.CTEs.iter())
                    .chain(forest.ScalarSubQueries.iter())
                {
                    for operator in tree {
                        ids.push(operator.Origin.id());
                    }
                }
            }
        } else {
            ids.push(self.Plan.id);
        }
        ids.sort_unstable();
        ids.dedup();
        let mut evidence = self.Ctx.StatementRURuntimeEvidence(&ids);
        if evidence.tikv_response_bytes.is_none() {
            evidence.tikv_response_bytes = self
                .StatementCtx
                .ru_metrics
                .as_deref()
                .filter(|metrics| !metrics.Bypass())
                .map(RUV2Metrics::TiKVCoprocessorResponseBytes);
        }
        if evidence.writes.is_none() {
            evidence.writes = self.StatementCtx.commit_details.as_deref().map(|details| {
                crate::statement_ru_plan_walk::snapshot_statement_ru_writes(Some(details))
            });
        }

        evidence.frontend_compile_bytes = self.Ctx.StatementRUFrontendCompileBytes(&self.StmtNode);
        self.StatementCtx.statement_ru_evidence = Some(Arc::new(evidence));
    }

    /// Traverse the original physical operators, if the statement has a typed plan.
    /// A missing or nonphysical plan is never replaced with `PlanInfo` estimates.
    pub fn TypedFlatPlan(&self) -> Option<Vec<astersql_planner_core::TypedFlatOperator<'_>>> {
        astersql_planner_core::FlattenTypedPhysicalPlan(self.TypedPlan.as_deref()?)
    }

    /// Classify the current typed target using the same wrapper order as Go RU.
    pub fn ClassifiedTypedPlan(
        &self,
    ) -> Option<crate::statement_ru_result::StatementRUPlanInfo<'_>> {
        Some(crate::statement_ru_result::classify_statement_ru_plan(
            self.TypedPlan.as_deref()?,
        ))
    }

    /// 返回语句节点。
    pub fn GetStmtNode(&self) -> &StatementNode {
        &self.StmtNode
    }

    /// 走 PointGet 快速路径：高优先级构建并打开点查执行器。
    pub fn PointGet(&mut self) -> AdapterResult<recordSet> {
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.point_get_inner()));
        let result = match result {
            Ok(result) => result,
            Err(panic) => Err(panicError(panic.as_ref(), "ExecStmt.PointGet")),
        };
        if result.is_err() {
            self.RecordStatementRUFinalOutcome(false);
        }
        result
    }

    fn point_get_inner(&mut self) -> AdapterResult<recordSet> {
        let mut context =
            self.observeStmtBeginForTopProfiling(self.GoCtx.clone().unwrap_or_default());
        context.sql_killer = self.Ctx.SQLKillerHandle();
        self.GoCtx = Some(context.clone());
        let start_ts = self.Ctx.StatementReadTS()?;
        self.StatementCtx.priority = Priority::High;
        self.Ctx.SetPriority(Priority::High);
        let mut executor =
            self.Ctx
                .BuildPointGetExecutor(&self.Plan, start_ts, self.PsStmt.as_deref())?;
        if let Err(error) = executor.Open() {
            let _ = executor.Close();
            return Err(error);
        }
        self.Ctx.SetProcessInfo(
            &self.Text(),
            SystemTime::now(),
            self.Ctx.Command(),
            self.Ctx.MaximumExecutionTime(),
        );
        Ok(self.new_record_set(executor, start_ts))
    }

    /// 原始 SQL 文本。
    pub fn OriginText(&self) -> String {
        self.StmtNode.original_text.clone()
    }

    /// 当前（可能改写后的）SQL 文本。
    pub fn Text(&self) -> String {
        self.StmtNode.text.clone()
    }

    /// 是否为预处理语句。
    pub fn IsPrepared(&self) -> bool {
        self.isPreparedStmt
    }

    /// 是否只读语句。
    pub fn IsReadOnly(&self) -> bool {
        self.Ctx.IsReadOnly(&self.StmtNode)
    }

    /// 重建执行计划并更新 InfoSchema 版本。
    pub fn RebuildPlan(&mut self) -> AdapterResult<i64> {
        let rebuilt = self
            .Ctx
            .RebuildPlan(&self.StmtNode, &self.Plan, &self.OutputNames)?;
        self.Plan = rebuilt.summary;
        self.TypedPlan = Some(rebuilt.typed);
        self.OutputNames = rebuilt.output_names;
        self.InfoSchema = rebuilt.schema_version;
        self.StatementCtx.plan = Some(self.Plan.clone());
        Ok(rebuilt.schema_version)
    }

    /// 执行语句入口（捕获 panic）。
    pub fn Exec(&mut self) -> AdapterResult<Option<Box<dyn RecordSet>>> {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.exec_inner()));
        let result = match result {
            Ok(result) => result,
            Err(panic) => Err(panicError(panic.as_ref(), "ExecStmt.Exec")),
        };
        let lock_metrics = self.Ctx.ExecLockMetrics();
        astersql_executor_metrics::executor_metrics::RecordExecLockMetrics(
            self.retryCount,
            lock_metrics.exclusive_keys,
            lock_metrics.shared_keys,
            lock_metrics.exclusive_duration,
            lock_metrics.pessimistic_lock_started,
        );
        self.Ctx.OnExecComplete(result.is_ok());
        if result.is_err() {
            self.RecordStatementRUFinalOutcome(false);
            self.Ctx.CancelMaximumExecutionTime();
        }
        result
    }

    /// 执行核心：构建执行器、外键、无延迟路径或返回 RecordSet。
    fn exec_inner(&mut self) -> AdapterResult<Option<Box<dyn RecordSet>>> {
        if self.isPreparedStmt && !self.Ctx.SupportsPreparedExecution() {
            return Err(errors::New(
                "session-bound typed table scan cannot execute unbound prepared statement",
            ));
        }
        if self.isSelectForUpdate && !self.Ctx.SupportsSelectForUpdate() {
            return Err(errors::New(
                "session-bound typed table scan cannot execute locking SELECT FOR UPDATE",
            ));
        }
        self.inheritContextFromExecuteStmt()?;
        if self.Plan.kind == PlanKind::PointGet {
            return self
                .PointGet()
                .map(|result| Some(Box::new(result) as Box<dyn RecordSet>));
        }
        let (_, plan_digest) = GetPlanDigest(&mut self.StatementCtx, self.Ctx.as_ref());
        self.Ctx.RunawayBeforeExecutor(
            &self.StmtNode,
            &self.StatementCtx.sql_digest.text,
            &plan_digest.text,
        )?;
        let mut context =
            self.observeStmtBeginForTopProfiling(self.GoCtx.clone().unwrap_or_default());
        context.sql_killer = self.Ctx.SQLKillerHandle();
        self.GoCtx = Some(context);
        let execution_started = SystemTime::now();
        let mut executor = self.buildExecutor()?;
        self.Ctx.SetProcessInfo(
            &self.getSQLForProcessInfo(),
            execution_started,
            self.Ctx.Command(),
            statementMaximumExecutionTime(
                &self.Plan,
                &self.StmtNode,
                self.Ctx.MaximumExecutionTime(),
                self.Ctx.DMLMaximumExecutionTime(),
            ),
        );
        if self.StatementCtx.priority == Priority::Unspecified {
            let priority = if self.LowerPriority {
                Priority::Low
            } else {
                Priority::Normal
            };
            self.StatementCtx.priority = priority;
            self.Ctx.SetPriority(priority);
        }
        if let Err(error) = self.openExecutor(executor.as_mut()) {
            let _ = executor.Close();
            return Err(error);
        }
        let pessimistic = self.Ctx.IsPessimistic();
        if self.isSelectForUpdate {
            if self.Ctx.LowResolutionTSO() {
                let _ = executor.Close();
                return Err(errors::New(
                    "can not execute select for update statement when 'tidb_low_resolution_tso' is set",
                ));
            }
            if pessimistic {
                return self.handlePessimisticSelectForUpdate(executor).map(Some);
            }
        }
        self.prepareFKCascadeContext(executor.as_mut());
        let (handled, record_set) = self.handleNoDelay(executor, pessimistic)?;
        if handled {
            return Ok(None);
        }
        let executor = record_set.ok_or_else(|| errors::New("query executor was not returned"))?;
        Ok(Some(Box::new(
            self.new_record_set(executor, self.Ctx.TransactionStartTS()),
        )))
    }

    /// EXECUTE 语句时继承预处理语句上下文。
    pub fn inheritContextFromExecuteStmt(&mut self) -> AdapterResult {
        if self.StmtNode.kind == StatementKind::Execute {
            self.Ctx.InheritExecuteStatement(&mut self.StmtNode)?;
        }
        Ok(())
    }

    /// 供 PROCESSLIST 展示的 SQL（预处理则用 prepared SQL）。
    pub fn getSQLForProcessInfo(&self) -> String {
        if self.isPreparedStmt {
            self.Ctx.PreparedStatementSQL()
        } else {
            self.Text()
        }
    }

    /// 若开启外键检查则准备级联上下文并处理触发器。
    pub fn handleStmtForeignKeyTrigger(
        &mut self,
        executor: &mut dyn ExecExecutor,
    ) -> AdapterResult {
        if executor.HasForeignKeyCascades() {
            self.Ctx.StmtCommit()?;
        }
        if let Err(error) = self.handleForeignKeyTrigger(executor, 1) {
            if let Err(trigger_error) = self.handleFKTriggerError() {
                return Err(errors::New(format!(
                    "handle foreign key trigger error failed, err: {trigger_error}, original_err: {error}"
                )));
            }
            return Err(error);
        }
        let savepoint = self.Ctx.ForeignKeySavepointName();
        if !savepoint.is_empty() {
            self.Ctx.ReleaseForeignKeySavepoint(&savepoint);
        }
        Ok(())
    }

    /// 递归处理外键检查与级联，受 MAX_FOREIGN_KEY_CASCADE_DEPTH 限制。
    pub fn handleForeignKeyTrigger(
        &mut self,
        executor: &mut dyn ExecExecutor,
        depth: usize,
    ) -> AdapterResult {
        executor.CheckForeignKeys()?;
        for mut cascade in executor.TakeForeignKeyCascades() {
            self.handleForeignKeyCascade(cascade.as_mut(), depth)?;
        }
        Ok(())
    }

    /// 消费级联批中待处理行，逐批构建子执行器。
    pub fn handleForeignKeyCascade(
        &mut self,
        cascade: &mut dyn CascadeBatch,
        depth: usize,
    ) -> AdapterResult {
        if !cascade.HasPendingRows() {
            return Ok(());
        }
        if depth > MAX_FOREIGN_KEY_CASCADE_DEPTH {
            return Err(errors::New(format!(
                "foreign-key cascade depth exceeded: {MAX_FOREIGN_KEY_CASCADE_DEPTH}"
            )));
        }
        self.Ctx.SetInHandleForeignKeyTrigger(true);
        let _trigger_guard = ForeignKeyTriggerGuard(self.Ctx.clone());
        while cascade.HasPendingRows() {
            let Some(mut executor) = cascade.BuildExecutor()? else {
                break;
            };
            if let Err(error) = self.openExecutor(executor.as_mut()) {
                let _ = executor.Close();
                return Err(error);
            }
            let mut output = executor.NewChunk();
            let run_result = self.next(executor.as_mut(), &mut output);
            let close_result = executor.Close();
            if let Err(error) = run_result.and(close_result) {
                return Err(error);
            }
            self.Ctx.StmtCommit()?;
            self.handleForeignKeyTrigger(executor.as_mut(), depth + 1)?;
            cascade.MarkBatchComplete();
        }
        Ok(())
    }

    /// 在运行时与执行器上准备外键级联上下文。
    pub fn prepareFKCascadeContext(&mut self, executor: &mut dyn ExecExecutor) {
        if !executor.HasForeignKeyCascades() {
            return;
        }
        self.Ctx.PrepareFKCascadeContext();
        executor.PrepareFKCascadeContext();
    }

    /// 处理外键触发器错误。
    pub fn handleFKTriggerError(&self) -> AdapterResult {
        self.Ctx.HandleFKTriggerError()
    }

    /// 无结果/无延迟计划：当场跑完执行器并收尾，不返回 RecordSet。
    pub fn handleNoDelay(
        &mut self,
        mut executor: Box<dyn ExecExecutor>,
        pessimistic: bool,
    ) -> AdapterResult<(bool, Option<Box<dyn ExecExecutor>>)> {
        if !executor.Schema().is_empty() && !executor.CalculateNoDelay() {
            return Ok((false, Some(executor)));
        }
        let result = if pessimistic && executor.Schema().is_empty() {
            self.handlePessimisticDML(executor.as_mut())
        } else {
            self.handleNoDelayExecutor(executor.as_mut())
        };
        self.Ctx.DetachTrackers();
        let cte_result = resetCTEStorageMap(self.Ctx.as_ref());
        result.and(cte_result)?;
        Ok((true, None))
    }

    /// 包装执行器为流式 recordSet。
    fn new_record_set(
        &self,
        executor: Box<dyn ExecExecutor>,
        transaction_start_ts: u64,
    ) -> recordSet {
        let schema = executor.Schema().to_vec();
        let config = executor.ChunkConfig();
        recordSet {
            fields: Vec::new(),
            executor: Some(executor),
            schema,
            stmt: self.clone(),
            lastErrs: Vec::new(),
            txnStartTS: transaction_start_ts,
            finished: false,
            traceID: self
                .GoCtx
                .as_ref()
                .map(|context| context.trace_id.clone())
                .unwrap_or_default(),
            chunk_config: config,
        }
    }
}

pub(crate) fn statementMaximumExecutionTime(
    plan: &PlanInfo,
    statement: &StatementNode,
    select_timeout: u64,
    dml_timeout: u64,
) -> u64 {
    if statement.kind == StatementKind::Select {
        select_timeout
    } else if plan.IsDML() {
        dml_timeout
    } else {
        0
    }
}

/// 是否可走快速计划路径（PointGet/TableDual/Set，或投影到它们）。
pub fn IsFastPlan(plan: &PlanInfo) -> bool {
    let plan = if plan.kind == PlanKind::Projection {
        plan.projection_child.as_deref().unwrap_or(plan)
    } else {
        plan
    };
    matches!(
        plan.kind,
        PlanKind::PointGet | PlanKind::TableDual | PlanKind::Set
    )
}

/// 是否无需向客户端返回结果行。
pub fn isNoResultPlan(plan: &PlanInfo) -> bool {
    plan.schema.is_empty() || (plan.kind == PlanKind::Projection && plan.calculate_no_delay)
}

/// 已物化到内存 chunk 列表的结果集（如悲观 SELECT FOR UPDATE）。
pub struct chunkRowRecordSet {
    pub chunks: Vec<chunk::Chunk>,
    pub chunkIndex: usize,
    pub rowIndex: usize,
    pub fields: Vec<ResultField>,
    pub chunkConfig: ChunkConfig,
    pub execStmt: ExecStmt,
    pub closed: bool,
}

impl chunkRowRecordSet {
    pub fn Fields(&self) -> &[ResultField] {
        &self.fields
    }

    pub fn Next(&mut self, output: &mut chunk::Chunk) -> AdapterResult {
        output.Reset();
        while !output.IsFull() && self.chunkIndex < self.chunks.len() {
            let source = &self.chunks[self.chunkIndex];
            let remaining = source.NumRows().saturating_sub(self.rowIndex);
            if remaining == 0 {
                self.chunkIndex += 1;
                self.rowIndex = 0;
                continue;
            }
            let capacity = output
                .RequiredRows()
                .saturating_sub(output.NumRows())
                .max(1);
            let count = remaining.min(capacity);
            output.Append(source, self.rowIndex, self.rowIndex + count);
            self.rowIndex += count;
        }
        Ok(())
    }

    pub fn NewChunk(&self) -> chunk::Chunk {
        self.execStmt.Ctx.NewChunk(&self.chunkConfig)
    }

    pub fn Close(&mut self) -> AdapterResult {
        if !self.closed {
            self.closed = true;
            self.execStmt
                .CloseRecordSet(self.execStmt.Ctx.TransactionStartTS(), None);
        }
        Ok(())
    }
}

impl RecordSet for chunkRowRecordSet {
    fn Fields(&mut self) -> &[ResultField] {
        chunkRowRecordSet::Fields(self)
    }

    fn Next(&mut self, output: &mut chunk::Chunk) -> AdapterResult {
        chunkRowRecordSet::Next(self, output)
    }

    fn NewChunk(&mut self) -> chunk::Chunk {
        chunkRowRecordSet::NewChunk(self)
    }

    fn Close(&mut self) -> AdapterResult {
        chunkRowRecordSet::Close(self)
    }
}

impl ExecStmt {
    /// 悲观 SELECT FOR UPDATE：加锁循环，失败可重建执行器重试。
    pub fn handlePessimisticSelectForUpdate(
        &mut self,
        mut executor: Box<dyn ExecExecutor>,
    ) -> AdapterResult<Box<dyn RecordSet>> {
        if self.Ctx.SnapshotTS() != 0 {
            let _ = executor.Close();
            return Err(errors::New(
                "can not execute write statement when 'tidb_snapshot' is set",
            ));
        }
        self.Ctx.OnPessimisticStmtStart()?;
        let result = loop {
            match self.runPessimisticSelectForUpdate(executor.as_mut()) {
                Ok(record_set) => break Ok(record_set),
                Err(error) => match self.handlePessimisticLockError(error) {
                    Ok(Some(retry)) => executor = retry,
                    Ok(None) => break Err(errors::New("pessimistic select retry lost executor")),
                    Err(error) => break Err(error),
                },
            }
        };
        let end = self.Ctx.OnPessimisticStmtEnd(result.is_ok());
        self.Ctx
            .FinalizePreparedExecution(executor.ScannedRows(), result.is_ok() && end.is_ok());
        match (result, end) {
            (Ok(result), Ok(())) => Ok(result),
            (Err(error), _) => Err(error),
            (_, Err(error)) => Err(error),
        }
    }

    /// 拉取全部行到 chunkRowRecordSet 并关闭执行器。
    pub fn runPessimisticSelectForUpdate(
        &mut self,
        executor: &mut dyn ExecExecutor,
    ) -> AdapterResult<Box<dyn RecordSet>> {
        let config = executor.ChunkConfig();
        let fields = colNames2ResultFields(
            executor.Schema(),
            &self.OutputNames,
            &self.Ctx.CurrentDatabase(),
        );
        let mut chunks = Vec::new();
        let context = self.GoCtx.clone().unwrap_or_default();
        let result = (|| -> AdapterResult {
            loop {
                self.Ctx.KillSignal()?;
                let mut chunk = executor.NewChunk();
                self.nextWithContext(&context, executor, &mut chunk)?;
                if chunk.NumRows() == 0 {
                    self.recordStatementRURootEOF();
                    break Ok(());
                }
                let keys = executor.TakeLockKeys();
                if !keys.is_empty() {
                    let started = Instant::now();
                    let locked = self.Ctx.LockKeys(&keys, false);
                    self.phaseLockDurations[0] += started.elapsed();
                    locked?;
                }
                chunks.push(chunk);
            }
        })();
        let close = executor.Close();
        result?;
        close?;
        Ok(Box::new(chunkRowRecordSet {
            chunks,
            chunkIndex: 0,
            rowIndex: 0,
            fields,
            chunkConfig: config,
            execStmt: self.clone(),
            closed: false,
        }))
    }

    /// 无延迟执行：校验快照限制，Next 一次后处理外键并关闭。
    pub fn handleNoDelayExecutor(&mut self, executor: &mut dyn ExecExecutor) -> AdapterResult {
        let result = (|| {
            if executor.IsWriteExecutor() {
                if self.Ctx.SnapshotTS() != 0 {
                    return Err(errors::New(
                        "can not execute write statement when 'tidb_snapshot' is set",
                    ));
                }
                if self.Ctx.LowResolutionTSO() {
                    return Err(errors::New(
                        "can not execute write statement when 'tidb_low_resolution_tso' is set",
                    ));
                }
            }
            let mut output = executor.NewChunk();
            self.next(executor, &mut output)?;
            if self.ClassifiedTypedPlan().is_some_and(|plan| {
                matches!(
                    plan.kind,
                    crate::statement_ru_result::StatementRUPlanKind::Analyze
                        | crate::statement_ru_result::StatementRUPlanKind::Write
                        | crate::statement_ru_result::StatementRUPlanKind::Commit
                )
            }) {
                self.recordStatementRURootEOF();
            }
            self.handleStmtForeignKeyTrigger(executor)
        })();
        let scanned_rows = executor.ScannedRows();
        let _ = executor.Close();
        self.Ctx
            .FinalizePreparedExecution(scanned_rows, result.is_ok());
        self.logAudit();
        result
    }

    /// 悲观 DML：起止钩子包裹 runPessimisticDML。
    pub fn handlePessimisticDML(&mut self, executor: &mut dyn ExecExecutor) -> AdapterResult {
        let mut transaction = self.Ctx.PessimisticTransaction()?;
        self.Ctx.OnPessimisticStmtStart()?;
        let result = self.runPessimisticDML(executor, transaction.as_mut());
        let end = self.Ctx.OnPessimisticStmtEnd(result.is_ok());
        let result = match (result, end) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), _) => Err(error),
            (_, Err(error)) => Err(error),
        };
        match result {
            Err(error) => match self
                .Ctx
                .AbortLazyUniquenessOnPessimisticDMLFailure(&error)?
            {
                Some(wrapped) => Err(wrapped),
                None => Err(error),
            },
            Ok(()) => Ok(()),
        }
    }

    /// 悲观 DML 核心：执行、收集锁键、排他/共享加锁，失败则重试。
    fn runPessimisticDML(
        &mut self,
        initial: &mut dyn ExecExecutor,
        transaction: &mut dyn pessimisticTxn,
    ) -> AdapterResult {
        let mut retry_executor: Option<Box<dyn ExecExecutor>> = None;
        loop {
            self.Ctx.ResetUnchangedKeysForLock();
            let executor: &mut dyn ExecExecutor = retry_executor
                .as_mut()
                .map(|executor| executor.as_mut())
                .unwrap_or(initial);
            let execution = self.handleNoDelayExecutor(executor);
            if !transaction.IsValid() {
                return execution;
            }
            if let Err(error) = execution {
                match self.handlePessimisticLockError(error)? {
                    Some(retry) => {
                        retry_executor = Some(retry);
                        continue;
                    }
                    None => return Err(errors::New("pessimistic DML retry lost executor")),
                }
            }
            let keys = transaction.KeysNeedToLock()?;
            let mut exclusive = self.Ctx.CollectUnchangedKeysForXLock(keys);
            let mut shared = self.Ctx.CollectUnchangedKeysForSLock(Vec::new());
            if !self.Ctx.ForeignKeyCheckInSharedLock() {
                exclusive.extend(shared);
                shared = Vec::new();
            } else {
                (exclusive, shared) =
                    moveWrittenSharedLockKeysToExclusive(&*transaction, exclusive, shared)?;
            }
            let lock_started = Instant::now();
            if let Err(error) = self.Ctx.LockKeys(&exclusive, false) {
                match self.handlePessimisticLockError(error)? {
                    Some(retry) => {
                        retry_executor = Some(retry);
                        continue;
                    }
                    None => return Err(errors::New("exclusive-lock retry lost executor")),
                }
            }
            if let Err(error) = self.Ctx.LockKeys(&shared, true) {
                match self.handlePessimisticLockError(error)? {
                    Some(retry) => {
                        retry_executor = Some(retry);
                        continue;
                    }
                    None => return Err(errors::New("shared-lock retry lost executor")),
                }
            }
            let executor: &mut dyn ExecExecutor = retry_executor
                .as_mut()
                .map(|executor| executor.as_mut())
                .unwrap_or(initial);
            updateFKCheckLockStats(executor, lock_started.elapsed());
            return Ok(());
        }
    }

    /// 按运行时决策返回错误或重建执行器以重试。
    pub fn handlePessimisticLockError(
        &mut self,
        lock_error: errors::SharedError,
    ) -> AdapterResult<Option<Box<dyn ExecExecutor>>> {
        match self.Ctx.OnPessimisticLockError(&lock_error)? {
            PessimisticErrorAction::ReturnError => Err(lock_error),
            PessimisticErrorAction::RetryReady => {
                if let PlanKind::LoadData(location) = self.Plan.kind {
                    if !canRetryPessimisticLoadData(location) {
                        return Err(lock_error);
                    }
                }
                if self.retryCount >= self.Ctx.MaximumPessimisticRetries() {
                    return Err(errors::New("pessimistic lock retry limit reached"));
                }
                self.retryCount += 1;
                self.retryStartTime = Some(Instant::now());
                self.Ctx.OnPessimisticStmtRetry()?;
                self.resetPhaseDurations();
                self.inheritContextFromExecuteStmt()?;
                let mut executor = self.buildExecutor()?;
                self.Ctx.RollbackStatementForRetry()?;
                self.Ctx.ResetStatementForRetry();
                self.openExecutor(executor.as_mut())?;
                Ok(Some(executor))
            }
        }
    }

    /// 构建执行器并累计 build 阶段耗时。
    pub fn buildExecutor(&mut self) -> AdapterResult<Box<dyn ExecExecutor>> {
        let started = Instant::now();
        let result = if self.isSelectForUpdate {
            self.Ctx
                .BuildExecutorForSelectLock(&self.Plan, self.Ti.as_ref())
        } else {
            self.Ctx.BuildExecutor(&self.Plan, self.Ti.as_ref())
        };
        self.phaseBuildDurations[0] += started.elapsed();
        result
    }

    /// 打开执行器并累计 open 阶段耗时。
    pub fn openExecutor(&mut self, executor: &mut dyn ExecExecutor) -> AdapterResult {
        let started = Instant::now();
        let result = executor.Open();
        self.phaseOpenDurations[0] += started.elapsed();
        result
    }

    /// 拉取下一批并累计 next 阶段耗时。
    pub fn next(
        &mut self,
        executor: &mut dyn ExecExecutor,
        output: &mut chunk::Chunk,
    ) -> AdapterResult {
        let started = Instant::now();
        let result = executor.Next(output);
        self.phaseNextDurations[0] += started.elapsed();
        result
    }

    pub fn nextWithContext(
        &mut self,
        context: &ExecutionContext,
        executor: &mut dyn ExecExecutor,
        output: &mut chunk::Chunk,
    ) -> AdapterResult {
        let started = Instant::now();
        let result = executor.NextWithContext(context, output);
        self.phaseNextDurations[0] += started.elapsed();
        result
    }

    /// 重试前将本轮阶段耗时累入历史槽并清零本轮。
    pub fn resetPhaseDurations(&mut self) {
        for durations in [
            &mut self.phaseBuildDurations,
            &mut self.phaseOpenDurations,
            &mut self.phaseNextDurations,
            &mut self.phaseLockDurations,
        ] {
            durations[1] += durations[0];
            durations[0] = Duration::ZERO;
        }
    }
}

/// 已写入的共享锁键提升为排他锁键集合。
pub fn moveWrittenSharedLockKeysToExclusive(
    transaction: &dyn pessimisticTxn,
    mut exclusive: Vec<Key>,
    shared: Vec<Key>,
) -> AdapterResult<(Vec<Key>, Vec<Key>)> {
    let mut exclusive_set = exclusive.iter().cloned().collect::<HashSet<_>>();
    let mut shared_only = Vec::with_capacity(shared.len());
    for key in shared {
        if exclusive_set.contains(&key) {
            continue;
        }
        if transaction.IsKeyWritten(&key)? {
            exclusive_set.insert(key.clone());
            exclusive.push(key);
        } else {
            shared_only.push(key);
        }
    }
    Ok((exclusive, shared_only))
}

/// 将加锁耗时计入外键检查统计。
pub fn updateFKCheckLockStats(executor: &mut dyn ExecExecutor, lock_duration: Duration) {
    executor.AddFKCheckLockDuration(lock_duration);
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 提交阶段耗时明细。
pub struct CommitDetails {
    pub prewrite_time: Duration,
    pub commit_time: Duration,
    pub get_commit_ts_time: Duration,
    pub get_latest_ts_time: Duration,
    pub local_latch_time: Duration,
    pub wait_prewrite_binlog_time: Duration,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FairLockingFinishMetrics {
    pub stmt_used: bool,
    pub stmt_effective: bool,
    pub txn_used: bool,
    pub txn_effective: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExecLockMetrics {
    pub exclusive_keys: i32,
    pub shared_keys: i32,
    pub exclusive_duration: Duration,
    pub pessimistic_lock_started: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SupplementaryFinishMetrics {
    pub tiflash: bool,
    pub read_from_table_cache: bool,
}

#[derive(Clone, Debug, Default)]
/// 语句摘要信息（供 statements_summary 系统表）。
pub struct StatementSummary {
    pub original_sql: String,
    pub normalized_sql: String,
    pub sql_digest: String,
    pub plan_digest: String,
    pub binary_plan: String,
    pub encoded_plan: String,
    pub success: bool,
    pub ru_version: u8,
    pub total_ru_v2: Option<f64>,
    pub is_write: bool,
    pub ru_details: Option<astersql_util_execdetails::execdetails::util::RUDetails>,
}

impl ExecStmt {
    /// 写出审计日志。
    pub fn logAudit(&self) {
        self.Ctx.Audit(&self.Text());
    }

    /// 上报各执行阶段及可选 commit/backoff 耗时。
    pub fn observePhaseDurations(&self, internal: bool, commit: Option<&CommitDetails>) {
        for (phase, duration) in [
            ("build_final", self.phaseBuildDurations[0]),
            ("build_locking", self.phaseBuildDurations[1]),
            ("open_final", self.phaseOpenDurations[0]),
            ("open_locking", self.phaseOpenDurations[1]),
            ("next_final", self.phaseNextDurations[0]),
            ("next_locking", self.phaseNextDurations[1]),
            ("lock_final", self.phaseLockDurations[0]),
            ("lock_locking", self.phaseLockDurations[1]),
        ] {
            if duration > Duration::ZERO {
                getPhaseDurationObserver(self.Ctx.as_ref(), phase, internal).Observe(duration);
            }
        }
        if let Some(commit) = commit {
            for (phase, duration) in [
                ("commit_prewrite", commit.prewrite_time),
                ("commit_commit", commit.commit_time),
                ("commit_wait_commit_ts", commit.get_commit_ts_time),
                ("commit_wait_latest_ts", commit.get_latest_ts_time),
                ("commit_wait_latch", commit.local_latch_time),
                ("commit_wait_binlog", commit.wait_prewrite_binlog_time),
            ] {
                if duration > Duration::ZERO {
                    getPhaseDurationObserver(self.Ctx.as_ref(), phase, internal).Observe(duration);
                }
            }
        }
    }

    /// 语句结束收尾：审计、TopSQL、指标、慢查询、摘要等。
    pub fn FinishExecuteStmt(
        &mut self,
        transaction_ts: u64,
        error: Option<errors::SharedError>,
        has_more_results: bool,
    ) {
        let success = error.is_none();
        let ru_already_published = self.StatementCtx.statement_ru_finalized.is_some();
        self.Ctx
            .OnFinishStatement(self.retryCount, success, self.StatementCtx.affected_rows);
        self.logAudit();
        self.checkPlanReplayerCapture(transaction_ts);
        self.Ctx.AttachFinishRuntimeStats(self.Plan.id);
        if self.StatementCtx.statement_ru_owner.is_none() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.SnapshotStatementRUEvidence();
            }));
        }
        self.StatementCtx.plan = Some(self.Plan.clone());
        self.Ctx.SetStatementContext(&self.StatementCtx);
        self.finalizeStatementRUV2Metrics();
        if self.StatementCtx.statement_ru_finalized.is_none() {
            self.StatementCtx.total_ru = 0.0;
        }
        self.updateNetworkTrafficStatsAndMetrics();
        if let Some(finalized) = self.finishStatementRU(error.as_ref()) {
            self.StatementCtx.total_ru = finalized.result.total_ru;
            self.StatementCtx.statement_ru_finalized = Some(Arc::new(finalized.clone()));
            crate::statement_ru_result::publish_statement_ru_finalized_snapshot(
                &crate::statement_ru_result::StatementRUContextSink {
                    context: self.Ctx.as_ref(),
                    ttl_job: self
                        .StatementCtx
                        .statement_ru_owner
                        .as_ref()
                        .is_some_and(|owner| owner.ttl_job_at_install),
                },
                &finalized,
            );
        }
        self.Ctx.SetStatementContext(&self.StatementCtx);
        self.LogSlowQuery(transaction_ts, success, has_more_results);
        self.SummaryStmt(success);
        if !ru_already_published {
            self.observeStmtFinishedForTopProfiling();
        }
        self.UpdatePlanCacheRuntimeInfo();
        let supplementary = self.Ctx.SupplementaryFinishMetrics();
        let rfc_error_code = error.as_ref().and_then(FinishErrorRFCCode);
        astersql_executor_metrics::executor_metrics::RecordSupplementaryFinishMetrics(
            supplementary.tiflash,
            success,
            rfc_error_code.as_deref(),
            supplementary.read_from_table_cache,
        );
        self.updatePrevStmt();
        self.recordLastQueryInfo(error.as_ref(), self.StatementCtx.total_ru);
        self.recordAffectedRows2Metrics();
        let commit = self.Ctx.CommitDetailsForFinish();
        self.observePhaseDurations(self.Ctx.RestrictedSQL(), commit.as_ref());
        if let Some(duration) = self.Ctx.ExecuteRunDurationForFinish() {
            astersql_executor_metrics::executor_metrics::RecordStatementExecuteRunDuration(
                self.Ctx.RestrictedSQL(),
                duration,
            );
        }
        self.Ctx
            .ObserveStatementDuration(&self.StatementCtx.statement_type);
        let fair = self.Ctx.FairLockingFinishMetrics();
        astersql_executor_metrics::executor_metrics::RecordFairLockingFinishMetrics(
            fair.stmt_used,
            fair.stmt_effective,
            fair.txn_used,
            fair.txn_effective,
        );
        self.Ctx.DetachTrackers();
        self.Ctx.CleanupAfterFinish();
    }

    /// 按语句类型记录受影响行与列乘积指标。
    pub fn recordAffectedRows2Metrics(&self) {
        let affected = self.StatementCtx.affected_rows as i64;
        let columns = self.Plan.schema.len() as i64;
        match self.StmtNode.kind {
            StatementKind::Insert | StatementKind::Replace => {
                recordInsertRowsColMultiply2Metrics(
                    self.Ctx.as_ref(),
                    &self.StatementCtx.statement_type,
                    affected.saturating_mul(columns),
                );
            }
            StatementKind::Update | StatementKind::Delete => {
                recordDMLRowsColMultiply2Metrics(
                    self.Ctx.as_ref(),
                    &self.StatementCtx.statement_type,
                    affected,
                    columns,
                );
            }
            _ => {}
        }
    }

    /// Transfer pending coprocessor response bytes before statement RU calculation.
    /// The terminal statement snapshot owns calculation and publication.
    pub fn finalizeStatementRUV2Metrics(&mut self) {
        let Some(metrics) = self.StatementCtx.ru_metrics.as_deref() else {
            return;
        };
        if metrics.Bypass() {
            return;
        }
        let details = self
            .GoCtx
            .as_ref()
            .and_then(|context| context.ru_details.as_deref());
        SyncRUV2MetricsFromRUDetails(Some(metrics), details);
    }

    /// 记录上次查询信息（含错误）。
    pub fn recordLastQueryInfo(&self, error: Option<&errors::SharedError>, total_ru_v2: f64) {
        self.Ctx
            .RecordLastQuery(error.map(|error| error.to_string()).as_deref(), total_ru_v2);
    }

    /// 检查是否触发 Plan Replayer 捕获任务。
    pub fn checkPlanReplayerCapture(&mut self, transaction_ts: u64) {
        checkPlanReplayerCaptureTask(self, transaction_ts);
        checkPlanReplayerContinuesCapture(self, transaction_ts);
    }

    /// RecordSet 关闭时调用 FinishExecuteStmt。
    pub fn CloseRecordSet(
        &mut self,
        transaction_start_ts: u64,
        last_error: Option<errors::SharedError>,
    ) {
        self.FinishExecuteStmt(transaction_start_ts, last_error, false);
    }

    /// 按需写入慢查询日志。
    pub fn LogSlowQuery(&self, transaction_ts: u64, success: bool, has_more_results: bool) {
        self.Ctx.SlowQuery(
            transaction_ts,
            &self.GetTextToLog(false),
            success,
            has_more_results,
        );
    }

    /// 上报网络收发与 MPP 流量。
    pub fn updateNetworkTrafficStatsAndMetrics(&self) {
        self.Ctx.RecordNetworkTraffic(
            self.StatementCtx.network_sent_bytes,
            self.StatementCtx.network_received_bytes,
            self.StatementCtx.mpp_network_bytes,
        );
    }

    /// 是否产生了 MPP 网络流量。
    pub fn updateMPPNetworkTraffic(&self) -> bool {
        self.StatementCtx.mpp_network_bytes > 0
    }

    /// 构造并上报语句摘要。
    pub fn SummaryStmt(&mut self, success: bool) {
        let (_, plan_digest) = GetPlanDigest(&mut self.StatementCtx, self.Ctx.as_ref());
        let ru_details = self
            .GoCtx
            .as_ref()
            .and_then(|context| context.ru_details.as_deref())
            .map(
                |details| astersql_util_execdetails::execdetails::util::RUDetails {
                    read_ru: details.RRU(),
                    write_ru: details.WRU(),
                    ru_wait_duration: details.RUWaitDuration(),
                    ..Default::default()
                },
            );
        let total_ru_v2 = self
            .StatementCtx
            .ru_metrics
            .as_deref()
            .filter(|metrics| !metrics.Bypass())
            .map(|_| self.StatementCtx.total_ru);
        let is_write = matches!(
            self.Plan.kind,
            PlanKind::Insert | PlanKind::Update | PlanKind::Delete
        ) || self
            .StmtNode
            .text
            .trim()
            .trim_end_matches(';')
            .eq_ignore_ascii_case("commit");
        let ru_details =
            SelectRUDetailsForStatementLog(ru_details, self.Ctx.RUVersion(), total_ru_v2, is_write);
        let summary = StatementSummary {
            original_sql: self.GetOriginalSQL(),
            normalized_sql: self.StatementCtx.sql_normalized.clone(),
            sql_digest: self.StatementCtx.sql_digest.text.clone(),
            plan_digest: plan_digest.text,
            binary_plan: self.GetBinaryPlan(),
            encoded_plan: self.GetEncodedPlan().0,
            success,
            ru_version: self.Ctx.RUVersion(),
            total_ru_v2,
            is_write,
            ru_details,
        };
        self.Ctx.Summary(&summary);
    }

    /// 原始 SQL。
    pub fn GetOriginalSQL(&self) -> String {
        self.OriginText()
    }

    /// 返回编码计划、二进制计划及可选编码错误。
    pub fn GetEncodedPlan(&mut self) -> (String, String, Option<errors::SharedError>) {
        if let Some(error) = self.StatementCtx.plan_encode_error.as_ref() {
            return (
                String::new(),
                String::new(),
                Some(errors::New(error.clone())),
            );
        }
        let (plan, hint) = getEncodedPlan(&self.StatementCtx, true);
        (plan, hint, None)
    }

    /// 返回二进制计划编码字符串。
    pub fn GetBinaryPlan(&self) -> String {
        getBinaryPlan(&self.StatementCtx)
    }

    pub fn GetPlanDigest(&mut self) -> String {
        GetPlanDigest(&mut self.StatementCtx, self.Ctx.as_ref()).0
    }

    pub fn GetBindingSQLAndDigest(&self) -> (String, String) {
        (
            self.StatementCtx.binding_sql.clone(),
            self.StatementCtx.binding_sql_digest.clone(),
        )
    }

    /// 生成写入日志的 SQL 文本（可选择保留 hint，并按需脱敏）。
    pub fn GetTextToLog(&self, keep_hint: bool) -> String {
        if self.Ctx.RedactLog() || REDACT_LOG_ENABLED.load(Ordering::Acquire) {
            return self.StmtNode.secure_text.clone();
        }
        let text = if keep_hint {
            self.Text()
        } else {
            removeSQLHints(&self.Text())
        };
        formatSQL(&text)
    }

    pub fn getLazyStmtText(&self) -> String {
        self.GetTextToLog(false)
    }

    /// 更新会话“上一条语句”缓存。
    pub fn updatePrevStmt(&self) {
        self.Ctx
            .UpdatePreviousStatement(&self.getLazyStmtText(), &self.StatementCtx.sql_digest.text);
    }

    /// TopSQL 语句开始：记录 digest 并返回带 trace 的上下文。
    pub fn observeStmtBeginForTopProfiling(
        &mut self,
        context: ExecutionContext,
    ) -> ExecutionContext {
        if !self.Ctx.RestrictedSQL() {
            let (_, plan_digest) = GetPlanDigest(&mut self.StatementCtx, self.Ctx.as_ref());
            self.Ctx
                .TopSQLStart(&self.StatementCtx.sql_digest.bytes, &plan_digest.bytes);
        }
        context
    }

    /// 更新计划缓存运行时命中/未命中信息。
    pub fn UpdatePlanCacheRuntimeInfo(&self) {
        self.Ctx.RecordPlanCache(
            self.StatementCtx.use_plan_cache,
            self.StatementCtx.plan_cache_unqualified.as_deref(),
        );
    }

    /// TopSQL 语句结束钩子。
    pub fn observeStmtFinishedForTopProfiling(&self) {
        self.Ctx.TopSQLFinish(self.StatementCtx.total_ru);
    }

    pub fn getSQLPlanDigest(&mut self) -> (Vec<u8>, Vec<u8>) {
        let (_, plan_digest) = GetPlanDigest(&mut self.StatementCtx, self.Ctx.as_ref());
        (
            self.StatementCtx.sql_digest.bytes.clone(),
            plan_digest.bytes,
        )
    }
}

pub fn FinishErrorRFCCode(error: &errors::SharedError) -> Option<String> {
    error
        .downcast_ref::<errors::Error>()
        .map(|error| error.RFCCode().to_string())
}

/// SQL 展示格式化包装（Display 输出折叠空白后的文本）。
pub struct SqlFormatter(String);

impl fmt::Display for SqlFormatter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&formatSQL(&self.0))
    }
}

/// 将 SQL 空白规范化后包装为 SqlFormatter。
pub fn FormatSQL(sql: &str) -> SqlFormatter {
    SqlFormatter(sql.to_owned())
}

/// 折叠制表符/换行等为空白的 SQL 字符串。
pub fn formatSQL(sql: &str) -> String {
    let max_query_len = QueryLogMaxLen.Load();
    let truncated = if max_query_len > 0 && sql.len() > max_query_len as usize {
        let mut end = max_query_len as usize;
        while !sql.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}(len:{})", &sql[..end], sql.len())
    } else {
        sql.to_owned()
    };
    truncated.replace(['\r', '\n', '\t'], " ")
}

/// 阶段耗时观察者：绑定 runtime 与阶段名。
pub struct PhaseDurationObserver<'a> {
    runtime: &'a dyn AdapterRuntime,
    phase: &'a str,
    internal: bool,
}

impl PhaseDurationObserver<'_> {
    pub fn Observe(&self, duration: Duration) {
        astersql_executor_metrics::executor_metrics::RecordPhaseDuration(
            self.phase,
            self.internal,
            duration,
        );
        self.runtime
            .ObservePhase(self.phase, self.internal, duration);
    }
}

/// 构造阶段耗时观察者。
pub fn getPhaseDurationObserver<'a>(
    runtime: &'a dyn AdapterRuntime,
    phase: &'a str,
    internal: bool,
) -> PhaseDurationObserver<'a> {
    PhaseDurationObserver {
        runtime,
        phase,
        internal,
    }
}

/// 记录 Update/Delete 受影响行与列数相关指标。
pub fn recordDMLRowsColMultiply2Metrics(
    runtime: &dyn AdapterRuntime,
    statement_type: &str,
    row_count: i64,
    column_count: i64,
) {
    if row_count <= 0 || column_count <= 0 {
        return;
    }
    runtime.RecordDMLMetric(statement_type, row_count.saturating_mul(column_count));
}

/// 记录 Insert/Replace 行×列指标。
pub fn recordInsertRowsColMultiply2Metrics(
    runtime: &dyn AdapterRuntime,
    statement_type: &str,
    rows_column_product: i64,
) {
    runtime.RecordDMLMetric(statement_type, rows_column_product);
}

/// 重置 CTE（公用表表达式）存储映射。
pub fn resetCTEStorageMap(runtime: &dyn AdapterRuntime) -> AdapterResult {
    runtime.ResetCTEStorage()
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 慢查询转储触发配置占位。
pub struct DumpTriggerConfig {
    pub slow_query: bool,
    pub plan_digests: HashSet<String>,
}

/// 检查是否应触发慢查询转储。
pub fn slowQueryDumpTriggerCheck(config: &DumpTriggerConfig) -> bool {
    config.slow_query
}

/// 获取或缓存扁平化计划。
pub fn getFlatPlan(statement_context: &mut StatementContext) -> Option<PlanInfo> {
    if let Some(flat) = statement_context.flat_plan.as_ref() {
        return Some(flat.clone());
    }
    let flat = statement_context.plan.clone()?;
    statement_context.flat_plan = Some(flat.clone());
    Some(flat)
}

/// 获取二进制计划编码。
pub fn getBinaryPlan(statement_context: &StatementContext) -> String {
    statement_context
        .flat_plan
        .as_ref()
        .or(statement_context.plan.as_ref())
        .map(|plan| plan.binary.clone())
        .unwrap_or_default()
}

/// 获取计划树文本。
pub fn getPlanTree(statement_context: &StatementContext) -> String {
    statement_context
        .plan
        .as_ref()
        .map(|plan| plan.encoded.clone())
        .unwrap_or_default()
}

/// 计算或返回计划 digest。
pub fn GetPlanDigest(
    statement_context: &mut StatementContext,
    runtime: &dyn AdapterRuntime,
) -> (String, Digest) {
    if let Some(cached) = statement_context.plan_digest.as_ref() {
        return cached.clone();
    }
    let normalized = getPlanTree(statement_context);
    let digest = runtime.Digest(&normalized);
    statement_context.plan_digest = Some((normalized.clone(), digest.clone()));
    (normalized, digest)
}

/// 获取编码计划文本与是否来自缓存。
pub fn getEncodedPlan(
    statement_context: &StatementContext,
    generate_hint: bool,
) -> (String, String) {
    statement_context.plan.as_ref().map_or_else(
        || (String::new(), String::new()),
        |plan| {
            (
                plan.encoded.clone(),
                if generate_hint {
                    plan.hints.clone()
                } else {
                    String::new()
                },
            )
        },
    )
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 计划 digest 别名包装。
pub struct planDigestAlias(pub Digest);

impl planDigestAlias {
    pub fn planDigestDumpTriggerCheck(&self, config: &DumpTriggerConfig) -> bool {
        config.plan_digests.contains(&self.0.text)
    }
}

/// 判断语句是否适合持续 Plan Replayer 捕获。
pub fn checkPlanReplayerContinuesCaptureValidStmt(statement: &StatementNode) -> bool {
    matches!(
        statement.kind,
        StatementKind::Select
            | StatementKind::Insert
            | StatementKind::Replace
            | StatementKind::Update
            | StatementKind::Delete
    )
}

/// 检查并派发一次性 Plan Replayer 捕获任务。
pub fn checkPlanReplayerCaptureTask(statement: &mut ExecStmt, start_ts: u64) {
    statement
        .Ctx
        .PlanReplayerCapture(&statement.StmtNode, start_ts, false);
}

/// 检查并派发持续 Plan Replayer 捕获。
pub fn checkPlanReplayerContinuesCapture(statement: &mut ExecStmt, start_ts: u64) {
    if checkPlanReplayerContinuesCaptureValidStmt(&statement.StmtNode) {
        sendPlanReplayerDumpTask(statement, start_ts, true);
    }
}

/// 向运行时发送 Plan Replayer dump 任务。
pub fn sendPlanReplayerDumpTask(statement: &mut ExecStmt, start_ts: u64, continuous: bool) {
    statement
        .Ctx
        .PlanReplayerCapture(&statement.StmtNode, start_ts, continuous);
}

/// 截断过长标识符到 MAX_ALIAS_IDENTIFIER_LEN。
fn truncateIdentifier(identifier: &str) -> String {
    identifier.chars().take(MAX_ALIAS_IDENTIFIER_LEN).collect()
}

/// 去除 SQL 中的优化器 hint 注释。
fn removeSQLHints(sql: &str) -> String {
    let mut result = String::with_capacity(sql.len());
    let mut remaining = sql;
    while let Some(start) = remaining.find("/*+") {
        result.push_str(&remaining[..start]);
        let Some(end) = remaining[start + 3..].find("*/") else {
            break;
        };
        remaining = &remaining[start + 3 + end + 2..];
    }
    result.push_str(remaining);
    result
}

/// 按全局开关对 SQL 做脱敏。
fn redactSQL(sql: &str) -> String {
    let mut result = String::with_capacity(sql.len());
    let mut in_quote = false;
    for character in sql.chars() {
        if character == '\'' {
            in_quote = !in_quote;
            if in_quote {
                result.push('?');
            }
            continue;
        }
        if !in_quote {
            result.push(character);
        }
    }
    result
}

/// 将 catch_unwind 的 panic 载荷转为 SharedError。
fn panicError(panic: &(dyn std::any::Any + Send), operation: &str) -> errors::SharedError {
    let message = panic
        .downcast_ref::<&str>()
        .map(|value| (*value).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned());
    errors::New(format!("{operation}: {message}"))
}

/// Client input is a one-shot stream: a retry must never reopen its executor.
/// The file location is carried by the LOAD DATA plan, rather than inferred from SQL text.
pub fn canRetryPessimisticLoadData(location: astersql_parser_ast::FileLocRef) -> bool {
    location != astersql_parser_ast::FileLocRef::Client
}
