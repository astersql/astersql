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

// 静态求值上下文（EvalContext）实现。
//
// 对应 Go 的静态 `EvalContext`：承载 SQL Mode、类型/错误上下文、时区、
// 当前库、语句时间（NOW）、告警、预处理参数、用户变量与可选属性等。
// 可通过 `Apply` / `LoadSystemVars` 派生新上下文，并支持 `MakeEvalContextStatic` 快照。

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, TimeZone, Utc};
use chrono_tz::{Tz, UTC};

use crate::{contextutil, errctx, exprctx, expropt, mysql, types, vardef, variable};

type SharedError = contextutil::errors::SharedError;
/// 取得“当前语句时间”的回调；成功结果会被 `TimeOnce` 缓存。
type CurrentTimeFn = Arc<dyn Fn() -> Result<DateTime<Tz>, SharedError> + Send + Sync>;
type SharedWarnHandler = Arc<dyn contextutil::WarnHandler + Send + Sync>;

/// `timeOnce`：仅缓存首次成功取得的语句时间；失败不会污染下一次重试。
struct TimeOnce {
    lock: Mutex<()>,
    time: OnceLock<DateTime<Tz>>,
    time_fn: Option<CurrentTimeFn>,
}

impl TimeOnce {
    /// 无回调：首次取时用墙钟时间。
    fn empty() -> Self {
        Self {
            lock: Mutex::new(()),
            time: OnceLock::new(),
            time_fn: None,
        }
    }

    /// 绑定外部时间回调（测试或固定 TIMESTAMP 系统变量）。
    fn with_fn(time_fn: CurrentTimeFn) -> Self {
        Self {
            lock: Mutex::new(()),
            time: OnceLock::new(),
            time_fn: Some(time_fn),
        }
    }

    /// 双重检查锁定：成功结果写入 OnceLock；失败直接返回以便重试。
    fn get_time(&self, location: Tz) -> Result<DateTime<Tz>, SharedError> {
        if let Some(value) = self.time.get() {
            return Ok(*value);
        }

        let _guard = self.lock.lock().expect("timeOnce mutex poisoned");
        if let Some(value) = self.time.get() {
            return Ok(*value);
        }

        let value = match &self.time_fn {
            Some(time_fn) => time_fn()?,
            None => Utc::now().with_timezone(&UTC),
        };
        // 按上下文时区规范化后再缓存。
        let value = value.with_timezone(&location);
        let _ = self.time.set(value);
        Ok(value)
    }
}

/// 让 types/errctx 持有同一告警处理器，而不是复制告警状态。
struct WarnHandlerAdapter {
    handler: SharedWarnHandler,
}

impl contextutil::WarnAppender for WarnHandlerAdapter {
    fn AppendWarning(&self, error: SharedError) {
        self.handler.AppendWarning(error);
    }

    fn AppendNote(&self, error: SharedError) {
        self.handler.AppendNote(error);
    }
}

/// 默认空用户变量读取器：任何名字都返回 None。
struct EmptyUserVarsReader;

impl exprctx::UserVarsReader for EmptyUserVarsReader {
    fn GetUserVarVal(&self, _name: &str) -> Option<types::Datum> {
        None
    }

    fn GetUserVarType(&self, _name: &str) -> Option<types::FieldType> {
        None
    }

    fn Clone(&self) -> Box<dyn exprctx::UserVarsReader> {
        Box::new(Self)
    }
}

/// 求值上下文内部可变状态；Clone 时共享告警处理器与语句时间等 Arc 资源。
pub struct EvalCtxState {
    warn_handler: SharedWarnHandler,
    sql_mode: mysql::SQLMode,
    type_ctx: types::Context,
    err_ctx: errctx::Context,
    current_db: String,
    current_time: Arc<TimeOnce>,
    max_allowed_packet: u64,
    enable_redact_log: String,
    default_week_format_mode: String,
    div_precision_increment: i32,
    param_list: Vec<types::Datum>,
    user_vars: Box<dyn exprctx::UserVarsReader>,
    props: Arc<expropt::OptionalEvalPropProviders>,
}

