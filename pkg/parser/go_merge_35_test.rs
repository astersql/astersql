// Copyright 2026 AsterSQL.

#[test]
fn go_merge_35_ast_depth_checker_tracks_nested_expression() {
    let mut parser = crate::New();
    let sql = format!("SELECT {}1", "1+".repeat(20));
    let (mut statements, _) = parser.Parse(&sql, "", "").expect(&sql);
    let mut statement = statements.remove(0);
    let error = crate::parser_impl::check_ast_depth_limit(statement.as_mut(), 10)
        .expect_err("nested expression must exceed a low test limit");
    assert!(
        error
            .to_string()
            .contains("AST nesting depth exceeds maximum 10")
    );
}

#[test]
fn go_merge_35_auto_presplit_feature_comment() {
    assert!(crate::tidbfeature::CanParseFeature(&[
        "pre_split",
        "auto_presplit"
    ]));
    assert!(!crate::tidbfeature::CanParseFeature(&["unknown"]));
}

#[test]
fn go_merge_35_public_ast_depth_limit() {
    for (name, sql) in [
        ("binary", format!("SELECT {}1", "1+".repeat(11_000))),
        ("unary", format!("SELECT {}1", "!".repeat(11_000))),
        (
            "case",
            format!(
                "SELECT {}1{}",
                "CASE WHEN TRUE THEN ".repeat(11_000),
                " ELSE 0 END".repeat(11_000)
            ),
        ),
    ] {
        let error = crate::New()
            .ParseOneStmt(&sql, "", "")
            .err()
            .unwrap_or_else(|| panic!("{name}: deep expression must be rejected"));
        assert!(
            error
                .to_string()
                .contains("AST nesting depth exceeds maximum"),
            "{name}: {error}"
        );
    }
}

#[test]
fn go_merge_35_parentheses_limit_accepts_boundary() {
    let sql = format!("SELECT {}a{}", "(".repeat(10_000), ")".repeat(10_000));
    let statement = crate::New()
        .ParseOneStmt(&sql, "", "")
        .expect("Go accepts exactly 10,000 nested parentheses");
    let select = statement
        .as_any()
        .downcast_ref::<crate::ast::SelectStmt>()
        .expect("SELECT statement");
    let mut expression = select.Fields.Fields[0]
        .Expr
        .as_ref()
        .expect("field expression");
    let mut depth = 0;
    while let crate::ast::ExprKind::Parentheses(inner) = &expression.Kind {
        assert_ne!(
            expression.GetFlag() & crate::ast::flag::FLAG_HAS_REFERENCE,
            0
        );
        depth += 1;
        expression = inner.as_ref();
    }
    assert_eq!(depth, 10_000);
}

#[test]
fn go_merge_35_parentheses_limit_rejects_function_and_hint_depth() {
    let function_sql = format!("SELECT {}1{}", "f(".repeat(10_001), ")".repeat(10_001));
    let hint_sql = format!(
        "SELECT /*+ LEADING({}t{}) */ * FROM t",
        "(".repeat(10_000),
        ")".repeat(10_000)
    );
    for sql in [function_sql, hint_sql] {
        let error = crate::New()
            .ParseOneStmt(&sql, "", "")
            .err()
            .expect("deep parentheses must be rejected");
        assert!(
            error
                .to_string()
                .contains("parentheses nesting depth exceeds maximum 10000"),
            "{error}"
        );
    }
}

#[test]
fn go_merge_35_long_literal_does_not_count_as_expression_depth() {
    let sql = format!("SELECT '{}'", "!+CASE END".repeat(1_100));
    crate::New()
        .ParseOneStmt(&sql, "", "")
        .expect("operators and CASE inside a string literal are not AST nodes");
}

