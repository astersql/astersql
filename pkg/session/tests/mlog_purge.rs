// Copyright 2026 AsterSQL.

use astersql_session::runtime::CreateAnalyzeSession;

#[test]
fn purge_materialized_view_log_requires_a_database() {
    let (domain, session) = CreateAnalyzeSession().expect("create MLog purge session");
    session
        .execute("DROP DATABASE test")
        .expect("drop the default database to clear the current database");
    let error = match session.execute("PURGE MATERIALIZED VIEW LOG ON base_table") {
        Ok(_) => panic!("PURGE without an explicit or current database must fail"),
        Err(error) => error,
    };
    assert_eq!(error.to_string(), "No database selected");
    domain.close();
}
