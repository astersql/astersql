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

// 单条 SQL 语句的执行期上下文（StatementContext）。
//
// 聚合类型/错误处理、行计数与告警、执行计划与 digest、内存/磁盘 Tracker、
// 逻辑计划构建快照、统计加载与慢日志相关状态，供会话、优化器与执行器共享。
// 对应 Go 包 `pkg/sessionctx/stmtctx`。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

use std::any::Any;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono_tz::Tz;
use errctx_crate::{contextutil as err_contextutil, errors as err_errors};
use execdetails_crate::execdetails::{ExecDetails, RuntimeStatsColl, SyncExecDetails};
use intset_crate::fast_int_set::FastIntSet;
use memory_crate::tracker::{NewTracker, Tracker};
use task_context as util_context;
use task_errctx as errctx_crate;
use task_execdetails as execdetails_crate;
use task_intset as intset_crate;
use task_memory as memory_crate;
use task_model as model_crate;
use task_parser as parser_crate;
use task_types as types_crate;
use util_context::plancache::{NewPlanCacheTracker, PlanCacheTracker};
use util_context::{
    NewStaticWarnHandler, StaticWarnHandler, WarnAppender, WarnHandler, WarnHandlerExt,
};

pub use errctx_crate::errctx;
pub use model_crate::group_1::{
    FlagDividedByZeroAsWarning, FlagEnableTiKVShortCircuitExpression, FlagIgnoreTruncate,
    FlagIgnoreZeroInDate, FlagInInsertStmt, FlagInLoadDataStmt, FlagInRestrictedSQL,
    FlagInSelectStmt, FlagInUpdateOrDeleteStmt, FlagOverflowAsWarning, FlagTruncateAsWarning,
};
pub use model_crate::group_4::{TableInfo, TableItemID};
pub use parser_crate::ast::ast::StatementKind;
pub use parser_crate::digester_impl::Digest;
pub use types_crate::scalar::{Context as TypeContext, DefaultStmtFlags, Flags};
pub use util_context::SQLWarn;
pub use util_context::errors;
pub use util_context::plancache::PlanCacheType;

/// 语句内可缓存的任意值（对应 Go `any`，跨线程共享）。
pub type CacheValue = Arc<dyn Any + Send + Sync>;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 资源组标签：把 keyspace / SQL digest / 计划 digest 打到请求上，便于配额与可观测。
pub struct ResourceGroupTagger {
    /// Keyspace 名称字节（多租户隔离标识）。
    pub keyspaceName: Vec<u8>,
    /// 规范化 SQL 的 digest 字节；空 SQL 时为 `None`。
    pub sqlDigest: Option<Vec<u8>>,
    /// 执行计划 digest 字节。
    pub planDigest: Option<Vec<u8>>,
}

/// 进程级默认 keyspace 名，供资源组标签读取。
static KEYSPACE_NAME: RwLock<Vec<u8>> = RwLock::new(Vec::new());

/// 设置进程级资源组标签所用的 keyspace 名。
pub fn SetResourceGroupTaggerKeyspaceName(name: Vec<u8>) {
    *KEYSPACE_NAME.write().expect("keyspace name lock poisoned") = name;
}

/// 将具体值装箱为 `CacheValue`。
pub fn cache_value<T>(value: T) -> CacheValue
where
    T: Any + Send + Sync,
{
    Arc::new(value)
}

/// 从 `CacheValue` 向下转型为具体类型引用。
pub fn cache_downcast_ref<T: Any>(value: &CacheValue) -> Option<&T> {
    value.as_ref().downcast_ref::<T>()
}

/// 进程内单调递增的语句执行 TaskID 分配器。
static taskIDAlloc: AtomicU64 = AtomicU64::new(0);

/// AllocateTaskID allocates a process-wide unique statement execution ID.
/// 分配进程内唯一的语句执行 TaskID。
pub fn AllocateTaskID() -> u64 {
    taskIDAlloc.fetch_add(1, Ordering::Relaxed) + 1
}

/// 从缓存的计划对象解析计划节点 ID 的回调。
pub type PlanIDCallback = fn(&CacheValue) -> Option<i32>;
/// 全局 PlanID 回调；用于从 RuntimeStats 取结果行数等。
static PLAN_ID_FUNC: RwLock<Option<PlanIDCallback>> = RwLock::new(None);

/// 注册或清空全局 PlanID 解析回调。
pub fn SetPlanIDFunc(callback: Option<PlanIDCallback>) {
    *PLAN_ID_FUNC
        .write()
        .expect("plan id callback lock poisoned") = callback;
}

/// ReferenceCount keeps the Go frozen sentinel and CAS transitions.
/// 引用计数：保留 Go 的冻结哨兵值与 CAS 增/减/冻结转移。
#[derive(Default)]
pub struct ReferenceCount(AtomicI32);

/// 冻结哨兵：不可再增加引用。
pub const ReferenceCountIsFrozen: i32 = -1;
/// 无引用时的计数值。
pub const ReferenceCountNoReference: i32 = 0;

