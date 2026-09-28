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

// NULL 拒绝（null-reject）证明：判断谓词在内表 Schema 列上是否恒为非真。
//
// 外连接优化中，若 ON/WHERE 谓词对内表行「拒绝 NULL」——即当内表列取 NULL 时
// 谓词不可能为 TRUE——则可将外连接改写为内连接等。
// 证明结合常量折叠、AND/OR/NOT/IN 语义以及 builtin 的 NULL 传递属性。

use expression::Expression as _;
use expression::exprctx::{BuildContext as _, EvalContext as _};
use parser_ast::functions as parser_ast;

use crate::null_misc_builtins::{
    NullRejectTestMode, is_null_reject_null_preserving, null_reject_test_mode,
};

#[derive(Clone, Copy, Default)]
/// 单表达式的 NULL 拒绝证明结果。
/// `non_true`：在内表列取 NULL 时结果不可能为 TRUE；
/// `must_null`：结果必然为 NULL。
struct NullRejectProof {
    /// 结果恒非 TRUE（可能为 FALSE 或 NULL）。
    non_true: bool,
    /// 结果必然为 NULL。
    must_null: bool,
}

/// 表达式是否可安全视为全常量（排除计划缓存过度优化与参数标记）。
fn allConstants(
    context: &dyn expression::exprctx::BuildContext,
    expr: &dyn expression::Expression,
) -> bool {
    if expression::MaybeOverOptimized4PlanCache(context, expr) {
        return false;
    }
    if let Some(function) = expr.as_any().downcast_ref::<expression::ScalarFunction>() {
        return function
            .GetArgs()
            .iter()
            .all(|argument| allConstants(context, argument.as_ref()));
    }
    expr.as_any()
        .downcast_ref::<expression::Constant>()
        .is_some_and(|constant| constant.ParamMarker.is_none() && constant.DeferredExpr.is_none())
}

/// 谓词在 `inner_schema` 列取 NULL 时是否恒非 TRUE（外连接转内连接等优化前提）。
pub fn IsNullRejected(
    context: &dyn plan_base::PlanContext,
    inner_schema: &expression::Schema,
    predicate: expression::ExprBox,
) -> bool {
    let mut rewrite_context = nullRejectFoldCtx(context);
    let predicate = expression::PushDownNot(&mut rewrite_context, predicate);
    proveNullRejected(context, inner_schema, predicate.as_ref(), true).non_true
}

/// 递归证明表达式的 null-reject 属性；可选先做「内表列置 NULL」折叠。
fn proveNullRejected(
    context: &dyn plan_base::PlanContext,
    inner_schema: &expression::Schema,
    expr: &dyn expression::Expression,
    allow_nullified_fold: bool,
) -> NullRejectProof {
    // 优先：把内表列当作 NULL 折叠，直接从常量结果取证明。
    if allow_nullified_fold {
        if let Some(constant) = tryFoldNullifiedConstant(context, inner_schema, expr) {
            return proofFromConstant(context, &constant);
        }
    }
    if let Some(column) = expr.as_any().downcast_ref::<expression::Column>() {
        return if inner_schema.Contains(column) {
            NullRejectProof {
                non_true: true,
                must_null: true,
            }
        } else {
            NullRejectProof::default()
        };
    }
    if let Some(constant) = expr.as_any().downcast_ref::<expression::Constant>() {
        if constant.ParamMarker.is_none() {
            if let Some(deferred) = &constant.DeferredExpr {
                return proveNullRejected(context, inner_schema, deferred.as_ref(), false);
            }
        }
        return proofFromConstant(context, constant);
    }
    expr.as_any()
        .downcast_ref::<expression::ScalarFunction>()
        .map_or_else(NullRejectProof::default, |function| {
            proveNullRejectedScalarFunc(context, inner_schema, function, allow_nullified_fold)
        })
}

