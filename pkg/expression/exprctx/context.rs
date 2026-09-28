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

// 表达式构建与求值上下文接口及包装器。
//
// 定义 EvalContext / BuildContext / ExprContext 等会话快照抽象，以及空拒绝检查、
// 常量传播检查、截断错误级别覆盖等装饰器。计划列 ID 分配器保证单调递增。

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use chrono::DateTime;
use chrono_tz::Tz;

use crate::{
    OptionalEvalPropKey, OptionalEvalPropKeySet, OptionalEvalPropProvider, ParamError, ParamValues,
    errctx, mathutil, mysql, types,
};

/// 对应 Go PlanColumnIDAllocator：为计划列分配单调递增 ID。
pub trait PlanColumnIDAllocator {
    fn AllocPlanColumnID(&self) -> i64;
    fn GetLastPlanColumnID(&self) -> i64;
}

/// 对应 Go SimplePlanColumnIDAllocator；AtomicI64 保留并发分配语义。
pub struct SimplePlanColumnIDAllocator {
    id: AtomicI64,
}

/// 创建分配器，并把首次分配前的偏移写入原子计数器。
pub fn NewSimplePlanColumnIDAllocator(offset: i64) -> SimplePlanColumnIDAllocator {
    SimplePlanColumnIDAllocator {
        id: AtomicI64::new(offset),
    }
}

impl PlanColumnIDAllocator for SimplePlanColumnIDAllocator {
    fn AllocPlanColumnID(&self) -> i64 {
        // fetch_add 返回旧值，故加一以对应 Go atomic.Int64.Add 的新值。
        self.id.fetch_add(1, Ordering::SeqCst).wrapping_add(1)
    }

    fn GetLastPlanColumnID(&self) -> i64 {
        self.id.load(Ordering::SeqCst)
    }
}

/// 对应 Go EvalContext：聚合一次表达式求值所需的稳定会话快照与可选属性。
pub trait EvalContext: contextutil::WarnHandler + ParamValues {
    fn CtxID(&self) -> u64;
    fn SQLMode(&self) -> mysql::SQLMode;
    fn TypeCtx(&self) -> types::Context;
    fn ErrCtx(&self) -> errctx::Context;
    fn Location(&self) -> Tz;
    /// 同一 CtxID 的多次调用应返回同一时刻；错误仍通过 Result 传播。
    fn CurrentTime(&self) -> Result<DateTime<Tz>, contextutil::errors::SharedError>;
    fn CurrentDB(&self) -> String;
    fn GetMaxAllowedPacket(&self) -> u64;
    fn GetTiDBRedactLog(&self) -> String;
    fn GetDefaultWeekFormatMode(&self) -> String;
    fn GetDivPrecisionIncrement(&self) -> i32;
    fn GetUserVarsReader(&self) -> &dyn UserVarsReader;
    fn GetOptionalPropSet(&self) -> OptionalEvalPropKeySet;
    fn GetOptionalPropProvider(
        &self,
        key: OptionalEvalPropKey,
    ) -> Option<&dyn OptionalEvalPropProvider>;

    /// Fetches an optional provider without applying decorator-specific access checks.
    /// Decorators override this to let another decorator reach the original context.
    fn GetOptionalPropProviderUnwrapped(
        &self,
        key: OptionalEvalPropKey,
    ) -> Option<&dyn OptionalEvalPropProvider> {
        self.GetOptionalPropProvider(key)
    }
}

