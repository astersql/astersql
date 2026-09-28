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
