// Copyright 2026 AsterSQL.
use crate::pg_session::SessionQuery;
#[test]
fn pg_introspection_namespace_parser_boundaries() {
    for sql in [
        "SET search_path TO public",
        "SET search_path = pg_catalog, public",
        "RESET search_path",
        "SHOW search_path",
        "SELECT pg_catalog.current_schema() AS schema",
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
