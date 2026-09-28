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

#![allow(non_snake_case)]

// 计划缓存（Plan Cache）核心工具类型与算法。
//
// Plan Cache 复用已优化的执行计划，避免重复优化。本模块定义缓存键构造、
// Prepared 语句元数据、Point Get 执行器槽位、缓存值运行时统计，以及
// 参数类型兼容性与安全 Point Get 路径判定。

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ast;
use sha2::{Digest, Sha256};

/// 可缓存的参数化 LIMIT 计数值上限。
pub const MaxCacheableLimitCount: usize = 10_000;
/// Prepared Plan Cache 允许占用的最大内存（字节）。
pub static PreparedPlanCacheMaxMemory: AtomicU64 = AtomicU64::new(u64::MAX);

/// 设置 Prepared Plan Cache 最大内存。
pub fn SetPreparedPlanCacheMaxMemory(bytes: u64) {
    PreparedPlanCacheMaxMemory.store(bytes, Ordering::Relaxed);
}

/// 读取 Prepared Plan Cache 最大内存。
pub fn GetPreparedPlanCacheMaxMemory() -> u64 {
    PreparedPlanCacheMaxMemory.load(Ordering::Relaxed)
}

#[derive(Clone, Debug, Default, PartialEq)]
/// SQL 中 `?` 参数标记：源码偏移、排序序号、EXECUTE 阶段值及填充状态。
pub struct PlanCacheParamMarker {
    pub offset: usize,
    pub order: usize,
    pub datum: Option<crate::Datum>,
    pub in_execute: bool,
}

/// 按 offset 排序参数标记，重写 order，并清除 in_execute。
pub fn ExtractAndSortParamMarkers(
    mut markers: Vec<PlanCacheParamMarker>,
) -> Vec<PlanCacheParamMarker> {
    markers.sort_by_key(|marker| marker.offset);
    for (order, marker) in markers.iter_mut().enumerate() {
        marker.order = order;
        marker.datum = None;
        marker.in_execute = false;
    }
    markers
}

/// 递归检查表达式是否包含 fts_match_word 全文检索函数。
fn expression_contains_fts(expression: &ast::ExprNode) -> bool {
    match &expression.Kind {
        ast::ExprKind::Function { FnName, Args, .. } => {
            FnName.L.eq_ignore_ascii_case("fts_match_word")
                || Args.iter().any(expression_contains_fts)
        }
        ast::ExprKind::Binary { L, R, .. } => {
            expression_contains_fts(L) || expression_contains_fts(R)
        }
        ast::ExprKind::Unary { V, .. } | ast::ExprKind::Parentheses(V) => {
            expression_contains_fts(V)
        }
        _ => false,
    }
}

