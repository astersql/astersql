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

// 会话表达式上下文：SessionContext → ExprContext / EvalContext。
//
// 对应 Go `sessionctx.go`。把会话侧变量、InfoSchema、KV、权限与语句时间
// 适配为表达式构建（BuildContext）与求值（EvalContext）所需接口；
// 可选属性（OptionalEvalProp）在 NewEvalContext 中一次性装齐。

use std::collections::HashSet;
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use chrono_tz::Tz;

use crate::{
    auth, contextutil, errctx, exprctx, expropt, exprstatic, infoschema, mathutil, mysql,
    privilege, types, vardef, variable,
};

/// The narrow session boundary used by expression construction and evaluation.
///
/// The associated types preserve Go's concrete InfoSchema, KV storage and SQL
/// executor values without forcing this leaf package to own those subsystems.
/// 表达式构建/求值使用的窄会话边界。
/// 关联类型保留 Go 中 InfoSchema、KV Store、SQLExecutor 的具体形态，避免叶包拥有这些子系统。
pub trait SessionContext: expropt::AdvisoryLockContext + Send + Sync + 'static {
    type InfoSchema: infoschema::MetaOnlyInfoSchema + Send + Sync + 'static;
    type Store: Send + Sync + 'static;
    type SqlExecutor: expropt::SQLExecutor + Send + Sync + 'static;

    fn session_vars(&self) -> Arc<variable::session::SessionVars>;
    fn current_user(&self) -> Arc<auth::UserIdentity>;
    fn active_roles(&self) -> Vec<Arc<auth::RoleIdentity>>;
    fn info_schema(&self) -> Arc<Self::InfoSchema>;
    fn latest_info_schema(&self) -> Arc<Self::InfoSchema>;
    fn store(&self) -> Arc<Self::Store>;
    fn restricted_sql_executor(&self) -> Arc<Self::SqlExecutor>;
    fn sequence_operator(
        &self,
        db: &str,
        name: &str,
    ) -> anyhow::Result<Box<dyn expropt::SequenceOperator>>;
    fn is_ddl_owner(&self) -> bool;
    fn privilege_manager(&self) -> Option<Arc<dyn PrivilegeManager>>;

    fn charset_info(&self) -> (String, String);
    fn default_collation_for_utf8mb4(&self) -> String;
    fn system_var(&self, name: &str) -> Option<String>;
    fn sysdate_is_now(&self) -> bool;
    fn noop_funcs_mode(&self) -> i32;
    fn rng(&self) -> Arc<mathutil::MysqlRng>;
    fn plan_cache_tracker(&self) -> Arc<contextutil::plancache::PlanCacheTracker>;
    fn alloc_plan_column_id(&self) -> i64;
    fn last_plan_column_id(&self) -> i64;
    fn windowing_use_high_precision(&self) -> bool;
    fn group_concat_max_len(&self) -> u64;
    fn set_group_concat_max_len_for_test(&self, value: u64);
    fn connection_id(&self) -> u64;
    fn readonly_user_vars(&self) -> HashSet<String>;

    fn context_id(&self) -> u64;
    fn sql_mode(&self) -> mysql::SQLMode;
    fn type_context(&self) -> types::Context;
    fn error_context(&self) -> errctx::Context;
    fn warning_handler(&self) -> Arc<dyn contextutil::WarnHandler + Send + Sync>;
    fn current_db(&self) -> String;
    fn stale_tso(&self) -> Result<u64, String>;
    /// Returns the `timestamp` system variable after applying its statement
    /// cache, matching Go's `GetSessionOrGlobalSystemVar("timestamp")`.
    fn timestamp_system_var(&self) -> Result<String, String>;
    fn max_allowed_packet(&self) -> u64;
    fn tidb_redact_log(&self) -> String;
    fn default_week_format_mode(&self) -> String;
    fn div_precision_increment(&self) -> i32;
    fn parameter_values(&self) -> Vec<types::Datum>;
    fn user_vars_reader(&self) -> &dyn exprctx::UserVarsReader;
}