#[test]
fn go_merge_35_materialized_view_restore() {
    let cases = [
        (
            "CREATE MATERIALIZED VIEW mv (a) AS SELECT 1",
            "CREATE MATERIALIZED VIEW `mv` (`a`) AS SELECT 1",
        ),
        (
            "CREATE MATERIALIZED VIEW mv (a) COMMENT = 'c1' SHARD_ROW_ID_BITS = 2 PRE_SPLIT_REGIONS = 3 REFRESH FAST NEXT 300 ATTRIBUTES = 'x' AS SELECT 1",
            "CREATE MATERIALIZED VIEW `mv` (`a`) COMMENT = 'c1' SHARD_ROW_ID_BITS = 2 PRE_SPLIT_REGIONS = 3 REFRESH FAST NEXT 300 ATTRIBUTES = 'x' AS SELECT 1",
        ),
        (
            "CREATE MATERIALIZED VIEW mv (a) COMMENT 'c1' AS SELECT 1",
            "CREATE MATERIALIZED VIEW `mv` (`a`) COMMENT = 'c1' AS SELECT 1",
        ),
        (
            "CREATE MATERIALIZED VIEW LOG ON t (a,b) PURGE IMMEDIATE ALERT ROWS 10",
            "CREATE MATERIALIZED VIEW LOG ON `t` (`a`, `b`) PURGE IMMEDIATE ALERT ROWS 10",
        ),
        (
            "CREATE MATERIALIZED VIEW LOG ON t (a) PURGE NEXT 300",
            "CREATE MATERIALIZED VIEW LOG ON `t` (`a`) PURGE NEXT 300",
        ),
        (
            "ALTER MATERIALIZED VIEW mv COMMENT = 'c2', REFRESH START WITH now() NEXT 300, ATTRIBUTES = 'y'",
            "ALTER MATERIALIZED VIEW `mv` COMMENT = 'c2', REFRESH START WITH NOW() NEXT 300, ATTRIBUTES = 'y'",
        ),
        (
            "ALTER MATERIALIZED VIEW mv COMMENT 'c2'",
            "ALTER MATERIALIZED VIEW `mv` COMMENT = 'c2'",
        ),
        (
            "ALTER MATERIALIZED VIEW mv REFRESH",
            "ALTER MATERIALIZED VIEW `mv` REFRESH",
        ),
        (
            "ALTER MATERIALIZED VIEW LOG ON t PURGE, ADD COLUMN (b,c)",
            "ALTER MATERIALIZED VIEW LOG ON `t` PURGE, ADD COLUMN (`b`, `c`)",
        ),
        (
            "ALTER MATERIALIZED VIEW LOG ON t PURGE",
            "ALTER MATERIALIZED VIEW LOG ON `t` PURGE",
        ),
        (
            "PURGE MATERIALIZED VIEW LOG ON t",
            "PURGE MATERIALIZED VIEW LOG ON `t`",
        ),
        (
            "PURGE MATERIALIZED VIEW LOG ON test.t",
            "PURGE MATERIALIZED VIEW LOG ON `test`.`t`",
        ),
        (
            "DROP MATERIALIZED VIEW IF EXISTS mv",
            "DROP MATERIALIZED VIEW IF EXISTS `mv`",
        ),
        (
            "DROP MATERIALIZED VIEW LOG IF EXISTS ON t",
            "DROP MATERIALIZED VIEW LOG IF EXISTS ON `t`",
        ),
    ];
    for (sql, expected) in cases {
        let statement = crate::New().ParseOneStmt(sql, "", "").expect(sql);
        assert_eq!(
            crate::ast::sql_restore::restore_node(statement.as_ref()).unwrap(),
            expected,
            "{sql}"
        );
    }
}

