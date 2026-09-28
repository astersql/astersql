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

// 规划器表达式构建与求值单元测试。
//
// 通过解析 SELECT 字段表达式，对比 `evalAstExpr` 与 `PlannerBuildSimpleExpr`
// 的求值结果；覆盖 BETWEEN、CASE、CAST、IN、IS NULL、用户变量、表 schema、
// 参数绑定及各类错误信息的规范化。

use crate::PlannerBuildSimpleExpr;
use crate::expression_rewriter::evalAstExpr;
use expression_dependency::chunk::mutrow::{GoAny, MutRowFromValues};
use expression_dependency::{chunk, exprctx, mysql, types};
use exprstatic_dependency::WithUserVarsReader;
use exprstatic_dependency::{
    NewExprContext, WithColumnIDAllocator, WithEvalCtx, WithOptionalProperty, WithParamList,
};
use parser_ast_dependency::{ExprKind, ExprNode, NewCIStr, SelectStmt, ValueDatum};
use std::sync::Arc;

/// 将表达式源码包进 SELECT 解析，取出第一个字段的 ExprNode。
fn parse_expr(source: &str) -> ExprNode {
    let statement = parser_dependency::New()
        .ParseOneStmt(&format!("select {source}"), "", "")
        .unwrap_or_else(|error| panic!("parse {source:?}: {error}"));
    let select = statement
        .into_any()
        .downcast::<SelectStmt>()
        .expect("SELECT must produce SelectStmt");
    select.Fields.Fields[0]
        .Expr
        .clone()
        .expect("SELECT field must contain an expression")
}

/// 同时走 AST 求值与 BuildSimpleExpr 求值，断言结果一致后返回。
fn evaluate_both(expression: &ExprNode) -> types::Datum {
    let context = NewExprContext(Vec::new());
    let evaluated = evalAstExpr(&context, expression)
        .expect("evalAstExpr must accept the expression produced by the parser");
    let built = PlannerBuildSimpleExpr(&context, expression, Vec::new())
        .expect("BuildSimpleExpr must accept the expression produced by the parser");
    let built_value = built
        .Eval(context.GetEvalCtx(), chunk::Row::default())
        .expect("built expression evaluation must succeed");
    assert_eq!(evaluated.Kind(), built_value.Kind());
    assert_eq!(evaluated.String(), built_value.String());
    evaluated
}

/// 解析并求值表达式，格式化为可断言的文本（NULL 为 `<nil>`）。
fn value_text(expression: &str) -> String {
    let value = evaluate_both(&parse_expr(expression));
    if value.Kind() == types::KindNull {
        "<nil>".to_owned()
    } else if value.Kind() == types::KindString || value.Kind() == types::KindBytes {
        value.GetString()
    } else {
        value.GetInt64().to_string()
    }
}

/// 批量断言 (源表达式, 期望文本) 用例。
fn check_cases(cases: &[(&str, &str)]) {
    for (source, expected) in cases {
        assert_eq!(value_text(source), *expected, "expression {source}");
    }
}

#[derive(Clone)]
/// 测试用用户变量读取器：仅识别 `@a` → `"abc"`。
struct TestUserVariables;

impl exprctx::UserVarsReader for TestUserVariables {
    fn GetUserVarVal(&self, name: &str) -> Option<types::Datum> {
        name.eq_ignore_ascii_case("a")
            .then(|| types::NewStringDatum("abc".to_owned()))
    }

    fn GetUserVarType(&self, name: &str) -> Option<types::FieldType> {
        name.eq_ignore_ascii_case("a")
            .then(|| *types::NewFieldType(mysql::TypeVarString))
    }

    fn Clone(&self) -> Box<dyn exprctx::UserVarsReader> {
        Box::new(Clone::clone(self))
    }
}

#[derive(Clone)]
/// 共享会话变量读取器，验证已构建表达式能够观察后续赋值。
struct SessionUserVariables {
    variables: Arc<variable_dependency::session::SessionVars>,
}

impl exprctx::UserVarsReader for SessionUserVariables {
    fn GetUserVarVal(&self, name: &str) -> Option<types::Datum> {
        self.variables
            .UserVars
            .GetUserVarVal(name)
            .map(types::NewStringDatum)
    }

    fn GetUserVarType(&self, name: &str) -> Option<types::FieldType> {
        self.variables.UserVars.GetUserVarType(name)
    }