/// The two privilege checks used by this package. Full privilege managers get a
/// blanket implementation, while focused session implementations can provide a
/// narrow checker without implementing unrelated authentication operations.
/// 本包用到的两类权限检查；完整权限管理器有 blanket 实现，窄会话可只实现本 trait。
pub trait PrivilegeManager: Send + Sync {
    fn request_verification(
        &self,
        active_roles: &[auth::RoleIdentity],
        db: &str,
        table: &str,
        column: &str,
        privilege_type: mysql::PrivilegeType,
    ) -> bool;
    fn request_dynamic_verification(
        &self,
        active_roles: &[auth::RoleIdentity],
        privilege_name: &str,
        grantable: bool,
    ) -> bool;
}

impl<T> PrivilegeManager for T
where
    T: privilege::Manager + Send + Sync,
{
    fn request_verification(
        &self,
        active_roles: &[auth::RoleIdentity],
        db: &str,
        table: &str,
        column: &str,
        privilege_type: mysql::PrivilegeType,
    ) -> bool {
        privilege::Manager::RequestVerification(
            self,
            active_roles,
            db,
            table,
            column,
            privilege_type,
        )
    }

    fn request_dynamic_verification(
        &self,
        active_roles: &[auth::RoleIdentity],
        privilege_name: &str,
        grantable: bool,
    ) -> bool {
        privilege::Manager::RequestDynamicVerification(
            self,
            active_roles,
            privilege_name,
            grantable,
        )
    }
}

/// 把 SessionContext 的 PrivilegeManager 适配为 expropt::PrivilegeChecker。
struct ContextPrivilegeChecker<C: SessionContext> {
    session: Arc<C>,
}

impl<C: SessionContext> expropt::PrivilegeChecker for ContextPrivilegeChecker<C> {
    fn request_verification(
        &self,
        db: &str,
        table: &str,
        column: &str,
        privilege_type: mysql::PrivilegeType,
    ) -> bool {
        // 未启用权限管理时默认放行，与 Go 测试夹具一致。
        let Some(manager) = self.session.privilege_manager() else {
            return true;
        };
        let roles = self
            .session
            .active_roles()
            .into_iter()
            .map(|role| role.as_ref().clone())
            .collect::<Vec<_>>();
        manager.request_verification(&roles, db, table, column, privilege_type)
    }

    fn request_dynamic_verification(&self, privilege_name: &str, grantable: bool) -> bool {
        let Some(manager) = self.session.privilege_manager() else {
            return true;
        };
        let roles = self
            .session
            .active_roles()
            .into_iter()
            .map(|role| role.as_ref().clone())
            .collect::<Vec<_>>();
        manager.request_dynamic_verification(&roles, privilege_name, grantable)
    }
}

/// ExprContext adapts a live session to the expression build interfaces.
/// ExprContext：把活动会话适配为表达式构建接口。
pub struct ExprContext<C: SessionContext> {
    sctx: Arc<C>,
    rng: Arc<mathutil::MysqlRng>,
    plan_cache_tracker: Arc<contextutil::plancache::PlanCacheTracker>,
    pub EvalContext: EvalContext<C>,
}

/// 从会话构造 ExprContext，并内嵌 NewEvalContext。
pub fn NewExprContext<C: SessionContext>(sctx: Arc<C>) -> ExprContext<C> {
    ExprContext {
        rng: sctx.rng(),
        plan_cache_tracker: sctx.plan_cache_tracker(),
        EvalContext: NewEvalContext(Arc::clone(&sctx)),
        sctx,
    }
}

