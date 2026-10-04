// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 顶层规划器编排入口，对应 Go `pkg/planner/optimize.go`。
//
// 职责边界与 Go 一致：AST 计划构建、预处理 EXECUTE 分发、可选非预处理计划缓存
// 查找、默认优化轮次及各类替代逻辑计划轮次。计划构建与预处理缓存执行通过
// 已安装服务注入（实现位于 planner/core 与 executor）；缺失服务视为硬错误。
//
// Top-level planner orchestration migrated from `pkg/planner/optimize.go`.
//
// This module owns the same boundary as Go: AST plan construction, prepared
// EXECUTE dispatch, optional non-prepared cache lookup, the default optimizer
// round, and every alternative logical-plan round. Plan construction and
// prepared-cache execution are installed services because those implementations
// live in `planner/core` and `executor`; missing services are hard errors.

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::Ordering;
use std::time::Instant;

use astersql_expression as expression;
use astersql_infoschema as infoschema;
use astersql_parser_ast as ast;
use astersql_planner_core as core;
use astersql_planner_core_base as base;
use astersql_planner_core_operator_logicalop as logicalop;
use astersql_planner_core_resolve as resolve;
use astersql_planner_core_rule as rule;
use astersql_sessionctx_stmtctx::{self as stmtctx, LogicalPlanBuildState, TableEntry};
use astersql_sessionctx_vardef as vardef;
use astersql_sessionctx_variable::session::RewritePhaseInfo;
use astersql_sessionctx_variable::session::SessionVars;
use astersql_types as types;
use astersql_util_dbterror_plannererrors as plannererrors;
use astersql_util_hint as hint;

/// hint_only 策略下缺少 use_plan_cache hint 时的旁路原因文案。
pub const nonPreparedPlanCacheHintOnlyNoHintReason: &str =
    "plan cache strategy is hint_only and use_plan_cache hint is absent";

/// 优化成功输出：物理/逻辑计划与输出列名切片。
pub type OptimizeOutput = (Box<dyn base::Plan>, base::types::NameSlice);
/// 优化结果：成功输出或表达式错误。
pub type OptimizeResult = Result<OptimizeOutput, expression::Error>;
/// 供 core 注册的 AST 优化函数指针类型。
pub type OptimizeAstNodeFn = core::OptimizeAstNodeFn;

/// 由 executor/session 注入的预处理计划缓存执行优化函数。
/// Prepared-plan-cache implementation supplied by executor/session wiring.
pub type ExecuteOptimizeFn = OptimizeAstNodeFn;

/// 参数化后的非预处理缓存语句：参数化 SQL 与 Datum 参数值。
pub struct ParameterizedPlanCacheStatement {
    pub sql: String,
    pub values: Vec<types::field::Datum>,
}

/// 非预处理计划缓存中的语句对象（可向下转型）。
pub trait NonPreparedCachedStatement: Any {
    fn as_any(&self) -> &dyn Any;
}
/// 非预处理缓存语句的共享引用。
pub type NonPreparedCachedStatementRef = Arc<dyn NonPreparedCachedStatement>;

/// 语句是否可缓存及不可缓存原因。
pub struct NonPreparedCacheability {
    pub cacheable: bool,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 语句 hint 产生的副作用：资源组与 SET_VAR。
pub struct StatementHintEffects {
    pub resource_group: Option<String>,
    pub set_vars: Vec<(String, String)>,
}

/// 五组 Go 逻辑构建状态的不透明不可变快照。
/// Opaque, immutable copy of the five Go logical-build state groups:
/// StatementContext::LogicalPlanBuildState, PlannerSelectBlockAsName,
/// scalar-subquery map, extended-column hash/unique-ID map, and rewrite phase.
pub trait LogicalPlanBuildStateSnapshot: Any {
    fn as_any(&self) -> &dyn Any;
}
impl<T: Any> LogicalPlanBuildStateSnapshot for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
}
pub type LogicalPlanBuildStateSnapshotRef = Arc<dyn LogicalPlanBuildStateSnapshot>;

/// 匹配到的 SQL Binding（计划基线）信息。
pub trait MatchedPlanBinding: Send + Sync {
    fn bind_sql(&self) -> &str;
    fn is_enabled(&self) -> bool;
    fn statement_hints(&self) -> Vec<ast::TableOptimizerHint>;
    fn contains_read_from_storage_hint(&self) -> bool;
}
/// 匹配 Binding 的共享引用。
pub type MatchedPlanBindingRef = Arc<dyn MatchedPlanBinding>;

/// 查询 hint 状态快照（绑定前后可恢复）。
pub trait QueryHintStateSnapshot: Any {}
impl<T: Any> QueryHintStateSnapshot for T {}
/// 查询 hint 状态快照引用。
pub type QueryHintStateSnapshotRef = Arc<dyn QueryHintStateSnapshot>;

/// 逻辑计划构建会话状态的保存/恢复/信号重置服务。
pub trait LogicalPlanSessionStateService {
    fn save_logical_plan_build_state(
        &self,
        sctx: &base::ContextRef,
    ) -> LogicalPlanBuildStateSnapshotRef;
    fn restore_logical_plan_build_state(
        &self,
        sctx: &base::ContextRef,
        state: &dyn LogicalPlanBuildStateSnapshot,
    );
    fn reset_alternative_logical_plan_signals(&self, sctx: &base::ContextRef);
}

/// 跨包规划运行时能力：由 session/executor/bindinfo 注入。
/// Cross-package planner operations supplied by session/executor/bindinfo.
pub trait OptimizeRuntimeService: Send + Sync {
    /// Implements Go's point-plan lookup: reuse PointPlanKey for multi-stmt
    /// execution when present, otherwise call core::TryFastPlan.
    fn try_fast_plan(
        &self,
        ctx: &dyn core::context::Context,
        sctx: &base::ContextRef,
        node: &resolve::NodeW,
    ) -> Result<Option<OptimizeOutput>, expression::Error>;
    fn is_statement(&self, node: &resolve::NodeW) -> bool;
    fn match_sql_binding(
        &self,
        sctx: &base::ContextRef,
        node: &resolve::NodeW,
    ) -> Result<Option<MatchedPlanBindingRef>, expression::Error>;
    fn try_add_extra_limit(
        &self,
        sctx: &base::ContextRef,
        node: &resolve::NodeW,
    ) -> Result<resolve::NodeW, expression::Error>;
    fn save_query_hint_state(&self, node: &resolve::NodeW) -> QueryHintStateSnapshotRef;
    fn bind_query_hints(
        &self,
        node: &resolve::NodeW,
        binding: &dyn MatchedPlanBinding,
    ) -> Result<(), expression::Error>;
    fn restore_query_hint_state(&self, node: &resolve::NodeW, state: &dyn QueryHintStateSnapshot);
    fn mark_binding_selected(
        &self,
        sctx: &base::ContextRef,
        bind_sql: &str,
        original_query_had_hints: bool,
    );
    fn advise_txn_warmup(&self, sctx: &base::ContextRef) -> Result<(), expression::Error>;
    fn is_select_statement(&self, node: &resolve::NodeW) -> bool;
    fn plan_contains_read_from_storage_hint(&self, plan: &dyn base::Plan) -> bool;
    fn install_statement_hints(&self, sctx: &base::ContextRef, hints: hint::StmtHints);
    fn statement_hint_effects(&self, sctx: &base::ContextRef) -> StatementHintEffects;
    fn set_skip_plan_cache(&self, sctx: &base::ContextRef, reason: &str);
    fn set_system_var_with_old_state(
        &self,
        sctx: &base::ContextRef,
        name: &str,
        value: &str,
    ) -> Result<String, expression::Error>;
    fn add_set_var_hint_restore(&self, sctx: &base::ContextRef, name: &str, old_value: &str);
    fn has_resource_group_admin_or_user(&self, sctx: &base::ContextRef) -> bool;
    fn set_statement_resource_group(&self, sctx: &base::ContextRef, resource_group: &str);
    fn set_valid_txn_resource_group(&self, sctx: &base::ContextRef, resource_group: &str);
    fn strict_mode_applies(
        &self,
        sctx: &base::ContextRef,
        node: &resolve::NodeW,
    ) -> Result<bool, expression::Error>;
    fn remove_tiflash_for_strict_mode(
        &self,
        sctx: &base::ContextRef,
    ) -> Result<bool, expression::Error>;
    fn restore_tiflash_after_strict_mode(&self, sctx: &base::ContextRef, was_present: bool);
    fn hypo_index_column_offset(
        &self,
        ctx: &dyn core::context::Context,
        info_schema: &dyn infoschema::InfoSchema,
        db: ast::CIStr,
        table: ast::CIStr,
        column: ast::CIStr,
    ) -> Result<i32, expression::Error>;
}

/// 会话服务 = 逻辑构建状态服务 + 运行时服务。
pub trait OptimizeSessionService:
    LogicalPlanSessionStateService + OptimizeRuntimeService + Send + Sync
{
}
impl<T> OptimizeSessionService for T where
    T: LogicalPlanSessionStateService + OptimizeRuntimeService + Send + Sync
{
}

