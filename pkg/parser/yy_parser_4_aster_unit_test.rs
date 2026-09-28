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

// `yy_parser` 的单元测试：覆盖特殊注释剥离、数字字面量转换、ParseParam、
// 语法接受边界以及事务相关 AST 规约是否与 Go 行为一致。

// 审计映射：辅助函数及 ParseParam 对应 yy_parser.go；SQL 边界与注释
// 对应 parser_test.go 的 TestSimple/TestSpecialComments（主体覆盖在 parser_test.rs）。
// 警告透传对应 lexer.go 的 startWithSlash 与 ParseSQL；TiDB/事务 AST
// 对应 parser.y 的 AdminStmt、BRIEStmt、TraceStmt、Begin/Commit/Use 规约。
// 全部使用真实 Parser/Scanner，无模拟、并发或外部资源生命周期。
use super::*;

/// 验证 TrimComment 按 MySQL 版本化特殊注释规则剥离首尾标记。
#[test]
fn trim_comment_matches_mysql_special_comment_rules() {
    assert_eq!(TrimComment("/*! SELECT 1 */"), "SELECT 1");
    assert_eq!(TrimComment("/*!40101 SET NAMES utf8 */"), "SET NAMES utf8");
    assert_eq!(
        TrimComment("/*!M100301 SET sql_mode='' */"),
        "SET sql_mode=''"
    );
    // Go applies specCodeEnd independently from specCodeStart.
    // 普通注释只剥尾部 `*/`，与 Go 独立应用两个正则的行为一致。
    assert_eq!(TrimComment("/* ordinary */"), "/* ordinary");
}

/// 验证 ParseErrorWith 将上下文截断到 ErrTextLength 并格式化为 near/line。
#[test]
fn parse_error_matches_go_truncation_and_line_format() {
    let input = "x".repeat(mysql::ErrTextLength as usize + 10);
    let err = ParseErrorWith(&input, 7);
    assert_eq!(
        err.to_string(),
        format!("near '{}' at line 7", "x".repeat(80))
    );
}

/// 验证 toInt/toFloat 与 getUint64FromNUM/getInt64FromNUM 的有符号、无符号与溢出行为。
#[test]
fn numeric_helpers_preserve_go_signed_unsigned_and_range_behavior() {
    assert_eq!(getUint64FromNUM(&12_i64), 12);
    assert_eq!(getUint64FromNUM(&u64::MAX), u64::MAX);
    assert_eq!(getUint64FromNUM(&"not numeric"), 0);

    assert_eq!(getInt64FromNUM(&-9_i64), (-9, String::new()));
    let (value, message) = getInt64FromNUM(&u64::MAX);
    assert_eq!(value, -1);
    assert_eq!(
        message,
        "18446744073709551615 is out of range [–9223372036854775808,9223372036854775807]"
    );
    assert_eq!(
        getInt64FromNUM(&"abc").1,
        "%!d(string=abc) is out of range [–9223372036854775808,9223372036854775807]"
    );

    let mut scanner = Scanner::default();
    let mut value = yySymType::default();
    assert_eq!(
        toInt(&mut scanner, &mut value, "9223372036854775808"),
        intLit
    );
    assert_eq!(
        *value
            .item
            .as_deref()
            .unwrap()
            .downcast_ref::<u64>()
            .unwrap(),
        9_223_372_036_854_775_808
    );
    assert_eq!(
        toInt(&mut scanner, &mut value, "18446744073709551616"),
        decLit
    );
    assert_eq!(
        value
            .item
            .as_deref()
            .unwrap()
            .downcast_ref::<parser_test_driver::MyDecimal>()
            .unwrap()
            .String(),
        "18446744073709551616"
    );
    assert_eq!(toFloat(&mut scanner, &mut value, "1e999"), invalid);
    assert_eq!(scanner.Errors().1.len(), 1);
}