/// ExprContext 的具体方法：转发会话配置并暴露静态化入口。
impl<C: SessionContext> ExprContext<C> {
    pub fn GetEvalCtx(&self) -> &dyn exprctx::EvalContext {
        &self.EvalContext
    }
    pub fn GetCharsetInfo(&self) -> (String, String) {
        self.sctx.charset_info()
    }
    pub fn GetDefaultCollationForUTF8MB4(&self) -> String {
        self.sctx.default_collation_for_utf8mb4()
    }
    pub fn GetBlockEncryptionMode(&self) -> String {
        self.sctx
            .system_var(vardef::BlockEncryptionMode)
            .unwrap_or_else(|| vardef::DefBlockEncryptionMode.to_owned())
    }
    pub fn GetSysdateIsNow(&self) -> bool {
        self.sctx.sysdate_is_now()
    }
    pub fn GetNoopFuncsMode(&self) -> i32 {
        self.sctx.noop_funcs_mode()
    }
    pub fn Rng(&self) -> &mathutil::MysqlRng {
        self.rng.as_ref()
    }
    pub fn IsUseCache(&self) -> bool {
        self.plan_cache_tracker.UseCache()
    }
    pub fn SetSkipPlanCache(&self, reason: &str) {
        self.plan_cache_tracker.SetSkipPlanCache(reason);
    }
    pub fn AllocPlanColumnID(&self) -> i64 {
        self.sctx.alloc_plan_column_id()
    }
    pub fn IsInNullRejectCheck(&self) -> bool {
        false
    }
    pub fn IsConstantPropagateCheck(&self) -> bool {
        false
    }
    pub fn GetWindowingUseHighPrecision(&self) -> bool {
        self.sctx.windowing_use_high_precision()
    }
    pub fn GetGroupConcatMaxLen(&self) -> u64 {
        self.sctx.group_concat_max_len()
    }
    pub fn SetGroupConcatMaxLenForTest(&self, value: u64) {
        self.sctx.set_group_concat_max_len_for_test(value);
    }
    pub fn ConnectionID(&self) -> u64 {
        self.sctx.connection_id()
    }
    pub fn IsReadonlyUserVar(&self, name: &str) -> bool {
        self.sctx.readonly_user_vars().contains(name)
    }
    pub fn IntoStatic(&self) -> exprstatic::ExprContext {
        exprstatic::MakeExprContextStatic(self)
    }
    pub fn GetStaticConvertibleEvalContext(&self) -> &dyn exprctx::StaticConvertibleEvalContext {
        &self.EvalContext
    }
    pub fn GetPlanCacheTracker(&self) -> &contextutil::plancache::PlanCacheTracker {
        self.plan_cache_tracker.as_ref()
    }
    pub fn GetLastPlanColumnID(&self) -> i64 {
        self.sctx.last_plan_column_id()
    }
}

/// 实现 BuildContext：委托到同名具体方法。
impl<C: SessionContext> exprctx::BuildContext for ExprContext<C> {
    fn GetEvalCtx(&self) -> &dyn exprctx::EvalContext {
        self.GetEvalCtx()
    }
    fn GetCharsetInfo(&self) -> (String, String) {
        self.GetCharsetInfo()
    }
    fn GetDefaultCollationForUTF8MB4(&self) -> String {
        self.GetDefaultCollationForUTF8MB4()
    }
    fn GetBlockEncryptionMode(&self) -> String {
        self.GetBlockEncryptionMode()
    }
    fn GetSysdateIsNow(&self) -> bool {
        self.GetSysdateIsNow()
    }
    fn GetNoopFuncsMode(&self) -> i32 {
        self.GetNoopFuncsMode()
    }
    fn Rng(&self) -> &mathutil::MysqlRng {
        self.Rng()
    }
    fn IsUseCache(&self) -> bool {
        self.IsUseCache()
    }
    fn SetSkipPlanCache(&self, reason: &str) {
        self.SetSkipPlanCache(reason);
    }
    fn AllocPlanColumnID(&self) -> i64 {
        self.AllocPlanColumnID()
    }
    fn IsInNullRejectCheck(&self) -> bool {
        false
    }
    fn IsConstantPropagateCheck(&self) -> bool {
        false
    }
    fn ConnectionID(&self) -> u64 {
        self.ConnectionID()
    }
    fn IsReadonlyUserVar(&self, name: &str) -> bool {
        self.IsReadonlyUserVar(name)
    }
}