/// 默认五组逻辑构建状态快照的具体载体。
struct DefaultLogicalPlanBuildState {
    statement_context: LogicalPlanBuildState,
    planner_select_block_names: Option<Arc<Vec<ast::HintTable>>>,
    scalar_subqueries: Vec<Rc<dyn Any>>,
    extended_column_unique_ids: HashMap<String, i64>,
    rewrite_phase_info: RewritePhaseInfo,
}

/// 生产接线用的会话服务实现：运行时回调经 `R` 委托，五组逻辑构建状态在此保存/恢复。
/// Concrete state owner used by production wiring. Runtime callbacks remain
/// mandatory through `R`; the five logical-build groups are handled here and
/// can be restored repeatedly without moving thread-affine scalar contexts.
pub struct DefaultOptimizeSessionService<R> {
    runtime: R,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 简要计划诊断数据：digest、hint 与行文本。
pub struct BriefPlanData {
    pub digest: String,
    pub hint: String,
    pub rows: Vec<Vec<String>>,
}

/// 规划诊断运行时：预处理、优化、代价、digest、brief plan。
pub trait PlannerDiagnosticRuntime: Send + Sync {
    fn preprocess(
        &self,
        sctx: &base::ContextRef,
        node: &resolve::NodeW,
    ) -> Result<(), expression::Error>;
    fn optimize_statement(&self, sctx: &base::ContextRef, node: &resolve::NodeW) -> OptimizeResult;
    fn is_physical_plan(&self, plan: &dyn base::Plan) -> bool;
    fn physical_plan_cost(&self, plan: &mut dyn base::Plan) -> Result<f64, expression::Error>;
    fn plan_digest(&self, plan: &dyn base::Plan) -> Result<String, expression::Error>;
    fn brief_plan(&self, plan: &dyn base::Plan) -> Result<BriefPlanData, expression::Error>;
    fn set_ignore_explain_id_suffix(&self, sctx: &base::ContextRef, ignore: bool);
}

/// 全局规划诊断运行时安装点。
static PLANNER_DIAGNOSTICS: OnceLock<Arc<dyn PlannerDiagnosticRuntime>> = OnceLock::new();

/// 安装规划诊断运行时，并注册 PlanID 回调；禁止重复安装。
pub fn InstallPlannerDiagnosticRuntime(
    runtime: Arc<dyn PlannerDiagnosticRuntime>,
) -> Result<(), expression::Error> {
    PLANNER_DIAGNOSTICS
        .set(runtime)
        .map_err(|_| planner_error("planner diagnostic runtime is already installed"))?;
    astersql_sessionctx_stmtctx::SetPlanIDFunc(Some(planIDFromCacheValue));
    Ok(())
}

/// 获取已安装的诊断运行时，未安装则报错。
fn plannerDiagnosticRuntime() -> Result<&'static dyn PlannerDiagnosticRuntime, expression::Error> {
    PLANNER_DIAGNOSTICS
        .get()
        .map(Arc::as_ref)
        .ok_or_else(|| planner_error("planner diagnostic runtime is not installed"))
}

impl<R> DefaultOptimizeSessionService<R> {
    pub fn new(runtime: R) -> Self {
        Self { runtime }
    }
}

impl<R> LogicalPlanSessionStateService for DefaultOptimizeSessionService<R> {
    fn save_logical_plan_build_state(
        &self,
        sctx: &base::ContextRef,
    ) -> LogicalPlanBuildStateSnapshotRef {
        let vars = sctx.GetSessionVars();
        Arc::new(DefaultLogicalPlanBuildState {
            statement_context: vars.StmtCtx.SaveLogicalPlanBuildState(),
            planner_select_block_names: vars.PlannerSelectBlockAsName.Load(),
            scalar_subqueries: vars.SnapshotScalarSubQueries(),
            extended_column_unique_ids: vars.SnapshotExtendedColumnUniqueIDs(),
            rewrite_phase_info: vars.SnapshotRewritePhaseInfo(),
        })
    }

    fn restore_logical_plan_build_state(
        &self,
        sctx: &base::ContextRef,
        state: &dyn LogicalPlanBuildStateSnapshot,
    ) {
        let state = state
            .as_any()
            .downcast_ref::<DefaultLogicalPlanBuildState>()
            .expect("logical-plan snapshot belongs to another session service");
        let vars = sctx.GetSessionVars();
        vars.StmtCtx
            .RestoreLogicalPlanBuildState(&state.statement_context);
        vars.PlannerSelectBlockAsName
            .Store(state.planner_select_block_names.as_deref().cloned());
        vars.RestoreScalarSubQueries(state.scalar_subqueries.clone());
        vars.RestoreExtendedColumnUniqueIDs(state.extended_column_unique_ids.clone());
        vars.RestoreRewritePhaseInfo(state.rewrite_phase_info.clone());
    }

    fn reset_alternative_logical_plan_signals(&self, sctx: &base::ContextRef) {
        sctx.GetSessionVars().StmtCtx.ResetAlternativeRoundSignals();
    }
}

impl<R: OptimizeRuntimeService> OptimizeRuntimeService for DefaultOptimizeSessionService<R> {
    fn try_fast_plan(
        &self,
        ctx: &dyn core::context::Context,
        sctx: &base::ContextRef,
        node: &resolve::NodeW,
    ) -> Result<Option<OptimizeOutput>, expression::Error> {
        self.runtime.try_fast_plan(ctx, sctx, node)
    }
    fn is_statement(&self, node: &resolve::NodeW) -> bool {
        self.runtime.is_statement(node)
    }
    fn match_sql_binding(
        &self,
        sctx: &base::ContextRef,
        node: &resolve::NodeW,
    ) -> Result<Option<MatchedPlanBindingRef>, expression::Error> {
        self.runtime.match_sql_binding(sctx, node)
    }
    fn try_add_extra_limit(
        &self,
        sctx: &base::ContextRef,
        node: &resolve::NodeW,
    ) -> Result<resolve::NodeW, expression::Error> {
        self.runtime.try_add_extra_limit(sctx, node)
    }
    fn save_query_hint_state(&self, node: &resolve::NodeW) -> QueryHintStateSnapshotRef {
        self.runtime.save_query_hint_state(node)
    }
    fn bind_query_hints(
        &self,
        node: &resolve::NodeW,
        binding: &dyn MatchedPlanBinding,
    ) -> Result<(), expression::Error> {
        self.runtime.bind_query_hints(node, binding)
    }
    fn restore_query_hint_state(&self, node: &resolve::NodeW, state: &dyn QueryHintStateSnapshot) {
        self.runtime.restore_query_hint_state(node, state)
    }
    fn mark_binding_selected(
        &self,
        sctx: &base::ContextRef,
        bind_sql: &str,
        original_query_had_hints: bool,
    ) {
        self.runtime
            .mark_binding_selected(sctx, bind_sql, original_query_had_hints)
    }
    fn advise_txn_warmup(&self, sctx: &base::ContextRef) -> Result<(), expression::Error> {
        self.runtime.advise_txn_warmup(sctx)
    }
    fn is_select_statement(&self, node: &resolve::NodeW) -> bool {
        self.runtime.is_select_statement(node)
    }
    fn plan_contains_read_from_storage_hint(&self, plan: &dyn base::Plan) -> bool {
        self.runtime.plan_contains_read_from_storage_hint(plan)
    }
    fn install_statement_hints(&self, sctx: &base::ContextRef, hints: hint::StmtHints) {
        self.runtime.install_statement_hints(sctx, hints)
    }
    fn statement_hint_effects(&self, sctx: &base::ContextRef) -> StatementHintEffects {
        self.runtime.statement_hint_effects(sctx)
    }
    fn set_skip_plan_cache(&self, sctx: &base::ContextRef, reason: &str) {
        self.runtime.set_skip_plan_cache(sctx, reason)
    }
    fn set_system_var_with_old_state(
        &self,
        sctx: &base::ContextRef,
        name: &str,
        value: &str,
    ) -> Result<String, expression::Error> {
        self.runtime
            .set_system_var_with_old_state(sctx, name, value)
    }
    fn add_set_var_hint_restore(&self, sctx: &base::ContextRef, name: &str, old_value: &str) {
        self.runtime.add_set_var_hint_restore(sctx, name, old_value)
    }
    fn has_resource_group_admin_or_user(&self, sctx: &base::ContextRef) -> bool {
        self.runtime.has_resource_group_admin_or_user(sctx)
    }
    fn set_statement_resource_group(&self, sctx: &base::ContextRef, resource_group: &str) {
        self.runtime
            .set_statement_resource_group(sctx, resource_group)
    }
    fn set_valid_txn_resource_group(&self, sctx: &base::ContextRef, resource_group: &str) {
        self.runtime
            .set_valid_txn_resource_group(sctx, resource_group)
    }
    fn strict_mode_applies(
        &self,
        sctx: &base::ContextRef,
        node: &resolve::NodeW,
    ) -> Result<bool, expression::Error> {
        self.runtime.strict_mode_applies(sctx, node)
    }
    fn remove_tiflash_for_strict_mode(
        &self,
        sctx: &base::ContextRef,
    ) -> Result<bool, expression::Error> {
        self.runtime.remove_tiflash_for_strict_mode(sctx)
    }
    fn restore_tiflash_after_strict_mode(&self, sctx: &base::ContextRef, was_present: bool) {
        self.runtime
            .restore_tiflash_after_strict_mode(sctx, was_present)
    }
    fn hypo_index_column_offset(
        &self,
        ctx: &dyn core::context::Context,
        info_schema: &dyn infoschema::InfoSchema,
        db: ast::CIStr,
        table: ast::CIStr,
        column: ast::CIStr,
    ) -> Result<i32, expression::Error> {
        self.runtime
            .hypo_index_column_offset(ctx, info_schema, db, table, column)
    }
}

