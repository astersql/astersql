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