    fn Clone(&self) -> Box<dyn exprctx::UserVarsReader> {
        Box::new(Clone::clone(self))
    }
}

#[test]
/// BETWEEN / NOT BETWEEN 及日期字符串比较。
fn test_between() {
    check_cases(&[
        ("1 between 2 and 3", "0"),
        ("1 not between 2 and 3", "1"),
        (
            "'2001-04-10 12:34:56' between cast('2001-01-01 01:01:01' as datetime) and '01-05-01'",
            "1",
        ),
        (
            "20010410123456 between cast('2001-01-01 01:01:01' as datetime) and 010501",
            "0",
        ),
        (
            "20010410123456 between cast('2001-01-01 01:01:01' as datetime) and 20010501123456",
            "1",
        ),
    ]);
}

#[test]
/// CASE WHEN 分支求值。
fn test_case_when() {
    check_cases(&[
        ("case 1 when 1 then 'str1' when 2 then 'str2' end", "str1"),
        ("case 2 when 1 then 'str1' when 2 then 'str2' end", "str2"),
        ("case 3 when 1 then 'str1' when 2 then 'str2' end", "<nil>"),
        (
            "case 4 when 1 then 'str1' when 2 then 'str2' else 'str3' end",
            "str3",
        ),
    ]);

    let mut expression = parse_expr("case 1 when 1 then 1 end");
    assert_eq!(value_text_from_ast(&expression), "1");
    let ExprKind::Case {
        Value: Some(value), ..
    } = &mut expression.Kind
    else {
        panic!("parser must retain the CASE value expression");
    };
    let ExprKind::Value(value) = &mut value.Kind else {
        panic!("CASE value must remain a ValueExpr");
    };
    value.Datum = ValueDatum::Int64(4);
    assert_eq!(value_text_from_ast(&expression), "<nil>");
}

/// 对已有 AST 求值并格式化为文本。
fn value_text_from_ast(expression: &ExprNode) -> String {
    let value = evaluate_both(expression);
    if value.Kind() == types::KindNull {
        "<nil>".to_owned()
    } else if value.Kind() == types::KindString || value.Kind() == types::KindBytes {
        value.GetString()
    } else {
        value.GetInt64().to_string()
    }
}

#[test]
/// CAST 到各类目标类型的基本行为。
fn test_cast() {
    check_cases(&[
        ("cast(1 as signed)", "1"),
        ("cast(1 as unsigned)", "1"),
        ("cast(1 as char binary)", "1"),
        ("cast(1 as char charset utf8mb4)", "1"),
        ("cast(NULL as signed)", "<nil>"),
    ]);
}