/// 非预处理缓存完整算法的强类型边界：规划器掌控顺序与资格，服务掌控解析/存储与取计划。
/// Strongly typed boundary for the complete non-prepared cache algorithm.
/// Planner owns ordering and qualification; the service owns parser/cache
/// storage and the plan-cache implementation that already belong to core.
pub trait NonPreparedPlanCacheService: Send + Sync {
    fn is_statement(&self, node: &dyn ast::Node) -> bool;
    fn binding_contains_use_plan_cache_hint(
        &self,
        sctx: &base::ContextRef,
        node: &resolve::NodeW,
    ) -> bool;
    fn cacheable(
        &self,
        sctx: &base::ContextRef,
        node: &resolve::NodeW,
        info_schema: &dyn infoschema::InfoSchema,
    ) -> Result<NonPreparedCacheability, expression::Error>;
    fn parameterize(
        &self,
        node: &resolve::NodeW,
    ) -> Result<ParameterizedPlanCacheStatement, expression::Error>;
    fn lookup_cached_statement(
        &self,
        sctx: &base::ContextRef,
        parameterized_sql: &str,
    ) -> Result<Option<NonPreparedCachedStatementRef>, expression::Error>;
    fn parse_parameterized_ast(
        &self,
        sctx: &base::ContextRef,
        parameterized_sql: &str,
    ) -> Result<ast::NodeRef, expression::Error>;
    fn set_parameter_values(
        &self,
        sctx: &base::ContextRef,
        values: &[types::field::Datum],
    ) -> Result<(), expression::Error>;
    fn generate_cached_statement(
        &self,
        ctx: &dyn core::context::Context,
        sctx: &base::ContextRef,
        parameterized_sql: &str,
        parameterized_ast: ast::NodeRef,
        info_schema: Arc<dyn infoschema::InfoSchema>,
    ) -> Result<NonPreparedCachedStatementRef, expression::Error>;
    fn store_cached_statement(
        &self,
        sctx: &base::ContextRef,
        parameterized_sql: &str,
        statement: NonPreparedCachedStatementRef,
    ) -> Result<(), expression::Error>;
    fn get_plan(
        &self,
        ctx: &dyn core::context::Context,
        sctx: &base::ContextRef,
        info_schema: Arc<dyn infoschema::InfoSchema>,
        statement: NonPreparedCachedStatementRef,
        values: &[types::field::Datum],
    ) -> OptimizeResult;
}

/// 结果集构建、EXECUTE 优化、非预处理缓存与会话服务的全局安装点。
static RESULT_SET_BUILDER: OnceLock<core::ResultSetBuildFn> = OnceLock::new();
static EXECUTE_OPTIMIZER: OnceLock<ExecuteOptimizeFn> = OnceLock::new();
static NON_PREPARED_PLAN_CACHE: OnceLock<Arc<dyn NonPreparedPlanCacheService>> = OnceLock::new();
static OPTIMIZE_SESSION_SERVICE: OnceLock<Arc<dyn OptimizeSessionService>> = OnceLock::new();
/// 只读模式下特权旁路检查回调类型。
pub type ReadOnlyPrivilegeBypassFn = fn(&base::ContextRef) -> bool;
/// 只读模式下 COMMIT 事务检查回调类型。
pub type ReadOnlyCommitCheckFn = fn(&base::ContextRef) -> Result<bool, expression::Error>;
static READ_ONLY_PRIVILEGE_BYPASS: OnceLock<ReadOnlyPrivilegeBypassFn> = OnceLock::new();
static READ_ONLY_COMMIT_CHECK: OnceLock<ReadOnlyCommitCheckFn> = OnceLock::new();

/// 构造规划器侧表达式错误。
fn planner_error(message: impl Into<String>) -> expression::Error {
    expression::errors::New(message)
}

/// 将任意错误包装为 StmtCtx 可追加的 SharedError。
fn statement_warning(
    error: impl std::error::Error + Send + Sync + 'static,
) -> stmtctx::errors::SharedError {
    stmtctx::errors::SharedError::new(error)
}

/// 安装全部跨包服务，并向 planner/core 发布两个 AST 优化入口；禁止重复安装以免静默切换实现。
/// Installs all cross-package services and publishes this module's two AST
/// optimizer entry points to planner/core. Reinstalling is rejected so a
/// running server cannot silently switch planner implementations.
pub fn InstallOptimizeCallbacks(
    result_set_builder: core::ResultSetBuildFn,
    execute_optimizer: ExecuteOptimizeFn,
    cache_service: Arc<dyn NonPreparedPlanCacheService>,
    session_service: Arc<dyn OptimizeSessionService>,
) -> Result<(), expression::Error> {
    RESULT_SET_BUILDER
        .set(result_set_builder)
        .map_err(|_| planner_error("planner result-set builder is already installed"))?;
    EXECUTE_OPTIMIZER
        .set(execute_optimizer)
        .map_err(|_| planner_error("prepared EXECUTE optimizer is already installed"))?;
    NON_PREPARED_PLAN_CACHE
        .set(cache_service)
        .map_err(|_| planner_error("non-prepared plan-cache service is already installed"))?;
    OPTIMIZE_SESSION_SERVICE
        .set(session_service)
        .map_err(|_| planner_error("planner session service is already installed"))?;
    core::InstallOptimizeAstNode(OptimizeAstNode, OptimizeAstNodeNoCache)
        .map_err(|_| planner_error("planner/core AST optimizer callbacks are already installed"))
}

/// 使用 DefaultOptimizeSessionService 包装运行时后安装全部回调。
pub fn InstallDefaultOptimizeCallbacks<R>(
    result_set_builder: core::ResultSetBuildFn,
    execute_optimizer: ExecuteOptimizeFn,
    cache_service: Arc<dyn NonPreparedPlanCacheService>,
    runtime_service: R,
) -> Result<(), expression::Error>
where
    R: OptimizeRuntimeService + 'static,
{
    InstallOptimizeCallbacks(
        result_set_builder,
        execute_optimizer,
        cache_service,
        Arc::new(DefaultOptimizeSessionService::new(runtime_service)),
    )
}

/// 获取已安装的会话服务。
fn optimizeSessionService() -> Result<&'static dyn OptimizeSessionService, expression::Error> {
    OPTIMIZE_SESSION_SERVICE
        .get()
        .map(Arc::as_ref)
        .ok_or_else(|| planner_error("planner session service is not installed"))
}

/// 安装只读准入相关的特权旁路与 COMMIT 检查回调。
pub fn InstallReadOnlyAdmissionCallbacks(
    privilege_bypass: ReadOnlyPrivilegeBypassFn,
    commit_check: ReadOnlyCommitCheckFn,
) -> Result<(), expression::Error> {
    READ_ONLY_PRIVILEGE_BYPASS
        .set(privilege_bypass)
        .map_err(|_| planner_error("read-only privilege callback is already installed"))?;
    READ_ONLY_COMMIT_CHECK
        .set(commit_check)
        .map_err(|_| planner_error("read-only commit callback is already installed"))
}

/// 判断已解析节点是否为 ExecuteStmt。
fn is_execute(node: &resolve::NodeW) -> bool {
    node.node
        .with_node(|node| node.as_any().is::<ast::ExecuteStmt>())
        .unwrap_or(false)
}

/// Go `Optimize`：EXECUTE 走预处理计划缓存，其余走非预处理缓存/默认优化管线。
/// Go `Optimize`: route EXECUTE through the prepared plan cache and all other
/// statements through the non-prepared-cache/default optimization pipeline.
pub fn Optimize(
    ctx: &dyn core::context::Context,
    sctx: &base::ContextRef,
    node: &resolve::NodeW,
    info_schema: Arc<dyn infoschema::InfoSchema>,
) -> OptimizeResult {
    let session_service = optimizeSessionService()?;
    let vars = sctx.GetSessionVars();
    if !vars.InRestrictedSQL
        && (vardef::RestrictedReadOnly.Load() || vardef::VarTiDBSuperReadOnly.Load())
        && !allowInReadOnlyMode(sctx, node)?
    {
        return Err(plannererrors::ErrSQLInReadOnlyMode
            .GenWithStackByArgs(&[])
            .into());
    }
    // EXECUTE 走预处理缓存；其余语句走非预处理缓存/默认管线。
    let result = if is_execute(node) {
        OptimizeExecStmt(ctx, sctx, node, info_schema)
    } else {
        optimizeCache(ctx, sctx, node, info_schema)
    };
    if result.is_ok() {
        applySuccessfulOptimizeEffects(session_service, sctx);
    }
    result
}