/// 实现 ExprContext：窗口高精度与 group_concat 长度。
impl<C: SessionContext> exprctx::ExprContext for ExprContext<C> {
    fn GetWindowingUseHighPrecision(&self) -> bool {
        self.GetWindowingUseHighPrecision()
    }
    fn GetGroupConcatMaxLen(&self) -> u64 {
        self.GetGroupConcatMaxLen()
    }
}

/// 静态可转换上下文：导出求值上下文与计划缓存/RNG 的 Arc。
impl<C: SessionContext> exprctx::StaticConvertibleExprContext for ExprContext<C> {
    fn GetStaticConvertibleEvalContext(&self) -> &dyn exprctx::StaticConvertibleEvalContext {
        &self.EvalContext
    }
    fn GetPlanCacheTracker(&self) -> &contextutil::plancache::PlanCacheTracker {
        self.plan_cache_tracker.as_ref()
    }
    fn GetLastPlanColumnID(&self) -> i64 {
        self.sctx.last_plan_column_id()
    }
    fn GetRngArc(&self) -> Option<Arc<mathutil::MysqlRng>> {
        Some(Arc::clone(&self.rng))
    }
    fn GetPlanCacheTrackerArc(&self) -> Option<Arc<contextutil::plancache::PlanCacheTracker>> {
        Some(Arc::clone(&self.plan_cache_tracker))
    }
}

/// EvalContext exposes statement-local state and all nine optional providers.
/// EvalContext：语句局部状态与全部九个可选属性提供者。
pub struct EvalContext<C: SessionContext> {
    sctx: Arc<C>,
    props: expropt::OptionalEvalPropProviders,
    warning_handler: Arc<dyn contextutil::WarnHandler + Send + Sync>,
}

/// 注册 CurrentUser、SessionVars、InfoSchema、KV、SQLExecutor、Sequence、
/// AdvisoryLock、DDLOwner、PrivilegeChecker 九类可选属性后返回求值上下文。
pub fn NewEvalContext<C: SessionContext>(sctx: Arc<C>) -> EvalContext<C> {
    let warning_handler = sctx.warning_handler();
    let mut ctx = EvalContext {
        sctx: Arc::clone(&sctx),
        props: expropt::OptionalEvalPropProviders::new(),
        warning_handler,
    };

    // 以下按固定顺序装载可选属性；结束时断言属性集合已满。
    let current_user_session = Arc::clone(&sctx);
    ctx.set_optional_prop(Box::new(expropt::CurrentUserPropProvider::new(move || {
        (
            current_user_session.current_user(),
            current_user_session.active_roles(),
        )
    })));
    ctx.set_optional_prop(Box::new(expropt::SessionVarsPropProvider::new(
        sctx.session_vars(),
    )));

    let info_schema_session = Arc::clone(&sctx);
    ctx.set_optional_prop(Box::new(
        expropt::InfoSchemaPropProvider::<C::InfoSchema>::new(move |is_domain| {
            // is_domain=true 取最新域级 InfoSchema，否则取会话快照。
            if is_domain {
                info_schema_session.latest_info_schema()
            } else {
                info_schema_session.info_schema()
            }
        }),
    ));

    let store_session = Arc::clone(&sctx);
    ctx.set_optional_prop(Box::new(expropt::KVStorePropProvider::<C::Store>::new(
        move || store_session.store(),
    )));

    let sql_session = Arc::clone(&sctx);
    ctx.set_optional_prop(Box::new(
        expropt::SQLExecutorPropProvider::<C::SqlExecutor>::new(move || {
            Ok(sql_session.restricted_sql_executor())
        }),
    ));

    let sequence_session = Arc::clone(&sctx);
    ctx.set_optional_prop(Box::new(expropt::SequenceOperatorProvider::new(
        move |db, name| sequence_session.sequence_operator(db, name),
    )));
    ctx.set_optional_prop(Box::new(expropt::AdvisoryLockPropProvider::new(
        Arc::clone(&sctx),
    )));

    let ddl_session = Arc::clone(&sctx);
    ctx.set_optional_prop(Box::new(expropt::DDLOwnerInfoProvider::new(move || {
        ddl_session.is_ddl_owner()
    })));

    let privilege_session = Arc::clone(&sctx);
    ctx.set_optional_prop(Box::new(expropt::PrivilegeCheckerProvider::new(
        move || {
            Arc::new(ContextPrivilegeChecker {
                session: Arc::clone(&privilege_session),
            })
        },
    )));

    assert!(
        ctx.props.prop_key_set().IsFull(),
        "session EvalContext must provide every optional property"
    );
    ctx
}

