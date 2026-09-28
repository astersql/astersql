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

// 静态表达式构建上下文（ExprContext）实现。
//
// 在 `EvalContext` 之上叠加字符集/校对、加密模式、noop 函数策略、
// 随机数、计划缓存跟踪、列 ID 分配与连接 ID 等构建期状态。
// 对应 Go 静态 `ExprContext`，支持 `Apply`、`LoadSystemVars` 与 `MakeExprContextStatic`。

use std::collections::HashMap;
use std::sync::Arc;

use crate::{
    EvalContext, MakeEvalContextStatic, NewEvalContext, ParsedSystemVars, charset, contextutil,
    exprctx, mathutil, mysql, parse_system_vars, vardef, variable,
};

/// 表达式上下文内部状态；Clone 时共享 RNG、计划缓存 tracker、列 ID 分配器等。
#[derive(Clone)]
pub struct ExprCtxState {
    eval_ctx: Arc<EvalContext>,
    charset: String,
    collation: String,
    default_collation_for_utf8mb4: String,
    block_encryption_mode: String,
    sysdate_is_now: bool,
    noop_funcs_mode: i32,
    rng: Arc<mathutil::MysqlRng>,
    plan_cache_tracker: Arc<contextutil::plancache::PlanCacheTracker>,
    column_id_allocator: Arc<dyn exprctx::PlanColumnIDAllocator>,
    connection_id: u64,
    windowing_use_high_precision: bool,
    group_concat_max_len: u64,
}

/// 构造时选项：一次性闭包写入 `ExprCtxState`。
pub type ExprCtxOption = Box<dyn FnOnce(&mut ExprCtxState)>;

/// 替换内嵌的求值上下文。
pub fn WithEvalCtx(context: Arc<EvalContext>) -> ExprCtxOption {
    Box::new(move |state| state.eval_ctx = context)
}

/// 设置连接字符集与校对规则。
pub fn WithCharset(charset: String, collation: String) -> ExprCtxOption {
    Box::new(move |state| {
        state.charset = charset;
        state.collation = collation;
    })
}

/// 设置 `default_collation_for_utf8mb4`。
pub fn WithDefaultCollationForUTF8MB4(collation: String) -> ExprCtxOption {
    Box::new(move |state| state.default_collation_for_utf8mb4 = collation)
}

/// 设置块加密模式（如 aes-128-ecb）。
pub fn WithBlockEncryptionMode(mode: String) -> ExprCtxOption {
    Box::new(move |state| state.block_encryption_mode = mode)
}

/// 设置 `tidb_sysdate_is_now`：SYSDATE 是否等同 NOW。
pub fn WithSysDateIsNow(now: bool) -> ExprCtxOption {
    Box::new(move |state| state.sysdate_is_now = now)
}

/// 设置 noop 函数模式：ON / OFF / WARN。
pub fn WithNoopFuncsMode(mode: i32) -> ExprCtxOption {
    assert!(
        mode == variable::OnInt || mode == variable::OffInt || mode == variable::WarnInt,
        "noop function mode must be ON, OFF, or WARN"
    );
    Box::new(move |state| state.noop_funcs_mode = mode)
}

/// 注入 MySQL 兼容随机数生成器。
pub fn WithRng(rng: Arc<mathutil::MysqlRng>) -> ExprCtxOption {
    Box::new(move |state| state.rng = rng)
}

/// 注入计划缓存跟踪器（决定本语句是否可缓存执行计划）。
pub fn WithPlanCacheTracker(
    tracker: Arc<contextutil::plancache::PlanCacheTracker>,
) -> ExprCtxOption {
    Box::new(move |state| state.plan_cache_tracker = tracker)
}

/// 注入计划列 ID 分配器。
pub fn WithColumnIDAllocator(allocator: Arc<dyn exprctx::PlanColumnIDAllocator>) -> ExprCtxOption {
    Box::new(move |state| state.column_id_allocator = allocator)
}

/// 设置连接 ID。
pub fn WithConnectionID(id: u64) -> ExprCtxOption {
    Box::new(move |state| state.connection_id = id)
}

/// 窗口函数是否使用高精度。
pub fn WithWindowingUseHighPrecision(use_high_precision: bool) -> ExprCtxOption {
    Box::new(move |state| state.windowing_use_high_precision = use_high_precision)
}