/// 供 core 消费的公开回调，函数指针形态与 Go 一致。
/// Public callback with the exact function-pointer shape consumed by core.
pub fn OptimizeAstNode(
    ctx: &dyn core::context::Context,
    sctx: &base::ContextRef,
    node: &resolve::NodeW,
    info_schema: Arc<dyn infoschema::InfoSchema>,
) -> OptimizeResult {
    // Go wires this callback to optimizeCache directly.  The public Optimize
    // entry point owns read-only admission and post-success hint effects;
    // applying those here would make core's callback observe them twice.
    optimizeCache(ctx, sctx, node, info_schema)
}

/// 强制绕过计划缓存的公开回调。
/// Public no-cache callback used by callers that must bypass plan cache.
pub fn OptimizeAstNodeNoCache(
    ctx: &dyn core::context::Context,
    sctx: &base::ContextRef,
    node: &resolve::NodeW,
    info_schema: Arc<dyn infoschema::InfoSchema>,
) -> OptimizeResult {
    optimizeNoCache(ctx, sctx, node, info_schema)
}

/// Go `OptimizeExecStmt`：由预处理计划缓存所有者提供实现；此处校验 AST 类型以免误判为缓存未命中。
/// Go `OptimizeExecStmt`: the prepared-plan-cache owner supplies the real
/// implementation. The AST type is checked here so invalid dispatch cannot be
/// mistaken for a cache miss.
pub fn OptimizeExecStmt(
    ctx: &dyn core::context::Context,
    sctx: &base::ContextRef,
    exec_ast: &resolve::NodeW,
    info_schema: Arc<dyn infoschema::InfoSchema>,
) -> OptimizeResult {
    if !is_execute(exec_ast) {
        return Err(planner_error(
            "invalid plan type for OptimizeExecStmt: expected ExecuteStmt",
        ));
    }
    let optimize = EXECUTE_OPTIMIZER
        .get()
        .ok_or_else(|| planner_error("prepared EXECUTE optimizer is not installed"))?;
    optimize(ctx, sctx, exec_ast, info_schema)
}

/// 获取已安装的非预处理计划缓存服务。
fn nonPreparedPlanCacheService()
-> Result<&'static dyn NonPreparedPlanCacheService, expression::Error> {
    NON_PREPARED_PLAN_CACHE
        .get()
        .map(Arc::as_ref)
        .ok_or_else(|| planner_error("non-prepared plan-cache service is not installed"))
}

/// 检查 SQL 或 Binding 中是否含 use_plan_cache 表 hint。
pub fn containUsePlanCacheHintInSQLOrBinding(
    sctx: &base::ContextRef,
    node: &resolve::NodeW,
) -> Result<bool, expression::Error> {
    let contains_sql_hint = node
        .node
        .with_node(|node| hint::ContainTableHintInStmtNode(node, hint::HintUsePlanCache))
        .ok_or_else(|| planner_error("resolved AST node is empty"))?;
    if contains_sql_hint {
        return Ok(true);
    }
    Ok(nonPreparedPlanCacheService()?.binding_contains_use_plan_cache_hint(sctx, node))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 非预处理缓存初始资格：旁路或可进入后续判定。
enum InitialNonPreparedCacheEligibility {
    Bypass,
    Eligible,
}

/// 按会话开关与语句形态判定是否进入非预处理缓存路径。
fn initialNonPreparedCacheEligibility(
    vars: &SessionVars,
    is_statement: bool,
    is_explain: bool,
) -> InitialNonPreparedCacheEligibility {
    if !vars.EnableNonPreparedPlanCache
        || !is_statement
        || vars.StmtCtx.InRestrictedSQL
        || is_explain
        || !vars.DisableTxnAutoRetry
        || vars.InMultiStmts
    {
        InitialNonPreparedCacheEligibility::Bypass
    } else {
        InitialNonPreparedCacheEligibility::Eligible
    }
}

/// 仅在 EXPLAIN FORMAT=plan_cache 时需要旁路警告。
fn shouldWarnPlanCacheBypass(vars: &SessionVars) -> bool {
    let (in_explain, _, format) = vars.StmtCtx.ExplainContext();
    in_explain && format == types::ExplainFormatPlanCache
}

/// 按需追加跳过非预处理计划缓存的警告。
fn appendPlanCacheBypassWarning(vars: &SessionVars, reason: &str) {
    if shouldWarnPlanCacheBypass(vars) {
        vars.StmtCtx
            .AppendWarning(statement_warning(planner_error(format!(
                "skip non-prepared plan-cache: {reason}"
            ))));
    }
}

/// hint_only 策略且缺少 hint 时旁路缓存。
fn shouldBypassHintOnlyStrategy(strategy: &str, contains_hint: bool) -> bool {
    strategy == vardef::TiDBPlanCacheStrategyHintOnly && !contains_hint
}

/// 可缓存标志为真时才允许查找。
fn cacheabilityAllowsLookup(cacheability: &NonPreparedCacheability) -> bool {
    cacheability.cacheable
}

/// 命中则直接返回；未命中则创建、写回并返回。
fn chooseCachedStatement(
    cached: Option<NonPreparedCachedStatementRef>,
    create: impl FnOnce() -> Result<NonPreparedCachedStatementRef, expression::Error>,
    store: impl FnOnce(NonPreparedCachedStatementRef) -> Result<(), expression::Error>,
) -> Result<NonPreparedCachedStatementRef, expression::Error> {
    if let Some(cached) = cached {
        return Ok(cached);
    }
    let created = create()?;
    store(Arc::clone(&created))?;
    Ok(created)
}

/// 非预处理计划缓存完整查找/生成/取计划流程；失败或旁路返回 None。
fn getPlanFromNonPreparedPlanCache(
    ctx: &dyn core::context::Context,
    sctx: &base::ContextRef,
    node: &resolve::NodeW,
    info_schema: Arc<dyn infoschema::InfoSchema>,
) -> Result<Option<OptimizeOutput>, expression::Error> {
    let vars = sctx.GetSessionVars();
    let is_explain = node
        .node
        .with_node(|node| node.as_any().is::<ast::ExplainStmt>())
        .unwrap_or(false);
    if initialNonPreparedCacheEligibility(vars, true, is_explain)
        == InitialNonPreparedCacheEligibility::Bypass
    {
        return Ok(None);
    }
    let service = nonPreparedPlanCacheService()?;
    let is_statement = node
        .node
        .with_node(|node| service.is_statement(node))
        .unwrap_or(false);
    if !is_statement {
        return Ok(None);
    }

    if vars.PlanCacheStrategy == vardef::TiDBPlanCacheStrategyHintOnly {
        let contains_cache_hint = containUsePlanCacheHintInSQLOrBinding(sctx, node)?;
        if shouldBypassHintOnlyStrategy(&vars.PlanCacheStrategy, contains_cache_hint) {
            appendPlanCacheBypassWarning(vars, nonPreparedPlanCacheHintOnlyNoHintReason);
            return Ok(None);
        }
    }

    let cacheability = service.cacheable(sctx, node, info_schema.as_ref())?;
    if !cacheabilityAllowsLookup(&cacheability) {
        appendPlanCacheBypassWarning(vars, &cacheability.reason);
        return Ok(None);
    }

    let parameterized = service.parameterize(node)?;
    let cached = service.lookup_cached_statement(sctx, &parameterized.sql)?;
    let statement = if let Some(cached) = cached {
        cached
    } else {
        let parameterized_ast = match service.parse_parameterized_ast(sctx, &parameterized.sql) {
            Ok(statement) => statement,
            Err(error) => {
                // A rare restore/parse failure bypasses cache and falls back to
                // normal optimization, exactly as the Go implementation does.
                vars.StmtCtx.AppendWarning(statement_warning(error));
                return Ok(None);
            }
        };
        // GeneratePlanCacheStmtWithAST may evaluate parameters, so install
        // them before generation exactly as Go does.
        service.set_parameter_values(sctx, &parameterized.values)?;
        let created = service.generate_cached_statement(
            ctx,
            sctx,
            &parameterized.sql,
            parameterized_ast,
            Arc::clone(&info_schema),
        )?;
        service.store_cached_statement(sctx, &parameterized.sql, Arc::clone(&created))?;
        created
    };
    let (plan, names) =
        service.get_plan(ctx, sctx, info_schema, statement, &parameterized.values)?;
    Ok(Some((plan, names)))
}

/// 先尝试非预处理缓存，未命中则走无缓存优化。
fn optimizeCache(
    ctx: &dyn core::context::Context,
    sctx: &base::ContextRef,
    node: &resolve::NodeW,
    info_schema: Arc<dyn infoschema::InfoSchema>,
) -> OptimizeResult {
    if let Some(cached) =
        getPlanFromNonPreparedPlanCache(ctx, sctx, node, Arc::clone(&info_schema))?
    {
        return Ok(cached);
    }
    optimizeNoCache(ctx, sctx, node, info_schema)
}

/// 无缓存优化主路径：hint、FastPlan、Binding、默认/演进优化。
fn optimizeNoCache(
    ctx: &dyn core::context::Context,
    sctx: &base::ContextRef,
    node: &resolve::NodeW,
    info_schema: Arc<dyn infoschema::InfoSchema>,
) -> OptimizeResult {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        optimizeNoCacheInner(ctx, sctx, node, info_schema)
    }))
    .unwrap_or_else(|panic| Err(planner_error(recoveredPanicMessage(panic))))
}

