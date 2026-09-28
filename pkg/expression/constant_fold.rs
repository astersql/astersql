// Copyright 2016 PingCAP, Inc.
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

// 常量折叠（constant folding）：在优化期把可确定结果的表达式求成常量。
//
// 对应 Go `constant_fold.go`：对 IF/IFNULL/CASE/ISNULL 做短路折叠，统一折叠普通标量函数，
// 并处理 DeferredExpr、ParamMarker 与 NULL-reject 探测。折叠只改写内存表达式树。

use crate::*;

// 本文件对应 pkg/expression/constant_fold.go，保留特殊函数短路、惰性标记、NULL-reject 探测和错误回退。
// 折叠只改写内存表达式；扩展函数因可能包含外部副作用而始终留到执行期求值。

use std::collections::HashMap;

/// 记录折叠求值失败；优化期不把错误抛给客户端，留给执行期复现。
fn log_fold_error(
    _ctx: &dyn BuildContext,
    _expression: &dyn Expression,
    error: impl std::fmt::Display,
) {
    logutil::BgLogger().warn(format!("constant folding failed: {error}"));
}

/// 记录 prepared 参数在折叠期求值失败。
fn log_param_error(error: impl std::fmt::Display) {
    logutil::BgLogger().warn(format!(
        "parameter evaluation during constant folding failed: {error}"
    ));
}

/// 特殊函数折叠处理器：输入 BuildContext 与标量函数，返回折叠结果及 deferred 标志。
type FoldHandler = fn(&dyn BuildContext, ScalarFunction) -> (Box<dyn Expression>, bool);

/// specialFoldHandler 仅包含需要短路语义的函数，普通标量函数走统一参数折叠流程。
pub fn specialFoldHandler() -> HashMap<&'static str, FoldHandler> {
    HashMap::from([
        (ast::If, ifFoldHandler as FoldHandler),
        (ast::Ifnull, ifNullFoldHandler as FoldHandler),
        (ast::Case, caseWhenHandler as FoldHandler),
        (ast::IsNull, isNullHandler as FoldHandler),
    ])
}

/// 对应 Go init；显式返回分派表，避免引入可变全局初始化状态。
pub fn init() -> HashMap<&'static str, FoldHandler> {
    specialFoldHandler()
}

/// FoldConstant 是公开入口；折叠后恢复原表达式的排序规则、字符集和 repertoire 元数据。
pub fn FoldConstant(ctx: &dyn BuildContext, expr: Box<dyn Expression>) -> Box<dyn Expression> {
    let coercibility = expr.Coercibility();
    let repertoire = expr.Repertoire();
    let charset = expr.GetType(ctx.GetEvalCtx()).GetCharset().to_owned();
    let collation = expr.GetType(ctx.GetEvalCtx()).GetCollate().to_owned();
    let (mut folded, _) = foldConstant(ctx, expr);
    folded.SetCoercibility(coercibility);
    folded.GetTypeMut().SetCharset(charset);
    folded.GetTypeMut().SetCollate(collation);
    folded.SetRepertoire(repertoire);
    folded
}

/// ISNULL：参数已是常量则直接求值；列带 NOT NULL 标志则折叠为 0。
fn isNullHandler(ctx: &dyn BuildContext, expr: ScalarFunction) -> (Box<dyn Expression>, bool) {
    let arg0 = &expr.GetArgs()[0];
    if let Some(constant) = arg0.as_constant() {
        let deferred = constant.DeferredExpr.is_some() || constant.ParamMarker.is_some();
        match expr.Eval(ctx.GetEvalCtx(), chunk::Row::default()) {
            Ok(value) if deferred => (
                Box::new(Constant::with_deferred(
                    value,
                    expr.RetType.clone().unwrap(),
                    expr,
                )),
                true,
            ),
            Ok(value) => (
                Box::new(Constant::with_type(value, expr.RetType.clone().unwrap())),
                false,
            ),
            Err(error) => {
                // 求值失败不能在优化期吞掉；保留原表达式，让执行期把同一错误返回客户端。
                log_fold_error(ctx, &expr, error);
                (Box::new(expr), deferred)
            }
        }
    } else if mysql::HasNotNullFlag(arg0.GetType(ctx.GetEvalCtx()).GetFlag()) {
        (Box::new(NewZero()), false)
    } else {
        (Box::new(expr), false)
    }
}