#[test]
fn go_merge_35_refresh_materialized_view_forms() {
    let accepted = [
        (
            "REFRESH MATERIALIZED VIEW mv FAST",
            "REFRESH MATERIALIZED VIEW `mv` FAST",
        ),
        (
            "REFRESH MATERIALIZED VIEW mv WITH ASYNC MODE FAST",
            "REFRESH MATERIALIZED VIEW `mv` WITH ASYNC MODE FAST",
        ),
        (
            "REFRESH MATERIALIZED VIEW mv FAST AS OF TIMESTAMP '2021-04-15 00:00:00' WITH PROFILE",
            "REFRESH MATERIALIZED VIEW `mv` FAST AS OF TIMESTAMP _UTF8MB4'2021-04-15 00:00:00' WITH PROFILE",
        ),
        (
            "REFRESH MATERIALIZED VIEW mv COMPLETE IN PLACE",
            "REFRESH MATERIALIZED VIEW `mv` COMPLETE IN PLACE",
        ),
        (
            "REFRESH MATERIALIZED VIEW mv WITH ASYNC MODE COMPLETE OUT OF PLACE DRY RUN",
            "REFRESH MATERIALIZED VIEW `mv` WITH ASYNC MODE COMPLETE OUT OF PLACE DRY RUN",
        ),
        (
            "REFRESH MATERIALIZED VIEW mv COMPLETE DELTA APPLY WITH PROFILE",
            "REFRESH MATERIALIZED VIEW `mv` COMPLETE DELTA APPLY WITH PROFILE",
        ),
        (
            "CANCEL MATERIALIZED VIEW REFRESH JOB 42",
            "CANCEL MATERIALIZED VIEW REFRESH JOB 42",
        ),
    ];
    for (sql, expected) in accepted {
        let statement = crate::New().ParseOneStmt(sql, "", "").expect(sql);
        assert_eq!(
            crate::ast::sql_restore::restore_node(statement.as_ref()).unwrap(),
            expected,
            "{sql}"
        );
    }
    for sql in [
        "REFRESH MATERIALIZED VIEW mv COMPLETE",
        "REFRESH MATERIALIZED VIEW mv FAST OUT OF PLACE",
        "REFRESH MATERIALIZED VIEW mv COMPLETE OUT OF PLACE DELTA APPLY",
        "CANCEL MATERIALIZED VIEW REFRESH JOB",
    ] {
        assert!(crate::New().ParseOneStmt(sql, "", "").is_err(), "{sql}");
    }
}

#[test]
fn go_merge_35_materialized_view_option_errors() {
    for (sql, expected) in [
        (
            "CREATE MATERIALIZED VIEW mv (a) COMMENT = 'c1' COMMENT = 'c2' AS SELECT 1",
            "Duplicate COMMENT specified in CREATE MATERIALIZED VIEW",
        ),
        (
            "CREATE MATERIALIZED VIEW mv (a) SHARD_ROW_ID_BITS = 1 SHARD_ROW_ID_BITS = 2 AS SELECT 1",
            "Duplicate SHARD_ROW_ID_BITS specified in CREATE MATERIALIZED VIEW",
        ),
        (
            "CREATE MATERIALIZED VIEW mv (a) PRE_SPLIT_REGIONS = 1 PRE_SPLIT_REGIONS = 2 AS SELECT 1",
            "Duplicate PRE_SPLIT_REGIONS specified in CREATE MATERIALIZED VIEW",
        ),
        (
            "CREATE MATERIALIZED VIEW LOG ON t (a) SHARD_ROW_ID_BITS = 1 SHARD_ROW_ID_BITS = 2",
            "Duplicate SHARD_ROW_ID_BITS specified in CREATE MATERIALIZED VIEW LOG",
        ),
        (
            "CREATE MATERIALIZED VIEW LOG ON t (a) PRE_SPLIT_REGIONS = 1 PRE_SPLIT_REGIONS = 2",
            "Duplicate PRE_SPLIT_REGIONS specified in CREATE MATERIALIZED VIEW LOG",
        ),
    ] {
        let error = crate::New().ParseOneStmt(sql, "", "").err().expect(sql);
        assert!(error.to_string().contains(expected), "{sql}: {error}");
    }
    for sql in [
        "CREATE MATERIALIZED VIEW mv (a) REFRESH FAST SHARD_ROW_ID_BITS = 4 AS SELECT 1",
        "CREATE MATERIALIZED VIEW mv (a) ATTRIBUTES = 'x' REFRESH FAST AS SELECT 1",
        "CREATE MATERIALIZED VIEW mv (a) REFRESH FAST REFRESH FAST AS SELECT 1",
        "CREATE MATERIALIZED VIEW mv (a) ATTRIBUTES = 'x' ATTRIBUTES = 'y' AS SELECT 1",
        "CREATE MATERIALIZED VIEW LOG ON t (a) PURGE START WITH now()",
        "CREATE MATERIALIZED VIEW LOG ON t (a) PURGE",
    ] {
        assert!(crate::New().ParseOneStmt(sql, "", "").is_err(), "{sql}");
    }
}