/// FTS plans are rebuilt for every prepared execution because their TiFlash
/// index resolution depends on live replica and transaction state.
/// 全文检索（FTS）计划依赖 TiFlash 副本与事务状态，每次执行需重新解析，故不可缓存。
pub fn FullTextPreparedCacheability(select: &ast::SelectStmt) -> (bool, String) {
    let contains_fts = select.Where.as_ref().is_some_and(expression_contains_fts)
        || select
            .Fields
            .Fields
            .iter()
            .filter_map(|field| field.Expr.as_ref())
            .any(expression_contains_fts)
        || select
            .OrderBy
            .iter()
            .any(|item| expression_contains_fts(&item.Expr));
    if contains_fts {
        (
            false,
            "full-text plans require per-execution resolution".to_owned(),
        )
    } else {
        (true, String::new())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 访问权限检查时记录的库/表/列信息。
pub struct PlanCacheVisitInfo {
    pub privilege: String,
    pub database: String,
    pub table: String,
    pub column: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 语句中的 LIMIT：offset/count 及其是否为参数占位。
pub struct PlanCacheLimit {
    pub offset: u64,
    pub count: u64,
    pub offset_is_parameter: bool,
    pub count_is_parameter: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 缓存相关表元数据：库名、表名、ID、修订号、统计版本、是否临时表。
pub struct PlanCacheTable {
    pub database: ast::CIStr,
    pub name: ast::CIStr,
    pub id: i64,
    pub revision: u64,
    pub stats_version: u64,
    pub temporary: bool,
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
/// SQL/计划摘要的字节摘要值。
pub struct PlanCacheDigest(pub Vec<u8>);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Binding（执行计划绑定）的归一化 SQL 与 digest。
pub struct PlanCacheBindingInfo {
    pub normalized_sql: String,
    pub digest: Option<PlanCacheDigest>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
/// 计划缓存键的字节序列封装。
pub struct PlanCacheKey(Vec<u8>);

impl PlanCacheKey {
    /// 以字节切片形式返回缓存键内容。
    pub fn AsBytes(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 隔离读引擎开关：是否允许 TiDB/TiKV/TiFlash。
pub struct PlanCacheIsolationReadEngines {
    pub tidb: bool,
    pub tikv: bool,
    pub tiflash: bool,
}

/// All session/domain values that participate in Go's plan-cache key. Keeping
/// this immutable snapshot prevents key construction from mutating live
/// `IsolationReadEngines` while temporarily excluding TiFlash for writes.
/// 参与缓存键的会话/域快照；不可变以免构造键时改写隔离读引擎集合。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PlanCacheKeyContext {
    pub user_name: String,
    pub host_name: String,
    pub current_database: String,
    pub latest_schema_version: i64,
    pub read_committed: bool,
    pub statement_read_only: bool,
    pub partition_prune_mode: String,
    pub sql_mode: u64,
    pub no_backslash_escapes_in_like: bool,
    pub timezone_offset_seconds: i64,
    pub isolation_read_engines: PlanCacheIsolationReadEngines,
    pub select_limit: u64,
    pub connection_charset: String,
    pub connection_collation: String,
    pub restricted_sql: bool,
    pub restricted_read_only: bool,
    pub super_read_only: bool,
    pub expression_pushdown_blacklist_version: i64,
    pub foreign_key_checks: bool,
    pub enable_plan_cache_for_subquery: bool,
    pub enable_plan_cache_for_parameterized_limit: bool,
    pub invalidate_on_fresh_stats: bool,
    pub skip_stats_when_binding: bool,
    pub dirty_table_ids: Vec<i64>,
    pub in_transaction: bool,
    pub autocommit: bool,
    pub pessimistic_autocommit: bool,
    pub share_lock_enabled_by_noop: bool,
    pub shared_lock_promotion: bool,
    pub allow_uninitialized_schema_version_for_test: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 命中的 Binding SQL 文本。
pub struct MatchedPlanCacheBinding {
    pub bind_sql: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 构造缓存键的结果：键、binding、是否可缓存及原因。
pub struct PlanCacheKeyResult {
    pub key: Option<PlanCacheKey>,
    pub binding: String,
    pub cacheable: bool,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 计划缓存相关错误消息。
pub struct PlanCacheError {
    pub message: String,
}

impl PlanCacheError {
    /// 由消息字符串构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PlanCacheError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PlanCacheError {}

/// 将 i64 按可比较字节序写入缓冲（符号位翻转后大端）。
fn encode_i64(buffer: &mut Vec<u8>, value: i64) {
    buffer.extend_from_slice(&((value as u64) ^ (1_u64 << 63)).to_be_bytes());
}

/// 将 u64 以大端字节序写入缓冲。
fn encode_u64(buffer: &mut Vec<u8>, value: u64) {
    buffer.extend_from_slice(&value.to_be_bytes());
}

/// 按 key 排序后将 HashMap<i64,u64> 编码进缓存键缓冲。
pub fn HashInt64Uint64Map(buffer: &mut Vec<u8>, values: &HashMap<i64, u64>) {
    let mut keys: Vec<_> = values.keys().copied().collect();
    keys.sort_unstable();
    for key in keys {
        encode_i64(buffer, key);
        encode_u64(buffer, values[&key]);
    }
}

/// 布尔编码为字节 '1'/'0'。
fn bool2Byte(flag: bool) -> u8 {
    if flag { b'1' } else { b'0' }
}

/// 快照时间戳（Snapshot TS）求值上下文标记 trait。
pub trait SnapshotTSEvaluationContext: Send + Sync {}

impl<T> SnapshotTSEvaluationContext for T where T: Send + Sync {}

/// 计算 AS OF / 快照读时间戳的回调类型。
pub type SnapshotTSEvaluator = Arc<
    dyn Fn(&dyn SnapshotTSEvaluationContext) -> Result<u64, SnapshotTSEvaluationError>
        + Send
        + Sync,
>;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 快照 TS 求值失败错误。
pub struct SnapshotTSEvaluationError {
    pub message: String,
}

impl SnapshotTSEvaluationError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SnapshotTSEvaluationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SnapshotTSEvaluationError {}

/// The three Go `any` slots are independent type parameters. Callers cannot
/// put the wrong payload in a slot and no dynamic downcast is required.
/// Point Get 执行器缓存的三槽位（列信息/执行器/快路径计划），用泛型代替 Go 的 any。
pub struct PointGetExecutorCache<ColumnInfo = (), Executor = (), FastPlan = ()> {
    column_infos: Mutex<Option<Vec<ColumnInfo>>>,
    executor: Mutex<Option<Executor>>,
    fast_plan: Mutex<Option<FastPlan>>,
}

impl<ColumnInfo, Executor, FastPlan> Default
    for PointGetExecutorCache<ColumnInfo, Executor, FastPlan>
{
    fn default() -> Self {
        Self {
            column_infos: Mutex::new(None),
            executor: Mutex::new(None),
            fast_plan: Mutex::new(None),
        }
    }
}

impl<ColumnInfo, Executor, FastPlan> PointGetExecutorCache<ColumnInfo, Executor, FastPlan> {
    /// 缓存列信息到 Point Get 槽位。
    pub fn CacheColumnInfos(&self, columns: Vec<ColumnInfo>) {
        *self
            .column_infos
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(columns);
    }

    /// 取出并清空已缓存的列信息。
    pub fn TakeColumnInfos(&self) -> Option<Vec<ColumnInfo>> {
        self.column_infos
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    /// 缓存 Point Get 执行器实例。
    pub fn CacheExecutor(&self, executor: Executor) {
        *self
            .executor
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(executor);
    }

    /// 取出并清空已缓存的执行器。
    pub fn TakeExecutor(&self) -> Option<Executor> {
        self.executor
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    /// 缓存快路径计划。
    pub fn CacheFastPlan(&self, plan: FastPlan) {
        *self
            .fast_plan
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(plan);
    }

    /// 取出并清空快路径计划。
    pub fn TakeFastPlan(&self) -> Option<FastPlan> {
        self.fast_plan
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    /// 清空三个缓存槽位。
    pub fn Reset(&self) {
        self.TakeColumnInfos();
        self.TakeExecutor();
        self.TakeFastPlan();
    }
}

impl<ColumnInfo: Clone, Executor, FastPlan> PointGetExecutorCache<ColumnInfo, Executor, FastPlan> {
    /// 克隆返回已缓存列信息（不取出）。
    pub fn CachedColumnInfos(&self) -> Option<Vec<ColumnInfo>> {
        self.column_infos
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl<ColumnInfo, Executor, FastPlan: Clone> PointGetExecutorCache<ColumnInfo, Executor, FastPlan> {
    /// Shared instance-cache plans are cloned before an execution mutates them.
    /// 共享实例缓存中的快路径计划在执行前克隆，避免并发写冲突。
    pub fn CloneFastPlan(&self) -> Option<FastPlan> {
        self.fast_plan
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

/// Full prepared-statement cache metadata. Generic PointGet payloads keep this
/// crate independent from server/executor crates while retaining one formal
/// `PlanCacheStmt` instead of per-consumer sidecars.
/// Prepared 语句完整缓存元数据；PointGet 载荷泛型化以隔离服务端依赖。
pub struct PlanCacheStmt<ColumnInfo = (), Executor = (), FastPlan = ()> {
    pub PreparedAst: ast::misc::Prepared,
    pub ResolveCtx: Option<resolve_dependency::Context>,
    pub StmtDB: String,
    pub VisitInfos: Vec<PlanCacheVisitInfo>,
    pub Params: Vec<PlanCacheParamMarker>,
    pub PointGet: PointGetExecutorCache<ColumnInfo, Executor, FastPlan>,
    pub SchemaVersion: i64,
    pub RelateVersion: HashMap<i64, u64>,
    pub StmtCacheable: bool,
    pub UncacheableReason: String,
    pub HasUsePlanCacheHint: bool,
    pub NormalizedSQL: String,
    pub NormalizedPlan: String,
    pub SQLDigest: Option<PlanCacheDigest>,
    pub PlanDigest: Option<PlanCacheDigest>,
    pub ForUpdateRead: bool,
    pub SnapshotTSEvaluator: Option<SnapshotTSEvaluator>,
    pub BindingInfo: PlanCacheBindingInfo,
    pub StmtText: String,
    limits: Vec<PlanCacheLimit>,
    has_subquery: bool,
    tables: Vec<PlanCacheTable>,
    db_names: Vec<ast::CIStr>,
    metadata_lock_tables: Vec<PlanCacheTable>,
}

impl<ColumnInfo, Executor, FastPlan> Default for PlanCacheStmt<ColumnInfo, Executor, FastPlan> {
    fn default() -> Self {
        Self {
            PreparedAst: ast::misc::Prepared::default(),
            ResolveCtx: None,
            StmtDB: String::new(),
            VisitInfos: Vec::new(),
            Params: Vec::new(),
            PointGet: PointGetExecutorCache::default(),
            SchemaVersion: 0,
            RelateVersion: HashMap::new(),
            StmtCacheable: false,
            UncacheableReason: String::new(),
            HasUsePlanCacheHint: false,
            NormalizedSQL: String::new(),
            NormalizedPlan: String::new(),
            SQLDigest: None,
            PlanDigest: None,
            ForUpdateRead: false,
            SnapshotTSEvaluator: None,
            BindingInfo: PlanCacheBindingInfo::default(),
            StmtText: String::new(),
            limits: Vec::new(),
            has_subquery: false,
            tables: Vec::new(),
            db_names: Vec::new(),
            metadata_lock_tables: Vec::new(),
        }
    }
}

impl<ColumnInfo, Executor, FastPlan> PlanCacheStmt<ColumnInfo, Executor, FastPlan> {
    /// 由 Prepared AST 与语句文本构造 PlanCacheStmt。
    pub fn new(prepared_ast: ast::misc::Prepared, stmt_text: impl Into<String>) -> Self {
        Self {
            PreparedAst: prepared_ast,
            StmtText: stmt_text.into(),
            ..Self::default()
        }
    }

    /// 返回语句中收集到的 LIMIT 列表。
    pub fn Limits(&self) -> &[PlanCacheLimit] {
        &self.limits
    }

    /// 语句是否包含子查询。
    pub fn HasSubquery(&self) -> bool {
        self.has_subquery
    }

    /// 返回引用表列表。
    pub fn Tables(&self) -> &[PlanCacheTable] {
        &self.tables
    }

    /// 写入 LIMIT、子查询标志与引用表元数据。
    pub fn CollectPlanCacheStmtInfo(
        &mut self,
        limits: Vec<PlanCacheLimit>,
        has_subquery: bool,
        tables: Vec<PlanCacheTable>,
    ) {
        self.limits = limits;
        self.has_subquery = has_subquery;
        self.tables = tables;
    }

    /// 返回元数据锁相关的库名列表。
    pub fn DBName(&self) -> &[ast::CIStr] {
        &self.db_names
    }

    /// 返回元数据锁相关的表列表。
    pub fn Tbls(&self) -> &[PlanCacheTable] {
        &self.metadata_lock_tables
    }

    /// EXECUTE may replace refreshed table entries and must not mutate a
    /// cached template's backing storage.
    /// EXECUTE 可替换刷新后的表条目，不得改写缓存模板的底层存储。
    pub fn SetDBNameAndTbls(
        &mut self,
        database_names: Vec<ast::CIStr>,
        tables: Vec<PlanCacheTable>,
    ) {
        self.db_names = database_names;
        self.metadata_lock_tables = tables;
    }

    /// 若存在求值器则计算快照 TS，否则返回 None。
    pub fn EvaluateSnapshotTS(
        &self,
        context: &dyn SnapshotTSEvaluationContext,
    ) -> Result<Option<u64>, SnapshotTSEvaluationError> {
        self.SnapshotTSEvaluator
            .as_ref()
            .map(|evaluator| evaluator(context))
            .transpose()
    }
}

impl<ColumnInfo: Clone, Executor, FastPlan> PlanCacheStmt<ColumnInfo, Executor, FastPlan> {
    pub fn CachedColumnInfos(&self) -> Option<Vec<ColumnInfo>> {
        self.PointGet.CachedColumnInfos()
    }

    /// 转发到 PointGet 槽位缓存列信息。
    pub fn CacheColumnInfos(&self, columns: Vec<ColumnInfo>) {
        self.PointGet.CacheColumnInfos(columns);
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// PREPARE 路径上的语句种类，用于拒绝不支持的类型。
pub enum PlanCacheStatementKind {
    Ddl,
    ImportInto,
    LoadData,
    Prepare,
    Execute,
    Deallocate,
    NonTransactionalDml,
    SelectInto,
    #[default]
    Other,
}

#[derive(Default)]
/// 生成 PlanCacheStmt 所需的全部 PREPARE 输入。
pub struct PlanCachePrepareInput {
    pub is_prepared_statement: bool,
    pub parameterized_sql: String,
    pub prepared_ast: ast::misc::Prepared,
    pub statement_kind: PlanCacheStatementKind,
    pub markers: Vec<PlanCacheParamMarker>,
    pub resolve_context: Option<resolve_dependency::Context>,
    pub current_database: String,
    pub visit_infos: Vec<PlanCacheVisitInfo>,
    pub for_update_read: bool,
    pub snapshot_ts_evaluator: Option<SnapshotTSEvaluator>,
    pub prepared_cache_enabled: bool,
    pub non_prepared_cache_enabled: bool,
    pub ast_cacheable: bool,
    pub uncacheable_reason: String,
    pub force_cache_fix_49736: bool,
    pub uses_static_partition_pruning: bool,
    pub has_use_plan_cache_hint: bool,
    pub normalized_sql: String,
    pub sql_digest: Option<PlanCacheDigest>,
    pub schema_version: i64,
    pub related_versions: HashMap<i64, u64>,
    pub database_names: Vec<ast::CIStr>,
    pub metadata_lock_tables: Vec<PlanCacheTable>,
    pub limits: Vec<PlanCacheLimit>,
    pub has_subquery: bool,
    pub referenced_tables: Vec<PlanCacheTable>,
}

/// PREPARE 运行时钩子：预处理、构建计划、权限检查与告警。
pub trait PlanCachePrepareRuntime {
    fn Preprocess(&self, input: &PlanCachePrepareInput) -> Result<(), PlanCacheError>;
    fn Build(
        &self,
        input: &PlanCachePrepareInput,
    ) -> Result<Box<dyn base_dependency::Plan>, PlanCacheError>;
    fn CheckPreparedPrivileges(&self, statement: &PlanCacheStmt) -> Result<(), PlanCacheError>;
    fn AppendWarning(&self, warning: &str);
}

/// Executes the same prepare ordering as Go: validate marker eligibility,
/// preprocess, sort/reset markers, build, apply cacheability overrides, collect
/// metadata, then perform the privilege check.
/// 与 Go 相同的 PREPARE 顺序：校验标记→预处理→排序→构建→可缓存覆盖→收集元数据→权限检查。
pub fn GeneratePlanCacheStmtWithAST(
    runtime: &dyn PlanCachePrepareRuntime,
    mut input: PlanCachePrepareInput,
) -> Result<(PlanCacheStmt, Box<dyn base_dependency::Plan>, usize), PlanCacheError> {
    if input.statement_kind == PlanCacheStatementKind::Ddl && !input.markers.is_empty() {
        return Err(PlanCacheError::new(
            "DDL statements cannot contain prepared parameters",
        ));
    }
    if matches!(
        input.statement_kind,
        PlanCacheStatementKind::ImportInto
            | PlanCacheStatementKind::LoadData
            | PlanCacheStatementKind::Prepare
            | PlanCacheStatementKind::Execute
            | PlanCacheStatementKind::Deallocate
            | PlanCacheStatementKind::NonTransactionalDml
            | PlanCacheStatementKind::SelectInto
    ) {
        return Err(PlanCacheError::new("unsupported prepared statement"));
    }
    if input.markers.len() > u16::MAX as usize {
        return Err(PlanCacheError::new("too many prepared parameters"));
    }

    runtime.Preprocess(&input)?;
    input.markers.sort_by_key(|marker| marker.offset);
    for (order, marker) in input.markers.iter_mut().enumerate() {
        marker.order = order;
        // Go only resets values collected for binary-protocol PREPARE. Values
        // produced by non-prepared parameterization are already initialized.
        if input.is_prepared_statement {
            marker.datum = None;
            marker.in_execute = false;
        }
    }
    let parameter_count = input.markers.len();
    let plan = runtime.Build(&input)?;

    // 按 Prepared/Non-Prepared 开关与 AST 可缓存性决定是否入缓存；可强制覆盖并告警。
    let enabled = if input.is_prepared_statement {
        input.prepared_cache_enabled
    } else {
        input.non_prepared_cache_enabled
    };
    let mut cacheable = enabled && (!input.is_prepared_statement || input.ast_cacheable);
    let mut reason = if enabled {
        input.uncacheable_reason.clone()
    } else {
        "plan cache is disabled".to_owned()
    };
    if !cacheable && input.force_cache_fix_49736 {
        runtime.AppendWarning(&format!(
            "force plan-cache: may use risky cached plan: {reason}"
        ));
        cacheable = true;
        reason.clear();
    } else if !cacheable {
        runtime.AppendWarning(&format!("skip prepared plan-cache: {reason}"));
    }
    if cacheable && input.uses_static_partition_pruning {
        cacheable = false;
        reason = "static partition prune mode used".to_owned();
        runtime.AppendWarning(&format!("skip prepared plan-cache: {reason}"));
    }

    let mut statement = PlanCacheStmt::new(input.prepared_ast, input.parameterized_sql);
    statement.ResolveCtx = input.resolve_context;
    statement.StmtDB = input.current_database;
    statement.VisitInfos = input.visit_infos;
    statement.Params = input.markers;
    statement.SchemaVersion = input.schema_version;
    statement.RelateVersion = input.related_versions;
    statement.StmtCacheable = cacheable;
    statement.UncacheableReason = reason;
    statement.HasUsePlanCacheHint = input.has_use_plan_cache_hint;
    statement.NormalizedSQL = input.normalized_sql;
    statement.SQLDigest = input.sql_digest;
    statement.ForUpdateRead = input.for_update_read;
    statement.SnapshotTSEvaluator = input.snapshot_ts_evaluator;
    statement.CollectPlanCacheStmtInfo(input.limits, input.has_subquery, input.referenced_tables);
    statement.SetDBNameAndTbls(input.database_names, input.metadata_lock_tables);
    runtime.CheckPreparedPrivileges(&statement)?;
    Ok((statement, plan, parameter_count))
}

/// 无 Binding 命中时构造计划缓存键。
pub fn NewPlanCacheKey<ColumnInfo, Executor, FastPlan>(
    context: &PlanCacheKeyContext,
    statement: &PlanCacheStmt<ColumnInfo, Executor, FastPlan>,
) -> Result<PlanCacheKeyResult, PlanCacheError> {
    NewPlanCacheKeyWithMatchedBinding(context, statement, None)
}

/// 结合会话上下文、语句元数据与可选 Binding 构造确定性缓存键。
pub fn NewPlanCacheKeyWithMatchedBinding<ColumnInfo, Executor, FastPlan>(
    context: &PlanCacheKeyContext,
    statement: &PlanCacheStmt<ColumnInfo, Executor, FastPlan>,
    matched_binding: Option<&MatchedPlanCacheBinding>,
) -> Result<PlanCacheKeyResult, PlanCacheError> {
    if statement.StmtText.is_empty() {
        return Err(PlanCacheError::new("no statement text"));
    }
    if statement.SchemaVersion == 0 && !context.allow_uninitialized_schema_version_for_test {
        return Err(PlanCacheError::new("schema version uninitialized"));
    }
    if statement.has_subquery && !context.enable_plan_cache_for_subquery {
        return Ok(PlanCacheKeyResult {
            key: None,
            binding: String::new(),
            cacheable: false,
            reason: "the switch 'tidb_enable_plan_cache_for_subquery' is off".to_owned(),
        });
    }
    if !statement.limits.is_empty() && !context.enable_plan_cache_for_parameterized_limit {
        return Ok(PlanCacheKeyResult {
            key: None,
            binding: String::new(),
            cacheable: false,
            reason: "the switch 'tidb_enable_plan_cache_for_param_limit' is off".to_owned(),
        });
    }

    let binding = matched_binding.map_or_else(String::new, |value| value.bind_sql.clone());
    let statement_database = if statement.StmtDB.is_empty() {
        &context.current_database
    } else {
        &statement.StmtDB
    };
    let latest_schema_version = if context.read_committed || statement.ForUpdateRead {
        context.latest_schema_version
    } else {
        0
    };

    // 按固定字段顺序编码会话与语句快照，保证相同输入得到相同缓存键。
    let mut key = Vec::new();
    key.extend_from_slice(context.user_name.as_bytes());
    key.extend_from_slice(context.host_name.as_bytes());
    key.extend_from_slice(statement_database.as_bytes());
    key.extend_from_slice(statement.StmtText.as_bytes());
    encode_i64(&mut key, statement.SchemaVersion);
    HashInt64Uint64Map(&mut key, &statement.RelateVersion);
    key.extend_from_slice(context.partition_prune_mode.as_bytes());
    encode_i64(&mut key, latest_schema_version);
    encode_i64(&mut key, context.sql_mode as i64);
    key.push(bool2Byte(context.no_backslash_escapes_in_like));
    encode_i64(&mut key, context.timezone_offset_seconds);
    if context.isolation_read_engines.tidb {
        key.extend_from_slice(b"tidb");
    }
    if context.isolation_read_engines.tikv {
        key.extend_from_slice(b"tikv");
    }
    if context.isolation_read_engines.tiflash && context.statement_read_only {
        key.extend_from_slice(b"tiflash");
    }
    encode_i64(&mut key, context.select_limit as i64);
    key.extend_from_slice(binding.as_bytes());
    key.extend_from_slice(context.connection_charset.as_bytes());
    key.extend_from_slice(context.connection_collation.as_bytes());
    key.push(bool2Byte(context.restricted_sql));
    key.push(bool2Byte(context.restricted_read_only));
    key.push(bool2Byte(context.super_read_only));
    encode_i64(&mut key, context.expression_pushdown_blacklist_version);
    key.push(bool2Byte(statement.has_subquery));
    key.push(bool2Byte(context.foreign_key_checks));

    // 仅编码参数化的 limit/offset；超过 MaxCacheableLimitCount 则不可缓存。
    if !statement.limits.is_empty() {
        key.push(b'|');
        for limit in &statement.limits {
            for (parameterized, value) in [
                (limit.count_is_parameter, limit.count),
                (limit.offset_is_parameter, limit.offset),
            ] {
                if !parameterized {
                    continue;
                }
                if value > MaxCacheableLimitCount as u64 {
                    return Ok(PlanCacheKeyResult {
                        key: None,
                        binding,
                        cacheable: false,
                        reason: "limit count is too large".to_owned(),
                    });
                }
                encode_u64(&mut key, value);
            }
        }
        key.push(b'|');
    }

    // 统计信息变更可失效缓存：将相关表 stats_version 之和编入键。
    if context.invalidate_on_fresh_stats && (binding.is_empty() || !context.skip_stats_when_binding)
    {
        encode_u64(
            &mut key,
            statement
                .tables
                .iter()
                .map(|table| table.stats_version)
                .sum(),
        );
    }
    let mut dirty_table_ids = context.dirty_table_ids.clone();
    dirty_table_ids.sort_unstable();
    dirty_table_ids.dedup();
    for table_id in dirty_table_ids {
        encode_i64(&mut key, table_id);
    }
    key.push(b'|');
    key.push(bool2Byte(context.in_transaction));
    key.push(bool2Byte(context.autocommit));
    key.push(bool2Byte(context.pessimistic_autocommit));
    key.push(bool2Byte(context.share_lock_enabled_by_noop));
    key.push(bool2Byte(context.shared_lock_promotion));

    Ok(PlanCacheKeyResult {
        key: Some(PlanCacheKey(key)),
        binding,
        cacheable: true,
        reason: String::new(),
    })
}

/// PREPARE 结果条目：语句、结果列与参数个数。
pub struct PrepareStmtCacheEntry<ColumnInfo = (), Executor = (), FastPlan = ()> {
    pub Stmt: Arc<PlanCacheStmt<ColumnInfo, Executor, FastPlan>>,
    pub Fields: Vec<resolve_dependency::ResultField>,
    pub ParamCount: usize,
}

/// 未知计划内存占用时的占位估计（50KiB）。
const UNKNOWN_PLAN_MEMORY_USAGE: i64 = 50 * 1024;

/// 缓存中的计划值：计划本体、参数类型、hints 与运行时统计。
pub struct PlanCacheValue {
    pub SQLDigest: String,
    pub SQLText: String,
    pub StmtType: String,
    pub ParseUser: String,
    pub Binding: String,
    pub OptimizerEnvHash: String,
    pub ParseValues: String,
    pub PlanDigest: String,
    pub BinaryPlan: String,
    pub LoadTime: SystemTime,
    /// 与会话上下文解耦、可跨线程共享的拥有型计划快照。
    pub Plan: Option<physicalop_dependency::CachedPlan>,
    pub OutputColumns: base_dependency::types::NameSlice,
    pub ParamTypes: Vec<types_dependency::metadata::FieldType>,
    pub StmtHints: hint_dependency::StmtHints,
    plan_memory_usage: i64,
    memory: AtomicI64,
    executions: AtomicI64,
    processed_keys: AtomicI64,
    total_keys: AtomicI64,
    sum_latency: AtomicI64,
    last_used_time_in_unix: AtomicI64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 构建 PlanCacheValue 时的附加展示/内存信息。
pub struct PlanCacheValueBuildInfo {
    pub parse_user: String,
    pub parse_values: String,
    pub binary_plan: String,
    pub plan_memory_usage: i64,
}

/// 将字节序列编码为小写十六进制字符串。
fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

impl PlanCacheValue {
    /// 原子累加执行次数、处理键数、总键数与延迟，并更新最近使用时间。
    pub fn UpdateRuntimeInfo(&self, processed_keys: i64, total_keys: i64, latency: i64) {
        self.executions.fetch_add(1, Ordering::Relaxed);
        self.processed_keys
            .fetch_add(processed_keys, Ordering::Relaxed);
        self.total_keys.fetch_add(total_keys, Ordering::Relaxed);
        self.sum_latency.fetch_add(latency, Ordering::Relaxed);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        self.last_used_time_in_unix.store(now, Ordering::Release);
    }

    /// 读取运行时统计快照与最近使用时间。
    pub fn RuntimeInfo(&self) -> (i64, i64, i64, i64, SystemTime) {
        let last_used = self.last_used_time_in_unix.load(Ordering::Acquire);
        (
            self.executions.load(Ordering::Relaxed),
            self.processed_keys.load(Ordering::Relaxed),
            self.total_keys.load(Ordering::Relaxed),
            self.sum_latency.load(Ordering::Relaxed),
            UNIX_EPOCH + Duration::from_secs(last_used.max(0) as u64),
        )
    }

    /// 估算并缓存本条目内存占用（计划 + 字符串 + 列/类型）。
    pub fn MemoryUsage(&self) -> i64 {
        let cached = self.memory.load(Ordering::Acquire);
        if cached > 0 {
            return cached;
        }
        let mut memory = if self.plan_memory_usage > 0 {
            self.plan_memory_usage
        } else {
            UNKNOWN_PLAN_MEMORY_USAGE
        };
        memory += std::mem::size_of::<Self>() as i64;
        memory += [
            &self.SQLDigest,
            &self.SQLText,
            &self.StmtType,
            &self.ParseUser,
            &self.Binding,
            &self.OptimizerEnvHash,
            &self.ParseValues,
            &self.PlanDigest,
            &self.BinaryPlan,
        ]
        .into_iter()
        .map(|value| value.capacity() as i64)
        .sum::<i64>();
        memory += self
            .OutputColumns
            .0
            .iter()
            .flatten()
            .map(|name| name.MemoryUsage())
            .sum::<i64>();
        memory += self
            .ParamTypes
            .iter()
            .map(|field_type| field_type.MemoryUsage())
            .sum::<i64>();
        self.memory.store(memory, Ordering::Release);
        memory
    }
}

/// 由语句与计划构建 PlanCacheValue，并用键+参数类型哈希优化器环境。
pub fn NewPlanCacheValue<ColumnInfo, Executor, FastPlan>(
    statement: &PlanCacheStmt<ColumnInfo, Executor, FastPlan>,
    cache_key: &PlanCacheKey,
    binding: impl Into<String>,
    plan: physicalop_dependency::CachedPlan,
    output_columns: base_dependency::types::NameSlice,
    parameter_types: &[types_dependency::metadata::FieldType],
    statement_hints: &hint_dependency::StmtHints,
    build_info: PlanCacheValueBuildInfo,
) -> PlanCacheValue {
    let binding = binding.into();
    let mut hasher = Sha256::new();
    hasher.update(cache_key.AsBytes());
    for parameter_type in parameter_types {
        hasher.update(parameter_type.String().as_bytes());
    }
    let optimizer_environment_hash = encode_hex(&hasher.finalize());
    let value = PlanCacheValue {
        SQLDigest: statement
            .SQLDigest
            .as_ref()
            .map_or_else(String::new, |digest| encode_hex(&digest.0)),
        SQLText: statement.StmtText.clone(),
        StmtType: statement.PreparedAst.stmt_type.clone(),
        ParseUser: build_info.parse_user,
        Binding: binding,
        OptimizerEnvHash: optimizer_environment_hash,
        ParseValues: build_info.parse_values,
        PlanDigest: statement
            .PlanDigest
            .as_ref()
            .map_or_else(String::new, |digest| encode_hex(&digest.0)),
        BinaryPlan: build_info.binary_plan,
        LoadTime: SystemTime::now(),
        Plan: Some(plan),
        OutputColumns: output_columns,
        ParamTypes: parameter_types.to_vec(),
        StmtHints: statement_hints.clone(),
        plan_memory_usage: build_info.plan_memory_usage,
        memory: AtomicI64::new(0),
        executions: AtomicI64::new(0),
        processed_keys: AtomicI64::new(0),
        total_keys: AtomicI64::new(0),
        sum_latency: AtomicI64::new(0),
        last_used_time_in_unix: AtomicI64::new(0),
    };
    value.MemoryUsage();
    value
}

#[cfg(test)]
/// 测试用：构造带指定 plan_memory_usage 的 PlanCacheValue。
pub(crate) fn NewPlanCacheValueForTest(plan_memory_usage: i64) -> PlanCacheValue {
    PlanCacheValue {
        SQLDigest: "digest".to_owned(),
        SQLText: "select 1".to_owned(),
        StmtType: "Select".to_owned(),
        ParseUser: "tester".to_owned(),
        Binding: String::new(),
        OptimizerEnvHash: "environment".to_owned(),
        ParseValues: "1".to_owned(),
        PlanDigest: "plan".to_owned(),
        BinaryPlan: "binary".to_owned(),
        LoadTime: SystemTime::now(),
        Plan: None,
        OutputColumns: base_dependency::types::NameSlice(Vec::new()),
        ParamTypes: Vec::new(),
        StmtHints: hint_dependency::StmtHints::default(),
        plan_memory_usage,
        memory: AtomicI64::new(0),
        executions: AtomicI64::new(0),
        processed_keys: AtomicI64::new(0),
        total_keys: AtomicI64::new(0),
        sum_latency: AtomicI64::new(0),
        last_used_time_in_unix: AtomicI64::new(0),
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// EXECUTE 语句：可按 ID 或名称定位 Prepared 语句。
pub struct ExecutePreparedStatement {
    pub prepared_statement_id: Option<u32>,
    pub name: String,
}

/// Prepared 语句存储：按 ID/名称查找。
pub trait PreparedStatementStore<ColumnInfo = (), Executor = (), FastPlan = ()> {
    fn GetByID(
        &self,
        statement_id: u32,
    ) -> Option<Arc<PlanCacheStmt<ColumnInfo, Executor, FastPlan>>>;
    fn GetIDByName(&self, name: &str) -> Option<u32>;
}

/// 解析 EXECUTE 目标语句；首次按名命中后缓存 prepared_statement_id。
pub fn GetPreparedStmt<ColumnInfo, Executor, FastPlan>(
    execute: &mut ExecutePreparedStatement,
    store: &dyn PreparedStatementStore<ColumnInfo, Executor, FastPlan>,
) -> Result<Arc<PlanCacheStmt<ColumnInfo, Executor, FastPlan>>, PlanCacheError> {
    if let Some(statement_id) = execute.prepared_statement_id {
        return store
            .GetByID(statement_id)
            .ok_or_else(|| PlanCacheError::new("prepared statement not found"));
    }
    if execute.name.is_empty() {
        return Err(PlanCacheError::new("prepared statement not found"));
    }
    let statement_id = store
        .GetIDByName(&execute.name)
        .ok_or_else(|| PlanCacheError::new("prepared statement not found"))?;
    let statement = store
        .GetByID(statement_id)
        .ok_or_else(|| PlanCacheError::new("prepared statement not found"))?;
    execute.prepared_statement_id = Some(statement_id);
    Ok(statement)
}

/// 检查缓存参数类型与当前参数类型是否兼容（字符集、符号、DECIMAL 精度）。
pub fn CheckTypesCompatibility4PC(
    expected: &[types_dependency::metadata::FieldType],
    actual: &[types_dependency::metadata::FieldType],
) -> bool {
    if expected.is_empty() || actual.is_empty() {
        return true;
    }
    if expected.len() != actual.len() {
        return false;
    }
    // 逐参数比较类型/字符集/排序规则；INT 还比无符号；DECIMAL 要求缓存侧精度足够宽。
    expected.iter().zip(actual).all(|(cached, current)| {
        use types_dependency::metadata::{ast_types, mysql};

        let cached_type = cached.GetType();
        let current_type = current.GetType();
        let compatible_type = cached_type == current_type
            || matches!(
                (cached_type, current_type),
                (mysql::TypeVarchar, mysql::TypeVarString)
                    | (mysql::TypeVarString, mysql::TypeVarchar)
            );
        if !compatible_type
            || cached.GetCharset() != current.GetCharset()
            || cached.GetCollate() != current.GetCollate()
            || (cached.EvalType() == ast_types::ETInt
                && mysql::HasUnsignedFlag(cached.GetFlag())
                    != mysql::HasUnsignedFlag(current.GetFlag()))
        {
            return false;
        }
        cached_type != mysql::TypeNewDecimal
            || (cached.GetFlen() >= current.GetFlen()
                && cached.GetDecimal() >= current.GetDecimal())
    })
}

/// 自由函数包装：转发到 PlanCacheStmt::CollectPlanCacheStmtInfo。
pub fn CollectPlanCacheStmtInfo<ColumnInfo, Executor, FastPlan>(
    statement: &mut PlanCacheStmt<ColumnInfo, Executor, FastPlan>,
    limits: Vec<PlanCacheLimit>,
    has_subquery: bool,
    tables: Vec<PlanCacheTable>,
) {
    statement.CollectPlanCacheStmtInfo(limits, has_subquery, tables);
}

/// 判断访问路径是否为可安全缓存的 Point Get；fix_44830 开启时扩展场景 2–4。
pub fn IsSafePointGetPath4PlanCache(
    fix_44830_enabled: bool,
    path: &planner_util_dependency::AccessPath,
) -> bool {
    IsSafePointGetPath4PlanCacheScenario1(path)
        || (fix_44830_enabled
            && (IsSafePointGetPath4PlanCacheScenario2(path)
                || IsSafePointGetPath4PlanCacheScenario3(path)
                || IsSafePointGetPath4PlanCacheScenario4(path)))
}

/// 场景1：全部等值条件，且 range 宽度等于条件数。
pub fn IsSafePointGetPath4PlanCacheScenario1(path: &planner_util_dependency::AccessPath) -> bool {
    let Some(range) = path.Ranges.first() else {
        return false;
    };
    range.Width() == path.AccessConds.len()
        && path.AccessConds.iter().all(|condition| {
            condition
                .as_any()
                .downcast_ref::<expression_dependency::ScalarFunction>()
                .is_some_and(|function| function.FuncName.L == ast::EQ)
        })
}

/// 场景2：单一 IN 条件，range 条数等于 IN 值个数。
pub fn IsSafePointGetPath4PlanCacheScenario2(path: &planner_util_dependency::AccessPath) -> bool {
    if path.Ranges.is_empty() || path.AccessConds.len() != 1 {
        return false;
    }
    path.AccessConds[0]
        .as_any()
        .downcast_ref::<expression_dependency::ScalarFunction>()
        .is_some_and(|function| {
            function.FuncName.L == ast::In
                && path.Ranges.len() == function.GetArgs().len().saturating_sub(1)
        })
}

/// 场景3：单一 OR 析取，每个分支为等值或等值合取。
pub fn IsSafePointGetPath4PlanCacheScenario3(path: &planner_util_dependency::AccessPath) -> bool {
    if path.Ranges.is_empty() || path.AccessConds.len() != 1 {
        return false;
    }
    let Some(or_function) = path.AccessConds[0]
        .as_any()
        .downcast_ref::<expression_dependency::ScalarFunction>()
    else {
        return false;
    };
    if or_function.FuncName.L != ast::functions::LogicOr {
        return false;
    }
    let disjunctions = expression_dependency::FlattenDNFConditions(or_function);
    if path.Ranges.len() != disjunctions.len() {
        return false;
    }
    let range_width = path.Ranges[0].Width();
    disjunctions.iter().all(|expression| {
        let Some(function) = expression
            .as_any()
            .downcast_ref::<expression_dependency::ScalarFunction>()
        else {
            return false;
        };
        if function.FuncName.L == ast::EQ {
            return true;
        }
        if function.FuncName.L != ast::LogicAnd {
            return false;
        }
        let conjunctions = expression_dependency::FlattenCNFConditions(function);
        conjunctions.len() == range_width
            && conjunctions.iter().all(|condition| {
                condition
                    .as_any()
                    .downcast_ref::<expression_dependency::ScalarFunction>()
                    .is_some_and(|function| function.FuncName.L == ast::EQ)
            })
    })
}

/// 场景4：等值与至多一个 IN 组合，range 数等于 IN 值个数。
pub fn IsSafePointGetPath4PlanCacheScenario4(path: &planner_util_dependency::AccessPath) -> bool {
    let Some(range) = path.Ranges.first() else {
        return false;
    };
    if path.AccessConds.len() < 2 || range.Width() != path.AccessConds.len() {
        return false;
    }
    let mut in_argument_count = None;
    for condition in &path.AccessConds {
        let Some(function) = condition
            .as_any()
            .downcast_ref::<expression_dependency::ScalarFunction>()
        else {
            return false;
        };
        match function.FuncName.L.as_str() {
            ast::EQ => {}
            ast::In if in_argument_count.is_none() => {
                in_argument_count = Some(function.GetArgs().len().saturating_sub(1));
            }
            _ => return false,
        }
    }
    in_argument_count.is_some_and(|argument_count| path.Ranges.len() == argument_count)
}

/// 从参数表达式解析 FieldType：常量取类型，否则查用户变量类型。
pub fn ParseParamTypes(
    context: &dyn base_dependency::PlanContext,
    parameters: &[expression_dependency::ExprBox],
) -> Vec<types_dependency::metadata::FieldType> {
    use expression_dependency::StringerWithCtx as _;
    use types_dependency::metadata::mysql;

    let evaluation_context = context.GetExprCtx().GetEvalCtx();
    parameters
        .iter()
        .map(|parameter| {
            if let Some(constant) = parameter
                .as_any()
                .downcast_ref::<expression_dependency::Constant>()
            {
                return constant.GetType(evaluation_context).unwrap_or_else(|| {
                    let mut field_type = types_dependency::metadata::FieldType::default();
                    field_type.SetType(mysql::TypeNull);
                    field_type
                });
            }
            let variable_name = parameter
                .as_any()
                .downcast_ref::<expression_dependency::ScalarFunction>()
                .and_then(|function| function.GetArgs().first())
                .map(|argument| {
                    argument.StringWithCtx(
                        Some(evaluation_context),
                        expression_dependency::errors::RedactLogDisable,
                    )
                });
            context
                .GetSessionVars()
                .GetUserVarType(variable_name.as_deref().unwrap_or_default())
                .map(|field_type| *field_type)
                .unwrap_or_else(|| {
                    let mut field_type = types_dependency::metadata::FieldType::default();
                    field_type.SetType(mysql::TypeNull);
                    field_type
                })
        })
        .collect()
}
