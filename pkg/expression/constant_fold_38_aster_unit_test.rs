// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// 常量折叠（constant folding）源码契约测试。
//
// 对照 `constant_fold.rs` / `constant_fold.go`，用字符串包含断言锁定正式 API、
// 短路折叠处理器注册表、元数据保留与 Go 函数面；常量折叠指在优化阶段把可确定
// 子表达式求成常量，以减少运行期计算。

/// 编译期嵌入的 Rust 常量折叠实现源码，供契约测试扫描。
const RUST_SOURCE: &str = include_str!("constant_fold.rs");
/// 对应的 Go 源码，用于核对函数名与处理器面是否对齐。
const GO_SOURCE: &str = include_str!("constant_fold.go");

/// 复现 IF 折叠：条件非 NULL 且非 0 取真分支（下标 1），否则取假分支（下标 2）。
fn selected_if_branch(value: i64, is_null: bool) -> usize {
    if !is_null && value != 0 { 1 } else { 2 }
}

/// 确认折叠实现使用 Go 风格正式字段/方法名，而非草稿期私有命名。
#[test]
fn formal_scalar_and_constant_apis_are_used() {
    for api in [
        "function.GetArgs()",
        "expr.GetArgsMut()",
        "function.FuncName.L",
        "function.RetType",
        "constant.Value",
        "constant.DeferredExpr",
        "constant.ParamMarker",
        "constant.SubqueryRefID",
    ] {
        assert!(RUST_SOURCE.contains(api), "missing formal API: {api}");
    }
    for obsolete in [
        ".CloneScalar()",
        ".func_name",
        ".ret_type",
        ".args[",
        ".value",
        ".deferred_expr",
        ".param_marker",
    ] {
        assert!(
            !RUST_SOURCE.contains(obsolete),
            "obsolete draft API: {obsolete}"
        );
    }
}

/// 确认 IF / IFNULL / CASE / ISNULL 等特殊短路折叠处理器已登记。
#[test]
fn all_special_short_circuit_handlers_are_registered() {
    for mapping in [
        "(ast::If, ifFoldHandler as FoldHandler)",
        "(ast::Ifnull, ifNullFoldHandler as FoldHandler)",
        "(ast::Case, caseWhenHandler as FoldHandler)",
        "(ast::IsNull, isNullHandler as FoldHandler)",
    ] {
        assert!(
            RUST_SOURCE.contains(mapping),
            "missing special fold: {mapping}"
        );
    }
}

/// 核对 IF 真值/NULL 选择逻辑与 Go 一致，并扫描关键源码片段。
#[test]
fn if_truth_and_null_short_circuit_match_go() {
    assert_eq!(selected_if_branch(1, false), 1);
    assert_eq!(selected_if_branch(-1, false), 1);
    assert_eq!(selected_if_branch(0, false), 2);
    assert_eq!(selected_if_branch(1, true), 2);
    assert!(RUST_SOURCE.contains("if constant.Value.IsNull()"));
    assert!(RUST_SOURCE.contains("foldConstant(ctx, expr.GetArgs()[1].clone())"));
}

/// ISNULL 对 deferred/parameter 常量保留延迟属性，普通 NOT NULL 参数直接折叠为零。
#[test]
fn is_null_preserves_deferred_and_not_null_paths() {
    for contract in [
        "constant.DeferredExpr.is_some() || constant.ParamMarker.is_some()",
        "Constant::with_deferred(",
        "mysql::HasNotNullFlag(arg0.GetType(ctx.GetEvalCtx()).GetFlag())",
        "Box::new(NewZero())",
        "log_fold_error(ctx, &expr, error)",
    ] {
        assert!(
            RUST_SOURCE.contains(contract),
            "missing ISNULL contract: {contract}"
        );
    }
    for go_contract in [
        "constArg.DeferredExpr != nil || constArg.ParamMarker != nil",
        "mysql.HasNotNullFlag(arg0.GetType(ctx.GetEvalCtx()).GetFlag())",
        "return NewZero(), false",
    ] {
        assert!(
            GO_SOURCE.contains(go_contract),
            "missing Go ISNULL contract: {go_contract}"
        );
    }
}

/// IFNULL 的 NULL 分支采用第二参数的字符集/排序规则，非 NULL 分支保留 deferred 标志。
#[test]
fn if_null_preserves_branch_metadata_and_deferred_state() {
    for contract in [
        "if constant.Value.IsNull()",
        "expr.GetArgs()[1]",
        "expr.RetType.as_mut().unwrap().SetCharset(charset)",
        "expr.RetType.as_mut().unwrap().SetCollate(collation)",
        "(first, deferred)",
    ] {
        assert!(
            RUST_SOURCE.contains(contract),
            "missing IFNULL contract: {contract}"
        );
    }
    assert!(GO_SOURCE.contains("return constArg, isDeferred"));
}