#[test]
/// CAST 返回类型不得与 AST FieldType 共享可变状态。
fn test_cast_ret_type_does_not_share_ast_field_type() {
    let context = NewExprContext(Vec::new());
    let table = expression_dependency::model::TableInfo {
        Name: NewCIStr("t"),
        Columns: vec![expression_dependency::model::ColumnInfo {
            Name: NewCIStr("a"),
            Offset: 0,
            State: expression_dependency::model::StatePublic,
            FieldType: *types::NewFieldType(mysql::TypeLonglong),
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut expression = parse_expr("cast(a as signed)");
    let ExprKind::Cast { Tp: target, .. } = &mut expression.Kind else {
        panic!("parser must produce a cast expression");
    };
    target.AddFlag(mysql::NotNullFlag);
    let original = target.clone();

    let mut built = PlannerBuildSimpleExpr(
        &context,
        &expression,
        vec![expression_dependency::WithTableInfo("", &table)],
    )
    .expect("first cast build must succeed");
    let scalar = built
        .as_any_mut()
        .downcast_mut::<expression_dependency::ScalarFunction>()
        .expect("cast must build a ScalarFunction");
    let first_type = scalar.RetType.as_mut().expect("cast has a return type");
    first_type.SetType(mysql::TypeString);
    first_type.AddFlag(mysql::UnsignedFlag);

    let second = PlannerBuildSimpleExpr(
        &context,
        &expression,
        vec![expression_dependency::WithTableInfo("", &table)],
    )
    .expect("second cast build must succeed");
    let second_type = second
        .as_any()
        .downcast_ref::<expression_dependency::ScalarFunction>()
        .and_then(|scalar| scalar.RetType.as_ref())
        .expect("second cast has an independent return type");
    let ExprKind::Cast { Tp: target, .. } = &expression.Kind else {
        unreachable!();
    };
    assert_eq!(target, &original);
    assert_eq!(target.GetType(), mysql::TypeLonglong);
    assert!(mysql::HasNotNullFlag(target.GetFlag()));
    assert_eq!(second_type.GetType(), mysql::TypeLonglong);
    assert!(!mysql::HasNotNullFlag(second_type.GetFlag()));
    assert!(!mysql::HasUnsignedFlag(second_type.GetFlag()));
}

#[test]
/// IN / NOT IN 列表匹配。
fn test_pattern_in() {
    check_cases(&[
        ("1 not in (1, 2, 3)", "0"),
        ("1 in (1, 2, 3)", "1"),
        ("1 in (2, 3)", "0"),
        ("NULL in (2, 3)", "<nil>"),
        ("NULL not in (2, 3)", "<nil>"),
        ("NULL in (NULL, 3)", "<nil>"),
        ("1 in (1, NULL)", "1"),
        ("1 in (NULL, 1)", "1"),
        ("2 in (1, NULL)", "<nil>"),
        ("(-(23)++46/51*+51) in (+23)", "0"),
    ]);
}

#[test]
/// IS NULL / IS NOT NULL。
fn test_is_null() {
    check_cases(&[
        ("1 IS NULL", "0"),
        ("1 IS NOT NULL", "1"),
        ("NULL IS NULL", "1"),
        ("NULL IS NOT NULL", "0"),
    ]);
}

#[test]
/// 行构造比较 (a,b) 与 (c,d)。
fn test_compare_row() {
    check_cases(&[
        ("row(1,2,3)=row(1,2,3)", "1"),
        ("row(1,2,3)=row(1+3,2,3)", "0"),
        ("row(1,2,3)<>row(1,2,3)", "0"),
        ("row(1,2,3)<>row(1+3,2,3)", "1"),
        ("row(1+3,2,3)<>row(1+3,2,3)", "0"),
        ("row(1,2,3)<row(1,NULL,3)", "<nil>"),
        ("row(1,2,3)<row(2,NULL,3)", "1"),
        ("row(1,2,3)>=row(0,NULL,3)", "1"),
        ("row(1,2,3)<=row(2,NULL,3)", "1"),
    ]);
}

#[test]
/// IS TRUE / IS FALSE / IS UNKNOWN。
fn test_is_truth() {
    check_cases(&[
        ("1 IS TRUE", "1"),
        ("2 IS TRUE", "1"),
        ("0 IS TRUE", "0"),
        ("NULL IS TRUE", "0"),
        ("1 IS FALSE", "0"),
        ("2 IS FALSE", "0"),
        ("0 IS FALSE", "1"),
        ("NULL IS NOT FALSE", "1"),
        ("1 IS NOT TRUE", "0"),
        ("2 IS NOT TRUE", "0"),
        ("0 IS NOT TRUE", "1"),
        ("NULL IS NOT TRUE", "1"),
        ("1 IS NOT FALSE", "1"),
        ("2 IS NOT FALSE", "1"),
        ("0 IS NOT FALSE", "0"),
        ("NULL IS NOT FALSE", "1"),
    ]);
}

#[test]
/// BuildSimpleExpr 错误信息应与规范文案一致。
fn test_build_expression_errors_are_canonical() {
    let context = NewExprContext(Vec::new());
    let expression = parse_expr("1+a");
    let error = match PlannerBuildSimpleExpr(&context, &expression, Vec::new()) {
        Ok(_) => panic!("unknown column must fail expression rewriting"),
        Err(error) => error,
    };
    assert_eq!(
        error.to_string(),
        "[planner:1054]Unknown column 'a' in 'expression'"
    );

    let table = build_expression_table();
    let error = PlannerBuildSimpleExpr(
        &context,
        &parse_expr("(1+a)*(3+b+c)"),
        vec![expression_dependency::WithTableInfo("", &table)],
    )
    .err()
    .expect("unknown column in a table-backed expression must fail");
    assert_eq!(
        error.to_string(),
        "[planner:1054]Unknown column 'c' in 'expression'"
    );

    let expression = parse_expr("cast(1 as signed array)");
    let error = match PlannerBuildSimpleExpr(&context, &expression, Vec::new()) {
        Ok(_) => panic!("CAST AS ARRAY outside a functional index must fail"),
        Err(error) => error,
    };
    assert_eq!(
        error.to_string(),
        "[expression:1235]This version of TiDB doesn't yet support 'Use of CAST( .. AS .. ARRAY) outside of functional index in CREATE(non-SELECT)/ALTER TABLE or in general expressions'"
    );

    let user_write_error = PlannerBuildSimpleExpr(&context, &parse_expr("@a := 1"), Vec::new())
        .err()
        .expect("writing a user variable requires session variables");
    assert_eq!(
        user_write_error.to_string(),
        "rewriting user variable requires 'OptPropSessionVars' in evalCtx"
    );

    for (source, expected) in [
        (
            "@@tidb_enable_async_commit",
            "planCtx is required when rewriting node: '*ast.VariableExpr', accessing system variable requires plan context",
        ),
        (
            "@@global.tidb_enable_async_commit",
            "planCtx is required when rewriting node: '*ast.VariableExpr', accessing system variable requires plan context",
        ),
    ] {
        let expression = parse_expr(source);
        let error = PlannerBuildSimpleExpr(&context, &expression, Vec::new())
            .err()
            .unwrap_or_else(|| panic!("{source} must require plan context"));
        assert_eq!(error.to_string(), expected, "{source}");
    }
}

#[test]
/// 子查询相关构建错误信息规范化。
fn test_build_expression_subquery_error_is_canonical() {
    let context = NewExprContext(Vec::new());
    let table = build_expression_table();
    let error = PlannerBuildSimpleExpr(
        &context,
        &parse_expr("a + (select b from t)"),
        vec![expression_dependency::WithTableInfo("", &table)],
    )
    .err()
    .expect("subquery requires plan context");
    assert_eq!(
        error.to_string(),
        "planCtx is required when rewriting node: '*ast.SubqueryExpr'"
    );
}

#[test]
/// 读取用户变量 `@a`。
fn test_build_expression_reads_user_variables() {
    let base = NewExprContext(Vec::new());
    let context = base
        .Apply(vec![WithEvalCtx(Arc::new(base.GetEvalCtx().Apply(vec![
            WithUserVarsReader(Box::new(TestUserVariables)),
        ])))]);
    let expression = PlannerBuildSimpleExpr(&context, &parse_expr("@a"), Vec::new())
        .expect("read user variable");
    let value = expression
        .Eval(context.GetEvalCtx(), chunk::Row::default())
        .expect("evaluate user variable");
    assert_eq!(value.Kind(), types::KindString);
    assert_eq!(value.GetString(), "abc");
}

#[test]
/// 赋值用户变量并读回。
fn test_build_expression_writes_user_variables() {
    let variables = Arc::new(variable_dependency::session::SessionVars::new());
    let provider =
        expression_expropt_dependency::SessionVarsPropProvider::new(Arc::clone(&variables));
    let base = NewExprContext(Vec::new());
    let context = base.Apply(vec![WithEvalCtx(Arc::new(base.GetEvalCtx().Apply(vec![
        WithUserVarsReader(Box::new(SessionUserVariables {
            variables: Arc::clone(&variables),
        })),
        WithOptionalProperty(vec![Box::new(provider)]),
    ])))]);
    let read_expression = PlannerBuildSimpleExpr(&context, &parse_expr("@a"), Vec::new())
        .expect("build user-variable reader before assignment");
    let expression = PlannerBuildSimpleExpr(&context, &parse_expr("@a := 'def'"), Vec::new())
        .expect("session variables allow user-variable assignment");
    let value = expression
        .Eval(context.GetEvalCtx(), chunk::Row::default())
        .expect("evaluate user-variable assignment");
    assert_eq!(value.Kind(), types::KindString);
    assert_eq!(value.GetString(), "def");
    assert_eq!(
        variables.UserVars.GetUserVarVal("a").as_deref(),
        Some("def")
    );
    let read_value = read_expression
        .Eval(context.GetEvalCtx(), chunk::Row::default())
        .expect("the existing user-variable reader sees the assignment");
    assert_eq!(read_value.Kind(), types::KindString);
    assert_eq!(read_value.GetString(), "def");
}

/// 构造带若干列的 TableInfo，供 schema 行/CAST/参数测试。
fn build_expression_table() -> expression_dependency::model::TableInfo {
    use expression_dependency::model::{ColumnInfo, DefaultValue, StatePublic, TableInfo};

    TableInfo {
        Name: NewCIStr("t"),
        Columns: vec![
            ColumnInfo {
                Name: NewCIStr("id"),
                Offset: 0,
                State: StatePublic,
                FieldType: *types::NewFieldType(mysql::TypeString),
                DefaultIsExpr: true,
                DefaultValue: Some(DefaultValue::String(b"uuid()".to_vec())),
                ..Default::default()
            },
            ColumnInfo {
                Name: NewCIStr("a"),
                Offset: 1,
                State: StatePublic,
                FieldType: *types::NewFieldType(mysql::TypeLonglong),
                ..Default::default()
            },
            ColumnInfo {
                Name: NewCIStr("b"),
                Offset: 2,
                State: StatePublic,
                FieldType: *types::NewFieldType(mysql::TypeLonglong),
                DefaultValue: Some(DefaultValue::String(b"123".to_vec())),
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

#[test]
/// 表 schema 列引用、行值、CAST 与预处理参数绑定。
fn test_build_expression_table_schema_rows_cast_and_parameters() {
    crate::InstallPlannerExpressionFactory().expect("install planner expression factory");
    let table = build_expression_table();
    let base_context = NewExprContext(Vec::new());
    let eval_context = base_context.GetEvalCtx();
    let (columns, names) = expression_dependency::ColumnInfos2ColumnsAndNames(
        &base_context,
        NewCIStr(""),
        table.Name.clone(),
        &table.Columns,
        &table,
    )
    .expect("table columns become expression schema");
    let schema = expression_dependency::NewSchema(columns);
    let ast = parse_expr("(1+a)*(3+b)");
    let context = base_context.Apply(vec![WithColumnIDAllocator(Arc::new(
        exprctx::NewSimplePlanColumnIDAllocator(0),
    ))]);
    let from_table = PlannerBuildSimpleExpr(
        &context,
        &ast,
        vec![expression_dependency::WithTableInfo("", &table)],
    )
    .expect("build using table metadata");
    assert_eq!(
        from_table.GetType(eval_context).GetType(),
        mysql::TypeLonglong
    );
    let parse_context = base_context.Apply(vec![WithColumnIDAllocator(Arc::new(
        exprctx::NewSimplePlanColumnIDAllocator(0),
    ))]);
    let parsed = expression_dependency::ParseSimpleExpr(
        &parse_context,
        "(1+a)*(3+b)",
        vec![expression_dependency::WithTableInfo("", &table)],
    )
    .expect("ParseSimpleExpr uses the installed planner bridge");
    assert!(from_table.Equal(eval_context, parsed.as_ref()));

    for (a, b, expected) in [(1, 2, 10), (3, 4, 28)] {
        let mutable_row = MutRowFromValues(vec![
            GoAny::String(String::new()),
            GoAny::Int64(a),
            GoAny::Int64(b),
        ]);
        let row = mutable_row.ToRow();
        assert_eq!(
            from_table.EvalInt(eval_context, row.clone()).unwrap(),
            (expected, false)
        );
        assert_eq!(
            parsed.EvalInt(eval_context, row).unwrap(),
            (expected, false)
        );
    }

    let from_schema = PlannerBuildSimpleExpr(
        &context,
        &ast,
        vec![expression_dependency::WithInputSchemaAndNames(
            &schema,
            names.Shallow(),
            None,
        )],
    )
    .expect("build using explicit schema and names");
    let mutable_row = MutRowFromValues(vec![
        GoAny::String(String::new()),
        GoAny::Int64(1),
        GoAny::Int64(2),
    ]);
    let row = mutable_row.ToRow();
    assert_eq!(from_schema.EvalInt(eval_context, row).unwrap(), (10, false));

    let cast_target = types::NewFieldType(mysql::TypeVarchar);
    let cast = PlannerBuildSimpleExpr(
        &context,
        &parse_expr("1+2+3"),
        vec![expression_dependency::WithCastExprTo(&cast_target)],
    )
    .expect("WithCastExprTo wraps the expression");
    let value = cast.Eval(eval_context, chunk::Row::default()).unwrap();
    assert_eq!(cast.GetType(eval_context).GetType(), mysql::TypeVarchar);
    assert_eq!(value.Kind(), types::KindString);
    assert_eq!(value.GetString(), "6");

    let parameter_context = context.Apply(vec![WithEvalCtx(Arc::new(
        eval_context.Apply(vec![WithParamList(vec![types::NewIntDatum(5)])]),
    ))]);
    let parameter = PlannerBuildSimpleExpr(
        &parameter_context,
        &parse_expr("a + ?"),
        vec![expression_dependency::WithTableInfo("", &table)],
    )
    .expect("parameter marker uses the evaluation parameter list");
    assert_eq!(
        parameter.GetType(parameter_context.GetEvalCtx()).GetType(),
        mysql::TypeLonglong
    );
    let mutable_row = MutRowFromValues(vec![
        GoAny::String(String::new()),
        GoAny::Int64(2),
        GoAny::Int64(3),
    ]);
    let row = mutable_row.ToRow();
    let value = parameter
        .Eval(parameter_context.GetEvalCtx(), row)
        .expect("evaluate parameter expression");
    assert_eq!(value.Kind(), types::KindInt64);
    assert_eq!(value.GetInt64(), 7);
}

#[test]
/// 缺少列源时的错误信息规范化。
fn test_build_expression_missing_source_error_is_canonical() {
    let table = build_expression_table();
    let context = NewExprContext(Vec::new());
    let (columns, names) = expression_dependency::ColumnInfos2ColumnsAndNames(
        &context,
        NewCIStr(""),
        table.Name.clone(),
        &table.Columns,
        &table,
    )
    .expect("table columns become expression schema");
    let schema = expression_dependency::NewSchema(columns);
    let error = PlannerBuildSimpleExpr(
        &context,
        &parse_expr("default(b)"),
        vec![expression_dependency::WithInputSchemaAndNames(
            &schema, names, None,
        )],
    )
    .err()
    .expect("DEFAULT with an input schema but no source table must fail");
    assert_eq!(
        error.to_string(),
        "Unsupported expr *ast.DefaultExpr when source table not provided"
    );
}

#[test]
/// CAST 到 ARRAY 类型。
fn test_build_expression_cast_array() {
    let context = NewExprContext(Vec::new());
    let eval_context = context.GetEvalCtx();

    let array = PlannerBuildSimpleExpr(
        &context,
        &parse_expr("cast(json_extract('{\"a\": [1, 2, 3]}', '$.a') as signed array)"),
        vec![expression_dependency::WithAllowCastArray(true)],
    )
    .expect("WithAllowCastArray enables functional-index array casts");
    let (json, is_null) = array
        .EvalJSON(eval_context, chunk::Row::default())
        .expect("evaluate JSON array cast");
    assert!(!is_null);
    assert_eq!(json.TypeCode, types::JSONTypeCodeArray);
    assert_eq!(json.String(), "[1, 2, 3]");
}

#[test]
/// DEFAULT 常量表达式。
fn test_build_expression_constant_default() {
    let context = NewExprContext(Vec::new());
    let eval_context = context.GetEvalCtx();
    let table = build_expression_table();

    let default_b = PlannerBuildSimpleExpr(
        &context,
        &parse_expr("default(b)"),
        vec![expression_dependency::WithTableInfo("", &table)],
    )
    .expect("constant column default builds");
    let value = default_b
        .Eval(eval_context, chunk::Row::default())
        .expect("evaluate constant default");
    assert_eq!(value.Kind(), types::KindInt64);
    assert_eq!(value.GetInt64(), 123);
}

#[test]
/// 表达式形式的 DEFAULT。
fn test_build_expression_expression_default() {
    let context = NewExprContext(Vec::new());
    let eval_context = context.GetEvalCtx();
    let table = build_expression_table();

    let default_id = PlannerBuildSimpleExpr(
        &context,
        &parse_expr("default(id)"),
        vec![expression_dependency::WithTableInfo("", &table)],
    )
    .expect("expression column default builds");
    let (uuid, is_null) = default_id
        .EvalString(eval_context, chunk::Row::default())
        .expect("evaluate UUID default");
    assert!(!is_null);
    assert_eq!(uuid.len(), 36, "{uuid}");

    let (columns, names) = expression_dependency::ColumnInfos2ColumnsAndNames(
        &context,
        NewCIStr(""),
        table.Name.clone(),
        &table.Columns,
        &table,
    )
    .expect("table columns become expression schema");
    let schema = expression_dependency::NewSchema(columns);
    let default_id = PlannerBuildSimpleExpr(
        &context,
        &parse_expr("default(id)"),
        vec![expression_dependency::WithInputSchemaAndNames(
            &schema,
            names,
            Some(&table),
        )],
    )
    .expect("expression default builds with an explicit input schema");
    let (uuid, is_null) = default_id
        .EvalString(eval_context, chunk::Row::default())
        .expect("evaluate input-schema UUID default");
    assert!(!is_null);
    assert_eq!(uuid.len(), 36, "{uuid}");
}