impl Clone for EvalCtxState {
    fn clone(&self) -> Self {
        Self {
            warn_handler: Arc::clone(&self.warn_handler),
            sql_mode: self.sql_mode,
            type_ctx: self.type_ctx.clone(),
            err_ctx: self.err_ctx.clone(),
            current_db: self.current_db.clone(),
            current_time: Arc::clone(&self.current_time),
            max_allowed_packet: self.max_allowed_packet,
            enable_redact_log: self.enable_redact_log.clone(),
            default_week_format_mode: self.default_week_format_mode.clone(),
            div_precision_increment: self.div_precision_increment,
            param_list: self.param_list.clone(),
            user_vars: self.user_vars.Clone(),
            props: Arc::clone(&self.props),
        }
    }
}

/// 构造时选项：一次性闭包，写入 `EvalCtxState`。
pub type EvalCtxOption = Box<dyn FnOnce(&mut EvalCtxState)>;

/// 覆盖告警处理器（与 TypeCtx/ErrCtx 共享同一 appender）。
pub fn WithWarnHandler(handler: SharedWarnHandler) -> EvalCtxOption {
    Box::new(move |state| state.warn_handler = handler)
}

/// 设置 SQL Mode（影响严格模式、日期合法性等）。
pub fn WithSQLMode(sql_mode: mysql::SQLMode) -> EvalCtxOption {
    Box::new(move |state| state.sql_mode = sql_mode)
}

/// 设置类型转换标志（Flags）。
pub fn WithTypeFlags(flags: types::Flags) -> EvalCtxOption {
    Box::new(move |state| state.type_ctx = state.type_ctx.WithFlags(flags))
}

/// 设置会话时区（影响 DATETIME/TIMESTAMP 解释）。
pub fn WithLocation(location: Tz) -> EvalCtxOption {
    Box::new(move |state| state.type_ctx = state.type_ctx.WithLocation(location))
}

/// 设置错误组级别映射（Error/Warn/Ignore）。
pub fn WithErrLevelMap(levels: errctx::LevelMap) -> EvalCtxOption {
    Box::new(move |state| state.err_ctx = state.err_ctx.WithErrGroupLevels(levels))
}

/// 设置当前数据库名。
pub fn WithCurrentDB(database: String) -> EvalCtxOption {
    Box::new(move |state| state.current_db = database)
}

/// 绑定“当前语句时间”回调（对应 NOW()/CURRENT_TIMESTAMP）。
pub fn WithCurrentTime(time_fn: CurrentTimeFn) -> EvalCtxOption {
    Box::new(move |state| state.current_time = Arc::new(TimeOnce::with_fn(time_fn)))
}

/// 设置 `max_allowed_packet`。
pub fn WithMaxAllowedPacket(size: u64) -> EvalCtxOption {
    Box::new(move |state| state.max_allowed_packet = size)
}

/// 设置 `default_week_format`（WEEK 函数模式）。
pub fn WithDefaultWeekFormatMode(mode: String) -> EvalCtxOption {
    Box::new(move |state| state.default_week_format_mode = mode)
}

/// 设置 `div_precision_increment`（除法小数位增量）。
pub fn WithDivPrecisionIncrement(increment: i32) -> EvalCtxOption {
    Box::new(move |state| state.div_precision_increment = increment)
}

/// 整体替换可选求值属性 Provider 集合。
pub fn WithOptionalProperty(
    providers: Vec<Box<dyn exprctx::OptionalEvalPropProvider>>,
) -> EvalCtxOption {
    Box::new(move |state| {
        let mut registry = expropt::OptionalEvalPropProviders::new();
        for provider in providers {
            registry.add(provider);
        }
        state.props = Arc::new(registry);
    })
}

/// 设置预处理语句参数列表（`?` 占位符对应 Datum）。
pub fn WithParamList(params: Vec<types::Datum>) -> EvalCtxOption {
    Box::new(move |state| state.param_list = params)
}

/// 设置 `tidb_redact_log`（日志脱敏开关）。
pub fn WithEnableRedactLog(value: String) -> EvalCtxOption {
    Box::new(move |state| state.enable_redact_log = value)
}

/// 设置用户变量读取器。
pub fn WithUserVarsReader(vars: Box<dyn exprctx::UserVarsReader>) -> EvalCtxOption {
    Box::new(move |state| state.user_vars = vars)
}