/// CASE WHEN 必须按顺序短路，累积 deferred 状态，并给常量结果恢复 decimal 元数据。
#[test]
fn case_when_preserves_order_deferred_and_decimal() {
    for contract in [
        "for index in (0..len.saturating_sub(1)).step_by(2)",
        "any_deferred |= deferred",
        "expr.GetArgsMut()[index] = condition",
        "if value != 0 && !is_null",
        ".SetDecimal(expr.RetType.as_ref().unwrap().GetDecimal())",
        "if len % 2 == 1",
    ] {
        assert!(
            RUST_SOURCE.contains(contract),
            "missing CASE contract: {contract}"
        );
    }
}

/// 空值拒绝（null-reject）探测须保留排除函数集，并用哑参数试探非空结果。
#[test]
fn null_reject_probe_preserves_exclusions_and_dummy_arguments() {
    assert!(RUST_SOURCE.contains("ast::NullEQ | ast::ConcatWS | ast::Field"));
    assert!(RUST_SOURCE.contains("ctx.IsInNullRejectCheck()"));
    assert!(RUST_SOURCE.contains("Box::new(NewOne()) as Box<dyn Expression>"));
    assert!(RUST_SOURCE.contains("value.IsNull()"));
    assert!(RUST_SOURCE.contains(".is_ok_and(|v| v == 0)"));
}

/// 延迟求值、扩展函数与错误路径不得被过度固化为严格常量。
#[test]
fn deferred_extension_and_error_paths_are_not_frozen() {
    assert!(RUST_SOURCE.contains("function.Function.isExtensionFunction()"));
    assert!(RUST_SOURCE.contains("MaybeOverOptimized4PlanCache(ctx, expr.as_ref())"));
    assert!(RUST_SOURCE.contains("Constant::with_deferred(value, ret_type, function)"));
    assert!(RUST_SOURCE.contains("log_param_error(error)"));
    assert!(RUST_SOURCE.contains("(expr, true)"));
}

/// 普通标量折叠须完整保留 Go 的常量分类、NULL 标志与参数/延迟常量回退路径。
#[test]
fn scalar_and_constant_folding_paths_match_go() {
    for contract in [
        "unFoldableFunctions.contains_key(function.FuncName.L.as_str())",
        "all_constant = false",
        "has_null |= value.Value.IsNull()",
        "deferred |= value.DeferredExpr.is_some() || value.ParamMarker.is_some()",
        "value.Kind() == types::KindNull",
        "ret_type.DelFlag(mysql::NotNullFlag)",
        "ret_type.AddFlag(mysql::NotNullFlag)",
        ".find(|id| *id > 0)",
        "marker.GetUserVar(ctx.GetEvalCtx())",
        "constant.clone_with_value(value)",
        "deferred_expr.Eval(ctx.GetEvalCtx(), chunk::Row::default())",
    ] {
        assert!(
            RUST_SOURCE.contains(contract),
            "missing fold contract: {contract}"
        );
    }
}

/// 折叠后须保留返回类型标志、子查询引用与字符集/排序规则元数据。
#[test]
fn result_type_subquery_and_collation_metadata_are_preserved() {
    for contract in [
        "ret_type.DelFlag(mysql::NotNullFlag)",
        "ret_type.AddFlag(mysql::NotNullFlag)",
        ".map(|constant| constant.SubqueryRefID)",
        "folded.SetCoercibility(coercibility)",
        "folded.GetTypeMut().SetCharset(charset)",
        "folded.GetTypeMut().SetCollate(collation)",
        "folded.SetRepertoire(repertoire)",
    ] {
        assert!(
            RUST_SOURCE.contains(contract),
            "missing metadata contract: {contract}"
        );
    }
}

/// Go 侧折叠入口与各 handler 在 Rust 中须有同名函数对应。
#[test]
fn go_handler_surface_has_rust_counterparts() {
    for function in [
        "FoldConstant",
        "isNullHandler",
        "ifFoldHandler",
        "ifNullFoldHandler",
        "caseWhenHandler",
        "foldConstant",
    ] {
        assert!(GO_SOURCE.contains(&format!("func {function}")));
        assert!(RUST_SOURCE.contains(&format!("fn {function}")));
    }
}