/// 按标量函数语义合并子证明：AND/OR/NOT/IN、WEEK 族、测试函数与 NULL 传递函数。
fn proveNullRejectedScalarFunc(
    context: &dyn plan_base::PlanContext,
    inner_schema: &expression::Schema,
    function: &expression::ScalarFunction,
    allow_nullified_fold: bool,
) -> NullRejectProof {
    let arguments = function.GetArgs();
    match function.FuncName.L.as_str() {
        // AND：一侧 non_true 则整体 non_true；两侧 must_null 才 must_null。
        parser_ast::LogicAnd => {
            let left = proveNullRejected(
                context,
                inner_schema,
                arguments[0].as_ref(),
                allow_nullified_fold,
            );
            let right = proveNullRejected(
                context,
                inner_schema,
                arguments[1].as_ref(),
                allow_nullified_fold,
            );
            return NullRejectProof {
                non_true: left.non_true || right.non_true,
                must_null: left.must_null && right.must_null,
            };
        }
        // OR：两侧都 non_true 才 non_true；两侧 must_null 才 must_null。
        parser_ast::LogicOr => {
            let left = proveNullRejected(
                context,
                inner_schema,
                arguments[0].as_ref(),
                allow_nullified_fold,
            );
            let right = proveNullRejected(
                context,
                inner_schema,
                arguments[1].as_ref(),
                allow_nullified_fold,
            );
            return NullRejectProof {
                non_true: left.non_true && right.non_true,
                must_null: left.must_null && right.must_null,
            };
        }
        // NOT：子式 must_null 则整体 non_true；特别处理 NOT(IS NULL)。
        parser_ast::UnaryNot => {
            if let Some(child) = arguments[0]
                .as_any()
                .downcast_ref::<expression::ScalarFunction>()
            {
                if child.FuncName.L == parser_ast::IsNull {
                    return NullRejectProof {
                        non_true: proveNullRejected(
                            context,
                            inner_schema,
                            child.GetArgs()[0].as_ref(),
                            allow_nullified_fold,
                        )
                        .must_null,
                        must_null: false,
                    };
                }
            }
            let child = proveNullRejected(
                context,
                inner_schema,
                arguments[0].as_ref(),
                allow_nullified_fold,
            );
            return NullRejectProof {
                non_true: child.must_null,
                must_null: child.must_null,
            };
        }
        parser_ast::In => {
            return proveNullRejectedIn(context, inner_schema, function, allow_nullified_fold);
        }
        parser_ast::IsNull => return NullRejectProof::default(),
        "week" | parser_ast::YearWeek => {
            let child = proveNullRejected(
                context,
                inner_schema,
                arguments[0].as_ref(),
                allow_nullified_fold,
            );
            return if child.must_null {
                NullRejectProof {
                    non_true: true,
                    must_null: true,
                }
            } else {
                NullRejectProof::default()
            };
        }
        _ => {}
    }

    if let Some(mode) = null_reject_test_mode(&function.FuncName.L) {
        let child = proveNullRejected(
            context,
            inner_schema,
            arguments[0].as_ref(),
            allow_nullified_fold,
        );
        return NullRejectProof {
            non_true: child.must_null,
            must_null: child.must_null && mode == NullRejectTestMode::KeepsNull,
        };
    }
    // NULL 传递函数：任一参数 must_null 则整体 must_null。
    if is_null_reject_null_preserving(&function.FuncName.L)
        && arguments.iter().any(|argument| {
            proveNullRejected(
                context,
                inner_schema,
                argument.as_ref(),
                allow_nullified_fold,
            )
            .must_null
        })
    {
        return NullRejectProof {
            non_true: true,
            must_null: true,
        };
    }
    NullRejectProof::default()
}

/// IN 谓词：探测值或列表全为 NULL 时结果必为 NULL，故 non_true。
fn proveNullRejectedIn(
    context: &dyn plan_base::PlanContext,
    inner_schema: &expression::Schema,
    function: &expression::ScalarFunction,
    allow_nullified_fold: bool,
) -> NullRejectProof {
    let arguments = function.GetArgs();
    if arguments.is_empty() {
        return NullRejectProof::default();
    }
    let value_must_be_null = proveNullRejected(
        context,
        inner_schema,
        arguments[0].as_ref(),
        allow_nullified_fold,
    )
    .must_null;
    let list_must_be_null = arguments[1..].iter().all(|argument| {
        proveNullRejected(
            context,
            inner_schema,
            argument.as_ref(),
            allow_nullified_fold,
        )
        .must_null
    });
    if value_must_be_null || list_must_be_null {
        NullRejectProof {
            non_true: true,
            must_null: true,
        }
    } else {
        NullRejectProof::default()
    }
}

