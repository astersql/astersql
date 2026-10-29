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

#[test]
fn interval_style_lifecycle() {
    use astersql_types::pg_temporal::{PgIntervalSetting as A, PgIntervalStyle as S};
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    let other = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    session.pg_interval_style(Some(A::Startup(S::Iso8601)));
    session.execute("BEGIN").unwrap();
    session.pg_interval_style(Some(A::Set {
        style: S::SqlStandard,
        local: false,
    }));
    session.execute("SAVEPOINT s").unwrap();
    session.pg_interval_style(Some(A::Set {
        style: S::PostgresVerbose,
        local: true,
    }));
    assert_eq!(session.pg_interval_style(None), S::PostgresVerbose);
    session.execute("ROLLBACK TO SAVEPOINT s").unwrap();
    assert_eq!(session.pg_interval_style(None), S::SqlStandard);
    session.execute("ROLLBACK").unwrap();
    assert_eq!(session.pg_interval_style(None), S::Iso8601);
    assert_eq!(other.pg_interval_style(None), S::Postgres);
    session.execute("BEGIN").unwrap();
    session.pg_interval_style(Some(A::Set {
        style: S::SqlStandard,
        local: false,
    }));
    session.pg_interval_style(Some(A::Set {
        style: S::PostgresVerbose,
        local: true,
    }));
    session.execute("COMMIT").unwrap();
    assert_eq!(session.pg_interval_style(None), S::SqlStandard);
    session.pg_interval_style(Some(A::Reset));
    assert_eq!(session.pg_interval_style(None), S::Iso8601);
    session.pg_interval_style(Some(A::Set {
        style: S::PostgresVerbose,
        local: true,
    }));
    assert_eq!(session.pg_interval_style(None), S::Iso8601);
    session.pg_interval_style(Some(A::Set {
        style: S::SqlStandard,
        local: false,
    }));
    let (id, _, _) = session
        .prepare_protocol_statement_with_parameter_types(
            "SELECT CAST('-1 2:03:04' AS interval), CAST(? AS interval)",
            &[25],
        )
        .unwrap();
    session.pg_interval_style(Some(A::Set {
        style: S::Postgres,
        local: false,
    }));
    let mut result = session
        .execute_protocol_statement(id, &[ConcretePreparedArgument::Text("-1 2:03:04".into())])
        .unwrap();
    assert_eq!(
        result[0].next_row().unwrap().unwrap(),
        ["-1 days -02:03:04", "-1 days +02:03:04"]
    );
    session.close_protocol_statement(id).unwrap();
    session.execute("BEGIN").unwrap();
    session.pg_interval_style(Some(A::Set {
        style: S::Iso8601,
        local: false,
    }));
    session.execute("SAVEPOINT release_style").unwrap();
    session.pg_interval_style(Some(A::Set {
        style: S::PostgresVerbose,
        local: true,
    }));
    session.execute("RELEASE SAVEPOINT release_style").unwrap();
    assert_eq!(session.pg_interval_style(None), S::PostgresVerbose);
    session.state.borrow_mut().next_dml_commit_error = Some("interval style commit failure".into());
    assert!(session.execute("COMMIT").is_err());
    assert_eq!(session.pg_interval_style(None), S::Postgres);
}

#[test]
fn interval_precision_prepared_constants_and_parameters() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    let (id, _, _) = session
        .prepare_protocol_statement_with_parameter_types(
            "SELECT CAST('1.2345 seconds' AS interval(3)), CAST(? AS interval HOUR TO MINUTE)",
            &[25],
        )
        .unwrap();
    for value in ["1 day 2:03:04", "1 day -2:03:04"] {
        let mut results = session
            .execute_protocol_statement(id, &[ConcretePreparedArgument::Text(value.into())])
            .unwrap();
        assert_eq!(
            results[0].next_row().unwrap().unwrap(),
            [
                "00:00:01.235",
                if value.contains('-') {
                    "1 day -02:03:00"
                } else {
                    "1 day 02:03:00"
                }
            ]
        );
    }
    let mut results = session
        .execute_protocol_statement(id, &[ConcretePreparedArgument::Null])
        .unwrap();
    assert_eq!(
        results[0].next_row().unwrap().unwrap(),
        ["00:00:01.235", "<nil>"]
    );
    session.close_protocol_statement(id).unwrap();
}

