// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;

#[test]
fn failure_returns_warnings_and_parser_reuse_clears_them() {
    let mut parser = New();
    let (stmts, warnings, error) = parser.ParseSQLWithWarnings("/*+ misplaced */ SELECT FROM", &[]);
    assert!(stmts.is_empty());
    assert!(error.is_some());
    assert_eq!(warnings.len(), 1);
    assert_eq!(parser.Warnings().len(), 1);
    let saved = warnings[0].to_string();
    let (stmts, warnings, error) = parser.ParseSQLWithWarnings("SELECT 1", &[]);
    assert_eq!(stmts.len(), 1);
    assert!(warnings.is_empty());
    assert!(error.is_none());
    assert!(parser.Warnings().is_empty());
    assert!(!saved.is_empty());
}

#[test]
fn hint_uses_live_scanner_mode_and_position() {
    for mode in [mysql::SQLMode::default(), mysql::ModeNoBackslashEscapes] {
        for hint in [r"/*+ SET_VAR(tidb_opt='a\b') */", "/*+ HASH_JOIN( */"] {
            let sql = format!("\n\nSELECT {hint} 1");
            let (expected, diagnostics) = lexer_support::ParseHint(
                hint,
                mode,
                Pos {
                    Line: 3,
                    Col: 8,
                    Offset: 9,
                },
            );
            let mut parser = New();
            parser.SetSQLMode(mode);
            let (statements, warnings) = parser.ParseSQL(&sql, &[]).unwrap();
            let select = statements[0]
                .as_any()
                .downcast_ref::<parser_ast::SelectStmt>()
                .unwrap();
            assert_eq!(
                format!("{:?}", select.TableHints),
                format!("{:?}", expected),
                "{sql}"
            );
            assert_eq!(
                warnings.iter().map(ToString::to_string).collect::<Vec<_>>(),
                diagnostics
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
                "{sql}"
            );
        }
    }
}

#[test]
fn real_ast_flags_cover_all_statement_containers() {
    use parser_ast::flag::*;
    struct Inspect {
        flags: Vec<u64>,
    }
    impl parser_ast::Visitor for Inspect {
        fn enter(&mut self, _: &dyn parser_ast::Node) -> bool {
            false
        }
        fn leave(&mut self, n: &dyn parser_ast::Node) -> bool {
            if let Some(e) = n.as_any().downcast_ref::<parser_ast::ExprNode>() {
                self.flags.push(e.GetFlag());
            }
            true
        }
    }
    for sql in [
        "SELECT abs(a + ?) FROM t WHERE b IN (SELECT c FROM u)",
        "UPDATE t SET a = abs(?) WHERE b = ?",
        "DELETE FROM t WHERE a = ?",
        "INSERT INTO t VALUES (abs(?)) ON DUPLICATE KEY UPDATE a = ?",
        "CREATE TABLE t (a INT DEFAULT (abs(1)), b INT GENERATED ALWAYS AS (a + 1), CHECK (a > 0))",
        "CREATE VIEW v AS SELECT abs(a) FROM t",
        "WITH c AS (SELECT ? a) SELECT a FROM c UNION SELECT abs(?)",
        "DO abs(?)",
        "SET @a = abs(?)",
        "EXPLAIN SELECT abs(?)",
        "CREATE PROCEDURE p() BEGIN SET @a = abs(?); SELECT ?; END",
    ] {
        let stmt = New()
            .ParseOneStmt(sql, "", "")
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
        let mut inspect = Inspect { flags: Vec::new() };
        assert!(stmt.accept(&mut inspect));
        assert!(
            inspect
                .flags
                .iter()
                .any(|f| f & (FLAG_HAS_FUNC | FLAG_HAS_REFERENCE | FLAG_HAS_PARAM_MARKER) != 0),
            "{sql}: {:?}",
            inspect.flags
        );
    }
    let stmt = New()
        .ParseOneStmt(
            "SELECT sum(a + ?), row_number() OVER (ORDER BY ?), @x := abs(?), EXISTS(SELECT ?)",
            "",
            "",
        )
        .unwrap();
    let select = stmt
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    let expected = [
        FLAG_HAS_AGGREGATE_FUNC | FLAG_HAS_REFERENCE | FLAG_HAS_PARAM_MARKER,
        FLAG_HAS_WINDOW_FUNC,
        FLAG_HAS_VARIABLE | FLAG_HAS_FUNC | FLAG_HAS_PARAM_MARKER,
        FLAG_HAS_SUBQUERY,
    ];
    for (field, flag) in select.Fields.Fields.iter().zip(expected) {
        assert_eq!(field.Expr.as_ref().unwrap().GetFlag(), flag);
    }
}

#[test]
fn float_overflow_retains_typed_error() {
    let mut parser = New();
    let error = parser.ParseSQL("SELECT 1e999", &[]).err().unwrap();
    assert!(types::ErrIllegalValueForType.Equal(Some(&error)), "{error}");
}