/// 将内表列替换为 NULL 后尝试常量折叠，得到可能的常量结果。
fn tryFoldNullifiedConstant(
    context: &dyn plan_base::PlanContext,
    inner_schema: &expression::Schema,
    expr: &dyn expression::Expression,
) -> Option<expression::Constant> {
    if let Some(constant) = tryFoldStaticConstant(context, expr) {
        return Some(constant);
    }
    if let Some(column) = expr.as_any().downcast_ref::<expression::Column>() {
        if inner_schema.Contains(column) {
            let mut field_type = column.RetType.clone()?;
            field_type.DelFlag(mysql::r#type::NotNullFlag);
            return Some(expression::NewNullWithFieldType(field_type));
        }
        return None;
    }
    if let Some(constant) = expr.as_any().downcast_ref::<expression::Constant>() {
        return (constant.ParamMarker.is_none() && constant.DeferredExpr.is_none())
            .then(|| constant.clone());
    }
    expr.as_any()
        .downcast_ref::<expression::ScalarFunction>()
        .and_then(|function| tryFoldNullifiedScalarFunc(context, inner_schema, function))
}

/// 对全常量表达式做 FoldConstant（忽略截断错误）。
fn tryFoldStaticConstant(
    context: &dyn plan_base::PlanContext,
    expr: &dyn expression::Expression,
) -> Option<expression::Constant> {
    let fold_context = nullRejectFoldCtx(context);
    if !allConstants(&fold_context, expr) {
        return None;
    }
    let folded = expression::FoldConstant(&fold_context, expr.CloneExpr());
    let constant = folded.as_any().downcast_ref::<expression::Constant>()?;
    (constant.ParamMarker.is_none() && constant.DeferredExpr.is_none()).then(|| constant.clone())
}

/// 折叠 COALESCE/IFNULL/IF 或 NULL 传递函数在参数置 NULL 后的结果。
fn tryFoldNullifiedScalarFunc(
    context: &dyn plan_base::PlanContext,
    inner_schema: &expression::Schema,
    function: &expression::ScalarFunction,
) -> Option<expression::Constant> {
    match function.FuncName.L.as_str() {
        parser_ast::Coalesce | parser_ast::Ifnull => {
            return tryFoldNullifiedCoalesceLike(context, inner_schema, function);
        }
        parser_ast::If => return tryFoldNullifiedIf(context, inner_schema, function),
        _ => {}
    }

    let mut arguments = Vec::with_capacity(function.GetArgs().len());
    let mut has_null = false;
    for argument in function.GetArgs() {
        let constant = tryFoldNullifiedConstant(context, inner_schema, argument.as_ref())?;
        has_null |= constant.Value.IsNull();
        arguments.push(Box::new(constant) as expression::ExprBox);
    }
    if has_null && is_null_reject_null_preserving(&function.FuncName.L) {
        return Some(expression::NewNull());
    }
    foldNullifiedFunction(context, function, arguments)
}

/// COALESCE/IFNULL：取首个非 NULL 折叠结果，全 NULL 则返回 NULL。
fn tryFoldNullifiedCoalesceLike(
    context: &dyn plan_base::PlanContext,
    inner_schema: &expression::Schema,
    function: &expression::ScalarFunction,
) -> Option<expression::Constant> {
    for argument in function.GetArgs() {
        let constant = tryFoldNullifiedConstant(context, inner_schema, argument.as_ref())?;
        if !constant.Value.IsNull() {
            return Some(constant);
        }
    }
    Some(expression::NewNull())
}

/// IF(cond, a, b)：折叠条件后选择对应分支再折叠。
fn tryFoldNullifiedIf(
    context: &dyn plan_base::PlanContext,
    inner_schema: &expression::Schema,
    function: &expression::ScalarFunction,
) -> Option<expression::Constant> {
    let arguments = function.GetArgs();
    if arguments.len() < 3 {
        return None;
    }
    let condition = tryFoldNullifiedConstant(context, inner_schema, arguments[0].as_ref())?;
    let fold_context = nullRejectFoldCtx(context);
    let (value, is_null) = condition
        .EvalInt(fold_context.GetEvalCtx(), chunk::Row::default())
        .ok()?;
    if !is_null && value != 0 {
        tryFoldNullifiedConstant(context, inner_schema, arguments[1].as_ref())
    } else {
        tryFoldNullifiedConstant(context, inner_schema, arguments[2].as_ref())
    }
}

/// 用已折叠参数重建函数并 FoldConstant。
fn foldNullifiedFunction(
    context: &dyn plan_base::PlanContext,
    function: &expression::ScalarFunction,
    arguments: Vec<expression::ExprBox>,
) -> Option<expression::Constant> {
    let fold_context = nullRejectFoldCtx(context);
    let expression = expression::NewFunction(
        &fold_context,
        &function.FuncName.L,
        function.RetType.clone()?,
        arguments,
    )
    .ok()?;
    let folded = expression::FoldConstant(&fold_context, expression);
    folded
        .as_any()
        .downcast_ref::<expression::Constant>()
        .filter(|constant| constant.ParamMarker.is_none() && constant.DeferredExpr.is_none())
        .cloned()
}

/// 由常量值得到证明：NULL → must_null；布尔假 → non_true。
fn proofFromConstant(
    context: &dyn plan_base::PlanContext,
    constant: &expression::Constant,
) -> NullRejectProof {
    if constant.ParamMarker.is_some() || constant.DeferredExpr.is_some() {
        return NullRejectProof::default();
    }
    if constant.Value.IsNull() {
        return NullRejectProof {
            non_true: true,
            must_null: true,
        };
    }
    let fold_context = nullRejectFoldCtx(context);
    if constant
        .Value
        .ToBool(fold_context.GetEvalCtx().TypeCtx())
        .ok()
        == Some(0)
    {
        NullRejectProof {
            non_true: true,
            must_null: false,
        }
    } else {
        NullRejectProof::default()
    }
}

/// 构造折叠用表达式上下文：截断错误级别设为 Ignore。
fn nullRejectFoldCtx(
    context: &dyn plan_base::PlanContext,
) -> expression::exprctx::CtxWithTruncateResult<'_> {
    expression::exprctx::CtxWithHandleTruncateErrLevel(
        context.GetNullRejectCheckExprCtx(),
        errctx::errctx::Level::LevelIgnore,
    )
}

/// 清除 schema 中 `[start, end)` 列的 NotNull 标志（外连接输出可空）。
pub fn ResetNotNullFlag(schema: &mut expression::Schema, start: usize, end: usize) {
    for column in &mut schema.Columns[start..end] {
        let mut cloned = column.clone();
        if let Some(field_type) = &mut cloned.RetType {
            field_type.DelFlag(mysql::r#type::NotNullFlag);
        }
        *column = cloned;
    }
}
