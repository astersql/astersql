// Copyright 2026 AsterSQL.

// constraint crate 迁移期单元测试。
//
// 验证 `DeleteTrueExprs` / `DeleteTrueExprsBySchema`：仅删除可安全证明为真的
// 常量与「NOT NULL 列上的 NOT(ISNULL(...))」谓词，并在计划缓存可变常量场景保留条件。

use crate::{DeleteTrueExprs, DeleteTrueExprsBySchema, ast, expression, mysql, stmtctx};
use types_crate::{Datum, NewFieldType, NewIntDatum, NewStringDatum};

/// 构造常量表达式；`mutable` 模拟计划缓存参数占位。
fn constant(value: Datum, mutable: bool) -> expression::Expression {
    expression::Expression::Constant(expression::Constant {
        Value: value,
        mutable,
    })
}

/// 构造列；`not_null` 为 true 时打上 MySQL NOT NULL 标志。
fn column(unique_id: i64, not_null: bool) -> expression::Column {
    let mut field_type = NewFieldType(mysql::TypeLonglong);
    if not_null {
        field_type.AddFlag(mysql::NotNullFlag);
    }
    expression::Column {
        UniqueID: unique_id,
        RetType: field_type,
    }
}

/// 构造标量函数表达式。
fn function(name: &str, args: Vec<expression::Expression>) -> expression::Expression {
    expression::Expression::ScalarFunction(expression::ScalarFunction {
        FuncName: expression::FuncName { L: name.to_owned() },
        Args: args,
    })
}

/// 仅删除求值恰为 1 的不可变常量；0/NULL/错误/其它表达式均保留；缓存可变常量也保留。
#[test]
fn deletes_only_safely_true_constants() {
    let statement = stmtctx::NewStmtCtx();
    // 1 可删；0、默认 Datum、非数字字符串、Other 均保留 → 剩 4 个。
    let conditions = vec![
        constant(NewIntDatum(1), false),
        constant(NewIntDatum(0), false),
        constant(Datum::default(), false),
        constant(NewStringDatum("not-a-number".to_owned()), false),
        expression::Expression::Other,
    ];
    let retained = DeleteTrueExprs(&Default::default(), &statement, conditions);
    assert_eq!(retained.len(), 4);
    assert!(matches!(
        &retained[0],
        expression::Expression::Constant(value)
            if value.Value.ToBool(statement.TypeCtx()) == Ok(0)
    ));
    assert!(matches!(
        &retained[1],
        expression::Expression::Constant(value)
            if value.Value.IsNull()
    ));
    assert!(matches!(
        &retained[2],
        expression::Expression::Constant(value)
            if !value.Value.IsNull() && value.Value.ToBool(statement.TypeCtx()).is_err()
    ));
    assert!(matches!(&retained[3], expression::Expression::Other));

    // 计划缓存 + 可变常量：即使值为 1 也不删。
    let cached = DeleteTrueExprs(
        &expression::BuildContext {
            use_plan_cache: true,
        },
        &statement,
        vec![constant(NewIntDatum(1), true)],
    );
    assert_eq!(cached.len(), 1);
    assert!(matches!(
        &cached[0],
        expression::Expression::Constant(value) if value.mutable
    ));
}

/// 仅当列在 Schema 中且带 NOT NULL 时，才删除 `NOT(ISNULL(col))`。
#[test]
fn deletes_not_isnull_only_for_schema_not_null_column() {
    let context = expression::EvalContext;
    // 列 7 为 NOT NULL；列 8 可空。
    let schema = expression::Schema {
        Columns: vec![column(7, true), column(8, false)],
    };
    let not_is_null = |unique_id| {
        function(
            ast::UnaryNot,
            vec![function(
                ast::IsNull,
                vec![expression::Expression::Column(column(unique_id, false))],
            )],
        )
    };
    // 仅 unique_id=7 被删；可空列、未知列、Other 保留 → 剩 3 个。
    let retained = DeleteTrueExprsBySchema(
        &context,
        &schema,
        vec![
            not_is_null(7),
            not_is_null(8),
            not_is_null(99),
            expression::Expression::Other,
        ],
    );
    assert_eq!(retained.len(), 3);
    assert!(matches!(
        &retained[0],
        expression::Expression::ScalarFunction(value)
            if value.GetArgs()[0].as_scalar_function().unwrap().GetArgs()[0]
                .as_column().unwrap().UniqueID == 8
    ));
    assert!(matches!(
        &retained[1],
        expression::Expression::ScalarFunction(value)
            if value.GetArgs()[0].as_scalar_function().unwrap().GetArgs()[0]
                .as_column().unwrap().UniqueID == 99
    ));
    assert!(matches!(&retained[2], expression::Expression::Other));
}

/// 函数名或参数形状不精确匹配 Go 的嵌套类型断言时，条件必须原样保留。
#[test]
fn retains_non_matching_function_shapes_in_original_order() {
    let context = expression::EvalContext;
    let schema = expression::Schema {
        Columns: vec![column(7, true)],
    };
    let conditions = vec![
        function(ast::UnaryNot, vec![]),
        function(
            ast::UnaryNot,
            vec![function(
                ast::IsNull,
                vec![
                    expression::Expression::Column(column(7, false)),
                    expression::Expression::Other,
                ],
            )],
        ),
        function(
            ast::UnaryNot,
            vec![function(
                "isnotnull",
                vec![expression::Expression::Column(column(7, false))],
            )],
        ),
        function(
            ast::IsNull,
            vec![expression::Expression::Column(column(7, false))],
        ),
    ];

    let retained = DeleteTrueExprsBySchema(&context, &schema, conditions);

    assert_eq!(retained.len(), 4);
    let names: Vec<&str> = retained
        .iter()
        .map(|item| item.as_scalar_function().unwrap().FuncName.L.as_str())
        .collect();
    assert_eq!(
        names,
        [ast::UnaryNot, ast::UnaryNot, ast::UnaryNot, ast::IsNull]
    );
}