fn recoveredPanicMessage(panic: Box<dyn Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<String>() {
        return message.clone();
    }
    if let Some(message) = panic.downcast_ref::<&str>() {
        return (*message).to_owned();
    }
    "panic during planner optimization".to_owned()
}

fn optimizeNoCacheInner(
    ctx: &dyn core::context::Context,
    sctx: &base::ContextRef,
    node: &resolve::NodeW,
    info_schema: Arc<dyn infoschema::InfoSchema>,
) -> OptimizeResult {
    let service = optimizeSessionService()?;
    let table_hints = node
        .node
        .with_node(|node| hint::ExtractTableHintsFromStmtNode(node, None))
        .ok_or_else(|| planner_error("resolved AST node is empty"))?;
    let (statement_hints, warnings) = parseStatementHints(
        ctx,
        service,
        info_schema.as_ref(),
        table_hints,
        &sctx.GetSessionVars().CurrentDB(),
    );
    let original_query_had_hints = statement_hints.QueryHasHints;
    let original_statement_hints = statement_hints.clone();
    let ignore_plan_cache = statement_hints.IgnorePlanCache;
    let set_vars = statement_hints.SetVars.clone();
    service.install_statement_hints(sctx, statement_hints);
    for warning in warnings {
        sctx.GetSessionVars()
            .StmtCtx
            .AppendWarning(statement_warning(warning));
    }
    if ignore_plan_cache {
        service.set_skip_plan_cache(sctx, "ignore_plan_cache hint used in SQL query");
    }
    applySetVarHints(service, sctx, set_vars.into_iter());

    let _strict_tiflash_guard = StrictTiFlashGuard::enter(service, sctx, node)?;
    if let Some(fast_plan) = tryFastPlanIfTiKV(
        sctx.GetSessionVars()
            .IsolationReadEngines
            .contains(&astersql_kv::StoreType::TiKV),
        || service.try_fast_plan(ctx, sctx, node),
    )? {
        return Ok(fast_plan);
    }

    let is_statement = service.is_statement(node);
    // Go intentionally ignores MatchSQLBinding lookup errors.
    let matched_binding = service.match_sql_binding(sctx, node).ok().flatten();
    let use_binding = shouldUsePlanBinding(
        sctx.GetSessionVars().UsePlanBaselines,
        is_statement,
        matched_binding.is_some(),
    );
    let limited_node = if is_statement {
        service.try_add_extra_limit(sctx, node)?
    } else {
        node.clone()
    };

    let mut best_from_binding = None;
    let mut chosen_binding_hints = None;
    if use_binding {
        let binding = matched_binding.as_ref().expect("checked above");
        let original_query_hint_state = service.save_query_hint_state(&limited_node);
        let _restore_query_hints = ScopeRestore::new(|| {
            service.restore_query_hint_state(&limited_node, original_query_hint_state.as_ref());
        });
        if binding.is_enabled() {
            service.bind_query_hints(&limited_node, binding.as_ref())?;
            let (binding_statement_hints, binding_warnings) = parseStatementHints(
                ctx,
                service,
                info_schema.as_ref(),
                binding.statement_hints(),
                &sctx.GetSessionVars().CurrentDB(),
            );
            let binding_ignores_plan_cache = binding_statement_hints.IgnorePlanCache;
            let binding_set_vars = binding_statement_hints.SetVars.clone();
            let binding_hints_for_restore = binding_statement_hints.clone();
            service.install_statement_hints(sctx, binding_statement_hints);
            if binding_ignores_plan_cache {
                service.set_skip_plan_cache(sctx, "ignore_plan_cache hint used in SQL binding");
            }
            applySetVarHints(service, sctx, binding_set_vars);
            match optimize(ctx, sctx, &limited_node, Arc::clone(&info_schema)) {
                Ok((plan, names, _)) => {
                    for warning in binding_warnings {
                        sctx.GetSessionVars()
                            .StmtCtx
                            .AppendWarning(statement_warning(warning));
                    }
                    service.mark_binding_selected(
                        sctx,
                        binding.bind_sql(),
                        original_query_had_hints,
                    );
                    chosen_binding_hints = Some(binding_hints_for_restore);
                    best_from_binding = Some((plan, names));
                }
                Err(error) => sctx
                    .GetSessionVars()
                    .StmtCtx
                    .AppendWarning(statement_warning(planner_error(format!(
                        "binding {} failed: {error}",
                        binding.bind_sql()
                    )))),
            }
        }
        if best_from_binding.is_none() {
            sctx.GetSessionVars()
                .StmtCtx
                .AppendWarning(statement_warning(planner_error(
                    "no plan generated from bindings",
                )));
        }
    }

    service.advise_txn_warmup(sctx)?;
    if let Some(plan) = best_from_binding {
        let binding = matched_binding
            .as_ref()
            .expect("binding plan requires match");
        if shouldTryBaselineEvolution(
            sctx.GetSessionVars().EvolvePlanBaselines,
            sctx.GetSessionVars().SelectLimit,
            service.is_select_statement(&limited_node),
            binding.contains_read_from_storage_hint(),
        ) {
            service.install_statement_hints(sctx, original_statement_hints.clone());
            let default_plan = optimize(ctx, sctx, &limited_node, Arc::clone(&info_schema));
            service.install_statement_hints(
                sctx,
                chosen_binding_hints.expect("binding winner preserves its hints"),
            );
            if let Ok((default_plan, _, _)) = default_plan {
                // Go ignores an evolution candidate whose default plan uses
                // READ_FROM_STORAGE (TiFlash); generation itself is enough to
                // feed the existing baseline evolution owner.
                let _ = service.plan_contains_read_from_storage_hint(default_plan.as_ref());
            }
        }
        return Ok(plan);
    }
    service.install_statement_hints(sctx, original_statement_hints);
    let (plan, names, _) = optimize(ctx, sctx, &limited_node, info_schema)?;
    Ok((plan, names))
}

/// 解析表 hint 为 StmtHints，并收集警告。
fn parseStatementHints(
    ctx: &dyn core::context::Context,
    service: &dyn OptimizeSessionService,
    info_schema: &dyn infoschema::InfoSchema,
    table_hints: Vec<ast::TableOptimizerHint>,
    current_db: &str,
) -> (hint::StmtHints, Vec<hint::errors::Error>) {
    let mut check_hypo_index = hypoIndexChecker(ctx, service, info_schema);
    let (statement_hints, _, warnings) = hint::ParseStmtHints(
        table_hints,
        setVarHintChecker,
        &mut check_hypo_index,
        current_db.to_owned(),
        astersql_kv::ReplicaReadType::ReplicaReadFollower as u8,
    );
    (statement_hints, warnings)
}

/// 假想索引（hypo index）列偏移校验闭包。
fn hypoIndexChecker<'a>(
    ctx: &'a dyn core::context::Context,
    service: &'a dyn OptimizeSessionService,
    info_schema: &'a dyn infoschema::InfoSchema,
) -> impl FnMut(ast::CIStr, ast::CIStr, ast::CIStr) -> (i32, Option<hint::errors::Error>) + 'a {
    move |db, table, column| match service.hypo_index_column_offset(
        ctx,
        info_schema,
        db,
        table,
        column,
    ) {
        Ok(offset) => (offset, None),
        Err(error) => (0, Some(hint::errors::New(error.to_string()))),
    }
}

/// 仅当隔离读包含 TiKV 时尝试点查/快速计划。
fn tryFastPlanIfTiKV<T>(
    isolation_read_contains_tikv: bool,
    try_fast_plan: impl FnOnce() -> Result<Option<T>, expression::Error>,
) -> Result<Option<T>, expression::Error> {
    if !isolation_read_contains_tikv {
        return Ok(None);
    }
    try_fast_plan()
}

/// 是否启用匹配到的计划 Binding。
fn shouldUsePlanBinding(use_plan_baselines: bool, is_statement: bool, matched: bool) -> bool {
    use_plan_baselines && is_statement && matched
}