/// EvalContext 具体 API：类型/错误上下文、告警、时间、参数与权限。
impl<C: SessionContext> EvalContext<C> {
    /// 按 key 注册可选属性，禁止重复。
    fn set_optional_prop(&mut self, prop: Box<dyn exprctx::OptionalEvalPropProvider>) {
        let key = prop.Desc().Key();
        assert!(
            !self.props.contains(key),
            "duplicate optional property {key}"
        );
        self.props.add(prop);
    }

    pub fn Sctx(&self) -> &C {
        self.sctx.as_ref()
    }
    pub fn CtxID(&self) -> u64 {
        self.sctx.context_id()
    }
    pub fn SQLMode(&self) -> mysql::SQLMode {
        self.sctx.sql_mode()
    }
    pub fn TypeCtx(&self) -> types::Context {
        self.sctx.type_context()
    }
    pub fn ErrCtx(&self) -> errctx::Context {
        self.sctx.error_context()
    }
    pub fn Location(&self) -> Tz {
        self.TypeCtx().Location()
    }
    pub fn AppendWarning(&self, error: contextutil::errors::SharedError) {
        self.warning_handler.AppendWarning(error);
    }
    pub fn AppendNote(&self, error: contextutil::errors::SharedError) {
        self.warning_handler.AppendNote(error);
    }
    pub fn WarningCount(&self) -> usize {
        self.warning_handler.WarningCount()
    }
    pub fn TruncateWarnings(&self, start: isize) -> Vec<contextutil::SQLWarn> {
        self.warning_handler.TruncateWarnings(start)
    }
    pub fn CopyWarnings(
        &self,
        destination: Vec<contextutil::SQLWarn>,
    ) -> Vec<contextutil::SQLWarn> {
        self.warning_handler.CopyWarnings(destination)
    }
    pub fn CurrentDB(&self) -> String {
        self.sctx.current_db()
    }
    pub fn CurrentTime(&self) -> Result<DateTime<Tz>, contextutil::errors::SharedError> {
        getStmtTimestamp(Some(self.sctx.as_ref()))
    }
    pub fn GetMaxAllowedPacket(&self) -> u64 {
        self.sctx.max_allowed_packet()
    }
    pub fn GetTiDBRedactLog(&self) -> String {
        self.sctx.tidb_redact_log()
    }
    pub fn GetDefaultWeekFormatMode(&self) -> String {
        let mode = self.sctx.default_week_format_mode();
        // 空字符串时回退为 MySQL 默认 week 模式 "0"。
        if mode.is_empty() {
            "0".to_owned()
        } else {
            mode
        }
    }
    pub fn GetDivPrecisionIncrement(&self) -> i32 {
        self.sctx.div_precision_increment()
    }
    pub fn GetOptionalPropSet(&self) -> exprctx::OptionalEvalPropKeySet {
        self.props.prop_key_set()
    }
    pub fn GetOptionalPropProvider(
        &self,
        key: exprctx::OptionalEvalPropKey,
    ) -> Option<&dyn exprctx::OptionalEvalPropProvider> {
        self.props.get(key)
    }
    pub fn RequestVerification(
        &self,
        db: &str,
        table: &str,
        column: &str,
        privilege_type: mysql::PrivilegeType,
    ) -> bool {
        expropt::PrivilegeChecker::request_verification(
            &ContextPrivilegeChecker {
                session: Arc::clone(&self.sctx),
            },
            db,
            table,
            column,
            privilege_type,
        )
    }
    pub fn RequestDynamicVerification(&self, privilege_name: &str, grantable: bool) -> bool {
        expropt::PrivilegeChecker::request_dynamic_verification(
            &ContextPrivilegeChecker {
                session: Arc::clone(&self.sctx),
            },
            privilege_name,
            grantable,
        )
    }
    pub fn GetParamValue(&self, index: usize) -> Result<types::Datum, exprctx::ParamError> {
        self.sctx
            .parameter_values()
            .get(index)
            .cloned()
            .ok_or(exprctx::ErrParamIndexExceedParamCounts)
    }
    pub fn GetUserVarsReader(&self) -> &dyn exprctx::UserVarsReader {
        self.sctx.user_vars_reader()
    }
    pub fn IntoStatic(&self) -> exprstatic::EvalContext {
        exprstatic::MakeEvalContextStatic(self)
    }
    pub fn AllParamValues(&self) -> Vec<types::Datum> {
        self.sctx.parameter_values()
    }
    pub fn GetWarnHandler(&self) -> &dyn contextutil::WarnHandler {
        self.warning_handler.as_ref()
    }
}