/// 设置 `group_concat_max_len`。
pub fn WithGroupConcatMaxLen(max_len: u64) -> ExprCtxOption {
    Box::new(move |state| state.group_concat_max_len = max_len)
}

/// 静态表达式构建上下文。
pub struct ExprContext {
    state: ExprCtxState,
}

/// 以服务器默认值创建 ExprContext，再应用选项。
pub fn NewExprContext(options: Vec<ExprCtxOption>) -> ExprContext {
    let charset = charset::GetCharsetInfo(mysql::DefaultCharset)
        .expect("the server default charset must exist");
    let eval_ctx = Arc::new(NewEvalContext(Vec::new()));
    let rng: Arc<mathutil::MysqlRng> = Arc::from(mathutil::NewWithTime());
    let column_id_allocator: Arc<dyn exprctx::PlanColumnIDAllocator> =
        Arc::new(exprctx::NewSimplePlanColumnIDAllocator(0));
    let plan_cache_tracker = Arc::new(contextutil::plancache::NewPlanCacheTracker(
        eval_ctx.WarnAppenderArc(),
    ));
    plan_cache_tracker.EnablePlanCache();
    let default_eval_ctx = Arc::clone(&eval_ctx);
    let default_plan_cache_tracker = Arc::clone(&plan_cache_tracker);

    let mut state = ExprCtxState {
        eval_ctx,
        charset: charset.Name,
        collation: charset.DefaultCollation,
        default_collation_for_utf8mb4: mysql::DefaultCollationName.to_owned(),
        block_encryption_mode: vardef::DefBlockEncryptionMode.to_owned(),
        sysdate_is_now: vardef::DefSysdateIsNow,
        noop_funcs_mode: variable::TiDBOptOnOffWarn(vardef::DefTiDBEnableNoopFuncs),
        rng,
        plan_cache_tracker,
        column_id_allocator,
        connection_id: 0,
        windowing_use_high_precision: true,
        group_concat_max_len: vardef::DefGroupConcatMaxLen,
    };
    for option in options {
        option(&mut state);
    }
    if !Arc::ptr_eq(&state.eval_ctx, &default_eval_ctx)
        && Arc::ptr_eq(&state.plan_cache_tracker, &default_plan_cache_tracker)
    {
        let tracker = Arc::new(contextutil::plancache::NewPlanCacheTracker(
            state.eval_ctx.WarnAppenderArc(),
        ));
        tracker.EnablePlanCache();
        state.plan_cache_tracker = tracker;
    }
    ExprContext { state }
}

impl ExprContext {
    /// 克隆状态并应用新选项；共享资源（tracker、列 ID）仍指向同一对象。
    pub fn Apply(&self, options: Vec<ExprCtxOption>) -> ExprContext {
        let mut state = self.state.clone();
        for option in options {
            option(&mut state);
        }
        ExprContext { state }
    }

    /// 内嵌求值上下文。
    pub fn GetEvalCtx(&self) -> &EvalContext {
        &self.state.eval_ctx
    }

    /// 与 `GetEvalCtx` 相同，强调静态语义。
    pub fn GetStaticEvalCtx(&self) -> &EvalContext {
        &self.state.eval_ctx
    }

    /// 返回 (charset, collation)。
    pub fn GetCharsetInfo(&self) -> (String, String) {
        (self.state.charset.clone(), self.state.collation.clone())
    }

    pub fn GetDefaultCollationForUTF8MB4(&self) -> String {
        self.state.default_collation_for_utf8mb4.clone()
    }

    pub fn GetBlockEncryptionMode(&self) -> String {
        self.state.block_encryption_mode.clone()
    }

    pub fn GetSysdateIsNow(&self) -> bool {
        self.state.sysdate_is_now
    }

    pub fn GetNoopFuncsMode(&self) -> i32 {
        self.state.noop_funcs_mode
    }

    pub fn Rng(&self) -> &mathutil::MysqlRng {
        &self.state.rng
    }

    /// 本语句是否仍允许使用计划缓存。
    pub fn IsUseCache(&self) -> bool {
        self.state.plan_cache_tracker.UseCache()
    }

    /// 标记跳过计划缓存并记录原因。
    pub fn SetSkipPlanCache(&self, reason: &str) {
        self.state.plan_cache_tracker.SetSkipPlanCache(reason);
    }