/// 懒加载并缓存服务器默认 SQL Mode。
fn default_sql_mode() -> mysql::SQLMode {
    static MODE: OnceLock<mysql::SQLMode> = OnceLock::new();
    *MODE.get_or_init(|| {
        mysql::GetSQLMode(&mysql::FormatSQLModeStr(mysql::DefaultSQLMode))
            .expect("the server default SQL mode must be valid")
    })
}

/// 静态求值上下文：带唯一 CtxID 的不可变外观，内部状态可共享。
pub struct EvalContext {
    id: u64,
    state: EvalCtxState,
}

/// 将 WarnHandler 适配为 types/errctx 使用的 WarnAppender。
fn appender_for(handler: &SharedWarnHandler) -> Arc<dyn contextutil::WarnAppender + Send + Sync> {
    Arc::new(WarnHandlerAdapter {
        handler: Arc::clone(handler),
    })
}

/// 在选项应用后，让 TypeCtx/ErrCtx 重新绑定到当前告警处理器。
fn bind_dependent_contexts(state: &mut EvalCtxState) {
    let flags = state.type_ctx.Flags();
    let location = state.type_ctx.Location();
    let levels = state.err_ctx.LevelMap();
    state.type_ctx = types::NewContext(flags, location, appender_for(&state.warn_handler));
    state.err_ctx = errctx::NewContextWithLevels(levels, appender_for(&state.warn_handler));
}

/// 以默认值创建求值上下文，再依次应用选项并绑定依赖上下文。
pub fn NewEvalContext(options: Vec<EvalCtxOption>) -> EvalContext {
    let warning_handler: SharedWarnHandler = Arc::new(contextutil::NewStaticWarnHandler(0));
    let appender = appender_for(&warning_handler);
    let mut state = EvalCtxState {
        warn_handler: warning_handler,
        sql_mode: default_sql_mode(),
        type_ctx: types::NewContext(types::StrictFlags, UTC, Arc::clone(&appender)),
        err_ctx: errctx::NewContext(appender),
        current_db: String::new(),
        current_time: Arc::new(TimeOnce::empty()),
        max_allowed_packet: vardef::DefMaxAllowedPacket,
        enable_redact_log: vardef::DefTiDBRedactLog.to_owned(),
        default_week_format_mode: vardef::DefDefaultWeekFormat.to_owned(),
        div_precision_increment: vardef::DefDivPrecisionIncrement as i32,
        param_list: Vec::new(),
        user_vars: Box::new(EmptyUserVarsReader),
        props: Arc::new(expropt::OptionalEvalPropProviders::new()),
    };
    for option in options {
        option(&mut state);
    }
    bind_dependent_contexts(&mut state);
    EvalContext {
        id: contextutil::context::GenContextID(),
        state,
    }
}

impl EvalContext {
    /// 上下文唯一 ID；每次 `New`/`Apply` 都会重新分配。
    pub fn CtxID(&self) -> u64 {
        self.id
    }

    /// 当前 SQL Mode。
    pub fn SQLMode(&self) -> mysql::SQLMode {
        self.state.sql_mode
    }

    /// 类型求值上下文（含 Flags、时区、告警 appender）。
    pub fn TypeCtx(&self) -> types::Context {
        self.state.type_ctx.clone()
    }

    /// 错误级别上下文。
    pub fn ErrCtx(&self) -> errctx::Context {
        self.state.err_ctx.clone()
    }

    /// 会话时区。
    pub fn Location(&self) -> Tz {
        self.state.type_ctx.Location()
    }

    /// 追加 Warning 级别告警。
    pub fn AppendWarning(&self, error: SharedError) {
        self.state.warn_handler.AppendWarning(error);
    }

    /// 追加 Note 级别提示。
    pub fn AppendNote(&self, error: SharedError) {
        self.state.warn_handler.AppendNote(error);
    }

    /// 当前告警条数。
    pub fn WarningCount(&self) -> usize {
        self.state.warn_handler.WarningCount()
    }

    /// 截断并返回从 `start` 起的告警，原列表保留前缀。
    pub fn TruncateWarnings(&self, start: isize) -> Vec<contextutil::SQLWarn> {
        self.state.warn_handler.TruncateWarnings(start)
    }

