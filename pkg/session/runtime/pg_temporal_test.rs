// Copyright 2026 AsterSQL.
use super::*;
use astersql_types::pg_temporal::PgDateOrder;

#[test]
fn date_clock_lifecycle_and_prepared_ast_constants() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    let date = |sql: &str| {
        session.execute(sql).unwrap()[0]
            .next_row()
            .unwrap()
            .unwrap()
    };
    session.execute("SET time_zone = 'Asia/Shanghai'").unwrap();
    session.execute("SET timestamp = 946681200").unwrap();
    session.execute("BEGIN").unwrap();
    session.execute("SET timestamp = 946854000").unwrap();
    assert_eq!(date("SELECT CAST('today' AS DATE)"), ["2000-01-01"]);
    session.execute("ROLLBACK").unwrap();
    assert_eq!(date("SELECT CAST('today' AS DATE)"), ["2000-01-03"]);
    session.execute("BEGIN").unwrap();
    session.execute("SET time_zone = 'UTC'").unwrap();
    assert_eq!(date("SELECT CAST('today' AS DATE)"), ["2000-01-02"]);
    session.execute("COMMIT").unwrap();
    session.pg_date_context(Some(PgDateOrder::Dmy)).unwrap();
    let (id, _, _) = session
        .prepare_protocol_statement_with_parameter_types(
            "SELECT CAST(? AS DATE), CAST('today' AS DATE), CAST('01/02/2003' AS DATE)",
            &[25],
        )
        .unwrap();
    session.execute("SET timestamp = 947026800").unwrap();
    session.pg_date_context(Some(PgDateOrder::Mdy)).unwrap();
    let mut sets = session
        .execute_protocol_statement(id, &[ConcretePreparedArgument::Text("01/02/2003".into())])
        .unwrap();
    assert_eq!(
        sets[0].next_row().unwrap().unwrap(),
        ["2003-01-02", "2000-01-02", "2003-02-01"]
    );
    session.close_protocol_statement(id).unwrap();
    // Frozen AST values must not leak into a subsequent simple query.
    assert_eq!(date("SELECT CAST('today' AS DATE)"), ["2000-01-04"]);
}

#[test]
fn timestamp_clock_lifecycle_and_prepared_ast_constants() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    let scalar = |sql: &str| {
        session.execute(sql).unwrap()[0]
            .next_row()
            .unwrap()
            .unwrap()
    };
    session.execute("SET time_zone = 'Asia/Shanghai'").unwrap();
    session.execute("SET timestamp = 946681200.123456").unwrap();
    session.execute("BEGIN").unwrap();
    session.execute("SET timestamp = 946854000").unwrap();
    assert_eq!(
        scalar("SELECT timestamp 'now'"),
        ["2000-01-01 07:00:00.123456"]
    );
    session.execute("COMMIT").unwrap();
    let (id, _, _) = session
        .prepare_protocol_statement_with_parameter_types(
            "SELECT timestamp 'now', CAST(? AS TIMESTAMP)",
            &[25],
        )
        .unwrap();
    session.execute("SET timestamp = 947026800").unwrap();
    let mut result = session
        .execute_protocol_statement(id, &[ConcretePreparedArgument::Text("now".into())])
        .unwrap();
    assert_eq!(
        result[0].next_row().unwrap().unwrap(),
        ["2000-01-03 07:00:00", "2000-01-05 07:00:00"]
    );
    session.close_protocol_statement(id).unwrap();
}

#[test]
fn pg_timestamptz_session_cast_identity() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    session.execute("SET time_zone = 'UTC'").unwrap();
    for (text, expected) in [
        ("2000-01-01 08:00:00+08", "2000-01-01 00:00:00+00"),
        ("2000-01-01 00:00:00+08", "1999-12-31 16:00:00+00"),
    ] {
        let mut records = session
            .execute(&format!("SELECT CAST('{text}' AS timestamptz)"))
            .unwrap();
        assert_eq!(records[0].next_row().unwrap().unwrap(), [expected]);
    }
    assert_eq!(
        session
            .protocol_parameter_types("SELECT ?::timestamptz", &[1184])
            .unwrap(),
        [1184]
    );
}

