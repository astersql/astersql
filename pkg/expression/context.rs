// Copyright 2023 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 表达式求值上下文（EvalContext）轻量转发与测试断言包装。
//
// 将 SQL mode、类型上下文、错误策略、时区与告警列表等会话能力暴露给表达式，
// 避免各 builtin 直接依赖具体 session；`assertionEvalContext` 在测试中审计
// 可选属性声明是否完整。

use crate::*;

pub use exprctx::{
    BuildContext, EvalContext, ExprContext as AggFuncBuildContext, OptionalEvalPropDesc,
    OptionalEvalPropKey, OptionalEvalPropKeySet, OptionalEvalPropProvider, ParamValues,
};

/// sqlMode 对应 Go 的轻量转发，避免各表达式直接依赖具体 session context。
pub fn sqlMode(ctx: &dyn EvalContext) -> mysql::SQLMode {
    ctx.SQLMode()
}

/// typeCtx 返回 Datum 转换所需的类型上下文，其中包含 SQL mode、时区和截断策略。
pub fn typeCtx(ctx: &dyn EvalContext) -> types::Context {
    ctx.TypeCtx()
}

/// errCtx 提供除零、溢出等表达式错误的处理策略。
pub fn errCtx(ctx: &dyn EvalContext) -> errctx::Context {
    ctx.ErrCtx()
}

/// location 必须与 TypeCtx 中的时区保持一致，wrapEvalAssert 会在测试模式验证该约束。
pub fn location(ctx: &dyn EvalContext) -> chrono_tz::Tz {
    ctx.Location()
}

/// 当前求值上下文中已累积的 SQL 告警条数。
pub fn warningCount(ctx: &dyn EvalContext) -> usize {
    ctx.WarningCount()
}

/// truncateWarnings 返回并移除 start 之后的告警，供优化期试算后恢复用户可见告警列表。
pub fn truncateWarnings(ctx: &dyn EvalContext, start: usize) -> Vec<contextutil::SQLWarn> {
    ctx.TruncateWarnings(start as isize)
}

/// assertionEvalContext 仅用于测试：记录当前 builtin，并审计它读取的可选属性是否已声明。
pub struct assertionEvalContext<'a> {
    eval_context: &'a dyn EvalContext,
    function: Option<&'a dyn builtinFunc>,
}

/// wrapEvalAssert 避免对同一 builtin 重复包裹，同时始终校验基础上下文的不变量。
pub fn wrapEvalAssert<'a>(
    ctx: &'a dyn EvalContext,
    function: &'a dyn builtinFunc,
) -> assertionEvalContext<'a> {
    checkEvalCtx(ctx);
    assertionEvalContext {
        eval_context: ctx,
        function: Some(function),
    }
}

/// checkEvalCtx 保证直接 Location 与 TypeCtx.Location 相同，否则日期/时间函数会出现不一致结果。
pub fn checkEvalCtx(ctx: &dyn EvalContext) {
    let type_context = ctx.TypeCtx();
    assert_eq!(
        ctx.Location(),
        type_context.Location(),
        "求值上下文与类型上下文的时区不一致"
    );
}

impl assertionEvalContext<'_> {
    #[cfg(test)]
    pub(crate) fn new_for_test<'a>(
        eval_context: &'a dyn EvalContext,
        function: &'a dyn builtinFunc,
    ) -> assertionEvalContext<'a> {
        assertionEvalContext {
            eval_context,
            function: Some(function),
        }
    }

    /// 读取可选属性前断言其已声明为 required 或 allowed。
    pub fn GetOptionalPropProvider(
        &self,
        key: OptionalEvalPropKey,
    ) -> Option<&dyn OptionalEvalPropProvider> {
        let declared = self
            .function
            .map_or_else(OptionalEvalPropKeySet::default, |function| {
                OptionalEvalPropKeySet(
                    function.RequiredOptionalEvalProps().0 | function.AllowedOptionalEvalProps().0,
                )
            });
        // builtin 读取未声明属性通常意味着向量化/缓存阶段缺少依赖，测试包装在访问点立即暴露问题。
        assert!(
            declared.Contains(key),
            "函数读取了未在 RequiredOptionalEvalProps 或 AllowedOptionalEvalProps 声明的可选属性: {key:?}"
        );
        self.eval_context.GetOptionalPropProviderUnwrapped(key)
    }
}

