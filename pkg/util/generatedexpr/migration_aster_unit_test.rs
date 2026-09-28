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

// 生成列表达式迁移补充单元测试。
//
// 覆盖 `ParseExpression` 的函数形态与 TiDB 语法错误前缀，以及
// `SimpleResolveName` 对嵌套/大小写不敏感列名与缺失列错误文案的 Go 对齐。

use crate::{ParseExpression, SimpleResolveName, ast, model};

/// 构造仅含名称与列名列表的最小 `TableInfo`。
fn table(name: &str, columns: &[&str]) -> model::TableInfo {
    model::TableInfo {
        Name: ast::NewCIStr(name),
        Columns: columns
            .iter()
            .map(|name| model::ColumnInfo {
                Name: ast::NewCIStr(name),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

/// 解析 `json_extract`：函数名小写、参数个数为 2。
#[test]
fn parse_expression_matches_go_function_shape() {
    let node = ParseExpression("json_extract(a, '$.a')").expect("expression should parse");
    let ast::ExprKind::Function { FnName, Args, .. } = node.Kind else {
        panic!("parsed node should be a function call");
    };
    assert_eq!(FnName.L, "json_extract");
    assert_eq!(Args.len(), 2);
}

/// 残缺表达式应返回以 TiDB SQL 语法错误前缀开头的错误。
#[test]
fn parse_expression_reports_tidb_syntax_error() {
    let error = ParseExpression("1 +").expect_err("incomplete expression must fail");
    assert!(
        error
            .to_string()
            .contains("You have an error in your SQL syntax")
    );
    let cause = crate::errors::Cause(Some(&error)).expect("syntax error must have a root cause");
    let terror = cause
        .downcast_ref::<crate::parser_core::terror::Error>()
        .expect("syntax error must retain TiDB terror classification");
    assert_eq!(
        crate::parser_core::terror::ToSQLError(terror).Code,
        crate::parser_core::mysql::ErrParse
    );
}

/// 嵌套二元运算中的列名按小写匹配；解析成功不改写原 AST。
#[test]
fn simple_resolve_name_accepts_nested_and_case_insensitive_columns() {
    let expression = ast::ExprNode::Function(
        ast::CIStr::default(),
        ast::NewCIStr("plus"),
        vec![
            ast::ExprNode::Column(ast::ColumnName {
                Name: ast::NewCIStr("A"),
                ..Default::default()
            }),
            ast::ExprNode::Binary(
                "+".into(),
                Box::new(ast::ExprNode::Column(ast::ColumnName {
                    Name: ast::NewCIStr("b"),
                    ..Default::default()
                })),
                Box::new(ast::ExprNode::Value("1".into())),
            ),
        ],
    );

    let resolved = SimpleResolveName(expression.clone(), &table("t", &["a", "B"]))
        .expect("known columns should resolve");
    assert_eq!(resolved, expression);
}

/// 未知列错误文案对齐 Go：`can't find column <col> in <table>`。
#[test]
fn simple_resolve_name_matches_go_missing_column_error() {
    let expression = ast::ExprNode::Column(ast::ColumnName {
        Name: ast::NewCIStr("missing"),
        ..Default::default()
    });

    let error = SimpleResolveName(expression, &table("widgets", &["present"]))
        .expect_err("unknown column must fail");
    assert_eq!(error.to_string(), "can't find column missing in widgets");
}