#[test]
fn go_merge_35_parser_table_cases() {
    let cases = [
        ("SELECT * FROM t1 FULL JOIN t2 ON t1.a=t2.a", true),
        ("SELECT * FROM t1 FULL OUTER JOIN t2 ON t1.a<=>t2.a", true),
        ("SHOW STORAGE_CLASS TRANSITIONS", true),
        ("SHOW STORAGE_CLASS TRANSITIONS LIKE 'orders%'", true),
        (
            "SHOW STORAGE_CLASS TRANSITIONS WHERE direction='TO_IA'",
            true,
        ),
        ("SHOW STORAGE CLASS TRANSITIONS", false),
        ("CANCEL MATERIALIZED VIEW LOG PURGE JOB", false),
        ("CANCEL MATERIALIZED VIEW LOG PURGE JOB 1", true),
        ("SET ROLE AUTO", false),
        ("SET ROLE `AUTO`", true),
        ("CREATE TABLE AUTO (AUTO INT)", true),
        ("CREATE ROLE AUTO", false),
        ("CREATE ROLE `AUTO`", true),
        ("SELECT INTERVAL()", false),
        ("SELECT INTERVAL(1)", false),
        ("SELECT INTERVAL(1, 0)", true),
        ("SELECT NOW() + INTERVAL(1+2) DAY `add`", true),
        ("SELECT d + INTERVAL (q - 1) QUARTER", true),
        ("SELECT d - INTERVAL (q - 1) QUARTER", true),
        ("SELECT INTERVAL (q - 1) QUARTER + d", true),
        ("SELECT ADDDATE(d, INTERVAL (q - 1) QUARTER)", true),
        ("SELECT SUBDATE(d, INTERVAL (q - 1) QUARTER)", true),
        ("SELECT ROW(ROW(1,2),3), ROW(1,ROW(2,3))", true),
        ("SELECT (1,2), ((1,2),3), (1,(2,3))", true),
        (
            "SELECT MAKEDATE(YEAR(d), 1) + INTERVAL (QUARTER(d) - 1) QUARTER",
            true,
        ),
        ("SELECT MAX_COUNT(c1,c2) FROM t", false),
        ("SELECT MAX_COUNT(DISTINCT c1) FROM t", false),
        ("SELECT MAX_COUNT(c2) FROM t", true),
        ("SELECT MAX_COUNT(ALL c1) FROM t", true),
        ("SELECT MIN_COUNT(c1,c2) FROM t", false),
        ("SELECT MIN_COUNT(DISTINCT c1) FROM t", false),
        ("SELECT MIN_COUNT(c2) FROM t", true),
        ("SELECT MIN_COUNT(ALL c1) FROM t", true),
        ("CREATE TABLE t (c TEXT) PRE_SPLIT_REGIONS AUTO", false),
        ("ALTER TABLE t ADD INDEX (a) PRE_SPLIT_REGIONS = AUTO", true),
        ("ALTER TABLE t ADD INDEX (a) PRE_SPLIT_REGIONS AUTO", true),
        (
            "ALTER TABLE t ADD INDEX (a) PRE_SPLIT_REGIONS AUTO PRE_SPLIT_REGIONS 4",
            true,
        ),
        (
            "ALTER TABLE t ADD INDEX (a) PRE_SPLIT_REGIONS 4 PRE_SPLIT_REGIONS AUTO",
            true,
        ),
        ("ALTER TABLE t ADD INDEX (a) PRE_SPLIT_REGIONS = FOO", false),
        (
            "ALTER TABLE t ADD INDEX (a) PRE_SPLIT_REGIONS = 'AUTO'",
            false,
        ),
        (
            "ALTER TABLE t ADD INDEX (a) PRE_SPLIT_REGIONS = `AUTO`",
            false,
        ),
        (
            "ALTER TABLE t ADD PRIMARY KEY (a) PRE_SPLIT_REGIONS AUTO",
            true,
        ),
        ("CREATE INDEX idx ON t (a,b) PRE_SPLIT_REGIONS AUTO", true),
        (
            "CREATE INDEX idx ON t (a,b) /*T![unsupported_auto_presplit] PRE_SPLIT_REGIONS = AUTO */",
            true,
        ),
        (
            "GRANT OPERATE VIEW ON db2.invoice TO 'jeffrey'@'localhost'",
            true,
        ),
        (
            "REVOKE OPERATE VIEW ON db2.invoice FROM 'jeffrey'@'localhost'",
            true,
        ),
        ("ANALYZE TABLE t WITH DEFAULT BUCKETS", true),
        ("ANALYZE TABLE t WITH DEFAULT TOPN", true),
        ("ANALYZE TABLE t WITH DEFAULT SAMPLES", true),
        ("ANALYZE TABLE t WITH DEFAULT SAMPLERATE", true),
        ("ANALYZE TABLE t WITH DEFAULT SAMPLES, 0.1 SAMPLERATE", true),
        (
            "ANALYZE TABLE t WITH DEFAULT BUCKETS, DEFAULT TOPN, DEFAULT SAMPLES, DEFAULT SAMPLERATE",
            true,
        ),
        ("ANALYZE TABLE t WITH 4 BUCKETS, DEFAULT TOPN", true),
        ("ANALYZE TABLE t PARTITION a WITH DEFAULT BUCKETS", true),
        ("ANALYZE TABLE t WITH DEFAULT CMSKETCH WIDTH", false),
        ("ANALYZE TABLE t WITH DEFAULT CMSKETCH DEPTH", false),
        ("ANALYZE TABLE t WITH DEFAULT NDVRATE", false),
    ];
    for (sql, accepted) in cases {
        let result = crate::New().ParseOneStmt(sql, "", "");
        assert_eq!(result.is_ok(), accepted, "{sql}: {:?}", result.err());
    }
}