/// 对应 Go BuildContext：提供表达式构建参数以及计划缓存、列 ID 等可变决策入口。
pub trait BuildContext {
    /// 允许会话上下文覆盖简单表达式解析；`None` 使用默认解析器。
    fn ParseSQL(
        &self,
        _sql: &str,
    ) -> Option<
        Result<
            (
                Vec<Box<dyn parser_ast::Node>>,
                Vec<parser_crate::errors::Error>,
            ),
            parser_crate::errors::Error,
        >,
    > {
        None
    }
    fn GetEvalCtx(&self) -> &dyn EvalContext;
    fn GetCharsetInfo(&self) -> (String, String);
    fn GetDefaultCollationForUTF8MB4(&self) -> String;
    fn GetBlockEncryptionMode(&self) -> String;
    fn GetSysdateIsNow(&self) -> bool;
    fn GetNoopFuncsMode(&self) -> i32;
    fn Rng(&self) -> &mathutil::MysqlRng;
    fn IsUseCache(&self) -> bool;
    fn SetSkipPlanCache(&self, reason: &str);
    fn AllocPlanColumnID(&self) -> i64;
    fn IsInNullRejectCheck(&self) -> bool;
    fn IsConstantPropagateCheck(&self) -> bool;
    fn ConnectionID(&self) -> u64;
    fn IsReadonlyUserVar(&self, name: &str) -> bool;
}

/// 对应 Go ExprContext，在 BuildContext 上补充窗口和 GROUP_CONCAT 配置。
pub trait ExprContext: BuildContext {
    fn GetWindowingUseHighPrecision(&self) -> bool;
    fn GetGroupConcatMaxLen(&self) -> u64;
}

/// UserVarsReader mirrors the read-only Go interface without coupling expression
/// contexts to the mutable SessionVars implementation.
/// 只读用户变量接口，避免表达式上下文直接依赖可变 SessionVars。
pub trait UserVarsReader: Send + Sync {
    fn GetUserVarVal(&self, name: &str) -> Option<types::Datum>;
    fn GetUserVarType(&self, name: &str) -> Option<types::FieldType>;
    fn Clone(&self) -> Box<dyn UserVarsReader>;
}

/// 空拒绝检查包装器；除目标标志外，其余调用由被包装上下文提供。
/// 空拒绝：在谓词中推断 NULL 不可满足时使用的优化检查模式。
pub struct NullRejectCheckExprContext<'a> {
    pub ExprContext: &'a dyn ExprContext,
}

/// 返回强制 `IsInNullRejectCheck == true` 的包装上下文。
pub fn WithNullRejectCheck(ctx: &dyn ExprContext) -> NullRejectCheckExprContext<'_> {
    NullRejectCheckExprContext { ExprContext: ctx }
}

impl NullRejectCheckExprContext<'_> {
    /// 始终处于空拒绝检查模式。
    pub fn IsInNullRejectCheck(&self) -> bool {
        true
    }
}

impl BuildContext for NullRejectCheckExprContext<'_> {
    fn ParseSQL(
        &self,
        sql: &str,
    ) -> Option<
        Result<
            (
                Vec<Box<dyn parser_ast::Node>>,
                Vec<parser_crate::errors::Error>,
            ),
            parser_crate::errors::Error,
        >,
    > {
        self.ExprContext.ParseSQL(sql)
    }
    fn GetEvalCtx(&self) -> &dyn EvalContext {
        self.ExprContext.GetEvalCtx()
    }
    fn GetCharsetInfo(&self) -> (String, String) {
        self.ExprContext.GetCharsetInfo()
    }
    fn GetDefaultCollationForUTF8MB4(&self) -> String {
        self.ExprContext.GetDefaultCollationForUTF8MB4()
    }
    fn GetBlockEncryptionMode(&self) -> String {
        self.ExprContext.GetBlockEncryptionMode()
    }
    fn GetSysdateIsNow(&self) -> bool {
        self.ExprContext.GetSysdateIsNow()
    }
    fn GetNoopFuncsMode(&self) -> i32 {
        self.ExprContext.GetNoopFuncsMode()
    }
    fn Rng(&self) -> &mathutil::MysqlRng {
        self.ExprContext.Rng()
    }
    fn IsUseCache(&self) -> bool {
        self.ExprContext.IsUseCache()
    }
    fn SetSkipPlanCache(&self, reason: &str) {
        self.ExprContext.SetSkipPlanCache(reason)
    }
    fn AllocPlanColumnID(&self) -> i64 {
        self.ExprContext.AllocPlanColumnID()
    }
    fn IsInNullRejectCheck(&self) -> bool {
        true
    }
    fn IsConstantPropagateCheck(&self) -> bool {
        self.ExprContext.IsConstantPropagateCheck()
    }
    fn ConnectionID(&self) -> u64 {
        self.ExprContext.ConnectionID()
    }
    fn IsReadonlyUserVar(&self, name: &str) -> bool {
        self.ExprContext.IsReadonlyUserVar(name)
    }
}