impl<C: SessionContext> contextutil::WarnAppender for EvalContext<C> {
    fn AppendWarning(&self, error: contextutil::errors::SharedError) {
        self.warning_handler.AppendWarning(error);
    }
    fn AppendNote(&self, error: contextutil::errors::SharedError) {
        self.warning_handler.AppendNote(error);
    }
}

impl<C: SessionContext> contextutil::WarnHandler for EvalContext<C> {
    fn WarningCount(&self) -> usize {
        self.warning_handler.WarningCount()
    }
    fn TruncateWarnings(&self, start: isize) -> Vec<contextutil::SQLWarn> {
        self.warning_handler.TruncateWarnings(start)
    }
    fn CopyWarnings(&self, destination: Vec<contextutil::SQLWarn>) -> Vec<contextutil::SQLWarn> {
        self.warning_handler.CopyWarnings(destination)
    }
}

impl<C: SessionContext> exprctx::ParamValues for EvalContext<C> {
    fn GetParamValue(&self, index: usize) -> Result<types::Datum, exprctx::ParamError> {
        self.GetParamValue(index)
    }
}

impl<C: SessionContext> exprctx::EvalContext for EvalContext<C> {
    fn CtxID(&self) -> u64 {
        self.CtxID()
    }
    fn SQLMode(&self) -> mysql::SQLMode {
        self.SQLMode()
    }
    fn TypeCtx(&self) -> types::Context {
        self.TypeCtx()
    }
    fn ErrCtx(&self) -> errctx::Context {
        self.ErrCtx()
    }
    fn Location(&self) -> Tz {
        self.Location()
    }
    fn CurrentTime(&self) -> Result<DateTime<Tz>, contextutil::errors::SharedError> {
        self.CurrentTime()
    }
    fn CurrentDB(&self) -> String {
        self.CurrentDB()
    }
    fn GetMaxAllowedPacket(&self) -> u64 {
        self.GetMaxAllowedPacket()
    }
    fn GetTiDBRedactLog(&self) -> String {
        self.GetTiDBRedactLog()
    }
    fn GetDefaultWeekFormatMode(&self) -> String {
        self.GetDefaultWeekFormatMode()
    }
    fn GetDivPrecisionIncrement(&self) -> i32 {
        self.GetDivPrecisionIncrement()
    }
    fn GetUserVarsReader(&self) -> &dyn exprctx::UserVarsReader {
        self.GetUserVarsReader()
    }
    fn GetOptionalPropSet(&self) -> exprctx::OptionalEvalPropKeySet {
        self.GetOptionalPropSet()
    }
    fn GetOptionalPropProvider(
        &self,
        key: exprctx::OptionalEvalPropKey,
    ) -> Option<&dyn exprctx::OptionalEvalPropProvider> {
        self.GetOptionalPropProvider(key)
    }
}