/// 是否尝试 Baseline 演进（生成默认计划供演进组件比较）。
fn shouldTryBaselineEvolution(
    evolve_plan_baselines: bool,
    select_limit: u64,
    is_select_statement: bool,
    binding_contains_read_from_storage: bool,
) -> bool {
    evolve_plan_baselines
        && select_limit == u64::MAX
        && is_select_statement
        && !binding_contains_read_from_storage
}

/// 校验 SET_VAR hint 中的系统变量是否存在及是否允许 hint 更新。
fn setVarHintChecker(var_name: String, hint_name: String) -> (bool, Option<hint::errors::Error>) {
    let Some(sys_var) = astersql_sessionctx_variable::GetSysVar(&var_name) else {
        return (
            false,
            Some(
                plannererrors::ErrUnresolvedHintName
                    .GenWithStackByArgs(&[var_name.into(), hint_name.into()])
                    .into(),
            ),
        );
    };
    let warning = (!sys_var.IsHintUpdatableVerified).then(|| {
        plannererrors::ErrNotHintUpdatable
            .GenWithStackByArgs(&[var_name.into()])
            .into()
    });
    (true, warning)
}

/// 应用 SET_VAR hint：成功则登记恢复，失败则警告。
fn applySetVarHints(
    service: &dyn OptimizeSessionService,
    sctx: &base::ContextRef,
    set_vars: impl IntoIterator<Item = (String, String)>,
) {
    for (name, value) in set_vars {
        match service.set_system_var_with_old_state(sctx, &name, &value) {
            Ok(old_value) => service.add_set_var_hint_restore(sctx, &name, &old_value),
            Err(error) => sctx
                .GetSessionVars()
                .StmtCtx
                .AppendWarning(statement_warning(error)),
        }
    }
}

/// 优化成功后应用资源组与 SET_VAR 等 hint 副作用。
fn applySuccessfulOptimizeEffects(service: &dyn OptimizeSessionService, sctx: &base::ContextRef) {
    let effects = service.statement_hint_effects(sctx);
    if let Some(resource_group) = effects.resource_group {
        let resource_group = resource_group.to_lowercase();
        if !vardef::EnableResourceControl.Load() {
            sctx.GetSessionVars()
                .StmtCtx
                .AppendWarning(statement_warning(planner_error(
                    "Resource control feature is disabled. Run `SET GLOBAL tidb_enable_resource_control='on'` to enable the feature",
                )));
        } else if vardef::EnableResourceControlStrictMode.Load()
            && !service.has_resource_group_admin_or_user(sctx)
        {
            sctx.GetSessionVars().StmtCtx.AppendWarning(
                plannererrors::ErrSpecificAccessDenied.GenWithStackByArgs(&[
                    "SUPER or RESOURCE_GROUP_ADMIN or RESOURCE_GROUP_USER".into(),
                ]),
            );
        } else {
            service.set_statement_resource_group(sctx, &resource_group);
            // Go ignores missing/invalid transactions and transaction lookup
            // errors at this point; the service performs the same guarded set.
            service.set_valid_txn_resource_group(sctx, &resource_group);
        }
    }
    applySetVarHints(service, sctx, effects.set_vars);
}

/// RAII：离开作用域时执行一次恢复闭包。
struct ScopeRestore<F: FnOnce()> {
    restore: Option<F>,
}

impl<F: FnOnce()> ScopeRestore<F> {
    fn new(restore: F) -> Self {
        Self {
            restore: Some(restore),
        }
    }
}

impl<F: FnOnce()> Drop for ScopeRestore<F> {
    fn drop(&mut self) {
        if let Some(restore) = self.restore.take() {
            restore();
        }
    }
}

/// 严格模式下临时移除 TiFlash 读引擎，退出时恢复。
struct StrictTiFlashGuard<'a> {
    _restore: ScopeRestore<Box<dyn FnOnce() + 'a>>,
}

impl<'a> StrictTiFlashGuard<'a> {
    fn enter(
        service: &'static dyn OptimizeSessionService,
        sctx: &'a base::ContextRef,
        node: &resolve::NodeW,
    ) -> Result<Self, expression::Error> {
        let active = service.strict_mode_applies(sctx, node)?;
        let was_present = if active {
            service.remove_tiflash_for_strict_mode(sctx)?
        } else {
            false
        };
        Ok(Self {
            _restore: ScopeRestore::new(Box::new(move || {
                if active {
                    service.restore_tiflash_after_strict_mode(sctx, was_present);
                }
            })),
        })
    }
}

/// 一轮优化持有的逻辑构建状态快照。
struct LogicalPlanBuildContext {
    session_state: LogicalPlanBuildStateSnapshotRef,
}

impl LogicalPlanBuildContext {
    fn take(service: &dyn OptimizeSessionService, sctx: &base::ContextRef) -> Self {
        Self {
            session_state: service.save_logical_plan_build_state(sctx),
        }
    }

    fn restore(&self, service: &dyn OptimizeSessionService, sctx: &base::ContextRef) {
        service.restore_logical_plan_build_state(sctx, self.session_state.as_ref());
    }
}

/// 多轮优化间保存/恢复逻辑构建状态；丢弃时回滚到初始快照。
struct OptimizationStateGuard<'a> {
    service: &'static dyn OptimizeSessionService,
    sctx: &'a base::ContextRef,
    initial: Option<LogicalPlanBuildContext>,
}

impl<'a> OptimizationStateGuard<'a> {
    fn new(service: &'static dyn OptimizeSessionService, sctx: &'a base::ContextRef) -> Self {
        Self {
            service,
            sctx,
            initial: Some(LogicalPlanBuildContext::take(service, sctx)),
        }
    }

    fn prepare_round(&self) {
        if let Some(initial) = &self.initial {
            initial.restore(self.service, self.sctx);
        }
    }

    fn commit(mut self, winner: LogicalPlanBuildContext) {
        winner.restore(self.service, self.sctx);
        self.initial = None;
    }
}