#[test]
fn error_context_uses_go_byte_limit() {
    for text in ["é".repeat(60), "界".repeat(40), "🙂".repeat(30)] {
        let prefix = String::from_utf8_lossy(&text.as_bytes()[..80]);
        assert_eq!(
            ParseErrorWith(&text, 3).to_string(),
            format!("near '{prefix}' at line 3")
        );
        let mut parser = New();
        let sql = format!("/*{text}");
        let prefix = String::from_utf8_lossy(&sql.as_bytes()[..80]);
        let error = parser.ParseSQL(&sql, &[]).err().unwrap();
        assert!(error.to_string().contains(prefix.as_ref()), "{error}");
        assert!(!error.to_string().contains(&text), "{error}");
    }
}

#[test]
fn go_flag_test_cases_use_real_parser_ast() {
    use parser_ast::flag::*;
    for (sql, flag) in [
        (r"1 between 0 and 2", FLAG_CONSTANT),
        (r"case 1 when 1 then 1 else 0 end", FLAG_CONSTANT),
        (r"case 1 when 1 then 1 else 0 end", FLAG_CONSTANT),
        (
            r"case 1 when a > 1 then 1 else 0 end",
            FLAG_CONSTANT | FLAG_HAS_REFERENCE,
        ),
        (
            r"1 = ANY (select 1) OR exists (select 1)",
            FLAG_HAS_SUBQUERY,
        ),
        (
            r"1 in (1) or 1 is true or null is null or 'abc' like 'abc' or 'abc' rlike 'abc'",
            FLAG_CONSTANT,
        ),
        (r"row (1, 1) = row (1, 1)", FLAG_CONSTANT),
        (r"(1 + a) > ?", FLAG_HAS_REFERENCE | FLAG_HAS_PARAM_MARKER),
        (r"trim('abc ')", FLAG_HAS_FUNC),
        (
            r"now() + EXTRACT(YEAR FROM '2009-07-02') + CAST(1 AS UNSIGNED)",
            FLAG_HAS_FUNC,
        ),
        (r"substring('abc', 1)", FLAG_HAS_FUNC),
        (r"sum(a)", FLAG_HAS_AGGREGATE_FUNC | FLAG_HAS_REFERENCE),
        (r"(select 1) as a", FLAG_HAS_SUBQUERY),
        (r"@auto_commit", FLAG_HAS_VARIABLE),
        (r"default(a)", FLAG_HAS_DEFAULT),
        (r"a is null", FLAG_HAS_REFERENCE),
        (r"1 is true", FLAG_CONSTANT),
        (
            r"a in (1, count(*), 3)",
            FLAG_CONSTANT | FLAG_HAS_REFERENCE | FLAG_HAS_AGGREGATE_FUNC,
        ),
        (r"'Michael!' REGEXP '.*'", FLAG_CONSTANT),
        (r"a REGEXP '.*'", FLAG_HAS_REFERENCE),
        (r"-a", FLAG_HAS_REFERENCE),
    ] {
        let stmt = New()
            .ParseOneStmt(&format!("SELECT {sql}"), "", "")
            .unwrap();
        let select = stmt
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        assert_eq!(
            select.Fields.Fields[0].Expr.as_ref().unwrap().GetFlag(),
            flag,
            "{sql}"
        );
    }
}

#[test]
fn positional_by_items_keep_go_reference_flags() {
    let stmt = New()
        .ParseOneStmt(
            "SELECT a FROM t GROUP BY 1 ORDER BY 1 DESC, 18446744073709551615",
            "",
            "",
        )
        .unwrap();
    let select = stmt
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert_eq!(
        select.GroupBy[0].Expr.GetFlag(),
        parser_ast::flag::FLAG_HAS_REFERENCE
    );
    assert_eq!(
        select.OrderBy[0].Expr.GetFlag(),
        parser_ast::flag::FLAG_HAS_REFERENCE
    );
    assert_eq!(select.OrderBy[1].Expr.GetFlag(), 0);
}

#[test]
fn rejected_params_do_not_leak_nested_parse_warnings() {
    struct Reject;
    impl ParseParam for Reject {
        fn ApplyOn(&self, parser: &mut Parser) -> Result<(), errors::Error> {
            parser.ParseSQL("/*+ misplaced */ SELECT 1", &[])?;
            Err(errors::New("rejected parameter"))
        }
    }
    let mut parser = New();
    let (statements, warnings, error) = parser.ParseSQLWithWarnings("SELECT 2", &[&Reject]);
    assert!(statements.is_empty());
    assert!(warnings.is_empty());
    assert_eq!(error.unwrap().to_string(), "rejected parameter");
    assert!(parser.Warnings().is_empty());
}
