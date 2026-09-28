// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

// 表达式 AST 节点的单元测试。
//
// 验证 BETWEEN/CASE/ROW 还原、二元运算括号与优先级、列名 Schema/表名/CTE
// （公用表表达式）省略规则、全文 MATCH...AGAINST 修饰符校验，以及 Visitor
// （访问者模式）的 enter/leave 遍历顺序。

use crate::expressions::*;

/// 构造原始 SQL 片段叶子表达式，便于拼装复合表达式。
fn raw(sql: &str) -> Expr {
    Expr::Raw(sql.to_owned())
}

/// 构造仅含列名的 `ColumnNameExpr`。
fn column(name: &str) -> Expr {
    Expr::ColumnName(ColumnNameExpr::new(ColumnName::new("", "", name)))
}

/// 校验 BETWEEN、CASE、ROW 等常见表达式形态的还原文本。
#[test]
fn restores_go_expression_shapes() {
    let between = Expr::Between(BetweenExpr {
        expr: Box::new(column("b")),
        left: Box::new(Expr::Value(Value::Int(1))),
        right: Box::new(Expr::Value(Value::Int(2))),
        not: true,
    });
    assert_eq!(between.to_sql(), "`b` NOT BETWEEN 1 AND 2");

    let case_expr = Expr::Case(CaseExpr {
        value: None,
        when_clauses: vec![WhenClause {
            expr: Expr::Value(Value::Int(1)),
            result: Expr::Value(Value::String("a".into())),
        }],
        else_clause: Some(Box::new(Expr::Value(Value::Bool(false)))),
    });
    assert_eq!(
        case_expr.to_sql(),
        "CASE WHEN 1 THEN _UTF8MB4'a' ELSE FALSE END"
    );

    let row = Expr::Row(RowExpr {
        values: vec![Expr::Value(Value::Int(1)), column("col2")],
    });
    assert_eq!(row.to_sql(), "ROW(1,`col2`)");
}

/// 校验在 SKIP_REDUNDANT_PARENTHESES / SPACES_AROUND_BINARY 标志下的括号与空格。
#[test]
fn restores_precedence_and_parentheses_like_go() {
    let expr = Expr::Binary(BinaryOperationExpr {
        op: Op::Mul,
        left: Box::new(Expr::Parentheses(ParenthesesExpr {
            expr: Box::new(Expr::Binary(BinaryOperationExpr {
                op: Op::Plus,
                left: Box::new(column("a")),
                right: Box::new(column("b")),
            })),
        })),
        right: Box::new(column("c")),
    });
    assert_eq!(
        expr.to_sql_with_flags(RestoreFlags::SKIP_REDUNDANT_PARENTHESES),
        "(`a`+`b`)*`c`"
    );

    let expr = Expr::Binary(BinaryOperationExpr {
        op: Op::Plus,
        left: Box::new(column("a")),
        right: Box::new(Expr::Parentheses(ParenthesesExpr {
            expr: Box::new(Expr::Binary(BinaryOperationExpr {
                op: Op::Mul,
                left: Box::new(column("b")),
                right: Box::new(column("c")),
            })),
        })),
    });
    assert_eq!(
        expr.to_sql_with_flags(RestoreFlags::SKIP_REDUNDANT_PARENTHESES),
        "`a`+`b`*`c`"
    );
    assert_eq!(
        expr.to_sql_with_flags(RestoreFlags::SPACES_AROUND_BINARY),
        "`a` + (`b` * `c`)"
    );
}

/// 列名还原应尊重 WITHOUT_SCHEMA_NAME 与 CTE 名集合（跳过 schema）。
#[test]
fn column_restore_honors_schema_table_and_cte_flags() {
    let name = ColumnName::new("db", "t", "c");
    assert_eq!(name.to_sql(), "`db`.`t`.`c`");
    assert_eq!(
        name.to_sql_with_flags(RestoreFlags::WITHOUT_SCHEMA_NAME),
        "`t`.`c`"
    );
    let mut ctx = RestoreCtx::new(RestoreFlags::empty());
    ctx.cte_names.insert("t".into());
    name.restore(&mut ctx).unwrap();
    assert_eq!(ctx.finish(), "`t`.`c`");
    assert!(ColumnName::new("", "t", "c").matches(&ColumnName::new("db", "t", "c")));
}

/// 全文检索修饰符：BOOLEAN MODE 与 QUERY EXPANSION 互斥校验。
#[test]
fn fulltext_modifier_matches_go_validation() {
    assert!(!FulltextSearchModifier::QUERY_EXPANSION.is_boolean_mode());
    assert!(FulltextSearchModifier::QUERY_EXPANSION.with_query_expansion());

    let expr = MatchAgainst {
        column_names: vec![ColumnName::new("", "", "content")],
        against: Box::new(Expr::Value(Value::String("search".into()))),
        modifier: FulltextSearchModifier::BOOLEAN_MODE,
    };
    assert_eq!(
        Expr::MatchAgainst(expr).to_sql(),
        "MATCH (`content`) AGAINST (_UTF8MB4'search' IN BOOLEAN MODE)"
    );

    let invalid = Expr::MatchAgainst(MatchAgainst {
        column_names: vec![ColumnName::new("", "", "content")],
        against: Box::new(raw("'search'")),
        modifier: FulltextSearchModifier::BOOLEAN_MODE | FulltextSearchModifier::QUERY_EXPANSION,
    });
    assert_eq!(
        invalid.try_to_sql().unwrap_err(),
        "BOOLEAN MODE doesn't support QUERY EXPANSION"
    );
}

/// 记录 enter/leave 事件序列，用于断言 AST 遍历顺序。
#[derive(Default)]
struct TraceVisitor(Vec<&'static str>);

impl Visitor for TraceVisitor {
    fn enter(&mut self, node: &mut Expr) -> bool {
        self.0.push(match node {
            Expr::Between(_) => "enter-between",
            Expr::Raw(_) => "enter-leaf",
            _ => "enter-other",
        });
        false
    }

    fn leave(&mut self, node: &mut Expr) -> bool {
        self.0.push(match node {
            Expr::Between(_) => "leave-between",
            Expr::Raw(_) => "leave-leaf",
            _ => "leave-other",
        });
        true
    }
}

/// Visitor 应保持 enter → 子节点 → leave 的深度优先顺序。
#[test]
fn visitor_keeps_enter_children_leave_order() {
    let mut expr = Expr::Between(BetweenExpr {
        expr: Box::new(raw("x")),
        left: Box::new(raw("l")),
        right: Box::new(raw("r")),
        not: false,
    });
    let mut visitor = TraceVisitor::default();
    assert!(expr.accept(&mut visitor));
    assert_eq!(
        visitor.0,
        vec![
            "enter-between",
            "enter-leaf",
            "leave-leaf",
            "enter-leaf",
            "leave-leaf",
            "enter-leaf",
            "leave-leaf",
            "leave-between"
        ]
    );
}