#[test]
fn go_merge_35_restore_storage_class_roles_and_privileges() {
    let cases = [
        (
            "SHOW STORAGE_CLASS TRANSITIONS",
            "SHOW STORAGE_CLASS TRANSITIONS",
        ),
        (
            "SHOW STORAGE_CLASS TRANSITIONS LIKE 'orders%'",
            "SHOW STORAGE_CLASS TRANSITIONS LIKE _UTF8MB4'orders%'",
        ),
        (
            "SHOW STORAGE_CLASS TRANSITIONS WHERE direction = 'TO_IA'",
            "SHOW STORAGE_CLASS TRANSITIONS WHERE `direction`=_UTF8MB4'TO_IA'",
        ),
        ("SET ROLE `auto`", "SET ROLE `auto`@`%`"),
        (
            "GRANT OPERATE VIEW ON db2.invoice TO 'jeffrey'@'localhost'",
            "GRANT OPERATE VIEW ON `db2`.`invoice` TO `jeffrey`@`localhost`",
        ),
        (
            "GRANT OPERATE VIEW ON *.* TO 'jeffrey'@'localhost'",
            "GRANT OPERATE VIEW ON *.* TO `jeffrey`@`localhost`",
        ),
        (
            "REVOKE OPERATE VIEW ON db2.invoice FROM 'jeffrey'@'localhost'",
            "REVOKE OPERATE VIEW ON `db2`.`invoice` FROM `jeffrey`@`localhost`",
        ),
    ];
    for (sql, expected) in cases {
        let statement = crate::New().ParseOneStmt(sql, "", "").expect(sql);
        assert_eq!(
            crate::ast::sql_restore::restore_node(statement.as_ref()).unwrap(),
            expected,
            "{sql}"
        );
    }
}
