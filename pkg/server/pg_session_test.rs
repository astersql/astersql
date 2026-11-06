// Copyright 2026 AsterSQL.
use crate::pg_session::SessionQuery;
#[test]
fn pg_introspection_namespace_parser_boundaries() {
    for sql in [
        "SET search_path TO public",
        "SET search_path = pg_catalog, public",
        "RESET search_path",
        "SHOW search_path",
    ] {
        assert!(SessionQuery::parse(sql).unwrap().is_some(), "{sql}");
    }
    for sql in [
        "SET search_path TO public; SELECT 1",
        "SET search_path TO NULL",
    ] {
        assert!(SessionQuery::parse(sql).is_err(), "{sql}");
    }
    for sql in [
        "SELECT 'current_schema()'",
        "SELECT current_schema_other()",
        "SELECT pg_catalog.current_schema() AS schema",
        "SELECT current_schema() FROM t",
    ] {
        assert!(SessionQuery::parse(sql).unwrap().is_none(), "{sql}");
    }
}

#[test]
fn date_style_parser_keeps_commands_and_literals_separate() {
    for sql in [
        "SET DateStyle TO 'ISO, DMY'",
        "SET DateStyle = ISO, MDY",
        "SET DateStyle TO YMD",
        "SHOW DateStyle",
        "RESET DateStyle",
        "SET DateStyle = DEFAULT",
    ] {
        assert!(SessionQuery::parse(sql).unwrap().is_some(), "{sql}");
    }
    for sql in [
        "SET DateStyle TO 'ISO, DMY'; SELECT 1",
        "SET DateStyle TO NULL",
        "SHOW DateStyle extra",
        "SET DateStyle TO",
        "SET DateStyle TO ISO,,DMY",
    ] {
        assert!(SessionQuery::parse(sql).is_err(), "{sql}");
    }
    assert!(
        SessionQuery::parse("SELECT 'SET DateStyle TO DMY'")
            .unwrap()
            .is_none()
    );
}

#[test]
fn interval_style_commands() {
    for sql in [
        "SET IntervalStyle TO 'postgres'",
        "SET SESSION IntervalStyle = iso_8601",
        "SET LOCAL IntervalStyle = DEFAULT",
        "SHOW IntervalStyle",
        "RESET IntervalStyle",
    ] {
        assert!(SessionQuery::parse(sql).unwrap().is_some(), "{sql}");
    }
    for (sql, state) in [
        ("SET IntervalStyle='bad'", "22023"),
        ("SET IntervalStyle='default'", "22023"),
        ("SET IntervalStyle = NULL", "22023"),
        ("SHOW IntervalStyle extra", "42601"),
        ("SET IntervalStyle TO", "42601"),
        ("SET IntervalStyle='postgres'; SELECT 1", "42601"),
    ] {
        assert_eq!(SessionQuery::parse(sql).unwrap_err().0, state, "{sql}");
    }
    assert!(
        SessionQuery::parse("SELECT 'SET IntervalStyle=iso_8601'")
            .unwrap()
            .is_none()
    );
}

#[test]
fn search_path_startup_and_reset_preserve_session_state() {
    use crate::pg_session::PgSession;
    for value in [Some("public"), Some("pg_catalog, public"), Some(""), None] {
        assert!(PgSession::from_startup(value).is_ok(), "{value:?}");
    }
    for value in ["public; SELECT 1", "public,,pg_catalog", "NULL", "DEFAULT"] {
        assert!(PgSession::from_startup(Some(value)).is_err(), "{value}");
    }
    for sql in [
        "SET SESSION search_path TO public",
        "SET SESSION search_path = DEFAULT",
    ] {
        assert!(SessionQuery::parse(sql).unwrap().is_some());
    }
    use crate::conn::{CancellationToken, SessionDriver};
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = crate::runtime::ConcreteSessionDriver::new_for_test(
        domain,
        crate::runtime::BootstrapAuthMode::InsecureRootOnly,
    );
    let context = driver.open_ctx(91101, 0, 45, "test", None).unwrap();
    let mut pg = PgSession::from_startup(Some("pg_catalog")).unwrap();
    pg.relation_error_position = Some(7);
    pg.error_detail = Some("kept detail");
    pg.function_error_hint = Some("kept hint");
    pg.execute(&SessionQuery::Set(vec!["public".into()]), context.as_ref())
        .unwrap();
    context
        .execute_query("BEGIN", false, &CancellationToken::new())
        .unwrap();
    pg.record_error(context.as_ref());
    // Even a direct reset must not erase failed-transaction bookkeeping.
    pg.execute(&SessionQuery::Reset, context.as_ref()).unwrap();
    assert_eq!(pg.transaction_status(context.as_ref()), b'E');
    context
        .execute_query("ROLLBACK", false, &CancellationToken::new())
        .unwrap();
    assert_eq!(pg.relation_error_position, Some(7));
    assert_eq!(pg.error_detail, Some("kept detail"));
    assert_eq!(pg.function_error_hint, Some("kept hint"));
}