    /// 复制告警列表到目标缓冲（语义对齐 Go CopyWarnings）。
    pub fn CopyWarnings(
        &self,
        destination: Vec<contextutil::SQLWarn>,
    ) -> Vec<contextutil::SQLWarn> {
        self.state.warn_handler.CopyWarnings(destination)
    }

    /// 当前数据库名。
    pub fn CurrentDB(&self) -> String {
        self.state.current_db.clone()
    }

    /// 取得（并可能缓存）本语句的当前时间。
    pub fn CurrentTime(&self) -> Result<DateTime<Tz>, SharedError> {
        self.state.current_time.get_time(self.Location())
    }

    /// `max_allowed_packet`。
    pub fn GetMaxAllowedPacket(&self) -> u64 {
        self.state.max_allowed_packet
    }

    /// `tidb_redact_log` 取值。
    pub fn GetTiDBRedactLog(&self) -> String {
        self.state.enable_redact_log.clone()
    }

    /// `default_week_format`。
    pub fn GetDefaultWeekFormatMode(&self) -> String {
        self.state.default_week_format_mode.clone()
    }

    /// `div_precision_increment`。
    pub fn GetDivPrecisionIncrement(&self) -> i32 {
        self.state.div_precision_increment
    }

    /// 用户变量读取器。
    pub fn GetUserVarsReader(&self) -> &dyn exprctx::UserVarsReader {
        self.state.user_vars.as_ref()
    }

    /// 已注册可选属性的 key 集合。
    pub fn GetOptionalPropSet(&self) -> exprctx::OptionalEvalPropKeySet {
        self.state.props.prop_key_set()
    }

    /// 按 key 取可选属性 Provider。
    pub fn GetOptionalPropProvider(
        &self,
        key: exprctx::OptionalEvalPropKey,
    ) -> Option<&dyn exprctx::OptionalEvalPropProvider> {
        self.state.props.get(key)
    }

    /// 克隆状态并应用新选项，分配新 CtxID；默认继承原语句时间。
    pub fn Apply(&self, options: Vec<EvalCtxOption>) -> EvalContext {
        let mut state = self.state.clone();
        // 未显式覆盖时，新上下文通过闭包复用旧 TimeOnce 的成功缓存。
        let previous_time = Arc::clone(&self.state.current_time);
        let previous_location = self.Location();
        state.current_time = Arc::new(TimeOnce::with_fn(Arc::new(move || {
            previous_time.get_time(previous_location)
        })));

        let flags = self.state.type_ctx.Flags();
        let location = self.Location();
        let levels = self.state.err_ctx.LevelMap();
        state.type_ctx = types::NewContext(flags, location, appender_for(&state.warn_handler));
        state.err_ctx = errctx::NewContextWithLevels(levels, appender_for(&state.warn_handler));
        for option in options {
            option(&mut state);
        }
        bind_dependent_contexts(&mut state);

        EvalContext {
            id: contextutil::context::GenContextID(),
            state,
        }
    }

    /// 按索引取预处理参数；越界返回 `ErrParamIndexExceedParamCounts`。
    pub fn GetParamValue(&self, index: usize) -> Result<types::Datum, exprctx::ParamError> {
        self.state
            .param_list
            .get(index)
            .cloned()
            .ok_or(exprctx::ErrParamIndexExceedParamCounts)
    }

    /// 返回全部预处理参数的拷贝。
    pub fn AllParamValues(&self) -> Vec<types::Datum> {
        self.state.param_list.clone()
    }

    /// 告警处理器引用。
    pub fn GetWarnHandler(&self) -> &dyn contextutil::WarnHandler {
        self.state.warn_handler.as_ref()
    }

    pub(crate) fn WarnHandlerArc(&self) -> SharedWarnHandler {
        Arc::clone(&self.state.warn_handler)
    }

    pub(crate) fn WarnAppenderArc(&self) -> Arc<dyn contextutil::WarnAppender + Send + Sync> {
        appender_for(&self.state.warn_handler)
    }

    /// 解析系统变量映射并 Apply 到新求值上下文。
    pub fn LoadSystemVars(
        &self,
        sys_vars: &HashMap<String, String>,
    ) -> Result<EvalContext, SharedError> {
        let parsed = parse_system_vars(sys_vars)?;
        Ok(self.load_system_vars_internal(&parsed, sys_vars))
    }

