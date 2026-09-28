// Copyright 2026 AsterSQL.

#[test]
fn duration_boundaries_match_go_through_parser() {
    for (value, accepted) in [
        ("+0", true),
        ("-0", true),
        ("0", true),
        ("9223372036854775807ns", true),
        ("-9223372036854775808ns", true),
        ("9223372036854775808ns", false),
        ("-9223372036854775809ns", false),
        ("2562048h", false),
        ("9223372036854775807ns1ns", false),
        (".5h", true),
        ("1.h", true),
        ("1µs2μs", true),
        ("0.00000000000000000000000001h", true),
        (".s", false),
    ] {
        let sql = format!(
            "CREATE RESOURCE GROUP rg RU_PER_SEC=100 QUERY_LIMIT(EXEC_ELAPSED='{value}' ACTION=KILL)"
        );
        assert_eq!(
            New().ParseOneStmt(&sql, "", "").is_ok(),
            accepted,
            "{value}"
        );
    }
}

#[test]
fn partition_validation_uses_mysql_errors() {
    let error = New()
        .ParseOneStmt(
            "CREATE TABLE t (a INT) PARTITION BY RANGE(a) (PARTITION p0)",
            "",
            "",
        )
        .err()
        .expect("missing VALUES must fail");
    assert!(error.to_string().contains("1479"), "{error}");
}

#[test]
fn nested_select_preserves_with_clause() {
    let statement = New()
        .ParseOneStmt(
            "SELECT 1 UNION (WITH c AS (SELECT 2) SELECT * FROM c)",
            "",
            "",
        )
        .unwrap();
    let set = statement
        .as_any()
        .downcast_ref::<parser_ast::SetOprStmt>()
        .unwrap();
    let nested = set.select_list.selects[1]
        .as_any()
        .downcast_ref::<parser_ast::SetOprSelectList>()
        .unwrap();
    let inner = nested.selects[0]
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    let outer_with = nested.With.as_ref().expect("nested WITH");
    let inner_with = inner.With.as_ref().expect("SELECT WITH");
    assert!(std::rc::Rc::ptr_eq(outer_with, inner_with));
    assert_eq!(outer_with.borrow().CTEs[0].Name.O, "c");
    outer_with.borrow_mut().CTEs[0].Name = parser_ast::NewCIStr("renamed");
    assert_eq!(inner_with.borrow().CTEs[0].Name.O, "renamed");
}

#[test]
fn custom_duration_options_match_go() {
    for prefix in [
        "CREATE TABLE t (a INT) TTL_JOB_INTERVAL=",
        "CALIBRATE RESOURCE DURATION ",
    ] {
        for (value, accepted) in [
            ("1d", true),
            ("", true),
            ("0", true),
            ("1h30m", true),
            ("1s", false),
            ("-1h", false),
        ] {
            let sql = format!("{prefix}'{value}'");
            assert_eq!(New().ParseOneStmt(&sql, "", "").is_ok(), accepted, "{sql}");
        }
    }
}

#[test]
fn duration_errors_retain_go_details() {
    for (value, detail) in [
        ("1x", "time: unknown unit \"x\" in duration \"1x\""),
        ("1", "time: missing unit in duration \"1\""),
        ("+", "time: invalid duration \"+\""),
        (
            "1µ",
            "time: unknown unit \"\\xc2\\xb5\" in duration \"1\\xc2\\xb5\"",
        ),
    ] {
        let sql = format!("TRAFFIC CAPTURE TO 's3://bucket/path' DURATION='{value}'");
        let error = New()
            .ParseOneStmt(&sql, "", "")
            .err()
            .expect("invalid duration");
        assert!(error.to_string().contains(detail), "{error}");
    }
}

#[test]
fn initial_parenthesized_set_operand_keeps_with_on_select_only() {
    let statement = New()
        .ParseOneStmt(
            "(WITH c AS (SELECT 2) SELECT * FROM c) UNION SELECT 1",
            "",
            "",
        )
        .unwrap();
    let set = statement
        .as_any()
        .downcast_ref::<parser_ast::SetOprStmt>()
        .unwrap();
    let nested = set.select_list.selects[0]
        .as_any()
        .downcast_ref::<parser_ast::SetOprSelectList>()
        .unwrap();
    assert!(nested.With.is_none());
    let select = nested.selects[0]
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert!(select.With.is_some());
}
