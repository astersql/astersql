// Copyright 2026 AsterSQL.

use std::sync::Arc;

use astersql_parser_auth::parser::auth::auth::UserIdentity;

use crate::runtime::{ConcreteSession, CreateAnalyzeSession};

fn identity(user: &str, host: &str) -> UserIdentity {
    UserIdentity {
        username: user.to_owned(),
        hostname: host.to_owned(),
        ..Default::default()
    }
}

#[test]
fn field_list_enforces_go_table_privilege_check() {
    let (domain, admin) = CreateAnalyzeSession().expect("canonical FIELD_LIST session");
    admin
        .execute("create table field_list_secret (id int primary key, secret varchar(16))")
        .expect("create FIELD_LIST target");
    admin
        .execute("create user 'field_list_user'@'localhost'")
        .expect("create restricted FIELD_LIST user");

    let mut restricted = ConcreteSession::new(Arc::clone(&domain));
    restricted
        .AuthenticateUserForTest(&identity("field_list_user", "localhost"))
        .expect("authenticate restricted FIELD_LIST user");

    let denied = match restricted.field_list("field_list_secret") {
        Ok(_) => panic!("Go session.FieldList denies metadata without table privileges"),
        Err(error) => error,
    };
    assert!(
        denied.to_string().contains("SELECT command denied"),
        "unexpected FIELD_LIST denial: {denied}"
    );

    admin
        .execute("grant select on test.field_list_secret to 'field_list_user'@'localhost'")
        .expect("grant FIELD_LIST table privilege");
    let fields = restricted
        .field_list("field_list_secret")
        .expect("granted FIELD_LIST metadata");
    assert_eq!(
        fields
            .iter()
            .map(|field| field.column_as_name.O.as_str())
            .collect::<Vec<_>>(),
        vec!["id", "secret"]
    );
}

#[test]
fn pg_timestamp_constant_union_precision() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    let mut records = pg.execute("SELECT '1999-12-31 23:59:59.5'::timestamp(0) UNION ALL SELECT '2000-01-01 00:00:00.5'::timestamp(0)").unwrap();
    assert_eq!(
        records[0].next_row().unwrap().unwrap()[0],
        "1999-12-31 23:59:59"
    );
    assert_eq!(
        records[0].next_row().unwrap().unwrap()[0],
        "2000-01-01 00:00:01"
    );
}

#[test]
fn pg_interval_constant_union_precision() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session =
        ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    let (id, _, _) = session
        .prepare_protocol_statement_with_parameter_types(
            "SELECT NULL::interval(3) UNION ALL SELECT CAST(? AS interval(3))",
            &[25],
        )
        .unwrap();
    for (input, expected) in [
        ("1.2345 seconds", "00:00:01.235"),
        ("-1.2345 seconds", "-00:00:01.235"),
    ] {
        let mut sets = session
            .execute_protocol_statement(
                id,
                &[crate::runtime::ConcretePreparedArgument::Text(input.into())],
            )
            .unwrap();
        sets[0].next_row().unwrap().unwrap();
        assert_eq!(sets[0].next_row().unwrap().unwrap(), [expected]);
        assert!(sets[0].next_row().unwrap().is_none());
    }
    session.close_protocol_statement(id).unwrap();
}