impl ExprContext for NullRejectCheckExprContext<'_> {
    fn GetWindowingUseHighPrecision(&self) -> bool {
        self.ExprContext.GetWindowingUseHighPrecision()
    }
    fn GetGroupConcatMaxLen(&self) -> u64 {
        self.ExprContext.GetGroupConcatMaxLen()
    }
}

/// 常量传播检查包装器，对应 Go 中仅覆盖 IsConstantPropagateCheck 的嵌入结构体。
/// 常量传播：把等式约束中的常量代入其它表达式以简化计划。
pub struct ConstantPropagateCheckContext<'a> {
    pub ExprContext: &'a dyn ExprContext,
}

/// 返回强制 `IsConstantPropagateCheck == true` 的包装上下文。
pub fn WithConstantPropagateCheck(ctx: &dyn ExprContext) -> ConstantPropagateCheckContext<'_> {
    ConstantPropagateCheckContext { ExprContext: ctx }
}

impl ConstantPropagateCheckContext<'_> {
    /// 始终处于常量传播检查模式。
    pub fn IsConstantPropagateCheck(&self) -> bool {
        true
    }
}

impl BuildContext for ConstantPropagateCheckContext<'_> {
    fn ParseSQL(
        &self,
        sql: &str,
    ) -> Option<
        Result<
            (
                Vec<Box<dyn parser_ast::Node>>,
                Vec<parser_crate::errors::Error>,
            ),
            parser_crate::errors::Error,
        >,
    > {
        self.ExprContext.ParseSQL(sql)
    }
    fn GetEvalCtx(&self) -> &dyn EvalContext {
        self.ExprContext.GetEvalCtx()
    }
    fn GetCharsetInfo(&self) -> (String, String) {
        self.ExprContext.GetCharsetInfo()
    }
    fn GetDefaultCollationForUTF8MB4(&self) -> String {
        self.ExprContext.GetDefaultCollationForUTF8MB4()
    }
    fn GetBlockEncryptionMode(&self) -> String {
        self.ExprContext.GetBlockEncryptionMode()
    }
    fn GetSysdateIsNow(&self) -> bool {
        self.ExprContext.GetSysdateIsNow()
    }
    fn GetNoopFuncsMode(&self) -> i32 {
        self.ExprContext.GetNoopFuncsMode()
    }
    fn Rng(&self) -> &mathutil::MysqlRng {
        self.ExprContext.Rng()
    }
    fn IsUseCache(&self) -> bool {
        self.ExprContext.IsUseCache()
    }
    fn SetSkipPlanCache(&self, reason: &str) {
        self.ExprContext.SetSkipPlanCache(reason)
    }
    fn AllocPlanColumnID(&self) -> i64 {
        self.ExprContext.AllocPlanColumnID()
    }
    fn IsInNullRejectCheck(&self) -> bool {
        self.ExprContext.IsInNullRejectCheck()
    }
    fn IsConstantPropagateCheck(&self) -> bool {
        true
    }
    fn ConnectionID(&self) -> u64 {
        self.ExprContext.ConnectionID()
    }
    fn IsReadonlyUserVar(&self, name: &str) -> bool {
        self.ExprContext.IsReadonlyUserVar(name)
    }
}

impl ExprContext for ConstantPropagateCheckContext<'_> {
    fn GetWindowingUseHighPrecision(&self) -> bool {
        self.ExprContext.GetWindowingUseHighPrecision()
    }
    fn GetGroupConcatMaxLen(&self) -> u64 {
        self.ExprContext.GetGroupConcatMaxLen()
    }
}