impl Drop for OptimizationStateGuard<'_> {
    fn drop(&mut self) {
        if let Some(initial) = &self.initial {
            initial.restore(self.service, self.sctx);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 替代逻辑计划轮次种类。
enum AlternativeRoundKind {
    NonDecorrelate,
    OrderAwareReorder,
    Correlate,
    SemiJoinRewrite,
    FtsLikeFallback,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 具名替代轮次及其种类。
struct AlternativeRound {
    name: &'static str,
    kind: AlternativeRoundKind,
}

/// 固定顺序的五类替代逻辑计划轮次。
const ALTERNATIVE_ROUNDS: [AlternativeRound; 5] = [
    AlternativeRound {
        name: "non-decorrelate",
        kind: AlternativeRoundKind::NonDecorrelate,
    },
    AlternativeRound {
        name: "order-aware-reorder",
        kind: AlternativeRoundKind::OrderAwareReorder,
    },
    AlternativeRound {
        name: "correlate",
        kind: AlternativeRoundKind::Correlate,
    },
    AlternativeRound {
        name: "semi-join-rewrite",
        kind: AlternativeRoundKind::SemiJoinRewrite,
    },
    AlternativeRound {
        name: "fts-like-fallback",
        kind: AlternativeRoundKind::FtsLikeFallback,
    },
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 默认轮次留下的、驱动替代轮次启用判定的信号位。
struct AlternativeSignals {
    decorrelated_apply: bool,
    same_order_index_join: bool,
    order_aware_join_reorder: bool,
    prefer_correlate: bool,
    semi_join_rewrite: bool,
    fts_like_fallback: bool,
    predicate_context_match: bool,
}

impl AlternativeSignals {
    fn from_default_round(vars: &SessionVars, result: &RoundResult) -> Self {
        let stmt = &vars.StmtCtx;
        let (
            decorrelated_apply,
            same_order_index_join,
            order_aware_join_reorder,
            semi_join_rewrite,
            fts_like_fallback,
            predicate_context_match,
        ) = stmt.AlternativeLogicalPlanSignals();
        Self {
            decorrelated_apply,
            same_order_index_join,
            order_aware_join_reorder: order_aware_join_reorder
                || (result.opt_flag & rule::FLAG_PUSH_DOWN_TOP_N != 0
                    && result.opt_flag & rule::FLAG_JOIN_REORDER != 0),
            prefer_correlate: stmt.AlternativeLogicalPlanPreferCorrelate(),
            semi_join_rewrite,
            fts_like_fallback: result.non_viable_fts || fts_like_fallback,
            predicate_context_match: result.predicate_match || predicate_context_match,
        }
    }
}

/// 是否尝试关闭解相关（decorrelate）的替代轮次。
fn shouldTryNonDecorrelationRound(vars: &SessionVars, signals: AlternativeSignals) -> bool {
    vars.EnableAlternativeLogicalPlans
        && signals.decorrelated_apply
        && !signals.same_order_index_join
}

/// 是否尝试顺序感知 Join 重排轮次。
fn shouldTryOrderAwareReorderRound(vars: &SessionVars, signals: AlternativeSignals) -> bool {
    vars.EnableAlternativeLogicalPlans && signals.order_aware_join_reorder
}

/// 是否尝试相关子查询（correlate）轮次。
fn shouldTryCorrelateRound(vars: &SessionVars, signals: AlternativeSignals) -> bool {
    vars.EnableAlternativeLogicalPlans && signals.prefer_correlate
}

/// 是否尝试半连接改写轮次（全局开关未开时才启用替代）。
fn shouldTrySemiJoinRewriteRound(vars: &SessionVars, signals: AlternativeSignals) -> bool {
    vars.EnableAlternativeLogicalPlans && signals.semi_join_rewrite && !vars.EnableSemiJoinRewrite
}

/// 是否尝试全文检索 LIKE 回退或谓词上下文匹配轮次。
fn shouldTryFtsLikeFallbackRound(vars: &SessionVars, signals: AlternativeSignals) -> bool {
    vars.EnableAlternativeLogicalPlans
        && (signals.fts_like_fallback || signals.predicate_context_match)
}

impl AlternativeRound {
    fn enabled(self, vars: &SessionVars, signals: AlternativeSignals) -> bool {
        match self.kind {
            AlternativeRoundKind::NonDecorrelate => shouldTryNonDecorrelationRound(vars, signals),
            AlternativeRoundKind::OrderAwareReorder => {
                shouldTryOrderAwareReorderRound(vars, signals)
            }
            AlternativeRoundKind::Correlate => shouldTryCorrelateRound(vars, signals),
            AlternativeRoundKind::SemiJoinRewrite => shouldTrySemiJoinRewriteRound(vars, signals),
            AlternativeRoundKind::FtsLikeFallback => shouldTryFtsLikeFallbackRound(vars, signals),
        }
    }

    fn adjust_flag(self, flag: u64) -> u64 {
        match self.kind {
            AlternativeRoundKind::NonDecorrelate => flag & !rule::FLAG_DECORRELATE,
            AlternativeRoundKind::OrderAwareReorder => flag | rule::FLAG_ORDER_AWARE_JOIN_REORDER,
            AlternativeRoundKind::Correlate => flag | rule::FLAG_CORRELATE,
            AlternativeRoundKind::SemiJoinRewrite | AlternativeRoundKind::FtsLikeFallback => flag,
        }
    }
}

/// 替代轮次期间临时覆盖会话标志，退出时还原。
struct AlternativeRoundGuard<'a> {
    vars: &'a SessionVars,
    kind: AlternativeRoundKind,
    previous: Option<bool>,
}

impl<'a> AlternativeRoundGuard<'a> {
    fn setup(vars: &'a SessionVars, kind: AlternativeRoundKind) -> Self {
        let previous = match kind {
            AlternativeRoundKind::Correlate => vars.SetAlternativeCorrelateOverride(Some(true)),
            AlternativeRoundKind::SemiJoinRewrite => {
                vars.SetAlternativeSemiJoinOverride(Some(true))
            }
            AlternativeRoundKind::FtsLikeFallback => {
                vars.SetAlternativeFTSLikeFallbackOverride(Some(true))
            }
            AlternativeRoundKind::NonDecorrelate | AlternativeRoundKind::OrderAwareReorder => None,
        };
        Self {
            vars,
            kind,
            previous,
        }
    }
}

impl Drop for AlternativeRoundGuard<'_> {
    fn drop(&mut self) {
        match self.kind {
            AlternativeRoundKind::Correlate => {
                self.vars.SetAlternativeCorrelateOverride(self.previous);
            }
            AlternativeRoundKind::SemiJoinRewrite => {
                self.vars.SetAlternativeSemiJoinOverride(self.previous);
            }
            AlternativeRoundKind::FtsLikeFallback => {
                self.vars
                    .SetAlternativeFTSLikeFallbackOverride(self.previous);
            }
            AlternativeRoundKind::NonDecorrelate | AlternativeRoundKind::OrderAwareReorder => {}
        }
    }
}

/// 单轮优化结果：物理计划、列名、代价、状态快照与信号输入。
struct RoundResult {
    plan: Box<dyn base::PhysicalPlan>,
    names: base::types::NameSlice,
    cost: f64,
    state: LogicalPlanBuildContext,
    opt_flag: u64,
    non_viable_fts: bool,
    predicate_match: bool,
}

/// 重置 PlanID/列 ID，构建 PlanBuilder 并生成运行时计划。
fn buildPlan(
    ctx: &dyn core::context::Context,
    sctx: &base::ContextRef,
    node: &resolve::NodeW,
    info_schema: Arc<dyn infoschema::InfoSchema>,
) -> Result<(core::PlanBuilder, core::BuiltRuntimePlan), expression::Error> {
    resetLogicalPlanBuildState(sctx.GetSessionVars());
    let result_set_builder = *RESULT_SET_BUILDER
        .get()
        .ok_or_else(|| planner_error("planner result-set builder is not installed"))?;
    let vars = sctx.GetSessionVars();
    vars.PlanID.store(0, Ordering::SeqCst);
    vars.PlanColumnID.store(0, Ordering::SeqCst);
    let hints = hint::NewQBHintHandler(None);
    let (mut builder, _) = core::NewPlanBuilder()
        .withResultSetBuilder(result_set_builder)
        .Init(Arc::clone(sctx), info_schema, hints);
    let begin_rewrite = Instant::now();
    let plan = builder.Build(ctx, node)?;
    let mut rewrite_phase_info = vars.SnapshotRewritePhaseInfo();
    rewrite_phase_info.DurationRewrite = begin_rewrite.elapsed();
    vars.RestoreRewritePhaseInfo(rewrite_phase_info);

    let mut seen_tables = HashSet::new();
    let tables = builder
        .GetVisitInfo()
        .iter()
        .filter_map(|visit| {
            (!visit.table.is_empty()).then(|| (visit.db.clone(), visit.table.clone()))
        })
        .filter(|table| seen_tables.insert(table.clone()))
        .map(|(db, table)| TableEntry {
            DB: db,
            Table: table,
        })
        .collect();
    vars.StmtCtx.SetLogicalPlanTables(tables);
    Ok((builder, plan))
}

/// 清理 Go `buildLogicalPlan` 在每轮构建前清空的会话瞬态状态。
fn resetLogicalPlanBuildState(vars: &SessionVars) {
    vars.RestoreScalarSubQueries(Vec::new());
    vars.RestoreExtendedColumnUniqueIDs(HashMap::new());
    vars.RestoreRewritePhaseInfo(RewritePhaseInfo::default());
}

/// 对已构建逻辑计划执行一轮 DoOptimize，并拍摄状态快照。
fn optimizeBuiltLogicalPlanRound(
    ctx: &dyn core::context::Context,
    sctx: &base::ContextRef,
    session_service: &dyn OptimizeSessionService,
    builder: core::PlanBuilder,
    mut logical: logicalop::LogicalPlanRef,
    round: Option<AlternativeRound>,
) -> Result<RoundResult, expression::Error> {
    let names = logical.OutputNames().Shallow();
    let base_flag = builder.GetOptFlag();
    let opt_flag = round.map_or(base_flag, |round| round.adjust_flag(base_flag));
    let (plan, cost) = core::DoOptimize(ctx, sctx, opt_flag, &mut logical)?;
    // Go records these signals after DoOptimize: logical rules may discover a
    // non-viable MATCH or a predicate-context MATCH while rewriting the plan.
    let non_viable_fts = builder.HasNonViableFTSMatch();
    let predicate_match = builder.HasPredicateMatch();
    let state = LogicalPlanBuildContext::take(session_service, sctx);
    Ok(RoundResult {
        plan,
        names,
        cost,
        state,
        opt_flag,
        non_viable_fts,
        predicate_match,
    })
}

/// 构建逻辑计划并执行指定替代轮次优化。
fn buildAndOptimizeLogicalPlanRound(
    ctx: &dyn core::context::Context,
    sctx: &base::ContextRef,
    session_service: &dyn OptimizeSessionService,
    node: &resolve::NodeW,
    info_schema: Arc<dyn infoschema::InfoSchema>,
    round: Option<AlternativeRound>,
) -> Result<RoundResult, expression::Error> {
    let (builder, plan) = buildPlan(ctx, sctx, node, info_schema)?;
    let core::BuiltRuntimePlan::Logical(logical) = plan else {
        return Err(planner_error(
            "alternative logical round produced a non-logical statement plan",
        ));
    };
    optimizeBuiltLogicalPlanRound(ctx, sctx, session_service, builder, logical, round)
}

/// 核心多轮优化：默认轮 + 启用的替代轮，按代价选胜者并提交状态。
fn optimize(
    ctx: &dyn core::context::Context,
    sctx: &base::ContextRef,
    node: &resolve::NodeW,
    info_schema: Arc<dyn infoschema::InfoSchema>,
) -> Result<(Box<dyn base::Plan>, base::types::NameSlice, f64), expression::Error> {
    let session_service = optimizeSessionService()?;
    let vars = sctx.GetSessionVars();
    let state_guard = OptimizationStateGuard::new(session_service, sctx);
    state_guard.prepare_round();
    session_service.reset_alternative_logical_plan_signals(sctx);
    let (first_builder, first_plan) = buildPlan(ctx, sctx, node, Arc::clone(&info_schema))?;
    let first = match first_plan {
        core::BuiltRuntimePlan::Logical(logical) => {
            optimizeBuiltLogicalPlanRound(ctx, sctx, session_service, first_builder, logical, None)?
        }
        core::BuiltRuntimePlan::NonLogical(plan) => {
            let names = plan.output_names();
            let state = LogicalPlanBuildContext::take(session_service, sctx);
            state_guard.commit(state);
            return Ok((plan, names, 0.0));
        }
    };
    let signals = AlternativeSignals::from_default_round(vars, &first);
    let enabled_rounds: Vec<_> = ALTERNATIVE_ROUNDS
        .into_iter()
        .filter(|round| round.enabled(vars, signals))
        .collect();

    // 默认轮若含不可行全文检索匹配，则暂不作为胜者，等待替代轮次。
    let mut winner = if first.non_viable_fts {
        None
    } else {
        Some(first)
    };
    let mut last_alternative_error = None;
    // 对每个启用的替代轮次重建计划并按代价竞选。
    for round in enabled_rounds {
        state_guard.prepare_round();
        let _round_guard = AlternativeRoundGuard::setup(vars, round.kind);
        match buildAndOptimizeLogicalPlanRound(
            ctx,
            sctx,
            session_service,
            node,
            Arc::clone(&info_schema),
            Some(round),
        ) {
            Ok(candidate) => {
                if winner
                    .as_ref()
                    .is_none_or(|current| candidate.cost < current.cost)
                {
                    winner = Some(candidate);
                }
            }
            Err(error) => {
                last_alternative_error = Some(error);
            }
        }
    }

    let winner = winner.ok_or_else(|| {
        last_alternative_error
            .unwrap_or_else(|| planner_error("failed to build a valid logical plan in any round"))
    })?;
    let RoundResult {
        plan,
        names,
        cost,
        state,
        ..
    } = winner;
    state_guard.commit(state);
    let plan: Box<dyn base::Plan> = plan;
    Ok((plan, names, cost))
}

/// 外键级联优化：绕过缓存但走同一构建/优化管线。
/// Foreign-key cascades bypass both cache paths but retain the same real build
/// and optimizer pipeline.
pub fn OptimizeForForeignKeyCascade(
    ctx: &dyn core::context::Context,
    sctx: &base::ContextRef,
    node: &resolve::NodeW,
    info_schema: Arc<dyn infoschema::InfoSchema>,
) -> Result<Box<dyn base::Plan>, expression::Error> {
    optimize(ctx, sctx, node, info_schema).map(|(plan, _, _)| plan)
}

/// 诊断：优化语句并返回物理计划代价。
pub fn queryPlanCost(
    sctx: &base::ContextRef,
    node: &resolve::NodeW,
) -> Result<f64, expression::Error> {
    let runtime = plannerDiagnosticRuntime()?;
    let (mut plan, _) = runtime.optimize_statement(sctx, node)?;
    if !runtime.is_physical_plan(plan.as_ref()) {
        return Err(planner_error("plan is not a physical plan"));
    }
    runtime.physical_plan_cost(plan.as_mut())
}

/// 诊断：预处理后优化并计算计划 digest。
pub fn calculatePlanDigestFunc(
    sctx: &base::ContextRef,
    node: &resolve::NodeW,
) -> Result<String, expression::Error> {
    let runtime = plannerDiagnosticRuntime()?;
    runtime.preprocess(sctx, node)?;
    let (plan, _) = runtime.optimize_statement(sctx, node)?;
    runtime.plan_digest(plan.as_ref())
}

/// 诊断：记录本次优化涉及的相关优化变量与修复项。
pub fn recordRelevantOptVarsAndFixes(
    sctx: &base::ContextRef,
    node: &resolve::NodeW,
) -> Result<(Vec<String>, Vec<u64>), expression::Error> {
    let runtime = plannerDiagnosticRuntime()?;
    let vars = sctx.GetSessionVars();
    vars.ResetRelevantOptVarsAndFixes(true);
    let _reset = ScopeRestore::new(|| vars.ResetRelevantOptVarsAndFixes(false));
    runtime.preprocess(sctx, node)?;
    runtime.optimize_statement(sctx, node)?;
    Ok(vars.RelevantOptVarsAndFixes())
}

/// 诊断：生成简要计划（忽略 explain id 后缀）。
pub fn genBriefPlanWithSCtx(
    sctx: &base::ContextRef,
    node: &resolve::NodeW,
) -> Result<BriefPlanData, expression::Error> {
    let runtime = plannerDiagnosticRuntime()?;
    runtime.preprocess(sctx, node)?;
    let (plan, _) = runtime.optimize_statement(sctx, node)?;
    runtime.set_ignore_explain_id_suffix(sctx, true);
    runtime.brief_plan(plan.as_ref())
}

/// 从可选 Plan 提取 ID；无 Plan 时返回 (0, false)。
fn planIDFunc(plan: Option<&dyn base::Plan>) -> (i32, bool) {
    plan.map_or((0, false), |plan| (plan.id(), true))
}

/// 从 StmtCtx 缓存值向下转型提取 Plan ID。
fn planIDFromCacheValue(value: &astersql_sessionctx_stmtctx::CacheValue) -> Option<i32> {
    if let Some(plan) =
        astersql_sessionctx_stmtctx::cache_downcast_ref::<Arc<dyn base::Plan>>(value)
    {
        return Some(plan.id());
    }
    astersql_sessionctx_stmtctx::cache_downcast_ref::<Box<dyn base::Plan>>(value)
        .map(|plan| plan.id())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 只读准入分类结果。
enum ReadOnlyAdmission {
    Allowed,
    Commit,
    Ast(bool),
}

/// 按 AST 节点类型分类只读准入（白名单 / COMMIT / IsReadOnly）。
fn classifyReadOnlyAdmission(node: &dyn ast::Node) -> ReadOnlyAdmission {
    if node.as_any().is::<ast::SetStmt>()
        || node.as_any().is::<ast::AnalyzeTableStmt>()
        || node.as_any().is::<ast::UseStmt>()
        || node.as_any().is::<ast::ShowStmt>()
        || node.as_any().is::<ast::CreateBindingStmt>()
        || node.as_any().is::<ast::DropBindingStmt>()
        || node.as_any().is::<ast::PrepareStmt>()
        || node.as_any().is::<ast::BeginStmt>()
        || node.as_any().is::<ast::RollbackStmt>()
    {
        return ReadOnlyAdmission::Allowed;
    }
    if node.as_any().is::<ast::CommitStmt>() {
        return ReadOnlyAdmission::Commit;
    }
    // Passing false is intentional and matches Go: changing global variables
    // remains possible while the cluster is read-only, so read-only mode can
    // be disabled again with SET GLOBAL.
    ReadOnlyAdmission::Ast(ast::util::IsReadOnly(node, false))
}

/// 根据分类结果解析是否允许；COMMIT 委托事务检查。
fn resolveReadOnlyAdmission(
    decision: ReadOnlyAdmission,
    check_commit: impl FnOnce() -> Result<bool, expression::Error>,
) -> Result<bool, expression::Error> {
    match decision {
        ReadOnlyAdmission::Allowed => Ok(true),
        ReadOnlyAdmission::Commit => check_commit(),
        ReadOnlyAdmission::Ast(read_only) => Ok(read_only),
    }
}

/// Go `allowInReadOnlyMode`：特权旁路由权限接线提供；COMMIT 委托事务接线，可写事务回滚且错误只传播一次。
/// Go `allowInReadOnlyMode`. Explicitly granted
/// RESTRICTED_REPLICA_WRITER_ADMIN is supplied by privilege wiring; COMMIT is
/// delegated to transaction wiring so a writable transaction is rolled back
/// and its error is propagated exactly once.
pub fn allowInReadOnlyMode(
    sctx: &base::ContextRef,
    node: &resolve::NodeW,
) -> Result<bool, expression::Error> {
    if READ_ONLY_PRIVILEGE_BYPASS
        .get()
        .is_some_and(|check| check(sctx))
    {
        return Ok(true);
    }
    node.node
        .with_node(|node| {
            resolveReadOnlyAdmission(classifyReadOnlyAdmission(node), || {
                let check = READ_ONLY_COMMIT_CHECK.get().ok_or_else(|| {
                    planner_error("read-only COMMIT transaction checker is not installed")
                })?;
                check(sctx)
            })
        })
        .ok_or_else(|| planner_error("resolved AST node is empty"))?
}

#[cfg(test)]
#[path = "optimize_aster_unit_test.rs"]
mod optimize_aster_unit_test;