/// 验证 resetParams 恢复默认字符集/排序规则，以及各 ParseParam 覆盖生效。
#[test]
fn parser_parameters_restore_defaults_and_apply_overrides() {
    let mut parser = *New();
    parser.charset = "latin1".to_owned();
    parser.collation = "latin1_bin".to_owned();
    resetParams(&mut parser);
    assert_eq!(parser.charset, mysql::DefaultCharset);
    assert_eq!(parser.collation, mysql::DefaultCollationName);

    CharsetConnection("utf8mb4".to_owned())
        .ApplyOn(&mut parser)
        .unwrap();
    CollationConnection("utf8mb4_bin".to_owned())
        .ApplyOn(&mut parser)
        .unwrap();
    assert_eq!(parser.charset, "utf8mb4");
    assert_eq!(parser.collation, "utf8mb4_bin");

    CharsetClient("latin1".to_owned())
        .ApplyOn(&mut parser)
        .unwrap();
    assert_eq!(parser.lexer.client.Name(), "latin1");
}

/// 验证解析器会执行 MySQL 特殊注释中的语句，与 Go 行为一致。
#[test]
fn parser_executes_mysql_special_comments_like_go() {
    let mut parser = *New();
    let (statements, warnings) = parser.Parse("/*! SET @x = 1; SELECT 2 */", "", "").unwrap();
    assert!(warnings.is_empty());
    assert_eq!(statements.len(), 2);
    assert!(statements[0].as_any().is::<parser_ast::SetStmt>());
    assert!(statements[1].as_any().is::<parser_ast::SelectStmt>());
    assert_eq!(statements[0].Text(), "/*! SET @x = 1;");
    assert_eq!(statements[1].Text(), " SELECT 2 */");

    let statement = parser.ParseOneStmt("SELECT /*! 1 */", "", "").unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert_eq!(select.Fields.Fields.len(), 1);
}

/// 验证错放 hint 等情况下词法器警告能透传到 ParseSQL 返回值。
#[test]
fn parser_returns_scanner_warnings_like_go() {
    let mut parser = *New();
    let (_, warnings) = parser.ParseSQL("/*+ misplaced */ SELECT 1", &[]).unwrap();
    assert_eq!(warnings.len(), 1);
}

/// 覆盖 Go 简单解析测试中的边界 SQL（占位符、特殊注释、数值与标识符等）。
#[test]
fn parser_accepts_go_test_simple_edge_cases() {
    let cases = [
        "SELECT id+?, id+? from t;",
        "CREATE TABLE foo (a SMALLINT UNSIGNED, b INT UNSIGNED); -- foo\nSelect --1 from foo;",
        "/*!40101 SET character_set_client = utf8 */;",
        "insert into blobtable (a) values ('/*! truncated */');",
        "SELECT CONVERT('111', SIGNED);",
        "create table t (c int key);",
        "create table t1(a NVARCHAR(100));",
        "use quote;",
        "select b'';",
        "select B'';",
        "CREATE TABLE t(_sms smallint signed, _smu smallint unsigned);",
        "CREATE TABLE t(c1 NATIONAL CHARACTER(10));",
        "insert into tb(v) (select v from tb);",
        "SELECT a as c having c = a;",
        "SELECT 9223372036854775807;",
        "SELECT 9223372036854775808;",
        "select 99e+r10 from t1;",
        "select t./*123*/*,@c3:=0 from t order by t.c1;",
        "select t.1e from test.t;",
        "select t. `a` > 10 from t;",
    ];
    for sql in cases {
        let mut parser = *New();
        parser
            .Parse(sql, "", "")
            .unwrap_or_else(|error| panic!("{sql:?}: {error}"));
    }

    let mut parser = *New();
    let statement = parser
        .ParseOneStmt("select 99e+r10 from t1", "", "")
        .unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    let Some(expression) = &select.Fields.Fields[0].Expr else {
        panic!("expected Go binary expression");
    };
    let parser_ast::ExprKind::Binary { Op, L, R } = &expression.Kind else {
        panic!("expected Go binary expression");
    };
    let parser_ast::ExprKind::Column(left) = &L.Kind else {
        panic!("expected left column")
    };
    let parser_ast::ExprKind::Column(right) = &R.Kind else {
        panic!("expected right column")
    };
    assert_eq!(Op, "+");
    assert_eq!(left.Name.O, "99e");
    assert_eq!(right.Name.O, "r10");

    let statement = parser
        .ParseOneStmt("select t.1e from test.t", "", "")
        .unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    let Some(expression) = &select.Fields.Fields[0].Expr else {
        panic!("expected qualified Go column");
    };
    let parser_ast::ExprKind::Column(column) = &expression.Kind else {
        panic!("expected qualified Go column");
    };
    assert_eq!(column.Table.O, "t");
    assert_eq!(column.Name.O, "1e");
}