impl<C: SessionContext> exprctx::StaticConvertibleEvalContext for EvalContext<C> {
    fn AllParamValues(&self) -> Vec<types::Datum> {
        self.sctx.parameter_values()
    }
    fn GetWarnHandler(&self) -> &dyn contextutil::WarnHandler {
        self.warning_handler.as_ref()
    }
    fn GetWarnHandlerArc(&self) -> Option<Arc<dyn contextutil::WarnHandler + Send + Sync>> {
        Some(Arc::clone(&self.warning_handler))
    }
}

/// Resolves statement time with the same order as Go: stale TSO, timestamp
/// system variable, then the caller's current time for timestamp=0.
/// 按 Go 相同优先级解析语句时间：stale TSO → timestamp 系统变量 → 调用方 now（timestamp=0）。
/// TSO（Timestamp Oracle）提供分布式时钟；物理时间取自 TSO 高位。
pub fn resolve_statement_timestamp<T>(
    stale_tso: Result<u64, String>,
    timestamp_text: Result<&str, String>,
    now: DateTime<T>,
) -> Result<DateTime<T>, contextutil::errors::SharedError>
where
    T: TimeZone,
{
    // 非零 stale TSO：右移 18 位得到毫秒时间戳。
    match stale_tso {
        Ok(value) if value != 0 => {
            let millis = i64::try_from(value >> 18)
                .map_err(|error| contextutil::errors::New(error.to_string()))?;
            let utc = Utc
                .timestamp_millis_opt(millis)
                .single()
                .ok_or_else(|| contextutil::errors::New("stale TSO timestamp out of range"))?;
            return Ok(utc.with_timezone(&now.timezone()));
        }
        Err(error) => log::error!("get stale tso failed: {error}"),
        _ => {}
    }

    let timestamp_text = timestamp_text.map_err(contextutil::errors::New)?;
    let timestamp = timestamp_text
        .parse::<f64>()
        .map_err(|error| contextutil::errors::New(error.to_string()))?;
    if !timestamp.is_finite() {
        return Err(contextutil::errors::New("timestamp must be finite"));
    }
    // timestamp=0 表示使用真实当前时间。
    if timestamp == 0.0 {
        return Ok(now);
    }

    let mut seconds = timestamp.trunc() as i64;
    let mut nanos = (timestamp.fract() * 1_000_000_000_f64) as i64;
    if nanos < 0 {
        seconds -= 1;
        nanos += 1_000_000_000;
    }
    let utc = Utc
        .timestamp_opt(seconds, nanos as u32)
        .single()
        .ok_or_else(|| contextutil::errors::New("timestamp out of range"))?;
    Ok(utc.with_timezone(&now.timezone()))
}

/// 从会话读取 stale TSO 与 timestamp 变量，解析语句时间；context 为空则用 UTC now。
pub fn getStmtTimestamp<C: SessionContext>(
    context: Option<&C>,
) -> Result<DateTime<Tz>, contextutil::errors::SharedError> {
    let Some(context) = context else {
        return Ok(Utc::now().with_timezone(&chrono_tz::UTC));
    };
    let now = Utc::now().with_timezone(&context.type_context().Location());
    let timestamp = context.timestamp_system_var();
    resolve_statement_timestamp(
        context.stale_tso(),
        timestamp.as_deref().map_err(ToOwned::to_owned),
        now,
    )
}