#[test]
fn pg06_task_077_kernel() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    session.execute("SET time_zone = '+08:00'").unwrap();
    for (input, precision, expected) in [
        (Some("1999-12-31 23:30:00+00"), -1, Some("07:30:00")),
        (
            Some("294276-12-31 23:59:59.999999+00"),
            -1,
            Some("07:59:59.999999"),
        ),
        (Some("2000-01-01 15:59:59.999500+00"), 3, Some("24:00:00")),
        (Some("infinity"), -1, None),
        (Some("-infinity"), -1, None),
        (None, -1, None),
    ] {
        assert_eq!(
            session
                .pg_timestamptz_to_time_value(input.map(str::to_owned), precision)
                .unwrap()
                .as_deref(),
            expected,
            "{input:?} precision={precision}"
        );
    }
    for input in ["garbage", "2023-02-29 00:00:00+00"] {
        assert!(
            session
                .pg_timestamptz_to_time_value(Some(input.into()), -1)
                .is_err()
        );
    }
}

#[test]
fn pg08_02_execution_context_snapshot() {
    use astersql_expression::EvalContext as _;
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    session.execute("SET timestamp = 1704159000").unwrap();
    session
        .execute("SET time_zone = 'America/New_York'")
        .unwrap();
    session.execute("BEGIN").unwrap();
    let before = session.pg_temporal_eval_context().unwrap();
    session.execute("SET timestamp = 1704245400").unwrap();
    session.execute("SET time_zone = '+05:30'").unwrap();
    let after = session.pg_temporal_eval_context().unwrap();
    assert_eq!(before.Location().name(), "America/New_York");
    assert_eq!(after.PgFixedTimeZone().unwrap().local_minus_utc(), 19800);
    assert_eq!(
        before.CurrentTime().unwrap().timestamp(),
        after.CurrentTime().unwrap().timestamp()
    );
    session.execute("ROLLBACK").unwrap();
}

#[test]
fn pg08_02_row_source() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    session
        .execute("SET time_zone = 'America/New_York'")
        .unwrap();
    for (expression, empty) in [
        ("date_trunc('day'::text, t)", false),
        (
            "date_trunc('day'::text, date_trunc('hour'::text, t))",
            false,
        ),
        ("date_trunc('day'::text, t)", true),
    ] {
        let sql = format!(
            "SELECT {expression} AS v FROM (SELECT '2024-01-02 01:30:00+00'::timestamptz AS t) AS source{}",
            if empty { " WHERE false" } else { "" }
        );
        let mut sets = session.execute(&sql).unwrap();
        assert_eq!(
            sets[0].result_fields()[0]
                .as_ref()
                .unwrap()
                .column
                .FieldType
                .GetType(),
            astersql_parser_mysql::r#type::TypePgTimestampTz
        );
        if empty {
            assert!(sets[0].next_row().unwrap().is_none());
        } else {
            assert_eq!(
                sets[0].next_row().unwrap().unwrap(),
                ["2024-01-01 00:00:00-05"]
            );
        }
    }
}

#[test]
fn pg08_03_timestamp_conversion() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (zone, expected) in [
        ("UTC", "2024-01-02 00:00:00+00"),
        ("America/New_York", "2024-01-01 19:00:00-05"),
    ] {
        session
            .execute(&format!("SET time_zone = '{zone}'"))
            .unwrap();
        let mut implicit = session
            .execute(
                "SELECT date_trunc('day'::text,'2024-01-02 01:30:00'::timestamp,'UTC'::text) AS v",
            )
            .unwrap();
        let mut explicit = session.execute("SELECT date_trunc('day'::text,('2024-01-02 01:30:00'::timestamp)::timestamptz,'UTC'::text) AS v").unwrap();
        assert_eq!(implicit[0].next_row().unwrap().unwrap(), [expected]);
        assert_eq!(explicit[0].next_row().unwrap().unwrap(), [expected]);
    }
    println!("pg08_03_timestamp_conversion: 2 scenarios passed");
}