/// 验证特殊注释内转义受 SQL mode（如 ModeNoBackslashEscapes）影响，与 Go 一致。
#[test]
fn parser_special_comments_respect_go_sql_mode() {
    let mut parser = *New();
    assert!(parser.ParseOneStmt(r"SELECT /*! '\' */;", "", "").is_err());

    parser.SetSQLMode(mysql::ModeNoBackslashEscapes);
    assert!(parser.ParseOneStmt(r"SELECT /*! '\' */;", "", "").is_ok());

    let statement = parser
        .ParseOneStmt("SELECT /*+ 😅 */ SLEEP(1);", "", "")
        .unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert!(select.TableHints.is_empty());
}

/// 验证 endOffset 按 Go 字节下标语义回退空白，可停在多字节空白中间。
#[test]
fn end_offset_preserves_go_byte_index_semantics() {
    let mut parser = *New();
    parser.src = "x \t".to_owned();
    let mut value = yySymType::default();
    value.offset = parser.src.len() as i32;
    assert_eq!(parser.endOffset(&value), 1);

    parser.src = "x\u{00a0}".to_owned();
    value.offset = parser.src.len() as i32;
    assert_eq!(parser.endOffset(&value), (parser.src.len() - 1) as i32);
}

/// 确认 TiDB 语法接受范围不被第三方 AST 转换收窄（ADMIN/BACKUP/TRACE 等）。
#[test]
fn tidb_grammar_acceptance_is_not_narrowed_by_ast_converter() {
    let mut parser = New();
    let (statements, warnings) = parser.Parse("ADMIN SHOW DDL", "", "").unwrap();
    assert!(warnings.is_empty());
    assert_eq!(
        statements[0]
            .as_any()
            .downcast_ref::<parser_ast::AdminStmt>()
            .unwrap()
            .statement_type,
        parser_ast::AdminStmtType::ShowDdl,
    );

    for sql in [
        "BACKUP DATABASE a TO 'local:///tmp/archive01/'",
        "TRACE SELECT 1",
    ] {
        let (statements, warnings) = parser.Parse(sql, "", "").unwrap();
        assert!(warnings.is_empty(), "{sql}");
        assert_eq!(statements.len(), 1, "{sql}");
        if sql.starts_with("BACKUP") {
            let backup = statements[0]
                .as_any()
                .downcast_ref::<parser_ast::BRIEStmt>()
                .unwrap();
            assert_eq!(backup.Schemas, ["a"]);
        } else {
            let trace = statements[0]
                .as_any()
                .downcast_ref::<parser_ast::TraceStmt>()
                .unwrap();
            assert!(trace.Stmt.as_any().is::<parser_ast::SelectStmt>());
        }
    }

    let (statements, _) = parser
        .Parse("ADMIN SHOW DDL; BACKUP DATABASE a TO 'x;y';", "", "")
        .unwrap();
    assert_eq!(statements.len(), 2);
    assert_eq!(
        statements[1]
            .as_any()
            .downcast_ref::<parser_ast::BRIEStmt>()
            .unwrap()
            .Storage,
        "x;y"
    );

    for invalid_sql in [
        "ADMIN SHOW DDL JOBS -1",
        "ADMIN PAUSE DDL JOBS",
        "BACKUP DATABASE a.b TO 'noop://'",
    ] {
        assert!(parser.Parse(invalid_sql, "", "").is_err(), "{invalid_sql}");
    }
}