    /// 仅对求值层相关变量生成选项并 Apply（字符集等留给 ExprContext）。
    pub(crate) fn load_system_vars_internal(
        &self,
        parsed: &ParsedSystemVars,
        sys_vars: &HashMap<String, String>,
    ) -> EvalContext {
        let mut options = Vec::with_capacity(8);
        for name in sys_vars.keys() {
            match name.to_ascii_lowercase().as_str() {
                vardef::TimeZone => options.push(WithLocation(parsed.location)),
                vardef::SQLModeVar => options.push(WithSQLMode(parsed.sql_mode)),
                vardef::Timestamp => {
                    if sys_vars
                        .get(name)
                        .is_some_and(|value| value == vardef::DefTimestamp)
                    {
                        options.push(WithCurrentTime(Arc::new(|| {
                            Ok(Utc::now().with_timezone(&UTC))
                        })));
                    } else {
                        let current_time = parsed.current_time;
                        options.push(WithCurrentTime(Arc::new(move || Ok(current_time))));
                    }
                }
                vardef::MaxAllowedPacket => {
                    options.push(WithMaxAllowedPacket(parsed.max_allowed_packet));
                }
                vardef::TiDBRedactLog => {
                    options.push(WithEnableRedactLog(parsed.enable_redact_log.clone()));
                }
                vardef::DefaultWeekFormat => options.push(WithDefaultWeekFormatMode(
                    parsed.default_week_format_mode.clone(),
                )),
                vardef::DivPrecisionIncrement => {
                    options.push(WithDivPrecisionIncrement(parsed.div_precision_increment))
                }
                _ => {}
            }
        }
        self.Apply(options)
    }
}

impl contextutil::WarnAppender for EvalContext {
    fn AppendWarning(&self, error: SharedError) {
        EvalContext::AppendWarning(self, error);
    }

    fn AppendNote(&self, error: SharedError) {
        EvalContext::AppendNote(self, error);
    }
}

impl contextutil::WarnHandler for EvalContext {
    fn WarningCount(&self) -> usize {
        EvalContext::WarningCount(self)
    }

    fn TruncateWarnings(&self, start: isize) -> Vec<contextutil::SQLWarn> {
        EvalContext::TruncateWarnings(self, start)
    }

    fn CopyWarnings(&self, destination: Vec<contextutil::SQLWarn>) -> Vec<contextutil::SQLWarn> {
        EvalContext::CopyWarnings(self, destination)
    }
}

impl exprctx::ParamValues for EvalContext {
    fn GetParamValue(&self, index: usize) -> Result<types::Datum, exprctx::ParamError> {
        EvalContext::GetParamValue(self, index)
    }
}

impl exprctx::EvalContext for EvalContext {
    fn CtxID(&self) -> u64 {
        EvalContext::CtxID(self)
    }

    fn SQLMode(&self) -> mysql::SQLMode {
        EvalContext::SQLMode(self)
    }

    fn TypeCtx(&self) -> types::Context {
        EvalContext::TypeCtx(self)
    }

    fn ErrCtx(&self) -> errctx::Context {
        EvalContext::ErrCtx(self)
    }

    fn Location(&self) -> Tz {
        EvalContext::Location(self)
    }

    fn CurrentTime(&self) -> Result<DateTime<Tz>, SharedError> {
        EvalContext::CurrentTime(self)
    }

    fn CurrentDB(&self) -> String {
        EvalContext::CurrentDB(self)
    }

    fn GetMaxAllowedPacket(&self) -> u64 {
        EvalContext::GetMaxAllowedPacket(self)
    }

    fn GetTiDBRedactLog(&self) -> String {
        EvalContext::GetTiDBRedactLog(self)
    }

    fn GetDefaultWeekFormatMode(&self) -> String {
        EvalContext::GetDefaultWeekFormatMode(self)
    }

    fn GetDivPrecisionIncrement(&self) -> i32 {
        EvalContext::GetDivPrecisionIncrement(self)
    }

    fn GetUserVarsReader(&self) -> &dyn exprctx::UserVarsReader {
        EvalContext::GetUserVarsReader(self)
    }

    fn GetOptionalPropSet(&self) -> exprctx::OptionalEvalPropKeySet {
        EvalContext::GetOptionalPropSet(self)
    }