/// 仅替换类型与错误上下文；其他 EvalContext 方法语义上转发给 inner。
pub struct InnerOverrideEvalContext<'a> {
    pub inner: &'a dyn EvalContext,
    pub typeCtx: types::Context,
    pub errCtx: errctx::Context,
}

impl contextutil::WarnAppender for InnerOverrideEvalContext<'_> {
    fn AppendWarning(&self, error: contextutil::errors::SharedError) {
        self.inner.AppendWarning(error);
    }

    fn AppendNote(&self, error: contextutil::errors::SharedError) {
        self.inner.AppendNote(error);
    }
}

impl contextutil::WarnHandler for InnerOverrideEvalContext<'_> {
    fn WarningCount(&self) -> usize {
        self.inner.WarningCount()
    }

    fn TruncateWarnings(&self, start: isize) -> Vec<contextutil::SQLWarn> {
        self.inner.TruncateWarnings(start)
    }

    fn CopyWarnings(&self, destination: Vec<contextutil::SQLWarn>) -> Vec<contextutil::SQLWarn> {
        self.inner.CopyWarnings(destination)
    }
}

impl ParamValues for InnerOverrideEvalContext<'_> {
    fn GetParamValue(&self, index: usize) -> Result<types::Datum, ParamError> {
        self.inner.GetParamValue(index)
    }
}

impl EvalContext for InnerOverrideEvalContext<'_> {
    fn CtxID(&self) -> u64 {
        self.inner.CtxID()
    }
    fn SQLMode(&self) -> mysql::SQLMode {
        self.inner.SQLMode()
    }
    fn TypeCtx(&self) -> types::Context {
        self.typeCtx.clone()
    }
    fn ErrCtx(&self) -> errctx::Context {
        self.errCtx.clone()
    }
    fn Location(&self) -> Tz {
        self.inner.Location()
    }
    fn CurrentTime(&self) -> Result<DateTime<Tz>, contextutil::errors::SharedError> {
        self.inner.CurrentTime()
    }
    fn CurrentDB(&self) -> String {
        self.inner.CurrentDB()
    }
    fn GetMaxAllowedPacket(&self) -> u64 {
        self.inner.GetMaxAllowedPacket()
    }
    fn GetTiDBRedactLog(&self) -> String {
        self.inner.GetTiDBRedactLog()
    }
    fn GetDefaultWeekFormatMode(&self) -> String {
        self.inner.GetDefaultWeekFormatMode()
    }
    fn GetDivPrecisionIncrement(&self) -> i32 {
        self.inner.GetDivPrecisionIncrement()
    }
    fn GetUserVarsReader(&self) -> &dyn UserVarsReader {
        self.inner.GetUserVarsReader()
    }
    fn GetOptionalPropSet(&self) -> OptionalEvalPropKeySet {
        self.inner.GetOptionalPropSet()
    }
    fn GetOptionalPropProvider(
        &self,
        key: OptionalEvalPropKey,
    ) -> Option<&dyn OptionalEvalPropProvider> {
        self.inner.GetOptionalPropProvider(key)
    }
}

/// 对应 innerOverrideBuildContext，只把 GetEvalCtx 指向覆盖后的求值上下文。
pub struct InnerOverrideBuildContext<'a> {
    pub inner: &'a dyn BuildContext,
    pub evalCtx: InnerOverrideEvalContext<'a>,
}

