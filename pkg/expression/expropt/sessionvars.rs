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

// 会话变量（SessionVars）可选求值属性的 Provider / Reader。
//
// SessionVars 保存会话级系统变量与语句运行时状态；表达式求值常依赖时区、SQL 模式等。
// 在断言开启时，Reader 会核对 EvalContext 与 SessionVars 的 location（时区位置）一致。

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::*;

// SessionVars is session-affine in Go and contains statement runtime state that
// is intentionally not Send/Sync. Optional property providers are also scoped
// to one EvalContext, so strengthening this boundary would reject the canonical
// SessionVars without adding any valid cross-thread usage.
// SessionVars 在 Go 侧与会话绑定，含语句运行时状态，刻意非 Send/Sync；
// 可选属性 Provider 也限定在单个 EvalContext，强化跨线程边界会拒绝规范 SessionVars
// 且无实际跨线程用法。
/// 从 Provider 取出 SessionVars，并可选提供语句级 location 名。
pub trait ExproptSessionVarsProvider {
    fn get_session_vars(&self) -> &variable::SessionVars;

    fn statement_location_name(&self) -> String {
        self.get_session_vars().location().to_string()
    }
}

impl ExproptSessionVarsProvider for variable::SessionVars {
    fn get_session_vars(&self) -> &variable::SessionVars {
        self
    }

    fn statement_location_name(&self) -> String {
        self.StmtCtx.TimeZone().to_string()
    }
}

/// 持有 `ExproptSessionVarsProvider` 的可选属性 Provider。
pub struct SessionVarsPropProvider {
    vars: Arc<dyn ExproptSessionVarsProvider>,
}

impl SessionVarsPropProvider {
    /// 用具体 SessionVars 实现构造 Provider。
    pub fn new<T: ExproptSessionVarsProvider + 'static>(provider: Arc<T>) -> Self {
        Self { vars: provider }
    }
}

impl exprctx::OptionalEvalPropProvider for SessionVarsPropProvider {
    fn Desc(&self) -> &'static exprctx::OptionalEvalPropDesc {
        exprctx::OptPropSessionVars.Desc()
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

pub(crate) fn assert_session_vars_location_matches(
    ctx_location: &str,
    vars: &dyn ExproptSessionVarsProvider,
) {
    let vars_location = vars.get_session_vars().location().to_string();
    let statement_location = vars.statement_location_name();
    assert!(
        ctx_location == vars_location && ctx_location == statement_location,
        "location mismatch, ctxLoc: {ctx_location}, varsLoc: {vars_location}, stmtLoc: {statement_location}"
    );
}

/// 声明并读取 `OptPropSessionVars` 的 Reader。
pub struct SessionVarsPropReader;

impl RequireOptionalEvalProps for SessionVarsPropReader {
    fn required_optional_eval_props(&self) -> exprctx::OptionalEvalPropKeySet {
        exprctx::OptPropSessionVars.AsPropKeySet()
    }
}

impl SessionVarsPropReader {
    /// 返回与 Provider 绑定的 SessionVars 引用。
    ///
    /// 在 `intest::EnableAssert` 开启且上下文提供 location 时，断言
    /// ctx / vars / statement 三处 location 字符串一致。
    pub fn get_session_vars<'a, C: OptionalEvalPropContext + ?Sized>(
        &self,
        ctx: &'a C,
    ) -> anyhow::Result<&'a variable::SessionVars> {
        let provider =
            get_prop_provider::<SessionVarsPropProvider, _>(ctx, exprctx::OptPropSessionVars)?;
        let vars = provider.vars.get_session_vars();

        // 断言模式下核对时区位置契约，避免 EvalContext 与 SessionVars 漂移
        if intest::EnableAssert.load(Ordering::Relaxed) {
            if let Some(ctx_location) = ctx.location_name() {
                assert_session_vars_location_matches(&ctx_location, provider.vars.as_ref());
            }
        }
        Ok(vars)
    }
}