    fn GetOptionalPropProvider(
        &self,
        key: exprctx::OptionalEvalPropKey,
    ) -> Option<&dyn exprctx::OptionalEvalPropProvider> {
        EvalContext::GetOptionalPropProvider(self, key)
    }
}

impl exprctx::StaticConvertibleEvalContext for EvalContext {
    fn AllParamValues(&self) -> Vec<types::Datum> {
        EvalContext::AllParamValues(self)
    }

    fn GetWarnHandler(&self) -> &dyn contextutil::WarnHandler {
        EvalContext::GetWarnHandler(self)
    }

    fn GetWarnHandlerArc(&self) -> Option<SharedWarnHandler> {
        Some(Arc::clone(&self.state.warn_handler))
    }
}

/// 从任意 `StaticConvertibleEvalContext` 物化为独立静态 `EvalContext` 快照。
/// 可选属性置空；告警处理器尽量复用原 Arc。
pub fn MakeEvalContextStatic(context: &dyn exprctx::StaticConvertibleEvalContext) -> EvalContext {
    let type_context = context.TypeCtx();
    let error_context = context.ErrCtx();
    let current_time = context.CurrentTime();
    let current_time_fn: CurrentTimeFn = Arc::new(move || current_time.clone());
    let warning_handler: SharedWarnHandler = context.GetWarnHandlerArc().unwrap_or_else(|| {
        Arc::new(contextutil::NewStaticWarnHandlerWithHandler(Some(
            context.GetWarnHandler(),
        )))
    });

    NewEvalContext(vec![
        WithWarnHandler(warning_handler),
        WithSQLMode(context.SQLMode()),
        WithTypeFlags(type_context.Flags()),
        WithLocation(type_context.Location()),
        WithErrLevelMap(error_context.LevelMap()),
        WithCurrentDB(context.CurrentDB()),
        WithCurrentTime(current_time_fn),
        WithMaxAllowedPacket(context.GetMaxAllowedPacket()),
        WithDefaultWeekFormatMode(context.GetDefaultWeekFormatMode()),
        WithDivPrecisionIncrement(context.GetDivPrecisionIncrement()),
        WithParamList(context.AllParamValues()),
        WithUserVarsReader(context.GetUserVarsReader().Clone()),
        WithOptionalProperty(Vec::new()),
        WithEnableRedactLog(context.GetTiDBRedactLog()),
    ])
}

/// `LoadSystemVars` 解析后的中间结果，同时服务 Eval/Expr 两层。
#[derive(Clone)]
pub(crate) struct ParsedSystemVars {
    pub location: Tz,
    pub sql_mode: mysql::SQLMode,
    pub current_time: DateTime<Tz>,
    pub max_allowed_packet: u64,
    pub enable_redact_log: String,
    pub default_week_format_mode: String,
    pub div_precision_increment: i32,
    pub charset: String,
    pub collation: String,
    pub default_collation_for_utf8mb4: String,
    pub block_encryption_mode: String,
    pub sysdate_is_now: bool,
    pub noop_funcs_mode: i32,
    pub windowing_use_high_precision: bool,
    pub group_concat_max_len: u64,
}

fn invalid_system_var(name: &str, value: &str) -> SharedError {
    contextutil::errors::NewNoStackError(format!(
        "invalid value '{value}' for system variable '{name}'"
    ))
}

/// 为静态上下文校验无关系统变量提供只读默认值访问器。
struct StaticGlobalVarAccessor;

impl variable::GlobalVarAccessor for StaticGlobalVarAccessor {
    fn get_global_sys_var(&self, name: &str) -> Result<String, variable::VariableError> {
        variable::GetSysVar(name)
            .map(|system_variable| system_variable.Value.clone())
            .ok_or_else(|| variable::VariableError::unknown(name))
    }

    fn set_global_sys_var_only(
        &mut self,
        _ctx: &variable::Context,
        name: &str,
        _value: &str,
        _update_local: bool,
    ) -> Result<(), variable::VariableError> {
        variable::GetSysVar(name)
            .map(|_| ())
            .ok_or_else(|| variable::VariableError::unknown(name))
    }

    fn get_tidb_table_value(&self, name: &str) -> Result<String, variable::VariableError> {
        Err(variable::VariableError::unknown(name))
    }