impl BuildContext for InnerOverrideBuildContext<'_> {
    fn ParseSQL(
        &self,
        sql: &str,
    ) -> Option<
        Result<
            (
                Vec<Box<dyn parser_ast::Node>>,
                Vec<parser_crate::errors::Error>,
            ),
            parser_crate::errors::Error,
        >,
    > {
        self.inner.ParseSQL(sql)
    }
    fn GetEvalCtx(&self) -> &dyn EvalContext {
        &self.evalCtx
    }
    fn GetCharsetInfo(&self) -> (String, String) {
        self.inner.GetCharsetInfo()
    }
    fn GetDefaultCollationForUTF8MB4(&self) -> String {
        self.inner.GetDefaultCollationForUTF8MB4()
    }
    fn GetBlockEncryptionMode(&self) -> String {
        self.inner.GetBlockEncryptionMode()
    }
    fn GetSysdateIsNow(&self) -> bool {
        self.inner.GetSysdateIsNow()
    }
    fn GetNoopFuncsMode(&self) -> i32 {
        self.inner.GetNoopFuncsMode()
    }
    fn Rng(&self) -> &mathutil::MysqlRng {
        self.inner.Rng()
    }
    fn IsUseCache(&self) -> bool {
        self.inner.IsUseCache()
    }
    fn SetSkipPlanCache(&self, reason: &str) {
        self.inner.SetSkipPlanCache(reason)
    }
    fn AllocPlanColumnID(&self) -> i64 {
        self.inner.AllocPlanColumnID()
    }
    fn IsInNullRejectCheck(&self) -> bool {
        self.inner.IsInNullRejectCheck()
    }
    fn IsConstantPropagateCheck(&self) -> bool {
        self.inner.IsConstantPropagateCheck()
    }
    fn ConnectionID(&self) -> u64 {
        self.inner.ConnectionID()
    }
    fn IsReadonlyUserVar(&self, name: &str) -> bool {
        self.inner.IsReadonlyUserVar(name)
    }
}

/// 按指定级别覆盖截断错误处理。
pub fn CtxWithHandleTruncateErrLevel<'a>(
    ctx: &'a dyn BuildContext,
    level: errctx::Level,
) -> CtxWithTruncateResult<'a> {
    let (truncate_as_warning, ignore_truncate) = match level {
        errctx::Level::LevelWarn => (true, false),
        errctx::Level::LevelIgnore => (false, true),
        _ => (false, false),
    };
    let eval_ctx = ctx.GetEvalCtx();
    let tc = eval_ctx.TypeCtx();
    let ec = eval_ctx.ErrCtx();
    let flags = tc
        .Flags()
        .WithTruncateAsWarning(truncate_as_warning)
        .WithIgnoreTruncateErr(ignore_truncate);

    // 配置没有变化时沿用原上下文，避免创建多层动态包装。
    if tc.Flags() == flags && ec.LevelForGroup(errctx::ErrGroup::ErrGroupTruncate) == level {
        return CtxWithTruncateResult::Original(ctx);
    }
    CtxWithTruncateResult::Overridden(InnerOverrideBuildContext {
        inner: ctx,
        evalCtx: InnerOverrideEvalContext {
            inner: eval_ctx,
            typeCtx: tc.WithFlags(flags),
            errCtx: ec.WithErrGroupLevel(errctx::ErrGroup::ErrGroupTruncate, level),
        },
    })
}

/// Rust 无法让函数同时直接返回借用值和拥有值，枚举保留 Go 的“原对象或新包装器”分支。
pub enum CtxWithTruncateResult<'a> {
    Original(&'a dyn BuildContext),
    Overridden(InnerOverrideBuildContext<'a>),
}

impl CtxWithTruncateResult<'_> {
    /// 取出底层 BuildContext 引用（原上下文或覆盖包装）。
    fn buildContext(&self) -> &dyn BuildContext {
        match self {
            Self::Original(context) => *context,
            Self::Overridden(context) => context,
        }
    }

    /// 是否创建了覆盖包装（配置相对原上下文发生了变化）。
    pub fn WasOverridden(&self) -> bool {
        matches!(self, Self::Overridden(_))
    }
}