    /// 分配下一个计划列 ID。
    pub fn AllocPlanColumnID(&self) -> i64 {
        self.state.column_id_allocator.AllocPlanColumnID()
    }

    /// 静态上下文默认不处于 NULL reject 检查。
    pub fn IsInNullRejectCheck(&self) -> bool {
        false
    }

    /// 静态上下文默认不处于常量传播检查。
    pub fn IsConstantPropagateCheck(&self) -> bool {
        false
    }

    pub fn ConnectionID(&self) -> u64 {
        self.state.connection_id
    }

    pub fn GetWindowingUseHighPrecision(&self) -> bool {
        self.state.windowing_use_high_precision
    }

    pub fn GetGroupConcatMaxLen(&self) -> u64 {
        self.state.group_concat_max_len
    }

    /// 最近一次已分配的计划列 ID。
    pub fn GetLastPlanColumnID(&self) -> i64 {
        self.state.column_id_allocator.GetLastPlanColumnID()
    }

    pub fn GetPlanCacheTracker(&self) -> &contextutil::plancache::PlanCacheTracker {
        &self.state.plan_cache_tracker
    }

    /// 供静态物化使用的求值上下文视图。
    pub fn GetStaticConvertibleEvalContext(&self) -> &dyn exprctx::StaticConvertibleEvalContext {
        self.state.eval_ctx.as_ref()
    }

    /// 静态实现默认用户变量均非只读。
    pub fn IsReadonlyUserVar(&self, _name: &str) -> bool {
        false
    }

    /// 解析系统变量并同步更新 Eval/Expr 两层。
    pub fn LoadSystemVars(
        &self,
        sys_vars: &HashMap<String, String>,
    ) -> Result<ExprContext, contextutil::errors::SharedError> {
        let parsed = parse_system_vars(sys_vars)?;
        Ok(self.load_system_vars_internal(&parsed, sys_vars))
    }

    /// 先刷新内嵌 EvalContext，再按出现的 Expr 层变量生成选项。
    fn load_system_vars_internal(
        &self,
        parsed: &ParsedSystemVars,
        sys_vars: &HashMap<String, String>,
    ) -> ExprContext {
        let eval_ctx = Arc::new(
            self.state
                .eval_ctx
                .load_system_vars_internal(parsed, sys_vars),
        );
        let mut options = Vec::with_capacity(9);
        options.push(WithEvalCtx(eval_ctx));
        for name in sys_vars.keys() {
            match name.to_ascii_lowercase().as_str() {
                vardef::CharacterSetConnection | vardef::CollationConnection => {
                    options.push(WithCharset(
                        parsed.charset.clone(),
                        parsed.collation.clone(),
                    ));
                }
                vardef::DefaultCollationForUTF8MB4 => {
                    options.push(WithDefaultCollationForUTF8MB4(
                        parsed.default_collation_for_utf8mb4.clone(),
                    ));
                }
                vardef::BlockEncryptionMode => options.push(WithBlockEncryptionMode(
                    parsed.block_encryption_mode.clone(),
                )),
                vardef::TiDBSysdateIsNow => {
                    options.push(WithSysDateIsNow(parsed.sysdate_is_now));
                }
                vardef::TiDBEnableNoopFuncs => {
                    options.push(WithNoopFuncsMode(parsed.noop_funcs_mode));
                }
                vardef::WindowingUseHighPrecision => options.push(WithWindowingUseHighPrecision(
                    parsed.windowing_use_high_precision,
                )),
                vardef::GroupConcatMaxLen => {
                    options.push(WithGroupConcatMaxLen(parsed.group_concat_max_len));
                }
                _ => {}
            }
        }
        self.Apply(options)
    }
}

impl exprctx::BuildContext for ExprContext {
    fn GetEvalCtx(&self) -> &dyn exprctx::EvalContext {
        ExprContext::GetEvalCtx(self)
    }

    fn GetCharsetInfo(&self) -> (String, String) {
        ExprContext::GetCharsetInfo(self)
    }

    fn GetDefaultCollationForUTF8MB4(&self) -> String {
        ExprContext::GetDefaultCollationForUTF8MB4(self)
    }

    fn GetBlockEncryptionMode(&self) -> String {
        ExprContext::GetBlockEncryptionMode(self)
    }

    fn GetSysdateIsNow(&self) -> bool {
        ExprContext::GetSysdateIsNow(self)
    }

    fn GetNoopFuncsMode(&self) -> i32 {
        ExprContext::GetNoopFuncsMode(self)
    }