    fn set_tidb_table_value(
        &mut self,
        name: &str,
        _value: &str,
        _comment: &str,
    ) -> Result<(), variable::VariableError> {
        Err(variable::VariableError::unknown(name))
    }
}

/// 解析 ON/OFF/1/0 布尔系统变量。
fn parse_bool(name: &str, value: &str) -> Result<bool, SharedError> {
    if value == "1" || value.eq_ignore_ascii_case(vardef::On) {
        Ok(true)
    } else if value == "0" || value.eq_ignore_ascii_case(vardef::Off) {
        Ok(false)
    } else {
        Err(invalid_system_var(name, value))
    }
}

/// 按 Go SysVar 的 TypeUnsigned 规则解析并裁剪到给定范围。
fn parse_clamped_unsigned(
    name: &str,
    value: &str,
    minimum: u64,
    maximum: u64,
) -> Result<u64, SharedError> {
    if value.starts_with('-') {
        value
            .parse::<i64>()
            .map_err(|_| invalid_system_var(name, value))?;
        return Ok(minimum);
    }
    value
        .parse::<u64>()
        .map(|parsed| parsed.clamp(minimum, maximum))
        .map_err(|_| invalid_system_var(name, value))
}

/// 按 Go SysVar 的 TypeEnum 规则接受枚举名称（忽略大小写）或零基索引。
fn normalize_enum(name: &str, value: &str, possible: &[&str]) -> Result<String, SharedError> {
    possible
        .iter()
        .enumerate()
        .find(|(index, candidate)| {
            candidate.eq_ignore_ascii_case(value) || index.to_string() == value
        })
        .map(|(_, candidate)| (*candidate).to_owned())
        .ok_or_else(|| invalid_system_var(name, value))
}

/// 解析 `timestamp` 系统变量：默认值表示“当前墙钟”，否则为带小数秒的 Unix 时间。
fn parse_timestamp(value: &str, location: Tz) -> Result<DateTime<Tz>, SharedError> {
    if value == vardef::DefTimestamp {
        return Ok(Utc::now().with_timezone(&location));
    }
    let timestamp = value
        .parse::<f64>()
        .map_err(|_| invalid_system_var(vardef::Timestamp, value))?;
    if !timestamp.is_finite() {
        return Err(invalid_system_var(vardef::Timestamp, value));
    }
    let mut seconds = timestamp.trunc() as i64;
    let mut nanos = (timestamp.fract() * 1_000_000_000_f64) as i64;
    if nanos < 0 {
        seconds -= 1;
        nanos += 1_000_000_000;
    }
    UTC.timestamp_opt(seconds, nanos as u32)
        .single()
        .map(|time| time.with_timezone(&location))
        .ok_or_else(|| invalid_system_var(vardef::Timestamp, value))
}