#[test]
fn pg_timestamptz_session_input_boundaries() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    let scalar = |sql: &str| {
        session.execute(sql).unwrap()[0]
            .next_row()
            .unwrap()
            .unwrap()
    };
    session.execute("SET time_zone = 'Asia/Shanghai'").unwrap();
    session.execute("SET timestamp = 946684800.123456").unwrap();
    assert_eq!(
        scalar("SELECT CAST('2000-01-01 00:00:00' AS timestamptz)"),
        ["2000-01-01 00:00:00+08"]
    );
    let (id, _, _) = session
        .prepare_protocol_statement_with_parameter_types(
            "SELECT CAST('now' AS timestamptz), CAST(? AS timestamptz)",
            &[25],
        )
        .unwrap();
    session.execute("SET timestamp = 946771200").unwrap();
    session.execute("SET time_zone = 'UTC'").unwrap();
    let mut result = session
        .execute_protocol_statement(id, &[ConcretePreparedArgument::Text("now".into())])
        .unwrap();
    assert_eq!(
        result[0].next_row().unwrap().unwrap(),
        ["2000-01-01 00:00:00.123456+00", "2000-01-02 00:00:00+00"]
    );
    session.close_protocol_statement(id).unwrap();
    session.execute("BEGIN").unwrap();
    session.execute("SET timestamp = 946857600").unwrap();
    assert_eq!(
        scalar("SELECT CAST('now' AS timestamptz)"),
        ["2000-01-02 00:00:00+00"]
    );
    session.execute("SET time_zone = 'Asia/Shanghai'").unwrap();
    assert_eq!(
        scalar("SELECT CAST('now' AS timestamptz)"),
        ["2000-01-02 08:00:00+08"]
    );
    for (word, expected) in [
        ("today", "2000-01-02 00:00:00+08"),
        ("yesterday", "2000-01-01 00:00:00+08"),
        ("tomorrow", "2000-01-03 00:00:00+08"),
    ] {
        assert_eq!(
            scalar(&format!("SELECT CAST('{word}' AS timestamptz)")),
            [expected]
        );
    }
    session.execute("ROLLBACK").unwrap();
    assert_eq!(
        scalar("SELECT CAST('now' AS timestamptz)"),
        ["2000-01-03 08:00:00+08"]
    );
}

#[test]
fn pg_timestamptz_dst_resolution() {
    println!("native_tzdata={}", chrono_tz::IANA_TZDB_VERSION);
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    session
        .execute("SET time_zone = 'America/New_York'")
        .unwrap();
    for (wall, utc) in [
        ("2018-03-11 02:30:00", "2018-03-11 07:30:00+00"),
        ("2018-11-04 01:30:00", "2018-11-04 06:30:00+00"),
        ("2018-11-04 01:30:00-04", "2018-11-04 05:30:00+00"),
        ("2018-11-04 01:30:00-05", "2018-11-04 06:30:00+00"),
    ] {
        assert_eq!(session.pg_timestamptz_input(wall).unwrap().to_string(), utc);
    }
}

#[test]
fn timestamptz_cast_rejects_native_timestamp_without_provenance() {
    let (domain, native) = CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE tz_cast_source").unwrap();
    native.execute("USE tz_cast_source").unwrap();
    native
        .execute("CREATE TABLE native_times (t TIMESTAMP(6), d DATETIME(6))")
        .unwrap();
    native.execute("INSERT INTO native_times VALUES ('2000-01-01 00:00:00', '2000-01-01 00:00:00'), (NULL, NULL)").unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    session.execute("USE tz_cast_source").unwrap();
    session.execute("SET time_zone = 'Asia/Shanghai'").unwrap();
    for suffix in ["", " WHERE false", " WHERE t IS NULL"] {
        let error = session
            .execute(&format!(
                "SELECT CAST(t AS timestamptz) FROM native_times{suffix}"
            ))
            .err()
            .expect("native TIMESTAMP has no conversion provenance");
        assert_eq!(
            error.pg_temporal_error(),
            Some(astersql_types::pg_temporal::PgTemporalError::TimestampBridge)
        );
    }
    let mut result = session
        .execute("SELECT CAST(d AS timestamptz) FROM native_times WHERE d IS NOT NULL")
        .unwrap();
    assert_eq!(
        result[0].next_row().unwrap().unwrap(),
        ["2000-01-01 00:00:00+08"]
    );
    // The guard is PostgreSQL-only; native MySQL reads retain their behavior.
    assert_eq!(
        native
            .execute("SELECT t FROM native_times WHERE t IS NOT NULL")
            .unwrap()[0]
            .next_row()
            .unwrap()
            .unwrap(),
        ["2000-01-01 00:00:00.000000"]
    );
}

#[test]
fn pg_timestamptz_null_union_session_display() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (zone, expected) in [
        ("UTC", "2000-01-01 00:00:00+00"),
        ("Asia/Shanghai", "2000-01-01 08:00:00+08"),
    ] {
        session
            .execute(&format!("SET time_zone = '{zone}'"))
            .unwrap();
        let mut sets = session.execute("SELECT NULL::timestamptz(3) UNION ALL SELECT '2000-01-01 00:00:00+00'::timestamptz(3)").unwrap();
        assert_eq!(sets[0].next_row().unwrap().unwrap(), [CONCRETE_NULL_VALUE]);
        assert_eq!(sets[0].next_row().unwrap().unwrap(), [expected]);
        assert!(sets[0].next_row().unwrap().is_none());
    }
}
