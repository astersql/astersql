// Copyright 2026 AsterSQL.

use crate::{Keywords, ParseHint, Pos, digester_impl, mysql};

#[test]
fn go_merge_29_keywords_and_auto_digest() {
    const NEW_WORDS: [&str; 12] = [
        "ALERT",
        "ASYNC",
        "AUTO",
        "COMPLETE",
        "DELTA",
        "FAST",
        "IMMEDIATE",
        "MATERIALIZED",
        "OPERATE",
        "PLACE",
        "STORAGE_CLASS",
        "TRANSITIONS",
    ];
    assert_eq!(Keywords.len(), 695);
    for word in NEW_WORDS {
        let entry = Keywords.iter().find(|keyword| keyword.Word == word);
        assert!(entry.is_some_and(|keyword| !keyword.Reserved), "{word}");
    }
    assert_eq!(
        digester_impl::NormalizeKeepHint("SELECT auto FROM auto"),
        "select auto from auto"
    );
    assert_ne!(
        digester_impl::NormalizeKeepHint("select auto from auto"),
        digester_impl::NormalizeKeepHint("select `auto` from `auto`")
    );
    assert_ne!(
        digester_impl::DigestHash("select auto from auto"),
        digester_impl::DigestHash("select `auto` from `auto`")
    );
}

#[test]
fn go_merge_29_full_outer_join() {
    let statement = crate::New()
        .ParseOneStmt("SELECT * FROM t1 FULL OUTER JOIN t2 ON t1.a=t2.a", "", "")
        .unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<crate::ast::SelectStmt>()
        .unwrap();
    assert_eq!(
        select.From.as_ref().unwrap().TableRefs.Tp,
        crate::ast::JoinType::FullJoin
    );
    let alias_statement = crate::New()
        .ParseOneStmt("SELECT * FROM t1 FULL JOIN t2", "", "")
        .unwrap();
    let alias_select = alias_statement
        .as_any()
        .downcast_ref::<crate::ast::SelectStmt>()
        .unwrap();
    assert_eq!(
        alias_select.From.as_ref().unwrap().TableRefs.Tp,
        crate::ast::JoinType::CrossJoin
    );
}

#[test]
fn go_merge_29_hint_depth_limit_public_api() {
    let input = format!(
        "/*+LEADING({}t{})*/",
        "(".repeat(10_000),
        ")".repeat(10_000)
    );
    let (_, errors) = ParseHint(
        &input,
        mysql::SQLMode::default(),
        Pos {
            Line: 1,
            ..Pos::default()
        },
    );
    assert!(
        errors.iter().any(|error| error
            .to_string()
            .contains("parentheses nesting depth exceeds maximum 10000")),
        "{errors:?}"
    );
}

#[test]
fn go_merge_29_sql_depth_limit_public_api() {
    let sql = format!("SELECT {}1{}", "(".repeat(10_001), ")".repeat(10_001));
    let error = crate::New()
        .Parse(&sql, "", "")
        .err()
        .expect("depth limit must fail");
    assert!(
        error
            .to_string()
            .contains("parentheses nesting depth exceeds maximum 10000")
    );
}

#[test]
fn go_merge_29_count_extrema_aggregate_forms() {
    for sql in [
        "SELECT MAX_COUNT(a) FROM t",
        "SELECT MAX_COUNT(ALL a) FROM t",
        "SELECT MIN_COUNT(a) FROM t",
        "SELECT MIN_COUNT(ALL a) FROM t",
    ] {
        let statement = crate::New().ParseOneStmt(sql, "", "").expect(sql);
        let select = statement
            .as_any()
            .downcast_ref::<crate::ast::SelectStmt>()
            .expect(sql);
        let Some(crate::ast::ExprKind::AggregateFunction {
            Name,
            Args,
            Distinct,
            ..
        }) = select.Fields.Fields[0]
            .Expr
            .as_ref()
            .map(|expression| &expression.Kind)
        else {
            panic!("{sql}: expected aggregate expression");
        };
        assert_eq!(
            Name.to_ascii_uppercase(),
            sql.split('(').next().unwrap().trim_start_matches("SELECT ")
        );
        assert_eq!(Args.len(), 1);
        assert!(!Distinct);
    }
    for sql in [
        "SELECT MAX_COUNT(a) OVER () FROM t",
        "SELECT MIN_COUNT(ALL a) OVER () FROM t",
    ] {
        let mut parser = crate::New();
        parser.EnableWindowFunc(true);
        let statement = parser.ParseOneStmt(sql, "", "").expect(sql);
        let select = statement
            .as_any()
            .downcast_ref::<crate::ast::SelectStmt>()
            .expect(sql);
        let Some(crate::ast::ExprKind::WindowFunction {
            Name,
            Args,
            Distinct,
            ..
        }) = select.Fields.Fields[0]
            .Expr
            .as_ref()
            .map(|expression| &expression.Kind)
        else {
            panic!("{sql}: expected window expression");
        };
        assert_eq!(
            Name.to_ascii_uppercase(),
            sql.split('(').next().unwrap().trim_start_matches("SELECT ")
        );
        assert_eq!(Args.len(), 1);
        assert!(!Distinct);
    }
}