impl BuildContext for CtxWithTruncateResult<'_> {
    fn ParseSQL(
        &self,
        sql: &str,
    ) -> Option<
        Result<
            (
                Vec<Box<dyn parser_ast::Node>>,
                Vec<parser_crate::errors::Error>,
            ),
            parser_crate::errors::Error,
        >,
    > {
        self.buildContext().ParseSQL(sql)
    }
    fn GetEvalCtx(&self) -> &dyn EvalContext {
        self.buildContext().GetEvalCtx()
    }
    fn GetCharsetInfo(&self) -> (String, String) {
        self.buildContext().GetCharsetInfo()
    }
    fn GetDefaultCollationForUTF8MB4(&self) -> String {
        self.buildContext().GetDefaultCollationForUTF8MB4()
    }
    fn GetBlockEncryptionMode(&self) -> String {
        self.buildContext().GetBlockEncryptionMode()
    }
    fn GetSysdateIsNow(&self) -> bool {
        self.buildContext().GetSysdateIsNow()
    }
    fn GetNoopFuncsMode(&self) -> i32 {
        self.buildContext().GetNoopFuncsMode()
    }
    fn Rng(&self) -> &mathutil::MysqlRng {
        self.buildContext().Rng()
    }
    fn IsUseCache(&self) -> bool {
        self.buildContext().IsUseCache()
    }
    fn SetSkipPlanCache(&self, reason: &str) {
        self.buildContext().SetSkipPlanCache(reason)
    }
    fn AllocPlanColumnID(&self) -> i64 {
        self.buildContext().AllocPlanColumnID()
    }
    fn IsInNullRejectCheck(&self) -> bool {
        self.buildContext().IsInNullRejectCheck()
    }
    fn IsConstantPropagateCheck(&self) -> bool {
        self.buildContext().IsConstantPropagateCheck()
    }
    fn ConnectionID(&self) -> u64 {
        self.buildContext().ConnectionID()
    }
    fn IsReadonlyUserVar(&self, name: &str) -> bool {
        self.buildContext().IsReadonlyUserVar(name)
    }
}

/// 测试辅助：断言上下文、会话及语句时区字符串完全一致。
pub trait SessionVarsLocation {
    fn SessionLocationName(&self) -> String;
    fn StatementLocationName(&self) -> String;
}

/// 断言求值时区与会话/语句时区名称三者一致。
pub fn AssertLocationWithSessionVars(ctx_loc: &Tz, vars: &dyn SessionVarsLocation) {
    let ctx_loc_str = ctx_loc.to_string();
    let vars_loc_str = vars.SessionLocationName();
    let stmt_loc_str = vars.StatementLocationName();
    assert!(
        ctx_loc_str == vars_loc_str && ctx_loc_str == stmt_loc_str,
        "location mismatch, ctxLoc: {}, varsLoc: {}, stmtLoc: {}",
        ctx_loc_str,
        vars_loc_str,
        stmt_loc_str
    );
}

/// 可转静态的表达式上下文，保留 Go 克隆流程需要的额外读取接口。
pub trait StaticConvertibleExprContext: ExprContext {
    fn GetStaticConvertibleEvalContext(&self) -> &dyn StaticConvertibleEvalContext;
    fn GetPlanCacheTracker(&self) -> &contextutil::plancache::PlanCacheTracker;
    fn GetLastPlanColumnID(&self) -> i64;

    /// Go 的静态化流程复用 RNG 指针；可提供 Arc 的实现覆盖此方法。
    fn GetRngArc(&self) -> Option<Arc<mathutil::MysqlRng>> {
        None
    }

    /// Go 的静态化流程复用 plan-cache tracker 指针；其它实现可走状态复制回退。
    fn GetPlanCacheTrackerArc(&self) -> Option<Arc<contextutil::plancache::PlanCacheTracker>> {
        None
    }
}

/// 可转静态的求值上下文，可导出全部参数和警告处理器。
pub trait StaticConvertibleEvalContext: EvalContext {
    fn AllParamValues(&self) -> Vec<types::Datum>;
    fn GetWarnHandler(&self) -> &dyn contextutil::WarnHandler;

    /// Go 的 WarnHandler 是可共享接口值。能够提供所有权的实现应返回同一处理器，
    /// 以便静态上下文继续与原上下文共享告警；其它实现保留复制回退路径。
    fn GetWarnHandlerArc(&self) -> Option<Arc<dyn contextutil::WarnHandler + Send + Sync>> {
        None
    }
}