impl contextutil::WarnAppender for assertionEvalContext<'_> {
    fn AppendWarning(&self, error: contextutil::errors::SharedError) {
        self.eval_context.AppendWarning(error);
    }

    fn AppendNote(&self, error: contextutil::errors::SharedError) {
        self.eval_context.AppendNote(error);
    }
}

impl contextutil::WarnHandler for assertionEvalContext<'_> {
    fn WarningCount(&self) -> usize {
        self.eval_context.WarningCount()
    }

    fn TruncateWarnings(&self, start: isize) -> Vec<contextutil::SQLWarn> {
        self.eval_context.TruncateWarnings(start)
    }

    fn CopyWarnings(&self, destination: Vec<contextutil::SQLWarn>) -> Vec<contextutil::SQLWarn> {
        self.eval_context.CopyWarnings(destination)
    }
}

impl ParamValues for assertionEvalContext<'_> {
    fn GetParamValue(&self, index: usize) -> Result<types::Datum, exprctx::ParamError> {
        self.eval_context.GetParamValue(index)
    }
}

impl EvalContext for assertionEvalContext<'_> {
    fn CtxID(&self) -> u64 {
        self.eval_context.CtxID()
    }
    fn SQLMode(&self) -> mysql::SQLMode {
        self.eval_context.SQLMode()
    }
    fn TypeCtx(&self) -> types::Context {
        self.eval_context.TypeCtx()
    }
    fn ErrCtx(&self) -> errctx::Context {
        self.eval_context.ErrCtx()
    }
    fn Location(&self) -> chrono_tz::Tz {
        self.eval_context.Location()
    }
    fn CurrentTime(
        &self,
    ) -> Result<chrono::DateTime<chrono_tz::Tz>, contextutil::errors::SharedError> {
        self.eval_context.CurrentTime()
    }
    fn CurrentDB(&self) -> String {
        self.eval_context.CurrentDB()
    }
    fn GetMaxAllowedPacket(&self) -> u64 {
        self.eval_context.GetMaxAllowedPacket()
    }
    fn GetTiDBRedactLog(&self) -> String {
        self.eval_context.GetTiDBRedactLog()
    }
    fn GetDefaultWeekFormatMode(&self) -> String {
        self.eval_context.GetDefaultWeekFormatMode()
    }
    fn GetDivPrecisionIncrement(&self) -> i32 {
        self.eval_context.GetDivPrecisionIncrement()
    }
    fn GetUserVarsReader(&self) -> &dyn exprctx::UserVarsReader {
        self.eval_context.GetUserVarsReader()
    }
    fn GetOptionalPropSet(&self) -> OptionalEvalPropKeySet {
        self.eval_context.GetOptionalPropSet()
    }
    fn GetOptionalPropProvider(
        &self,
        key: OptionalEvalPropKey,
    ) -> Option<&dyn OptionalEvalPropProvider> {
        assertionEvalContext::GetOptionalPropProvider(self, key)
    }

    fn GetOptionalPropProviderUnwrapped(
        &self,
        key: OptionalEvalPropKey,
    ) -> Option<&dyn OptionalEvalPropProvider> {
        self.eval_context.GetOptionalPropProviderUnwrapped(key)
    }
}

/// StringerWithCtx 对应 Go 接口；实现必须接受空/缺省参数上下文而不 panic。
pub trait StringerWithCtx {
    fn StringWithCtx(&self, ctx: Option<&dyn ParamValues>, redact: &str) -> String;
}