/// IF：仅当条件折叠为常量后才进入 then/else 分支，保持短路语义。
fn ifFoldHandler(ctx: &dyn BuildContext, expr: ScalarFunction) -> (Box<dyn Expression>, bool) {
    let (condition, _) = foldConstant(ctx, expr.GetArgs()[0].clone());
    let Some(constant) = condition.as_constant() else {
        return (Box::new(expr), false);
    };
    match constant.EvalInt(ctx.GetEvalCtx(), chunk::Row::default()) {
        Ok((value, is_null)) => {
            // IF 对 NULL/0 走 else，只有非 NULL 且非零才折叠 then 分支，保持短路求值。
            let branch = if !is_null && value != 0 { 1 } else { 2 };
            foldConstant(ctx, expr.GetArgs()[branch].clone())
        }
        Err(error) => {
            log_fold_error(ctx, &expr, error);
            (Box::new(expr), false)
        }
    }
}

/// IFNULL：第一参数为非 NULL 常量则取之；为 NULL 则折叠第二参数并同步字符集/排序规则。
fn ifNullFoldHandler(
    ctx: &dyn BuildContext,
    mut expr: ScalarFunction,
) -> (Box<dyn Expression>, bool) {
    let (first, deferred) = foldConstant(ctx, expr.GetArgs()[0].clone());
    let Some(constant) = first.as_constant() else {
        return (Box::new(expr), false);
    };
    if constant.Value.IsNull() {
        // 第一参数折叠为 NULL 时结果来自第二参数，IFNULL 的字符集和排序规则也必须随第二参数。
        let charset = expr.GetArgs()[1]
            .GetType(ctx.GetEvalCtx())
            .GetCharset()
            .to_owned();
        let collation = expr.GetArgs()[1]
            .GetType(ctx.GetEvalCtx())
            .GetCollate()
            .to_owned();
        expr.RetType.as_mut().unwrap().SetCharset(charset);
        expr.RetType.as_mut().unwrap().SetCollate(collation);
        foldConstant(ctx, expr.GetArgs()[1].clone())
    } else {
        (first, deferred)
    }
}

/// CASE WHEN：按条件对依次折叠；命中首个真条件则只折叠对应 THEN；否则走 ELSE。
fn caseWhenHandler(
    ctx: &dyn BuildContext,
    mut expr: ScalarFunction,
) -> (Box<dyn Expression>, bool) {
    let len = expr.GetArgs().len();
    let mut any_deferred = false;
    for index in (0..len.saturating_sub(1)).step_by(2) {
        let (condition, deferred) = foldConstant(ctx, expr.GetArgs()[index].clone());
        any_deferred |= deferred;
        expr.GetArgsMut()[index] = condition;
        let Some(constant) = expr.GetArgs()[index].as_constant() else {
            // 条件非常量时后续分支是否执行未知，不能提前求值可能有错误或副作用的表达式。
            return (Box::new(expr), false);
        };
        let Ok((value, is_null)) = constant.EvalInt(ctx.GetEvalCtx(), chunk::Row::default()) else {
            return (Box::new(expr), false);
        };
        if value != 0 && !is_null {
            let (mut body, deferred) = foldConstant(ctx, expr.GetArgs()[index + 1].clone());
            any_deferred |= deferred;
            if body.as_constant().is_some() {
                body.GetTypeMut()
                    .SetDecimal(expr.RetType.as_ref().unwrap().GetDecimal());
            }
            return (body, any_deferred);
        }
    }
    if len % 2 == 1 {
        let (mut otherwise, deferred) = foldConstant(ctx, expr.GetArgs()[len - 1].clone());
        any_deferred |= deferred;
        if otherwise.as_constant().is_some() {
            otherwise
                .GetTypeMut()
                .SetDecimal(expr.RetType.as_ref().unwrap().GetDecimal());
        }
        return (otherwise, any_deferred);
    }
    (Box::new(expr), any_deferred)
}

