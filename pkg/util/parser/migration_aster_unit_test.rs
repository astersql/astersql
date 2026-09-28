// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 迁移期单元测试：用真实 SQL AST 验证 matcher / 池 / restore。
//
// 覆盖 Match/Space/Digit/Number、单字节匹配、Parser 池复用、GetDefaultDB、
// SimpleCases 守卫，以及 RestoreWithDefaultDB / RestoreWithoutDB 的标志传递。

#![allow(non_snake_case, non_upper_case_globals)]

#[path = "ast.rs"]
mod ast;
#[path = "parser.rs"]
mod parser;

fn parse(sql: &str) -> Box<dyn crate::parser_core::ast::Node> {
    crate::parser_core::New()
        .ParseOneStmt(sql, "", "")
        .expect("SQL should parse")
}

/// 验证 Match/Space/Space0/Digit/Number 与 Go 成功/失败行为一致。
#[test]
fn parser_matchers_match_go_success_and_failure_behavior() {
    assert_eq!(
        parser::Match("123abc", |byte| byte.is_ascii_digit(), 2),
        (b"123".to_vec(), b"abc".to_vec(), None)
    );
    assert_eq!(
        parser::Match("1abc", |byte| byte.is_ascii_digit(), 2),
        (
            Vec::new(),
            b"1abc".to_vec(),
            Some(parser::ErrPatternNotMatch)
        )
    );
    assert_eq!(
        parser::Match("abc", |byte| byte.is_ascii_digit(), -1),
        (Vec::new(), b"abc".to_vec(), None)
    );
    assert_eq!(parser::Space(" \t\nrest", 3), (b"rest".to_vec(), None));
    assert_eq!(
        parser::Space("x", 1),
        (b"x".to_vec(), Some(parser::ErrPatternNotMatch))
    );
    assert_eq!(parser::Space0("x"), b"x".to_vec());
    assert_eq!(
        parser::Digit("456 121", 3),
        (b"456".to_vec(), b" 121".to_vec(), None)
    );
    assert_eq!(parser::Number("123abc"), (123, b"abc".to_vec(), None));
    assert_eq!(
        parser::Number("abc"),
        (
            0,
            b"abc".to_vec(),
            Some(parser::ParseError::PatternNotMatch)
        )
    );
    let overflow = format!("{}x", "9".repeat(100));
    let (number, rest, error) = parser::Number(&overflow);
    assert_eq!(isize::MAX, number);
    assert_eq!(b"x".to_vec(), rest);
    assert!(matches!(error, Some(parser::ParseError::InvalidNumber(_))));
}

/// 验证 AnyPunct/AnyChar/Char 单字节匹配器行为。
#[test]
fn single_byte_matchers_match_go_behavior() {
    assert_eq!(parser::AnyPunct(",rest"), (b"rest".to_vec(), None));
    assert_eq!(parser::AnyPunct([0xA1, b'x']), (b"x".to_vec(), None));
    assert_eq!(
        parser::AnyPunct("arest"),
        (b"arest".to_vec(), Some(parser::ErrPatternNotMatch))
    );
    assert_eq!(parser::AnyChar("1int"), (b"int".to_vec(), None));
    assert_eq!(parser::Char("int", b'i'), (b"nt".to_vec(), None));
    assert_eq!(
        parser::Char("int", b'x'),
        (b"int".to_vec(), Some(parser::ErrPatternNotMatch))
    );
    assert_eq!(
        parser::AnyChar(""),
        (Vec::new(), Some(parser::ErrPatternNotMatch))
    );
    assert_eq!(parser::AnyChar("é"), ("é".as_bytes()[1..].to_vec(), None));
}

/// 验证 GetParser/DestroyParser 能复位并复用真实 Parser 实例。
#[test]
fn parser_pool_resets_and_reuses_real_parser_values() {
    let parser = parser::GetParser();
    parser::DestroyParser(parser);
    let parser = parser::GetParser();
    parser::DestroyParser(parser);
}

/// 验证 GetDefaultDB：全表有 schema 返回空；存在隐式表则返回默认库。
#[test]
fn default_database_matches_go_visitor_result() {
    let explicit = parse("select a from db.t");
    let implicit = parse("select a from t");
    assert_eq!(ast::GetDefaultDB(explicit.as_ref(), "test"), "");
    assert_eq!(ast::GetDefaultDB(implicit.as_ref(), "test"), "test");
}

/// 验证 SimpleCases 成功用例与 has_select 等守卫导致的失败路径。
#[test]
fn simple_insert_fast_path_matches_go_cases_and_guards() {
    for (sql, _schema, expected) in [
        (
            "insert into t values(1, 2)",
            "",
            "insert into test.t values(1, 2)",
        ),
        (
            "insert into mydb.t values(1, 2)",
            "mydb",
            "insert into mydb.t values(1, 2)",
        ),
        (
            "insert into t(a, b) values(1, 2)",
            "",
            "insert into test.t(a, b) values(1, 2)",
        ),
        (
            "insert into value value(2, 3)",
            "",
            "insert into test.value value(2, 3)",
        ),
    ] {
        let statement = parse(sql);
        assert_eq!(
            ast::SimpleCases(statement.as_ref(), "test", sql),
            (expected.into(), true)
        );
    }

    let statement = parse("insert into t select 1");
    assert_eq!(
        ast::SimpleCases(statement.as_ref(), "test", "insert into t select 1"),
        (String::new(), false)
    );
}

/// 验证真实 AST restore 补全/移除 schema，且不支持节点的错误吞并为空串。
#[test]
fn restore_fallback_passes_go_flags_and_swallows_errors() {
    let statement = parse("select a from t where a = 1");
    assert_eq!(
        ast::RestoreWithDefaultDB(statement.as_ref(), "test", "select a from t where a = 1"),
        "SELECT `a` FROM `test`.`t` WHERE `a` = 1"
    );
    assert_eq!(
        ast::RestoreWithoutDB(statement.as_ref()),
        "SELECT `a` FROM `t` WHERE `a` = 1"
    );

    let unsupported = crate::parser_core::ast::DoStmt::default();
    assert_eq!(ast::RestoreWithDefaultDB(&unsupported, "test", "do 1"), "");
}