/// 验证公开 Parser 经迁移后的 goyacc 能规约出事务相关 AST（BEGIN/COMMIT/USE）。
#[test]
fn public_parser_uses_migrated_goyacc_transaction_ast() {
    let mut parser = New();
    let (statements, _) = parser
        .Parse("BEGIN PESSIMISTIC; COMMIT RELEASE; USE db1", "", "")
        .unwrap();
    assert_eq!(statements.len(), 3);
    assert_eq!(
        statements[0]
            .as_any()
            .downcast_ref::<parser_ast::BeginStmt>()
            .unwrap()
            .Mode,
        parser_ast::Pessimistic,
    );
    assert_eq!(
        statements[1]
            .as_any()
            .downcast_ref::<parser_ast::CommitStmt>()
            .unwrap()
            .CompletionType,
        parser_ast::CompletionTypeRelease,
    );
    assert_eq!(
        statements[2]
            .as_any()
            .downcast_ref::<parser_ast::UseStmt>()
            .unwrap()
            .DBName,
        "db1",
    );
}

/// Go StatementList owns each statement slice; reuse and errors cannot overwrite it.
#[test]
fn parser_statement_text_survives_reuse_and_tracks_sql_mode() {
    let mut parser = New();
    let (statements, _) = parser.Parse("/*! SET x = 1; SELECT 2 */", "", "").unwrap();
    assert_eq!(statements[0].Text(), "/*! SET x = 1;");
    assert_eq!(statements[1].Text(), " SELECT 2 */");
    assert_eq!(statements[0].OriginalText(), b"/*! SET x = 1;");
    assert_eq!(statements[1].OriginalText(), b" SELECT 2 */");
    assert!(parser.Parse("SELECT FROM", "", "").is_err());
    parser.Reset();
    let next = parser.ParseOneStmt("USE db1", "", "").unwrap();
    assert_eq!(next.Text(), "USE db1");
    assert_eq!(statements[0].Text(), "/*! SET x = 1;");
    assert_eq!(statements[1].Text(), " SELECT 2 */");

    let (lines, _) = parser.Parse("\nSELECT 1;\nSELECT 2\n", "", "").unwrap();
    assert_eq!(lines[0].Text(), "SELECT 1;");
    assert_eq!(lines[1].Text(), "SELECT 2");

    let client = CharsetClient("latin1".to_owned());
    let (encoded, _) = parser.ParseSQL("SELECT 'é'", &[&client]).unwrap();
    assert_eq!(encoded[0].OriginalText(), "SELECT 'é'".as_bytes());
    // encoding_latin1.go deliberately preserves bytes for TiDB compatibility.
    assert_eq!(encoded[0].Text(), "SELECT 'é'");
    let client = CharsetClient("gbk".to_owned());
    let (encoded, _) = parser.ParseSQL("SELECT 'é'", &[&client]).unwrap();
    assert_eq!(encoded[0].OriginalText(), "SELECT 'é'".as_bytes());
    assert_eq!(encoded[0].Text(), "SELECT '茅'");

    let escaped = parser.ParseOneStmt("SELECT '\\n\x01'", "", "").unwrap();
    assert_eq!(escaped.Text(), "SELECT 0x0a01");
    assert_eq!(escaped.OriginalText(), b"SELECT '\\n\x01'");
    parser.SetSQLMode(mysql::ModeNoBackslashEscapes);
    let literal = parser.ParseOneStmt("SELECT '\\n\x01'", "", "").unwrap();
    assert_eq!(literal.Text(), "SELECT 0x5c6e01");
    assert_eq!(escaped.Text(), "SELECT 0x0a01");
}