    fn Rng(&self) -> &mathutil::MysqlRng {
        ExprContext::Rng(self)
    }

    fn IsUseCache(&self) -> bool {
        ExprContext::IsUseCache(self)
    }

    fn SetSkipPlanCache(&self, reason: &str) {
        ExprContext::SetSkipPlanCache(self, reason);
    }

    fn AllocPlanColumnID(&self) -> i64 {
        ExprContext::AllocPlanColumnID(self)
    }

    fn IsInNullRejectCheck(&self) -> bool {
        false
    }

    fn IsConstantPropagateCheck(&self) -> bool {
        false
    }

    fn ConnectionID(&self) -> u64 {
        ExprContext::ConnectionID(self)
    }

    fn IsReadonlyUserVar(&self, name: &str) -> bool {
        ExprContext::IsReadonlyUserVar(self, name)
    }
}

impl exprctx::ExprContext for ExprContext {
    fn GetWindowingUseHighPrecision(&self) -> bool {
        ExprContext::GetWindowingUseHighPrecision(self)
    }

    fn GetGroupConcatMaxLen(&self) -> u64 {
        ExprContext::GetGroupConcatMaxLen(self)
    }
}

impl exprctx::StaticConvertibleExprContext for ExprContext {
    fn GetStaticConvertibleEvalContext(&self) -> &dyn exprctx::StaticConvertibleEvalContext {
        ExprContext::GetStaticConvertibleEvalContext(self)
    }

    fn GetPlanCacheTracker(&self) -> &contextutil::plancache::PlanCacheTracker {
        ExprContext::GetPlanCacheTracker(self)
    }

    fn GetLastPlanColumnID(&self) -> i64 {
        ExprContext::GetLastPlanColumnID(self)
    }

    fn GetRngArc(&self) -> Option<Arc<mathutil::MysqlRng>> {
        Some(Arc::clone(&self.state.rng))
    }

    fn GetPlanCacheTrackerArc(&self) -> Option<Arc<contextutil::plancache::PlanCacheTracker>> {
        Some(Arc::clone(&self.state.plan_cache_tracker))
    }
}

/// 从任意 `StaticConvertibleExprContext` 物化为独立静态 `ExprContext`。
/// 优先复用 RNG/tracker Arc；否则按种子或 Save/Restore 重建。
pub fn MakeExprContextStatic(context: &dyn exprctx::StaticConvertibleExprContext) -> ExprContext {
    let eval_ctx = Arc::new(MakeEvalContextStatic(
        context.GetStaticConvertibleEvalContext(),
    ));

    let rng = context.GetRngArc().unwrap_or_else(|| {
        let cloned: Arc<mathutil::MysqlRng> = Arc::from(mathutil::NewWithSeed(0));
        cloned.SetSeed1(context.Rng().GetSeed1());
        cloned.SetSeed2(context.Rng().GetSeed2());
        cloned
    });

    let tracker = context.GetPlanCacheTrackerArc().unwrap_or_else(|| {
        let cloned = Arc::new(contextutil::plancache::NewPlanCacheTracker(
            eval_ctx.WarnAppenderArc(),
        ));
        let (use_cache, cache_type, reason, force_cache, always_warn) =
            context.GetPlanCacheTracker().Save();
        cloned.Restore(use_cache, cache_type, reason, force_cache, always_warn);
        cloned
    });

    NewExprContext(vec![
        WithEvalCtx(eval_ctx),
        WithCharset(context.GetCharsetInfo().0, context.GetCharsetInfo().1),
        WithDefaultCollationForUTF8MB4(context.GetDefaultCollationForUTF8MB4()),
        WithBlockEncryptionMode(context.GetBlockEncryptionMode()),
        WithSysDateIsNow(context.GetSysdateIsNow()),
        WithNoopFuncsMode(context.GetNoopFuncsMode()),
        WithRng(rng),
        WithPlanCacheTracker(tracker),
        WithColumnIDAllocator(Arc::new(exprctx::NewSimplePlanColumnIDAllocator(
            context.GetLastPlanColumnID(),
        ))),
        WithConnectionID(context.ConnectionID()),
        WithWindowingUseHighPrecision(context.GetWindowingUseHighPrecision()),
        WithGroupConcatMaxLen(context.GetGroupConcatMaxLen()),
    ])
}