impl ReferenceCount {
    /// 尝试增加引用；已冻结则返回 false。
    pub fn TryIncrease(&self) -> bool {
        let mut current = self.0.load(Ordering::Acquire);
        loop {
            if current == ReferenceCountIsFrozen {
                return false;
            }
            match self.0.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(next) => current = next,
            }
        }
    }

    /// 减少引用计数（CAS 循环）。
    pub fn Decrease(&self) {
        let mut current = self.0.load(Ordering::Acquire);
        loop {
            match self.0.compare_exchange_weak(
                current,
                current - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(next) => current = next,
            }
        }
    }

    /// 在无引用时尝试冻结；成功则后续不可再 Increase。
    pub fn TryFreeze(&self) -> bool {
        self.0
            .compare_exchange(
                ReferenceCountNoReference,
                ReferenceCountIsFrozen,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// 解除冻结，恢复为无引用状态。
    pub fn UnFreeze(&self) {
        self.0.store(ReferenceCountNoReference, Ordering::Release);
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 预留自增/隐式 RowID 区间分配器：在 `[base, max]` 内顺序消费。
pub struct ReservedRowIDAlloc {
    base: i64,
    max: i64,
}

impl ReservedRowIDAlloc {
    /// 当前预留区间；MLog 插入后用它恢复基表的分配进度。
    pub fn Current(&self) -> (i64, i64) {
        (self.base, self.max)
    }

    /// 重置可分配区间为 `(base, max]`。
    pub fn Reset(&mut self, base: i64, maxv: i64) {
        self.base = base;
        self.max = maxv;
    }

    /// 消费下一个 ID；耗尽时返回 `(0, false)`。
    pub fn Consume(&mut self) -> (i64, bool) {
        // 半开区间 (base, max]：先自增再返回，耗尽则 (0, false)。
        if self.base < self.max {
            self.base += 1;
            (self.base, true)
        } else {
            (0, false)
        }
    }

    /// 是否已无剩余可分配 ID。
    pub fn Exhausted(&self) -> bool {
        self.base >= self.max
    }
}

#[derive(Default)]
/// 需互斥保护的行计数与消息字段（对应 Go 的 mu 域）。
struct stmtCtxMu {
    foundRows: u64,
    records: u64,
    deleted: u64,
    updated: u64,
    copied: u64,
    touched: u64,
    message: String,
}

impl stmtCtxMu {
    /// 清空行计数与消息。
    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// 过期读（stale read）TSO 求值闭包。
type StaleEvaluator = Box<dyn FnMut() -> Result<u64, errors::SharedError> + Send>;

#[derive(Default)]
/// 过期读 TSO 提供者：成功值可记忆化，错误不缓存。
struct staleTSOProvider {
    value: Option<u64>,
    eval: Option<StaleEvaluator>,
}

impl staleTSOProvider {
    /// 清空已记忆的 TSO 与求值闭包。
    fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Default)]
/// 语句级键值缓存（如 now/safe/external TS）。
struct stmtCache {
    data: HashMap<StmtCacheKey, CacheValue>,
}

impl stmtCache {
    /// 清空语句级缓存映射。
    fn reset(&mut self) {
        self.data.clear();
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 逻辑计划构建期记录的库表引用。
pub struct TableEntry {
    /// 数据库名。
    pub DB: String,
    /// 表名。
    pub Table: String,
}

#[derive(Clone, Default)]
/// 外键级联触发执行时的保存点与级联标记。
pub struct ForeignKeyTriggerState {
    /// 外键触发使用的保存点名。
    pub SavepointName: String,
    /// 本语句是否发生外键级联。
    pub HasFKCascades: bool,
}

#[derive(Default)]
/// MPP（大规模并行处理）查询标识与任务/Gather ID 分配状态。
pub struct MPPQueryState {
    /// MPP 查询 ID。
    pub QueryID: AtomicU64,
    /// MPP 查询时间戳。
    pub QueryTS: AtomicU64,
    /// 已分配的 MPP Task ID。
    pub AllocatedMPPTaskID: AtomicI64,
    /// 已分配的 MPP Gather ID。
    pub AllocatedMPPGatherID: AtomicU64,
}

#[derive(Default)]
/// 同步加载统计信息的超时、待加载项与结果通道状态。
pub struct StatsLoadState {
    /// 同步加载超时。
    pub Timeout: Duration,
    /// 待加载的统计项列表。
    pub NeededItems: Mutex<Vec<CacheValue>>,
    /// 加载结果缓冲（对应 Go channel 语义的占位）。
    pub ResultCh: Vec<CacheValue>,
    /// 本轮加载开始时间。
    pub LoadStartTime: Option<SystemTime>,
}

/// Statement-hint state used while selecting a physical plan.
/// 物理计划选择阶段使用的语句 hint 状态（ForceNthPlan / 写慢日志）。
pub struct StatementHints {
    force_nth_plan: AtomicI64,
    write_slow_log: AtomicBool,
}

impl Default for StatementHints {
    fn default() -> Self {
        Self {
            force_nth_plan: AtomicI64::new(-1),
            write_slow_log: AtomicBool::new(false),
        }
    }
}

impl StatementHints {
    /// 读取 ForceNthPlan；`-1` 表示未指定。
    pub fn ForceNthPlan(&self) -> i64 {
        self.force_nth_plan.load(Ordering::Acquire)
    }

    /// 原子交换 ForceNthPlan，返回旧值。
    pub fn SwapForceNthPlan(&self, value: i64) -> i64 {
        self.force_nth_plan.swap(value, Ordering::AcqRel)
    }

    /// 写入 ForceNthPlan。
    pub fn StoreForceNthPlan(&self, value: i64) {
        self.force_nth_plan.store(value, Ordering::Release);
    }

    /// 是否强制写入慢日志。
    pub fn WriteSlowLog(&self) -> bool {
        self.write_slow_log.load(Ordering::Acquire)
    }

    /// 设置是否强制写入慢日志。
    pub fn StoreWriteSlowLog(&self, value: bool) {
        self.write_slow_log.store(value, Ordering::Release);
    }
}

/// 逻辑计划构建期可保存/恢复的快照（告警、表引用、分区裁剪、计划缓存标志等）。
pub struct LogicalPlanBuildState {
    warnings: Vec<SQLWarn>,
    extraWarnings: Vec<SQLWarn>,
    tables: Vec<TableEntry>,
    tableStats: HashMap<i64, CacheValue>,
    lockTableIDs: HashSet<i64>,
    tblInfo2UnionScan: Vec<(Arc<TableInfo>, bool)>,
    useDynamicPruneMode: bool,
    viewDepth: i32,
    colRefFromUpdatePlan: FastIntSet,
    planCacheUseCache: bool,
    planCacheType: PlanCacheType,
    planCacheUnqualified: String,
    planCacheForce: bool,
    planCacheAlwaysWarn: bool,
}

/// 逻辑计划构建期可变状态（与 `LogicalPlanBuildState` 中可变部分对应）。
struct LogicalPlanMutableState {
    tables: Vec<TableEntry>,
    table_stats: HashMap<i64, CacheValue>,
    lock_table_ids: HashSet<i64>,
    tbl_info_to_union_scan: Vec<(Arc<TableInfo>, bool)>,
    use_dynamic_prune_mode: bool,
    view_depth: i32,
    col_ref_from_update_plan: FastIntSet,
}

impl Default for LogicalPlanMutableState {
    fn default() -> Self {
        Self {
            tables: Vec::new(),
            table_stats: HashMap::new(),
            lock_table_ids: HashSet::new(),
            tbl_info_to_union_scan: Vec::new(),
            use_dynamic_prune_mode: false,
            view_depth: 0,
            col_ref_from_update_plan: FastIntSet::default(),
        }
    }
}

/// 将 errctx 告警桥接到语句 `WarnHandler`。
struct ErrWarnBridge {
    warnings: Arc<StaticWarnHandler>,
}

/// 将类型系统告警桥接到语句 `WarnHandler`。
struct TypeWarnBridge {
    warnings: Arc<StaticWarnHandler>,
}

impl types_crate::scalar::TypeWarnAppender for TypeWarnBridge {
    fn AppendWarning(&self, err: errors::SharedError) {
        WarnAppender::AppendWarning(self.warnings.as_ref(), err);
    }

    fn AppendNote(&self, err: errors::SharedError) {
        WarnAppender::AppendNote(self.warnings.as_ref(), err);
    }
}

impl err_contextutil::WarnAppender for ErrWarnBridge {
    fn AppendWarning(&self, err: err_errors::SharedError) {
        WarnAppender::AppendWarning(
            self.warnings.as_ref(),
            errors::NewNoStackError(err.to_string()),
        );
    }

    fn AppendNote(&self, err: err_errors::SharedError) {
        WarnAppender::AppendNote(
            self.warnings.as_ref(),
            errors::NewNoStackError(err.to_string()),
        );
    }
}

/// 默认语句错误级别：除零默认 Warn，其余为 Error。
fn default_stmt_err_levels() -> errctx::LevelMap {
    let mut levels = [errctx::Level::LevelError; errctx::errGroupCount];
    levels[errctx::ErrGroup::ErrGroupDividedByZero as usize] = errctx::Level::LevelWarn;
    levels
}

/// 导出默认语句错误级别映射。
pub fn DefaultStmtErrLevels() -> errctx::LevelMap {
    default_stmt_err_levels()
}

/// 按类型标志中的截断策略构造错误上下文，并合并其它错误组级别。
fn newErrCtx(
    type_ctx: &TypeContext,
    mut other_levels: errctx::LevelMap,
    handler: errctx::WarnAppenderRef,
) -> errctx::Context {
    let flags = type_ctx.Flags();
    let level = if flags.IgnoreTruncateErr() {
        errctx::Level::LevelIgnore
    } else if flags.TruncateAsWarning() {
        errctx::Level::LevelWarn
    } else {
        errctx::Level::LevelError
    };
    other_levels[errctx::ErrGroup::ErrGroupTruncate as usize] = level;
    errctx::NewContextWithLevels(other_levels, handler)
}

/// StatementContext contains all state scoped to one SQL statement.
/// 单条 SQL 语句作用域内的全部状态（类型/错误、计数、计划、统计、MPP 等）。
pub struct StatementContext {
    ctxID: u64,
    typeCtx: TypeContext,
    errCtx: errctx::Context,
    errWarnBridge: errctx::WarnAppenderRef,
    distSQLCtxCache: Mutex<Option<CacheValue>>,
    rangerCtxCache: OnceLock<CacheValue>,
    buildPBCtxCache: OnceLock<CacheValue>,

    pub IsDDLJobInQueue: AtomicBool,
    pub DDLJobID: i64,
    pub InInsertStmt: bool,
    pub InUpdateStmt: bool,
    pub InDeleteStmt: bool,
    pub InSelectStmt: bool,
    pub InLoadDataStmt: bool,
    explainContext: Mutex<Option<(bool, bool, String)>>,
    pub InExplainStmt: bool,
    pub InExplainAnalyzeStmt: bool,
    pub StmtHints: StatementHints,
    pub ExplainFormat: String,
    pub InCreateOrAlterStmt: bool,
    pub InSetSessionStatesStmt: bool,
    pub InShowWarning: bool,
    pub PlanCacheTracker: PlanCacheTracker,
    pub IgnoreExplainIDSuffix: bool,
    pub MultiSchemaInfo: Option<CacheValue>,
    pub IsStaleness: AtomicBool,
    pub InRestrictedSQL: bool,
    mu: Arc<Mutex<stmtCtxMu>>,
    affectedRows: AtomicU64,
    pub WarnHandler: Arc<StaticWarnHandler>,
    pub ExtraWarnHandler: Arc<StaticWarnHandler>,
    pub SyncExecDetails: SyncExecDetails,
    pub PrevAffectedRows: i64,
    pub PrevLastInsertID: u64,
    pub LastInsertID: u64,
    pub LastInsertIDSet: bool,
    pub InsertID: u64,
    pub ReservedRowIDAlloc: Mutex<ReservedRowIDAlloc>,
    pub NotFillCache: bool,
    pub MemTracker: Option<Box<Tracker>>,
    pub MemSensitive: bool,
    pub DiskTracker: Option<Box<Tracker>>,
    pub ResourceGroupName: String,
    pub RunawayChecker: Option<CacheValue>,
    pub IsTiKV: AtomicBool,
    pub IsTiFlash: AtomicBool,
    pub RuntimeStatsColl: Option<Arc<RuntimeStatsColl>>,
    pub IndexUsageCollector: Option<CacheValue>,
    pub TableIDs: Mutex<Vec<i64>>,
    pub IndexNames: Mutex<Vec<String>>,
    pub StmtType: String,
    pub OriginalSQL: String,
    digestMemo: Mutex<Option<(String, Digest)>>,
    pub BindSQL: String,
    pub MatchSQLBindingCacheKey: Option<CacheValue>,
    pub MatchSQLBindingCache: Option<CacheValue>,
    pub ExecRetryCount: u64,
    adapterExecRetryCount: AtomicU64,
    pub ExecSuccess: bool,
    planDigest: Mutex<(String, Option<Digest>)>,
    encodedPlan: String,
    planHint: String,
    planHintSet: bool,
    binaryPlan: String,
    indexForce: bool,
    flatPlan: Mutex<Option<CacheValue>>,
    plan: Option<CacheValue>,
    logicalPlanBuild: Mutex<LogicalPlanMutableState>,
    lockWaitStartTime: AtomicI64,
    pub TaskID: AtomicU64,
    pub TaskMapBakTS: u64,
    stmtCache: Arc<Mutex<stmtCache>>,
    pub CTEStorageMap: Option<CacheValue>,
    pub SetVarHintRestore: HashMap<String, String>,
    pub ReadFromTableCache: AtomicBool,
    useChunkAlloc: AtomicBool,
    pub InVerboseExplain: bool,
    pub EnableOptimizeTrace: bool,
    pub OptimizeTracer: Option<CacheValue>,
    pub EnableOptimizerCETrace: bool,
    pub OptimizerCETrace: Vec<CacheValue>,
    pub EnableOptimizerDebugTrace: bool,
    pub OptimizerDebugTrace: Option<CacheValue>,
    pub WaitLockLeaseTime: Duration,
    pub KvExecCounter: Option<CacheValue>,
    pub WeakConsistency: bool,
    pub StatsLoad: StatsLoadState,
    pub SysdateIsNow: bool,
    /// Whether TiKV should short-circuit logical expression evaluation.
    pub EnableTiKVShortCircuitExpression: bool,
    pub RCCheckTS: bool,
    pub IsSQLRegistered: AtomicBool,
    pub IsSQLAndPlanRegistered: AtomicBool,
    pub IsReadOnly: bool,
    usedStatsInfo: OnceLock<Arc<UsedStatsInfo>>,
    pub IsSyncStatsFailed: AtomicBool,
    statsSyncWaitNanos: AtomicU64,
    statsSyncWaitError: Mutex<Option<String>>,
    AlternativeLogicalPlanDecorrelatedApply: AtomicBool,
    AlternativeLogicalPlanSameOrderIndexJoin: AtomicBool,
    AlternativeLogicalPlanOrderAwareJoinReorder: AtomicBool,
    AlternativeLogicalPlanPreferCorrelate: AtomicBool,
    AlternativeLogicalPlanSemiJoinRewrite: AtomicBool,
    AlternativeLogicalPlanFTSLikeFallback: AtomicBool,
    AlternativeLogicalPlanHasPredicateContextMatch: AtomicBool,
    AlternativeLogicalPlanMixedStorageEngines: AtomicBool,
    AlternativeLogicalPlanMissingTiFlashPath: AtomicBool,
    AlternativeLogicalPlanHasStoreTypeHint: AtomicBool,
    pub FTSFunctionIsUsed: bool,
    pub IsExplainAnalyzeDML: bool,
    pub InHandleForeignKeyTrigger: AtomicBool,
    pub ForeignKeyTriggerCtx: ForeignKeyTriggerState,
    pub MPPQueryInfo: MPPQueryState,
    pub TiFlashEngineRemovedDueToStrictSQLMode: bool,
    StaleTSOProvider: Arc<Mutex<staleTSOProvider>>,
    pub RelatedTableIDs: HashSet<i64>,
    pub ForShareLockEnabledByNoop: bool,
    pub OperatorNum: u64,
    plannerUsedStatsLoadStatus: Mutex<HashMap<(i64, i64, bool), String>>,
}

impl StatementContext {
    /// Set per-statement EXPLAIN state through a shared session context.
    /// Consumers use the value accessors; public fields remain the initial
    /// values for contexts constructed with exclusive ownership.
    pub fn SetExplainContext(&self, in_explain: bool, analyze: bool, format: &str) {
        *self
            .explainContext
            .lock()
            .expect("explain context lock poisoned") =
            Some((in_explain, analyze, format.to_owned()));
    }

    pub fn ExplainContext(&self) -> (bool, bool, String) {
        self.explainContext
            .lock()
            .expect("explain context lock poisoned")
            .clone()
            .unwrap_or_else(|| {
                (
                    self.InExplainStmt,
                    self.InExplainAnalyzeStmt,
                    self.ExplainFormat.clone(),
                )
            })
    }

    pub fn IsInExplainStmt(&self) -> bool {
        self.explainContext
            .lock()
            .expect("explain context lock poisoned")
            .as_ref()
            .map_or(self.InExplainStmt, |context| context.0)
    }

    pub fn IsInExplainAnalyzeStmt(&self) -> bool {
        self.explainContext
            .lock()
            .expect("explain context lock poisoned")
            .as_ref()
            .map_or(self.InExplainAnalyzeStmt, |context| context.1)
    }

    pub fn ExplainFormatValue(&self) -> String {
        self.ExplainContext().2
    }

    pub fn SetStaleness(&self, stale: bool) {
        self.IsStaleness.store(stale, Ordering::Release);
    }

    pub fn IsStalenessValue(&self) -> bool {
        self.IsStaleness.load(Ordering::Acquire)
    }

    pub fn AddAdapterExecRetryCount(&self, retries: u64) {
        self.adapterExecRetryCount
            .fetch_add(retries, Ordering::AcqRel);
    }

    pub fn ExecRetryCountValue(&self) -> u64 {
        self.ExecRetryCount
            .saturating_add(self.adapterExecRetryCount.load(Ordering::Acquire))
    }

    pub fn ResetAdapterExecRetryCount(&self) {
        self.adapterExecRetryCount.store(0, Ordering::Release);
    }

    /// 构造语句上下文：挂接类型/错误告警桥、计划缓存 Tracker 与各默认字段。
    fn build(
        tz: Tz,
        warnings: Arc<StaticWarnHandler>,
        extra_warnings: Arc<StaticWarnHandler>,
        mu: Arc<Mutex<stmtCtxMu>>,
        stmt_cache: Arc<Mutex<stmtCache>>,
        stale_provider: Arc<Mutex<staleTSOProvider>>,
    ) -> Self {
        let type_handler: Arc<dyn types_crate::scalar::TypeWarnAppender + Send + Sync> =
            Arc::new(TypeWarnBridge {
                warnings: warnings.clone(),
            });
        let type_ctx = types_crate::scalar::NewContext(DefaultStmtFlags, tz, type_handler);
        let err_bridge: errctx::WarnAppenderRef = Arc::new(ErrWarnBridge {
            warnings: warnings.clone(),
        });
        let err_ctx = newErrCtx(&type_ctx, default_stmt_err_levels(), err_bridge.clone());
        let plan_cache_handler: Arc<dyn WarnAppender + Send + Sync> = warnings.clone();

        Self {
            ctxID: util_context::context::GenContextID(),
            typeCtx: type_ctx,
            errCtx: err_ctx,
            errWarnBridge: err_bridge,
            distSQLCtxCache: Mutex::new(None),
            rangerCtxCache: OnceLock::new(),
            buildPBCtxCache: OnceLock::new(),
            IsDDLJobInQueue: AtomicBool::new(false),
            DDLJobID: 0,
            InInsertStmt: false,
            InUpdateStmt: false,
            InDeleteStmt: false,
            InSelectStmt: false,
            InLoadDataStmt: false,
            explainContext: Mutex::new(None),
            InExplainStmt: false,
            InExplainAnalyzeStmt: false,
            StmtHints: StatementHints::default(),
            ExplainFormat: String::new(),
            InCreateOrAlterStmt: false,
            InSetSessionStatesStmt: false,
            InShowWarning: false,
            PlanCacheTracker: NewPlanCacheTracker(plan_cache_handler),
            IgnoreExplainIDSuffix: false,
            MultiSchemaInfo: None,
            IsStaleness: AtomicBool::new(false),
            InRestrictedSQL: false,
            mu,
            affectedRows: AtomicU64::new(0),
            WarnHandler: warnings,
            ExtraWarnHandler: extra_warnings,
            SyncExecDetails: SyncExecDetails::default(),
            PrevAffectedRows: 0,
            PrevLastInsertID: 0,
            LastInsertID: 0,
            LastInsertIDSet: false,
            InsertID: 0,
            ReservedRowIDAlloc: Mutex::new(ReservedRowIDAlloc::default()),
            NotFillCache: false,
            MemTracker: None,
            MemSensitive: false,
            DiskTracker: None,
            ResourceGroupName: String::new(),
            RunawayChecker: None,
            IsTiKV: AtomicBool::new(false),
            IsTiFlash: AtomicBool::new(false),
            RuntimeStatsColl: None,
            IndexUsageCollector: None,
            TableIDs: Mutex::new(Vec::new()),
            IndexNames: Mutex::new(Vec::new()),
            StmtType: String::new(),
            OriginalSQL: String::new(),
            digestMemo: Mutex::new(None),
            BindSQL: String::new(),
            MatchSQLBindingCacheKey: None,
            MatchSQLBindingCache: None,
            ExecRetryCount: 0,
            adapterExecRetryCount: AtomicU64::new(0),
            ExecSuccess: false,
            planDigest: Mutex::new((String::new(), None)),
            encodedPlan: String::new(),
            planHint: String::new(),
            planHintSet: false,
            binaryPlan: String::new(),
            indexForce: false,
            flatPlan: Mutex::new(None),
            plan: None,
            logicalPlanBuild: Mutex::new(LogicalPlanMutableState::default()),
            lockWaitStartTime: AtomicI64::new(0),
            TaskID: AtomicU64::new(0),
            TaskMapBakTS: 0,
            stmtCache: stmt_cache,
            CTEStorageMap: None,
            SetVarHintRestore: HashMap::new(),
            ReadFromTableCache: AtomicBool::new(false),
            useChunkAlloc: AtomicBool::new(false),
            InVerboseExplain: false,
            EnableOptimizeTrace: false,
            OptimizeTracer: None,
            EnableOptimizerCETrace: false,
            OptimizerCETrace: Vec::new(),
            EnableOptimizerDebugTrace: false,
            OptimizerDebugTrace: None,
            WaitLockLeaseTime: Duration::ZERO,
            KvExecCounter: None,
            WeakConsistency: false,
            StatsLoad: StatsLoadState::default(),
            SysdateIsNow: false,
            EnableTiKVShortCircuitExpression: false,
            RCCheckTS: false,
            IsSQLRegistered: AtomicBool::new(false),
            IsSQLAndPlanRegistered: AtomicBool::new(false),
            IsReadOnly: false,
            usedStatsInfo: OnceLock::new(),
            IsSyncStatsFailed: AtomicBool::new(false),
            statsSyncWaitNanos: AtomicU64::new(0),
            statsSyncWaitError: Mutex::new(None),
            AlternativeLogicalPlanDecorrelatedApply: AtomicBool::new(false),
            AlternativeLogicalPlanSameOrderIndexJoin: AtomicBool::new(false),
            AlternativeLogicalPlanOrderAwareJoinReorder: AtomicBool::new(false),
            AlternativeLogicalPlanPreferCorrelate: AtomicBool::new(false),
            AlternativeLogicalPlanSemiJoinRewrite: AtomicBool::new(false),
            AlternativeLogicalPlanFTSLikeFallback: AtomicBool::new(false),
            AlternativeLogicalPlanHasPredicateContextMatch: AtomicBool::new(false),
            AlternativeLogicalPlanMixedStorageEngines: AtomicBool::new(false),
            AlternativeLogicalPlanMissingTiFlashPath: AtomicBool::new(false),
            AlternativeLogicalPlanHasStoreTypeHint: AtomicBool::new(false),
            FTSFunctionIsUsed: false,
            IsExplainAnalyzeDML: false,
            InHandleForeignKeyTrigger: AtomicBool::new(false),
            ForeignKeyTriggerCtx: ForeignKeyTriggerState::default(),
            MPPQueryInfo: MPPQueryState::default(),
            TiFlashEngineRemovedDueToStrictSQLMode: false,
            StaleTSOProvider: stale_provider,
            RelatedTableIDs: HashSet::new(),
            ForShareLockEnabledByNoop: false,
            OperatorNum: 0,
            plannerUsedStatsLoadStatus: Mutex::new(HashMap::new()),
        }
    }

    /// Reset refuses to race with users of the three Go mutex domains.
    /// 重置语句状态；若三把 Go 域互斥锁任一被占用则返回 false，避免竞态。
    pub fn Reset(&mut self) -> bool {
        let mu = Arc::clone(&self.mu);
        let stmt_cache = Arc::clone(&self.stmtCache);
        let stale_provider = Arc::clone(&self.StaleTSOProvider);
        // 三把域锁均需 try_lock 成功才继续，任一占用则放弃 Reset。
        let Ok(mut mu_guard) = mu.try_lock() else {
            return false;
        };
        let Ok(mut cache_guard) = stmt_cache.try_lock() else {
            return false;
        };
        let Ok(mut stale_guard) = stale_provider.try_lock() else {
            return false;
        };

        // 下列字段在 Reset 后仍复用，避免丢失会话级可复用状态。
        let cte_storage = self.CTEStorageMap.clone();
        let logical_plan_build = self
            .logicalPlanBuild
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Go 只复用这三张 map；Tables、动态裁剪、ViewDepth 和列引用等
        // 其余逻辑计划构建态必须随语句 Reset 清零。
        let table_stats = std::mem::take(&mut logical_plan_build.table_stats);
        let lock_table_ids = std::mem::take(&mut logical_plan_build.lock_table_ids);
        let tbl_info_to_union_scan = std::mem::take(&mut logical_plan_build.tbl_info_to_union_scan);
        let related_table_ids = std::mem::take(&mut self.RelatedTableIDs);
        let index_collector = self.IndexUsageCollector.clone();
        let warnings = Arc::clone(&self.WarnHandler);
        let extra_warnings = Arc::clone(&self.ExtraWarnHandler);

        mu_guard.reset();
        cache_guard.reset();
        stale_guard.reset();
        warnings.Reset();
        extra_warnings.Reset();

        let mut reset = Self::build(
            chrono_tz::UTC,
            warnings,
            extra_warnings,
            Arc::clone(&mu),
            Arc::clone(&stmt_cache),
            Arc::clone(&stale_provider),
        );
        reset.CTEStorageMap = cte_storage;
        let reset_logical_plan_build = reset
            .logicalPlanBuild
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reset_logical_plan_build.table_stats = table_stats;
        reset_logical_plan_build.lock_table_ids = lock_table_ids;
        reset_logical_plan_build.tbl_info_to_union_scan = tbl_info_to_union_scan;
        reset.RelatedTableIDs = related_table_ids;
        reset.IndexUsageCollector = index_collector;
        drop(stale_guard);
        drop(cache_guard);
        drop(mu_guard);
        *self = reset;
        true
    }

    /// 保存逻辑计划构建期快照（含计划缓存 Tracker 状态）。
    pub fn SaveLogicalPlanBuildState(&self) -> LogicalPlanBuildState {
        let (use_cache, cache_type, reason, force, always_warn) = self.PlanCacheTracker.Save();
        let state = self
            .logicalPlanBuild
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        LogicalPlanBuildState {
            warnings: self.GetWarnings(),
            extraWarnings: self.GetExtraWarnings(),
            tables: state.tables.clone(),
            tableStats: state.table_stats.clone(),
            lockTableIDs: state.lock_table_ids.clone(),
            tblInfo2UnionScan: state.tbl_info_to_union_scan.clone(),
            useDynamicPruneMode: state.use_dynamic_prune_mode,
            viewDepth: state.view_depth,
            colRefFromUpdatePlan: state.col_ref_from_update_plan.Copy(),
            planCacheUseCache: use_cache,
            planCacheType: cache_type,
            planCacheUnqualified: reason,
            planCacheForce: force,
            planCacheAlwaysWarn: always_warn,
        }
    }

    /// 恢复逻辑计划构建期快照，覆盖当前构建态与计划缓存标志。
    pub fn RestoreLogicalPlanBuildState(&self, state: &LogicalPlanBuildState) {
        self.SetWarnings(state.warnings.clone());
        self.SetExtraWarnings(state.extraWarnings.clone());
        let mut current = self
            .logicalPlanBuild
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        current.tables = state.tables.clone();
        current.table_stats = state.tableStats.clone();
        current.lock_table_ids = state.lockTableIDs.clone();
        current.tbl_info_to_union_scan = state.tblInfo2UnionScan.clone();
        current.use_dynamic_prune_mode = state.useDynamicPruneMode;
        current.view_depth = state.viewDepth;
        current
            .col_ref_from_update_plan
            .CopyFrom(&state.colRefFromUpdatePlan);
        drop(current);
        self.PlanCacheTracker.Restore(
            state.planCacheUseCache,
            state.planCacheType,
            state.planCacheUnqualified.clone(),
            state.planCacheForce,
            state.planCacheAlwaysWarn,
        );
    }

    /// 设置逻辑计划涉及的库表列表。
    pub fn SetLogicalPlanTables(&self, tables: Vec<TableEntry>) {
        self.logicalPlanBuild
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .tables = tables;
    }
    /// 读取逻辑计划涉及的库表列表。
    pub fn LogicalPlanTables(&self) -> Vec<TableEntry> {
        self.logicalPlanBuild
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .tables
            .clone()
    }
    /// 记录表 ID 对应的统计信息缓存项。
    pub fn InsertLogicalPlanTableStats(&self, id: i64, value: CacheValue) {
        self.logicalPlanBuild
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .table_stats
            .insert(id, value);
    }
    /// 是否已记录该表的统计信息。
    pub fn ContainsLogicalPlanTableStats(&self, id: i64) -> bool {
        self.logicalPlanBuild
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .table_stats
            .contains_key(&id)
    }
    /// 清空逻辑计划表统计缓存。
    pub fn ClearLogicalPlanTableStats(&self) {
        self.logicalPlanBuild
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .table_stats
            .clear();
    }
    /// 记录需加锁的表 ID。
    pub fn InsertLogicalPlanLockTableID(&self, id: i64) {
        self.logicalPlanBuild
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .lock_table_ids
            .insert(id);
    }
    /// 返回需加锁的表 ID 集合。
    pub fn LogicalPlanLockTableIDs(&self) -> HashSet<i64> {
        self.logicalPlanBuild
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .lock_table_ids
            .clone()
    }
    /// 设置是否使用动态分区裁剪（Dynamic Prune）。
    pub fn SetUseDynamicPruneMode(&self, enabled: bool) {
        self.logicalPlanBuild
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .use_dynamic_prune_mode = enabled;
    }
    /// 设置当前视图展开深度。
    pub fn SetLogicalPlanViewDepth(&self, depth: i32) {
        self.logicalPlanBuild
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .view_depth = depth;
    }
    /// 读取当前视图展开深度。
    pub fn LogicalPlanViewDepth(&self) -> i32 {
        self.logicalPlanBuild
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .view_depth
    }
    /// 记录 UPDATE 计划引用的列 ID。
    pub fn InsertLogicalPlanColumnReference(&self, id: i32) {
        self.logicalPlanBuild
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .col_ref_from_update_plan
            .Insert(id);
    }
    /// 是否已记录该列引用。
    pub fn HasLogicalPlanColumnReference(&self, id: i32) -> bool {
        self.logicalPlanBuild
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .col_ref_from_update_plan
            .Has(id)
    }
    /// 返回标记为 dirty 的 UnionScan 表 ID。
    pub fn DirtyUnionScanTableIDs(&self) -> Vec<i64> {
        self.logicalPlanBuild
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .tbl_info_to_union_scan
            .iter()
            .filter_map(|(table, dirty)| dirty.then_some(table.ID))
            .collect()
    }

    /// 重置备选逻辑计划信号，并清除 FTS 使用标记等会话级标志。
    pub fn ResetAlternativeLogicalPlanSignals(&mut self) {
        self.ResetAlternativeRoundSignals();
        self.AlternativeLogicalPlanPreferCorrelate
            .store(false, Ordering::Release);
        self.FTSFunctionIsUsed = false;
    }

    /// 重置本轮优化中的备选计划布尔信号。
    pub fn ResetAlternativeRoundSignals(&self) {
        self.AlternativeLogicalPlanDecorrelatedApply
            .store(false, Ordering::Release);
        self.AlternativeLogicalPlanSameOrderIndexJoin
            .store(false, Ordering::Release);
        self.AlternativeLogicalPlanOrderAwareJoinReorder
            .store(false, Ordering::Release);
        self.AlternativeLogicalPlanSemiJoinRewrite
            .store(false, Ordering::Release);
        self.AlternativeLogicalPlanFTSLikeFallback
            .store(false, Ordering::Release);
        self.AlternativeLogicalPlanHasPredicateContextMatch
            .store(false, Ordering::Release);
        self.AlternativeLogicalPlanMixedStorageEngines
            .store(false, Ordering::Release);
        self.AlternativeLogicalPlanMissingTiFlashPath
            .store(false, Ordering::Release);
        self.AlternativeLogicalPlanHasStoreTypeHint
            .store(false, Ordering::Release);
        self.AlternativeLogicalPlanPreferCorrelate
            .store(false, Ordering::Release);
    }

    /// 标记发生了 Decorrelate Apply 的备选计划变换。
    pub fn MarkAlternativeLogicalPlanDecorrelatedApply(&self) {
        self.AlternativeLogicalPlanDecorrelatedApply
            .store(true, Ordering::Release);
    }
    /// 标记发生了同序 IndexJoin 备选计划。
    pub fn MarkAlternativeLogicalPlanSameOrderIndexJoin(&self) {
        self.AlternativeLogicalPlanSameOrderIndexJoin
            .store(true, Ordering::Release);
    }
    /// 标记发生了顺序感知的 Join 重排。
    pub fn MarkAlternativeLogicalPlanOrderAwareJoinReorder(&self) {
        self.AlternativeLogicalPlanOrderAwareJoinReorder
            .store(true, Ordering::Release);
    }
    /// 标记偏好 Correlate（相关子查询）形态。
    pub fn MarkAlternativeLogicalPlanPreferCorrelate(&self) {
        self.AlternativeLogicalPlanPreferCorrelate
            .store(true, Ordering::Release);
    }

    /// 是否偏好 Correlate 形态。
    pub fn AlternativeLogicalPlanPreferCorrelate(&self) -> bool {
        self.AlternativeLogicalPlanPreferCorrelate
            .load(Ordering::Acquire)
    }

    /// 一次性读取本轮全部备选计划信号。
    pub fn AlternativeLogicalPlanSignals(&self) -> (bool, bool, bool, bool, bool, bool) {
        (
            self.AlternativeLogicalPlanDecorrelatedApply
                .load(Ordering::Acquire),
            self.AlternativeLogicalPlanSameOrderIndexJoin
                .load(Ordering::Acquire),
            self.AlternativeLogicalPlanOrderAwareJoinReorder
                .load(Ordering::Acquire),
            self.AlternativeLogicalPlanSemiJoinRewrite
                .load(Ordering::Acquire),
            self.AlternativeLogicalPlanFTSLikeFallback
                .load(Ordering::Acquire),
            self.AlternativeLogicalPlanHasPredicateContextMatch
                .load(Ordering::Acquire),
        )
    }

    /// 是否触发了全文检索（FTS）LIKE 回退路径。
    pub fn AlternativeFTSLikeFallback(&self) -> bool {
        self.AlternativeLogicalPlanFTSLikeFallback
            .load(Ordering::Acquire)
    }

    /// 记录规划器侧某表/列或索引的统计加载状态文案。
    pub fn RecordUsedStatsLoadStatus(
        &self,
        table_id: i64,
        item_id: i64,
        is_index: bool,
        status: String,
    ) {
        self.plannerUsedStatsLoadStatus
            .lock()
            .expect("used-stats status lock poisoned")
            .insert((table_id, item_id, is_index), status);
    }

    /// 返回规划器记录的全部统计加载状态。
    pub fn UsedStatsLoadStatus(&self) -> HashMap<(i64, i64, bool), String> {
        self.plannerUsedStatsLoadStatus
            .lock()
            .expect("used-stats status lock poisoned")
            .clone()
    }

    /// 待同步加载的统计项数量。
    pub fn PendingStatsLoadItems(&self) -> usize {
        self.StatsLoad
            .NeededItems
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// 清空待加载统计项列表。
    pub fn ConsumePendingStatsLoadItems(&self) {
        self.StatsLoad
            .NeededItems
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// 同步加载统计是否已失败。
    pub fn IsSyncStatsFailed(&self) -> bool {
        self.IsSyncStatsFailed.load(Ordering::Acquire)
    }

    /// 同步加载成功收尾：记录耗时、清失败标记与待加载项。
    pub fn CompleteStatsSyncWait(&self, elapsed: Duration) {
        self.statsSyncWaitNanos.store(
            elapsed.as_nanos().min(u64::MAX as u128) as u64,
            Ordering::Release,
        );
        self.IsSyncStatsFailed.store(false, Ordering::Release);
        *self
            .statsSyncWaitError
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.ConsumePendingStatsLoadItems();
    }

    /// 同步加载失败收尾：记录耗时与错误信息。
    pub fn FailStatsSyncWait(&self, elapsed: Duration, error: String) {
        self.statsSyncWaitNanos.store(
            elapsed.as_nanos().min(u64::MAX as u128) as u64,
            Ordering::Release,
        );
        self.IsSyncStatsFailed.store(true, Ordering::Release);
        *self
            .statsSyncWaitError
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
    }

    /// 最近一次统计同步等待耗时。
    pub fn StatsSyncWaitDuration(&self) -> Duration {
        Duration::from_nanos(self.statsSyncWaitNanos.load(Ordering::Acquire))
    }

    /// 最近一次统计同步失败错误文案。
    pub fn StatsSyncWaitError(&self) -> Option<String> {
        self.statsSyncWaitError
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    /// 标记发生了 SemiJoin 重写。
    pub fn MarkAlternativeLogicalPlanSemiJoinRewrite(&self) {
        self.AlternativeLogicalPlanSemiJoinRewrite
            .store(true, Ordering::Release);
    }

    /// 记录默认轮物理计划同时使用 TiKV 与 TiFlash。
    pub fn MarkAlternativeLogicalPlanMixedStorageEngines(&self) {
        self.AlternativeLogicalPlanMixedStorageEngines
            .store(true, Ordering::Release);
    }

    /// 记录至少一个数据源缺少 TiFlash 访问路径。
    pub fn MarkAlternativeLogicalPlanMissingTiFlashPath(&self) {
        self.AlternativeLogicalPlanMissingTiFlashPath
            .store(true, Ordering::Release);
    }

    /// 记录语句含显式 READ_FROM_STORAGE 引擎提示。
    pub fn MarkAlternativeLogicalPlanHasStoreTypeHint(&self) {
        self.AlternativeLogicalPlanHasStoreTypeHint
            .store(true, Ordering::Release);
    }

    /// 一次性读取引擎限定替代轮所需的三个信号。
    pub fn AlternativeLogicalPlanEngineSignals(&self) -> (bool, bool, bool) {
        (
            self.AlternativeLogicalPlanMixedStorageEngines
                .load(Ordering::Acquire),
            self.AlternativeLogicalPlanMissingTiFlashPath
                .load(Ordering::Acquire),
            self.AlternativeLogicalPlanHasStoreTypeHint
                .load(Ordering::Acquire),
        )
    }

    /// 本语句上下文唯一 ID。
    pub fn CtxID(&self) -> u64 {
        self.ctxID
    }
    /// 当前类型上下文时区。
    pub fn TimeZone(&self) -> Tz {
        self.typeCtx.Location()
    }
    /// 设置类型上下文时区。
    pub fn SetTimeZone(&mut self, tz: Tz) {
        self.typeCtx = self.typeCtx.WithLocation(tz);
    }
    /// 克隆类型求值上下文。
    pub fn TypeCtx(&self) -> TypeContext {
        self.typeCtx.clone()
    }
    /// 克隆错误处理上下文。
    pub fn ErrCtx(&self) -> errctx::Context {
        self.errCtx.clone()
    }

    /// 设置错误组级别并重建错误上下文（截断级别仍由类型标志推导）。
    pub fn SetErrLevels(&mut self, levels: errctx::LevelMap) {
        self.errCtx = newErrCtx(&self.typeCtx, levels, self.errWarnBridge.clone());
    }

    /// 当前错误组级别映射。
    pub fn ErrLevels(&self) -> errctx::LevelMap {
        self.errCtx.LevelMap()
    }
    /// 查询某一错误组的处理级别。
    pub fn ErrGroupLevel(&self, group: errctx::ErrGroup) -> errctx::Level {
        self.errCtx.LevelForGroup(group)
    }
    /// 当前类型求值标志位。
    pub fn TypeFlags(&self) -> Flags {
        self.typeCtx.Flags()
    }

    /// 设置类型标志并按新截断策略重建错误上下文。
    pub fn SetTypeFlags(&mut self, flags: Flags) {
        self.typeCtx = self.typeCtx.WithFlags(flags);
        self.errCtx = newErrCtx(
            &self.typeCtx,
            self.errCtx.LevelMap(),
            self.errWarnBridge.clone(),
        );
    }

    /// 按类型标志处理截断错误（忽略/告警/报错）。
    pub fn HandleTruncate<T>(
        &self,
        value: T,
        err: errors::SharedError,
    ) -> types_crate::scalar::ValueResult<T> {
        self.typeCtx.HandleTruncate(value, err)
    }

    /// 按错误上下文处理可选错误。
    pub fn HandleError(
        &self,
        err: Option<err_errors::SharedError>,
    ) -> Option<err_errors::SharedError> {
        self.errCtx.HandleError(err)
    }

    /// 按错误上下文处理带内部错误/告警别名的错误。
    pub fn HandleErrorWithAlias(
        &self,
        internal: Option<&err_errors::SharedError>,
        err: err_errors::SharedError,
        warning: err_errors::SharedError,
    ) -> Option<err_errors::SharedError> {
        self.errCtx.HandleErrorWithAlias(internal, err, warning)
    }

    /// 若键不存在则写入并返回缓存值。
    pub fn GetOrStoreStmtCache(&self, key: StmtCacheKey, value: CacheValue) -> CacheValue {
        let mut cache = self
            .stmtCache
            .lock()
            .expect("statement cache lock poisoned");
        cache.data.entry(key).or_insert(value).clone()
    }

    /// 缓存未命中时求值并写入；命中则直接返回（成功值记忆化）。
    pub fn GetOrEvaluateStmtCache<F>(
        &self,
        key: StmtCacheKey,
        evaluator: F,
    ) -> Result<CacheValue, errors::SharedError>
    where
        F: FnOnce() -> Result<CacheValue, errors::SharedError>,
    {
        let mut cache = self
            .stmtCache
            .lock()
            .expect("statement cache lock poisoned");
        if let Some(value) = cache.data.get(&key) {
            return Ok(value.clone());
        }
        let value = evaluator()?;
        cache.data.insert(key, value.clone());
        Ok(value)
    }

    /// 删除语句缓存中的指定键。
    pub fn ResetInStmtCache(&self, key: StmtCacheKey) {
        self.stmtCache
            .lock()
            .expect("statement cache lock poisoned")
            .data
            .remove(&key);
    }

    /// 清空全部语句缓存。
    pub fn ResetStmtCache(&self) {
        self.stmtCache
            .lock()
            .expect("statement cache lock poisoned")
            .data
            .clear();
    }

    /// 获取规范化 SQL 与 digest；首次计算后记忆化。
    pub fn SQLDigest(&self) -> (String, Digest) {
        let mut memo = self.digestMemo.lock().expect("digest memo lock poisoned");
        let (normalized, digest) = memo
            .get_or_insert_with(|| parser_crate::digester_impl::NormalizeDigest(&self.OriginalSQL));
        (normalized.clone(), digest.clone())
    }

    /// 若尚未记忆化则预置规范化 SQL 与 digest。
    pub fn InitSQLDigest(&self, normalized: String, digest: Digest) {
        let mut memo = self.digestMemo.lock().expect("digest memo lock poisoned");
        if memo.is_none() {
            *memo = Some((normalized, digest));
        }
    }

    /// 用新 SQL 重新计算并覆盖 digest 记忆。
    pub fn ResetSQLDigest(&self, sql: &str) {
        *self.digestMemo.lock().expect("digest memo lock poisoned") =
            Some(parser_crate::digester_impl::NormalizeDigest(sql));
    }

    /// 返回规范化计划文本与计划 digest。
    pub fn GetPlanDigest(&self) -> (String, Option<Digest>) {
        self.planDigest
            .lock()
            .expect("plan digest lock poisoned")
            .clone()
    }
    /// 当前物理/逻辑计划对象（若有）。
    pub fn GetPlan(&self) -> Option<CacheValue> {
        self.plan.clone()
    }
    /// 设置当前计划对象。
    pub fn SetPlan(&mut self, plan: Option<CacheValue>) {
        self.plan = plan;
    }
    /// 扁平化计划对象（若有）。
    pub fn GetFlatPlan(&self) -> Option<CacheValue> {
        self.flatPlan
            .lock()
            .expect("flat plan lock poisoned")
            .clone()
    }
    /// 设置扁平化计划对象。
    pub fn SetFlatPlan(&self, plan: Option<CacheValue>) {
        *self.flatPlan.lock().expect("flat plan lock poisoned") = plan;
    }
    /// 二进制编码计划字符串。
    pub fn GetBinaryPlan(&self) -> String {
        self.binaryPlan.clone()
    }
    /// 设置二进制编码计划字符串。
    pub fn SetBinaryPlan(&mut self, plan: impl Into<String>) {
        self.binaryPlan = plan.into();
    }

    /// 组装当前语句的资源组标签（keyspace + SQL/计划 digest）。
    pub fn GetResourceGroupTagger(&self) -> ResourceGroupTagger {
        let (normalized, digest) = self.SQLDigest();
        ResourceGroupTagger {
            keyspaceName: KEYSPACE_NAME
                .read()
                .expect("keyspace name lock poisoned")
                .clone(),
            sqlDigest: (!normalized.is_empty()).then(|| digest.Bytes().to_vec()),
            planDigest: self
                .GetPlanDigest()
                .1
                .as_ref()
                .map(|digest| digest.Bytes().to_vec()),
        }
    }

    /// 标记本语句使用 Chunk 分配器。
    pub fn SetUseChunkAlloc(&self) {
        self.useChunkAlloc.store(true, Ordering::SeqCst);
    }
    /// 清除 Chunk 分配器使用标记。
    pub fn ClearUseChunkAlloc(&self) {
        self.useChunkAlloc.store(false, Ordering::SeqCst);
    }
    /// 是否使用 Chunk 分配器。
    pub fn GetUseChunkAllocStatus(&self) -> bool {
        self.useChunkAlloc.load(Ordering::SeqCst)
    }

    /// 在 digest 存在时设置规范化计划与计划 digest。
    pub fn SetPlanDigest(&self, normalized: impl Into<String>, digest: Option<Digest>) {
        if let Some(digest) = digest {
            *self.planDigest.lock().expect("plan digest lock poisoned") =
                (normalized.into(), Some(digest));
        }
    }

    /// 编码后的计划文本。
    pub fn GetEncodedPlan(&self) -> String {
        self.encodedPlan.clone()
    }
    /// 设置编码后的计划文本。
    pub fn SetEncodedPlan(&mut self, plan: impl Into<String>) {
        self.encodedPlan = plan.into();
    }
    /// 返回计划 hint 文本及是否已设置。
    pub fn GetPlanHint(&self) -> (String, bool) {
        (self.planHint.clone(), self.planHintSet)
    }
    /// 是否强制使用索引。
    pub fn GetIndexForce(&self) -> bool {
        self.indexForce
    }
    /// 初始化磁盘溢出 Tracker。
    pub fn InitDiskTracker(&mut self, label: i32, bytes_limit: i64) {
        self.DiskTracker = Some(NewTracker(label, bytes_limit));
    }
    /// 初始化内存 Tracker。
    pub fn InitMemTracker(&mut self, label: i32, bytes_limit: i64) {
        self.MemTracker = Some(NewTracker(label, bytes_limit));
    }
    /// 设置计划 hint 并标记已设置。
    pub fn SetPlanHint(&mut self, hint: impl Into<String>) {
        self.planHintSet = true;
        self.planHint = hint.into();
    }
    /// 标记强制走索引。
    pub fn SetIndexForce(&mut self) {
        self.indexForce = true;
    }
    /// 将 hint 相关原因追加为 Warning。
    pub fn SetHintWarning(&self, reason: impl Into<String>) {
        self.AppendWarning(errors::NewNoStackError(reason.into()));
    }
    /// 将已有错误对象追加为 Warning。
    pub fn SetHintWarningFromError(&self, reason: errors::SharedError) {
        self.AppendWarning(reason);
    }

    /// 累加受影响行数；外键触发器执行期间不计。
    pub fn AddAffectedRows(&self, rows: u64) {
        if !self.InHandleForeignKeyTrigger.load(Ordering::Relaxed) {
            self.affectedRows.fetch_add(rows, Ordering::Relaxed);
        }
    }
    /// 直接设置受影响行数。
    pub fn SetAffectedRows(&self, rows: u64) {
        self.affectedRows.store(rows, Ordering::Relaxed);
    }
    /// 读取受影响行数。
    pub fn AffectedRows(&self) -> u64 {
        self.affectedRows.load(Ordering::Relaxed)
    }

    fn read_rows(&self, f: impl FnOnce(&stmtCtxMu) -> u64) -> u64 {
        f(&self.mu.lock().expect("statement row counter lock poisoned"))
    }
    fn add_rows(&self, f: impl FnOnce(&mut stmtCtxMu), _rows: u64) {
        f(&mut self.mu.lock().expect("statement row counter lock poisoned"));
    }
    /// SELECT 找到的行数（FOUND_ROWS）。
    pub fn FoundRows(&self) -> u64 {
        self.read_rows(|mu| mu.foundRows)
    }
    /// 累加 FOUND_ROWS。
    pub fn AddFoundRows(&self, rows: u64) {
        self.add_rows(|mu| mu.foundRows += rows, rows);
    }
    /// 插入/替换等记录行数。
    pub fn RecordRows(&self) -> u64 {
        self.read_rows(|mu| mu.records)
    }
    /// 累加记录行数。
    pub fn AddRecordRows(&self, rows: u64) {
        self.add_rows(|mu| mu.records += rows, rows);
    }
    /// 删除行数。
    pub fn DeletedRows(&self) -> u64 {
        self.read_rows(|mu| mu.deleted)
    }
    /// 累加删除行数。
    pub fn AddDeletedRows(&self, rows: u64) {
        self.add_rows(|mu| mu.deleted += rows, rows);
    }
    /// 更新行数。
    pub fn UpdatedRows(&self) -> u64 {
        self.read_rows(|mu| mu.updated)
    }
    /// 累加更新行数。
    pub fn AddUpdatedRows(&self, rows: u64) {
        self.add_rows(|mu| mu.updated += rows, rows);
    }
    /// 复制行数。
    pub fn CopiedRows(&self) -> u64 {
        self.read_rows(|mu| mu.copied)
    }
    /// 累加复制行数。
    pub fn AddCopiedRows(&self, rows: u64) {
        self.add_rows(|mu| mu.copied += rows, rows);
    }
    /// 触碰行数（含未真正修改的更新）。
    pub fn TouchedRows(&self) -> u64 {
        self.read_rows(|mu| mu.touched)
    }
    /// 累加触碰行数。
    pub fn AddTouchedRows(&self, rows: u64) {
        self.add_rows(|mu| mu.touched += rows, rows);
    }
    /// 一次读取全部行计数器快照。
    pub fn RowCounters(&self) -> (u64, u64, u64, u64, u64, u64) {
        let mu = self.mu.lock().expect("statement row counter lock poisoned");
        (
            mu.foundRows,
            mu.records,
            mu.deleted,
            mu.updated,
            mu.copied,
            mu.touched,
        )
    }
    /// 语句完成消息（如 Records/Duplicates 文案）。
    pub fn GetMessage(&self) -> String {
        self.mu
            .lock()
            .expect("statement message lock poisoned")
            .message
            .clone()
    }
    /// 设置语句完成消息。
    pub fn SetMessage(&self, message: impl Into<String>) {
        self.mu
            .lock()
            .expect("statement message lock poisoned")
            .message = message.into();
    }

    /// 获取主告警列表。
    pub fn GetWarnings(&self) -> Vec<SQLWarn> {
        self.WarnHandler.GetWarnings()
    }
    /// 将告警拷贝到目标缓冲（可复用容量）。
    pub fn CopyWarnings(&self, dst: Vec<SQLWarn>) -> Vec<SQLWarn> {
        self.WarnHandler.CopyWarnings(dst)
    }
    /// 截断并返回从 `start` 起被移除的告警。
    pub fn TruncateWarnings(&self, start: isize) -> Vec<SQLWarn> {
        self.WarnHandler.TruncateWarnings(start)
    }
    /// 告警数量；`SHOW WARNINGS` 路径返回 0 以免递归。
    pub fn WarningCount(&self) -> u16 {
        if self.InShowWarning {
            0
        } else {
            self.WarnHandler.WarningCount() as u16
        }
    }
    /// 返回错误级与告警级条目数量。
    pub fn NumErrorWarnings(&self) -> (u16, usize) {
        self.WarnHandler.NumErrorWarnings()
    }
    /// 覆盖主告警列表。
    pub fn SetWarnings(&self, warnings: Vec<SQLWarn>) {
        self.WarnHandler.SetWarnings(warnings);
    }
    /// 追加一条 Warning。
    pub fn AppendWarning(&self, warning: errors::SharedError) {
        WarnAppender::AppendWarning(self.WarnHandler.as_ref(), warning);
    }
    /// 批量追加告警。
    pub fn AppendWarnings(&self, warnings: Vec<SQLWarn>) {
        self.WarnHandler.AppendWarnings(warnings);
    }
    /// 追加一条 Note。
    pub fn AppendNote(&self, warning: errors::SharedError) {
        WarnHandlerExt::AppendNote(self.WarnHandler.as_ref(), warning);
    }
    /// 追加一条 Error 级告警条目。
    pub fn AppendError(&self, warning: errors::SharedError) {
        self.WarnHandler.AppendError(warning);
    }
    /// 获取额外告警列表（不计入客户端 warning count）。
    pub fn GetExtraWarnings(&self) -> Vec<SQLWarn> {
        self.ExtraWarnHandler.GetWarnings()
    }
    /// 覆盖额外告警列表。
    pub fn SetExtraWarnings(&self, warnings: Vec<SQLWarn>) {
        self.ExtraWarnHandler.SetWarnings(warnings);
    }
    /// 追加额外 Warning。
    pub fn AppendExtraWarning(&self, warning: errors::SharedError) {
        WarnAppender::AppendWarning(self.ExtraWarnHandler.as_ref(), warning);
    }
    /// 追加额外 Note。
    pub fn AppendExtraNote(&self, warning: errors::SharedError) {
        WarnHandlerExt::AppendNote(self.ExtraWarnHandler.as_ref(), warning);
    }
    /// 追加额外 Error。
    pub fn AppendExtraError(&self, warning: errors::SharedError) {
        self.ExtraWarnHandler.AppendError(warning);
    }

    /// 重试前清零受影响行数与 mu 域计数/消息。
    fn resetMuForRetry(&self) {
        self.affectedRows.store(0, Ordering::Relaxed);
        *self.mu.lock().expect("statement row counter lock poisoned") = stmtCtxMu::default();
    }

    /// 语句重试复位：清计数/告警/部分缓存并分配新 TaskID。
    pub fn ResetForRetry(&self) {
        self.resetMuForRetry();
        self.ReservedRowIDAlloc
            .lock()
            .expect("reserved row ID lock poisoned")
            .Reset(0, 0);
        self.TableIDs
            .lock()
            .expect("statement table IDs lock poisoned")
            .clear();
        self.IndexNames
            .lock()
            .expect("statement index names lock poisoned")
            .clear();
        self.TaskID.store(AllocateTaskID(), Ordering::Release);
        self.WarnHandler.TruncateWarnings(0);
        self.ExtraWarnHandler.TruncateWarnings(0);
        *self
            .distSQLCtxCache
            .lock()
            .expect("DistSQL cache lock poisoned") = None;
    }

    /// Resets the statement-scoped cached-table marker before planning starts.
    /// 规划开始前清除「读自表缓存」标记。
    pub fn ResetReadFromTableCache(&self) {
        self.ReadFromTableCache.store(false, Ordering::Release);
    }

    /// Records that this statement reads through TiDB's table-cache path.
    /// 标记本语句走了表缓存读取路径。
    pub fn SetReadFromTableCache(&self) {
        self.ReadFromTableCache.store(true, Ordering::Release);
    }

    /// 是否读自表缓存。
    pub fn IsReadFromTableCache(&self) -> bool {
        self.ReadFromTableCache.load(Ordering::Acquire)
    }

    /// 获取同步执行明细快照。
    pub fn GetExecDetails(&self) -> ExecDetails {
        self.SyncExecDetails.GetExecDetails()
    }

    /// 生成下推到 TiKV/TiFlash 的语句标志位（语句类型 + 类型/错误策略）。
    pub fn PushDownFlags(&self) -> u64 {
        // 先编码类型/错误策略，再按当前语句类型叠加 Insert/Update/Select 等位。
        let mut flags = PushDownFlagsWithTypeFlagsAndErrLevels(self.TypeFlags(), self.ErrLevels());
        if self.EnableTiKVShortCircuitExpression {
            flags |= FlagEnableTiKVShortCircuitExpression;
        }
        if self.InInsertStmt {
            flags |= FlagInInsertStmt;
        } else if self.InUpdateStmt || self.InDeleteStmt {
            flags |= FlagInUpdateOrDeleteStmt;
        } else if self.InSelectStmt {
            flags |= FlagInSelectStmt;
        }
        if self.InLoadDataStmt {
            flags |= FlagInLoadDataStmt;
        }
        if self.InRestrictedSQL {
            flags |= FlagInRestrictedSQL;
        }
        flags
    }

    /// 从 protobuf 下推 flags 与时区反向初始化语句类型标志与错误级别。
    pub fn InitFromPBFlagAndTz(&mut self, flags: u64, tz: Tz) {
        self.InInsertStmt = flags & FlagInInsertStmt != 0;
        self.InSelectStmt = flags & FlagInSelectStmt != 0;
        self.InDeleteStmt = flags & FlagInUpdateOrDeleteStmt != 0;
        self.EnableTiKVShortCircuitExpression = flags & FlagEnableTiKVShortCircuitExpression != 0;
        let mut levels = self.ErrLevels();
        levels[errctx::ErrGroup::ErrGroupDividedByZero as usize] =
            errctx::ResolveErrLevel(false, flags & FlagDividedByZeroAsWarning != 0);
        self.SetErrLevels(levels);
        self.SetTimeZone(tz);
        self.SetTypeFlags(
            DefaultStmtFlags
                .WithIgnoreTruncateErr(flags & FlagIgnoreTruncate != 0)
                .WithTruncateAsWarning(flags & FlagTruncateAsWarning != 0)
                .WithIgnoreZeroInDate(flags & FlagIgnoreZeroInDate != 0)
                .WithAllowNegativeToUnsigned(!self.InInsertStmt),
        );
    }

    /// 悲观锁等待是否已开始。
    pub fn PessimisticLockStarted(&self) -> bool {
        self.lockWaitStartTime.load(Ordering::Acquire) > 0
    }

    /// 首次调用时记录锁等待起点，之后返回同一起点时间。
    pub fn GetLockWaitStartTime(&self) -> SystemTime {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .min(i64::MAX as u128) as i64;
        // CAS：仅首次把起点从 0 写成 now，后续读到已有起点。
        let stored = self
            .lockWaitStartTime
            .compare_exchange(0, now, Ordering::AcqRel, Ordering::Acquire)
            .unwrap_or_else(|value| value);
        UNIX_EPOCH + Duration::from_nanos(stored as u64)
    }

    /// 是否启用动态分区裁剪。
    pub fn UseDynamicPartitionPrune(&self) -> bool {
        self.logicalPlanBuild
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .use_dynamic_prune_mode
    }

    /// 从父 Tracker 树卸载内存与磁盘 Tracker。
    pub fn DetachMemDiskTracker(&mut self) {
        if let Some(tracker) = self.MemTracker.as_mut() {
            tracker.Detach();
        }
        if let Some(tracker) = self.DiskTracker.as_mut() {
            tracker.Detach();
        }
    }

    /// 若不存在求值器则注册过期读 TSO 提供者。
    pub fn SetStaleTSOProviderIfNotExist<F>(&self, evaluator: F)
    where
        F: FnMut() -> Result<u64, errors::SharedError> + Send + 'static,
    {
        let mut provider = self
            .StaleTSOProvider
            .lock()
            .expect("stale TSO provider lock poisoned");
        if provider.eval.is_none() {
            provider.value = None;
            provider.eval = Some(Box::new(evaluator));
        }
    }

    /// 获取过期读 TSO：成功值记忆化，错误不缓存。
    pub fn GetStaleTSO(&self) -> Result<u64, errors::SharedError> {
        let mut provider = self
            .StaleTSOProvider
            .lock()
            .expect("stale TSO provider lock poisoned");
        // 成功值已记忆则直接返回；错误路径不会写入 value。
        if let Some(value) = provider.value {
            return Ok(value);
        }
        let Some(eval) = provider.eval.as_mut() else {
            return Ok(0);
        };
        let value = eval()?;
        provider.value = Some(value);
        Ok(value)
    }

    /// 记录 SET_VAR hint 需恢复的旧变量值（同名只保留首次）。
    pub fn AddSetVarHintRestore(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.SetVarHintRestore
            .entry(name.into())
            .or_insert_with(|| value.into());
    }

    /// 获取本语句已用统计信息容器；可按需惰性初始化。
    pub fn GetUsedStatsInfo(&self, init_if_nil: bool) -> Option<Arc<UsedStatsInfo>> {
        if init_if_nil {
            Some(
                self.usedStatsInfo
                    .get_or_init(|| Arc::new(UsedStatsInfo::default()))
                    .clone(),
            )
        } else {
            self.usedStatsInfo.get().cloned()
        }
    }

    /// 已记录的列/索引统计加载状态条目总数。
    pub fn RecordedStatsLoadStatusCnt(&self) -> usize {
        self.GetUsedStatsInfo(false)
            .map(|all| {
                all.Values()
                    .iter()
                    .map(|status| status.recordedColIdxCount())
                    .sum()
            })
            .unwrap_or(0)
    }

    /// 返回类型上下文（始终有值，等价于 `TypeCtx`）。
    pub fn TypeCtxOrDefault(&self) -> TypeContext {
        self.typeCtx.clone()
    }

    /// 惰性初始化并缓存 DistSQL 上下文。
    pub fn GetOrInitDistSQLFromCache<F>(&self, create: F) -> CacheValue
    where
        F: FnOnce() -> CacheValue,
    {
        let mut cached = self
            .distSQLCtxCache
            .lock()
            .expect("DistSQL cache lock poisoned");
        cached.get_or_insert_with(create).clone()
    }
    /// Clear DistSQL capture at a new statement boundary without changing
    /// warnings, row counters, or other statement state.
    pub fn ResetDistSQLFromCache(&self) {
        *self
            .distSQLCtxCache
            .lock()
            .expect("DistSQL cache lock poisoned") = None;
    }
    /// 惰性初始化并缓存 Ranger（范围推导）上下文。
    pub fn GetOrInitRangerCtxFromCache<F>(&self, create: F) -> CacheValue
    where
        F: FnOnce() -> CacheValue,
    {
        self.rangerCtxCache.get_or_init(create).clone()
    }
    /// 惰性初始化并缓存 BuildPB 上下文。
    pub fn GetOrInitBuildPBCtxFromCache<F>(&self, create: F) -> CacheValue
    where
        F: FnOnce() -> CacheValue,
    {
        self.buildPBCtxCache.get_or_init(create).clone()
    }

    /// 通过 RuntimeStats 与 PlanID 回调读取结果行数。
    pub fn GetResultRowsCount(&self) -> i64 {
        let Some(runtime) = &self.RuntimeStatsColl else {
            return 0;
        };
        let Some(plan) = &self.plan else { return 0 };
        let Some(callback) = *PLAN_ID_FUNC.read().expect("plan id callback lock poisoned") else {
            return 0;
        };
        callback(plan)
            .map(|id| runtime.GetPlanActRows(id))
            .unwrap_or(0)
    }

    /// 记录计划缓存因范围过大而回退，并可能追加告警。
    pub fn RecordRangeFallback(&self, max_size: i64) {
        util_context::plancache::NewRangeFallbackHandler(
            &self.PlanCacheTracker,
            self.WarnHandler.as_ref(),
        )
        .RecordRangeFallback(max_size);
    }
}

/// 使用 UTC 时区构造新的语句上下文。
pub fn NewStmtCtx() -> Box<StatementContext> {
    NewStmtCtxWithTimeZone(chrono_tz::UTC)
}

/// 使用指定时区构造新的语句上下文。
pub fn NewStmtCtxWithTimeZone(tz: Tz) -> Box<StatementContext> {
    Box::new(StatementContext::build(
        tz,
        Arc::new(NewStaticWarnHandler(0)),
        Arc::new(NewStaticWarnHandler(0)),
        Arc::new(Mutex::new(stmtCtxMu::default())),
        Arc::new(Mutex::new(stmtCache::default())),
        Arc::new(Mutex::new(staleTSOProvider::default())),
    ))
}

/// 仅由类型标志与错误级别推导下推 flags（不含语句类型位）。
pub fn PushDownFlagsWithTypeFlagsAndErrLevels(type_flags: Flags, levels: errctx::LevelMap) -> u64 {
    let mut flags = 0;
    if type_flags.IgnoreTruncateErr() {
        flags |= FlagIgnoreTruncate;
    } else if type_flags.TruncateAsWarning() {
        flags |= FlagTruncateAsWarning | FlagOverflowAsWarning;
    }
    if type_flags.IgnoreZeroInDate() {
        flags |= FlagIgnoreZeroInDate;
    }
    if levels[errctx::ErrGroup::ErrGroupDividedByZero as usize] != errctx::Level::LevelError {
        flags |= FlagDividedByZeroAsWarning;
    }
    flags
}

#[derive(Default)]
/// 单表已用统计元信息及列/索引加载状态，供 EXPLAIN / 慢日志输出。
pub struct UsedStatsInfoForTable {
    pub Name: String,
    pub TblInfo: Option<Arc<TableInfo>>,
    pub Version: u64,
    pub RealtimeCount: i64,
    pub ModifyCount: i64,
    pub ColumnStatsLoadStatus: HashMap<i64, String>,
    pub IndexStatsLoadStatus: HashMap<i64, String>,
    pub ColAndIdxStatus: Option<CacheValue>,
}

impl UsedStatsInfoForTable {
    /// 格式化为 EXPLAIN 用的 `stats:pseudo` / `stats:partial[...]` 文案。
    pub fn FormatForExplain(&self) -> String {
        if self.Version == 0 {
            return "stats:pseudo".to_owned();
        }
        if self.ColumnStatsLoadStatus.is_empty() && self.IndexStatsLoadStatus.is_empty() {
            return String::new();
        }
        // EXPLAIN 最多展示 3 条明细，其余按状态聚合到 more。
        let mut output_left = 3;
        let mut counts = BTreeMap::<String, u64>::new();
        let mut values =
            self.collectFromColOrIdxStatus(false, Some(&mut output_left), Some(&mut counts));
        values.extend(self.collectFromColOrIdxStatus(
            true,
            Some(&mut output_left),
            Some(&mut counts),
        ));
        let mut result = format!("stats:partial[{}", values.join(", "));
        if !counts.is_empty() {
            let more = counts
                .into_iter()
                .map(|(status, count)| format!("{count} {status}"))
                .collect::<Vec<_>>()
                .join(", ");
            result.push_str(&format!("...(more: {more})"));
        }
        result.push(']');
        result
    }

    /// 将表级统计元信息与列/索引状态写入慢日志。
    pub fn WriteToSlowLog<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        let version = if self.Version == 0 {
            "pseudo".to_owned()
        } else {
            self.Version.to_string()
        };
        write!(
            writer,
            "{}:stats_meta_version={}[realtime_count={};modify_count={}]",
            self.Name, version, self.RealtimeCount, self.ModifyCount
        )?;
        // pseudo 统计只写 meta，不再输出列/索引加载明细。
        if self.Version == 0 {
            return Ok(());
        }
        if !self.ColumnStatsLoadStatus.is_empty() || !self.IndexStatsLoadStatus.is_empty() {
            let indexes = self.collectFromColOrIdxStatus(false, None, None).join(",");
            let columns = self.collectFromColOrIdxStatus(true, None, None).join(",");
            write!(writer, "[{indexes}][{columns}]")?;
        }
        Ok(())
    }

    /// 按 ID 排序收集列或索引加载状态；可选限制输出条数并累计省略计数。
    fn collectFromColOrIdxStatus(
        &self,
        for_column: bool,
        mut output_left: Option<&mut usize>,
        mut counts: Option<&mut BTreeMap<String, u64>>,
    ) -> Vec<String> {
        let statuses = if for_column {
            &self.ColumnStatsLoadStatus
        } else {
            &self.IndexStatsLoadStatus
        };
        let mut ids: Vec<_> = statuses.keys().copied().collect();
        ids.sort_unstable();
        let mut result = Vec::with_capacity(ids.len());
        for id in ids {
            let can_output = output_left.as_ref().is_none_or(|left| **left > 0);
            if can_output {
                let mut name = self
                    .TblInfo
                    .as_ref()
                    .map(|table| {
                        if for_column {
                            table.FindColumnNameByID(id)
                        } else {
                            table.FindIndexNameByID(id)
                        }
                    })
                    .unwrap_or_default();
                if name.is_empty() {
                    name = format!("ID {id}");
                }
                result.push(format!("{name}:{}", statuses[&id]));
                if let Some(left) = output_left.as_deref_mut() {
                    *left -= 1;
                }
            } else if let Some(counts) = counts.as_deref_mut() {
                *counts.entry(statuses[&id].clone()).or_default() += 1;
            }
        }
        result
    }

    /// 已记录的列+索引状态条目数。
    fn recordedColIdxCount(&self) -> usize {
        self.IndexStatsLoadStatus.len() + self.ColumnStatsLoadStatus.len()
    }
}

#[derive(Default)]
/// 本语句涉及各表的已用统计信息映射。
pub struct UsedStatsInfo {
    store: RwLock<HashMap<i64, Arc<UsedStatsInfoForTable>>>,
}

impl UsedStatsInfo {
    /// 按表 ID 读取已用统计。
    pub fn GetUsedInfo(&self, table_id: i64) -> Option<Arc<UsedStatsInfoForTable>> {
        self.store
            .read()
            .expect("used stats lock poisoned")
            .get(&table_id)
            .cloned()
    }
    /// 记录/覆盖某表的已用统计。
    pub fn RecordUsedInfo(&self, table_id: i64, info: Arc<UsedStatsInfoForTable>) {
        self.store
            .write()
            .expect("used stats lock poisoned")
            .insert(table_id, info);
    }
    /// 全部表 ID 键。
    pub fn Keys(&self) -> Vec<i64> {
        self.store
            .read()
            .expect("used stats lock poisoned")
            .keys()
            .copied()
            .collect()
    }
    /// 全部表级统计值。
    pub fn Values(&self) -> Vec<Arc<UsedStatsInfoForTable>> {
        self.store
            .read()
            .expect("used stats lock poisoned")
            .values()
            .cloned()
            .collect()
    }
}

/// 单项统计同步加载结果（表项 ID + 可选错误）。
pub struct StatsLoadResult {
    pub Item: TableItemID,
    pub Error: Option<errors::SharedError>,
}

impl StatsLoadResult {
    /// 是否加载失败。
    pub fn HasError(&self) -> bool {
        self.Error.is_some()
    }
    /// 格式化错误消息（含 tableID/id/isIndex）。
    pub fn ErrorMsg(&self) -> String {
        self.Error
            .as_ref()
            .map(|error| {
                format!(
                    "tableID:{}, id:{}, isIndex:{}, err:{error}",
                    self.Item.TableID, self.Item.ID, self.Item.IsIndex
                )
            })
            .unwrap_or_default()
    }
}

#[derive(Clone, Default)]
/// 语句标签上下文：可覆盖默认的语句类型标签。
pub struct StmtLabelContext {
    label: Option<String>,
}

/// 为上下文设置自定义语句标签。
pub fn WithStmtLabel(mut context: StmtLabelContext, label: impl Into<String>) -> StmtLabelContext {
    context.label = Some(label.into());
    context
}

/// 取自定义标签，否则回退为语句种类的默认标签。
pub fn GetStmtLabel(context: &StmtLabelContext, node: &StatementKind) -> String {
    context
        .label
        .clone()
        .unwrap_or_else(|| parser_crate::ast::ast::GetStmtLabel(node))
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// 语句缓存键（对应 Go 中的整型常量键）。
pub struct StmtCacheKey(pub i32);

/// 缓存键：当前时间戳。
pub const StmtNowTsCacheKey: StmtCacheKey = StmtCacheKey(0);
/// 缓存键：安全时间戳（SafeTS）。
pub const StmtSafeTSCacheKey: StmtCacheKey = StmtCacheKey(1);
/// 缓存键：外部时间戳。
pub const StmtExternalTSCacheKey: StmtCacheKey = StmtCacheKey(2);

/// 计划缓存类型别名占位（与 Go 整型枚举对齐）。
pub type PlanCacheTypeAlias = i32;