/// foldConstant 返回折叠结果及“结果依赖执行上下文”标志，后者阻止计划缓存固化参数/非确定值。
pub fn foldConstant(
    ctx: &dyn BuildContext,
    expr: Box<dyn Expression>,
) -> (Box<dyn Expression>, bool) {
    if let Some(function) = expr.as_scalar_function() {
        let function = function.clone_scalar();
        if unFoldableFunctions.contains_key(function.FuncName.L.as_str()) {
            return (expr, false);
        }
        if function.Function.isExtensionFunction() {
            // 扩展函数可能有外部副作用，即使参数全为常量也不得在优化期调用。
            return (expr, false);
        }
        if !crate::core_support::MaybeOverOptimized4PlanCache(ctx, expr.as_ref()) {
            if let Some(handler) = specialFoldHandler().get(function.FuncName.L.as_str()) {
                return handler(ctx, function);
            }
        }

        let mut all_constant = true;
        let mut has_null = false;
        let mut deferred = false;
        let mut constant_flags = Vec::with_capacity(function.GetArgs().len());
        for argument in function.GetArgs() {
            if let Some(value) = argument.as_constant() {
                deferred |= value.DeferredExpr.is_some() || value.ParamMarker.is_some();
                has_null |= value.Value.IsNull();
                constant_flags.push(true);
            } else {
                all_constant = false;
                constant_flags.push(false);
            }
        }

        if !all_constant {
            let excluded = matches!(
                function.FuncName.L.as_str(),
                ast::NullEQ | ast::ConcatWS | ast::Field
            );
            if !has_null || !ctx.IsInNullRejectCheck() || excluded {
                return (expr, deferred);
            }
            // NULL-reject 检查以 1 替换非常量参数，只判断结果能否确定为 NULL/false；临时 Constant 不保留 DeferredExpr。
            let dummy_args = function
                .GetArgs()
                .iter()
                .zip(constant_flags.iter())
                .map(|(arg, is_constant)| {
                    if *is_constant {
                        arg.clone()
                    } else {
                        Box::new(NewOne()) as Box<dyn Expression>
                    }
                })
                .collect();
            let Ok(dummy) = NewFunctionBase(
                ctx,
                &function.FuncName.L,
                function.RetType.clone().unwrap(),
                dummy_args,
            ) else {
                return (expr, deferred);
            };
            let Ok(value) = dummy.Eval(ctx.GetEvalCtx(), chunk::Row::default()) else {
                return (expr, deferred);
            };
            if value.IsNull()
                || value
                    .ToBool(ctx.GetEvalCtx().TypeCtx())
                    .is_ok_and(|v| v == 0)
            {
                return (
                    Box::new(Constant::with_type(
                        value,
                        function.RetType.clone().unwrap(),
                    )),
                    false,
                );
            }
            return (expr, deferred);
        }

        let value = match function.Eval(ctx.GetEvalCtx(), chunk::Row::default()) {
            Ok(value) => value,
            Err(error) => {
                log_fold_error(ctx, &function, error);
                return (expr, deferred);
            }
        };
        let mut ret_type = function.RetType.clone().unwrap();
        if !has_null {
            if value.Kind() == types::KindNull {
                ret_type.DelFlag(mysql::NotNullFlag);
            } else {
                ret_type.AddFlag(mysql::NotNullFlag);
            }
        }
        if deferred {
            return (
                Box::new(Constant::with_deferred(value, ret_type, function)),
                true,
            );
        }
        // 子查询展示 ID 从首个带标记的常量参数上传递到折叠结果。
        let subquery_ref = function
            .GetArgs()
            .iter()
            .filter_map(|arg| arg.as_constant())
            .map(|constant| constant.SubqueryRefID)
            .find(|id| *id > 0)
            .unwrap_or(0);
        return (
            Box::new(Constant::with_subquery(value, ret_type, subquery_ref)),
            false,
        );
    }

    if let Some(constant) = expr.as_constant() {
        if let Some(marker) = &constant.ParamMarker {
            return match marker.GetUserVar(ctx.GetEvalCtx()) {
                Ok(value) => (Box::new(constant.clone_with_value(value)), true),
                Err(error) => {
                    log_param_error(error);
                    (expr, true)
                }
            };
        }
        if let Some(deferred_expr) = &constant.DeferredExpr {
            return match deferred_expr.Eval(ctx.GetEvalCtx(), chunk::Row::default()) {
                Ok(value) => (Box::new(constant.clone_with_value(value)), true),
                Err(error) => {
                    log_fold_error(ctx, constant, error);
                    (expr, true)
                }
            };
        }
    }
    (expr, false)
}