#[test]
fn pg11_30_bounded_startup_options() {
    use crate::pg_session::PgSession;
    for options in [
        "",
        "-c search_path=pg_catalog",
        "-csearch_path=public",
        "--search_path=public",
        "-c search_path=",
        "-c search_path=pg_catalog,\\ public",
        "-c search_path=public -c search_path=pg_catalog",
    ] {
        assert!(
            PgSession::from_startup_options(None, Some(options)).is_ok(),
            "{options}"
        );
    }
    for options in [
        "-c",
        "-x",
        "-c work_mem=1MB",
        "--timezone=UTC",
        "-c search_path=public -c work_mem=1MB",
        "-c search_path=public\\",
        "-c search_path=NULL",
        "-c search_path=public;SELECT",
    ] {
        assert!(
            PgSession::from_startup_options(None, Some(options)).is_err(),
            "{options}"
        );
    }
}

#[test]
fn pg11_30_default_path_retains_set_command_tag() {
    let set = SessionQuery::parse("SET search_path TO DEFAULT")
        .unwrap()
        .unwrap();
    assert_eq!(set.command(), "SET");
    let reset = SessionQuery::parse("RESET search_path").unwrap().unwrap();
    assert_eq!(reset.command(), "RESET");
}

#[test]
fn pg13_033_column_privilege_parser() {
    for privilege in ["SELECT", "INSERT", "UPDATE", "REFERENCES"] {
        for (action, connector) in [("GRANT", "TO"), ("REVOKE", "FROM")] {
            let sql = format!("{action} {privilege}(id) ON public.column_acl {connector} reader");
            assert!(SessionQuery::parse(&sql).unwrap().is_some(), "{sql}");
        }
    }
    assert!(SessionQuery::parse("GRANT DELETE(id) ON public.column_acl TO reader").is_err());
    assert!(
        SessionQuery::parse("GRANT DELETE ON public.column_acl TO reader")
            .unwrap()
            .is_some()
    );
}

#[test]
fn pg13_037_constraint_utility_boundaries() {
    assert_eq!(SessionQuery::parse("CREATE TABLE t(a integer,CONSTRAINT u UNIQUE NULLS NOT DISTINCT(a) DEFERRABLE INITIALLY DEFERRED)").err().unwrap().0,"0A000");
    assert!(
        SessionQuery::parse("CREATE TABLE t(a text DEFAULT 'nulls not distinct')")
            .unwrap()
            .is_none()
    );
    for verb in ["GRANT", "REVOKE"] {
        let suffix = if verb == "GRANT" { "TO" } else { "FROM" };
        assert!(
            SessionQuery::parse(&format!("{verb} TRIGGER ON public.t {suffix} reader"))
                .unwrap()
                .is_some()
        );
        assert_eq!(
            SessionQuery::parse(&format!("{verb} TRIGGER(a) ON public.t {suffix} reader"))
                .err()
                .unwrap()
                .0,
            "0A000"
        );
    }
}

#[test]
fn cat14_06_shared_comment_gate() {
    for target in ["DATABASE comment_db", "ROLE root", "TABLESPACE pg_default"] {
        for value in ["'库 中文'", "''", "NULL"] {
            let sql = format!("/* gate */ COMMENT ON {target} IS {value};");
            assert_eq!(SessionQuery::parse(&sql).unwrap_err().0, "0A000", "{sql}");
        }
    }
    assert!(
        SessionQuery::parse("SELECT 'COMMENT ON DATABASE x IS NULL'")
            .unwrap()
            .is_none()
    );
}