/// 解析系统变量表；未知变量名报错。字符集/校对可分步覆盖。
pub(crate) fn parse_system_vars(
    vars: &HashMap<String, String>,
) -> Result<ParsedSystemVars, SharedError> {
    let mut validation_session = variable::SessionVars::new(Box::new(StaticGlobalVarAccessor));
    let mut parsed = ParsedSystemVars {
        location: UTC,
        sql_mode: default_sql_mode(),
        current_time: Utc::now().with_timezone(&UTC),
        max_allowed_packet: vardef::DefMaxAllowedPacket,
        enable_redact_log: vardef::DefTiDBRedactLog.to_owned(),
        default_week_format_mode: vardef::DefDefaultWeekFormat.to_owned(),
        div_precision_increment: vardef::DefDivPrecisionIncrement as i32,
        charset: mysql::DefaultCharset.to_owned(),
        collation: mysql::DefaultCollationName.to_owned(),
        default_collation_for_utf8mb4: mysql::DefaultCollationName.to_owned(),
        block_encryption_mode: vardef::DefBlockEncryptionMode.to_owned(),
        sysdate_is_now: vardef::DefSysdateIsNow,
        noop_funcs_mode: variable::TiDBOptOnOffWarn(vardef::DefTiDBEnableNoopFuncs),
        windowing_use_high_precision: true,
        group_concat_max_len: vardef::DefGroupConcatMaxLen,
    };
    let mut requested_charset = None;
    let mut requested_collation = None;
    let mut timestamp_value = None;

    for (original_name, value) in vars {
        let name = original_name.to_ascii_lowercase();
        match name.as_str() {
            vardef::TimeZone => {
                parsed.location =
                    Tz::from_str(value).map_err(|_| invalid_system_var(&name, value))?;
            }
            vardef::SQLModeVar => {
                let normalized = mysql::FormatSQLModeStr(value);
                parsed.sql_mode =
                    mysql::GetSQLMode(&normalized).map_err(|_| invalid_system_var(&name, value))?;
            }
            // TIMESTAMP 依赖最终时区，延后解析。
            vardef::Timestamp => timestamp_value = Some(value.clone()),
            vardef::MaxAllowedPacket => {
                let packet_size = parse_clamped_unsigned(
                    &name,
                    value,
                    1024,
                    vardef::MaxOfMaxAllowedPacket.Load(),
                )?;
                parsed.max_allowed_packet = packet_size - packet_size % 1024;
            }
            // Go 的 exprstatic 特例直接调用 SetGlobalFromHook，不做枚举归一化。
            vardef::TiDBRedactLog => parsed.enable_redact_log = value.clone(),
            vardef::DefaultWeekFormat => {
                let mode = parse_clamped_unsigned(&name, value, 0, 7)?;
                parsed.default_week_format_mode = mode.to_string();
            }
            vardef::DivPrecisionIncrement => {
                parsed.div_precision_increment =
                    parse_clamped_unsigned(&name, value, 0, 30)? as i32;
            }
            vardef::CharacterSetConnection => requested_charset = Some(value.clone()),
            vardef::CollationConnection => requested_collation = Some(value.clone()),
            vardef::DefaultCollationForUTF8MB4 => {
                let collation = crate::charset::GetCollationByName(value)
                    .map_err(|_| invalid_system_var(&name, value))?;
                if collation.CharsetName != "utf8mb4" {
                    return Err(invalid_system_var(&name, value));
                }
                parsed.default_collation_for_utf8mb4 = collation.Name;
            }
            vardef::BlockEncryptionMode => {
                parsed.block_encryption_mode = normalize_enum(
                    &name,
                    value,
                    &[
                        "aes-128-ecb",
                        "aes-192-ecb",
                        "aes-256-ecb",
                        "aes-128-cbc",
                        "aes-192-cbc",
                        "aes-256-cbc",
                        "aes-128-ofb",
                        "aes-192-ofb",
                        "aes-256-ofb",
                        "aes-128-cfb",
                        "aes-192-cfb",
                        "aes-256-cfb",
                    ],
                )?;
            }
            vardef::TiDBSysdateIsNow => parsed.sysdate_is_now = parse_bool(&name, value)?,
            vardef::TiDBEnableNoopFuncs => {
                let normalized =
                    normalize_enum(&name, value, &[vardef::Off, vardef::On, vardef::Warn])?;
                parsed.noop_funcs_mode = variable::TiDBOptOnOffWarn(&normalized);
            }
            vardef::WindowingUseHighPrecision => {
                parsed.windowing_use_high_precision = parse_bool(&name, value)?;
            }
            vardef::GroupConcatMaxLen => {
                parsed.group_concat_max_len = parse_clamped_unsigned(&name, value, 4, u64::MAX)?;
            }
            _ => {
                validation_session
                    .SetSystemVar(original_name, value)
                    .map_err(|error| contextutil::errors::NewNoStackError(error.to_string()))?;
            }
        }
    }

    // 字符集默认校对，再被显式 collation_connection 覆盖。
    if let Some(charset_name) = requested_charset {
        let charset = crate::charset::GetCharsetInfo(&charset_name)
            .map_err(|_| invalid_system_var(vardef::CharacterSetConnection, &charset_name))?;
        parsed.charset = charset.Name;
        parsed.collation = charset.DefaultCollation;
    }
    if let Some(collation_name) = requested_collation {
        let collation = crate::charset::GetCollationByName(&collation_name)
            .map_err(|_| invalid_system_var(vardef::CollationConnection, &collation_name))?;
        parsed.charset = collation.CharsetName;
        parsed.collation = collation.Name;
    }
    if let Some(value) = timestamp_value {
        parsed.current_time = parse_timestamp(&value, parsed.location)?;
    }

    Ok(parsed)
}
