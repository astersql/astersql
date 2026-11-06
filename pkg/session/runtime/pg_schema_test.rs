// Copyright 2026 AsterSQL.
use super::*;

#[test]
fn namespace_identity_relation_ownership_and_recreation() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    pg.pg_schema_command(&PgSchemaCommand::Create("ownership_probe".into()))
        .unwrap();
    let identity = pg
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find(|f| f.name == "ownership_probe")
        .unwrap()
        .id;
    assert!(identity > 0);
    let second = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    assert_eq!(
        second
            .pg_namespace_facts()
            .unwrap()
            .into_iter()
            .find(|f| f.name == "ownership_probe")
            .unwrap()
            .id,
        identity
    );
    pg.pg_search_path(vec!["ownership_probe".into()]);
    let physical = pg.pg_bind_relation(None, "same_name", true).unwrap();
    pg.execute(&format!("CREATE TABLE \"{physical}\" (id integer)"))
        .unwrap();
    let table = domain
        .info_schema()
        .ModelTableInfoByExactName(&astersql_infoschema::CiString::new("test"), &physical)
        .unwrap();
    assert_eq!(
        table.PostgreSQLNamespace,
        Some((identity, "same_name".into()))
    );
    let bytes = astersql_meta_model::EncodeTableInfo(&table).unwrap();
    assert_eq!(
        astersql_meta_model::DecodeTableInfo(&bytes)
            .unwrap()
            .PostgreSQLNamespace,
        table.PostgreSQLNamespace
    );
    assert_eq!(
        second
            .pg_bind_relation(Some("ownership_probe"), "same_name", false)
            .unwrap(),
        physical
    );
    assert!(
        second
            .pg_bind_relation(Some("public"), &physical, false)
            .is_err()
    );
    assert!(
        pg.pg_schema_command(&PgSchemaCommand::Drop("ownership_probe".into()))
            .is_err()
    );
    pg.execute(&format!("DROP TABLE \"{physical}\"")).unwrap();
    pg.pg_schema_command(&PgSchemaCommand::Drop("ownership_probe".into()))
        .unwrap();
    pg.pg_schema_command(&PgSchemaCommand::Create("ownership_probe".into()))
        .unwrap();
    let recreated = pg
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find(|f| f.name == "ownership_probe")
        .unwrap()
        .id;
    assert_ne!(identity, recreated);
    assert!(pg.pg_bind_relation(None, "same_name", false).is_err());
    pg.pg_schema_command(&PgSchemaCommand::Drop("ownership_probe".into()))
        .unwrap();
    pg.pg_search_path(vec!["missing".into()]);
    assert_eq!(
        pg.pg_bind_relation(None, "absent", true)
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "3F000"
    );
}

#[test]
fn legacy_empty_namespace_requires_explicit_migration_without_reusing_global_ids() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let database = domain
        .info_schema()
        .SchemaByName(&astersql_infoschema::CiString::new("test"))
        .unwrap();
    let context = kv::Context::default();
    domain.storage_handle().with_storage(|store| {
        kv::RunInNewTxn(&context, store, true, |_, transaction| {
            transaction.Set(astersql_meta::transaction_meta_hash_key(b"PgNamespaces", database.id.to_string().as_bytes()),
                br#"{"public":{"owner":"root","usage":["PUBLIC"]},"pg_catalog":{"owner":"root","usage":["PUBLIC"]},"legacy":{"owner":"root","usage":[]}}"#.to_vec())
        })
    }).unwrap();
    let first = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    assert_eq!(
        first
            .pg_namespace_facts()
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "0A000"
    );
    first
        .pg_schema_command(&PgSchemaCommand::Rename {
            schema: "legacy".into(),
            name: "migrated".into(),
        })
        .unwrap();
    let identity = first
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find(|f| f.name == "migrated")
        .unwrap()
        .id;
    let second = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    assert_eq!(
        second
            .pg_namespace_facts()
            .unwrap()
            .into_iter()
            .find(|f| f.name == "migrated")
            .unwrap()
            .id,
        identity
    );
    second
        .pg_schema_command(&PgSchemaCommand::Create("new_namespace".into()))
        .unwrap();
    assert!(
        second
            .pg_namespace_facts()
            .unwrap()
            .into_iter()
            .find(|f| f.name == "new_namespace")
            .unwrap()
            .id
            > identity
    );
    for schema in ["migrated", "new_namespace"] {
        second
            .pg_schema_command(&PgSchemaCommand::Drop(schema.into()))
            .unwrap();
    }
}

#[test]
fn sequence_allocation_identity_and_recovery() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let a = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    let b = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    a.pg_schema_command(&PgSchemaCommand::Create("seq_space".into()))
        .unwrap();
    let create = PgSequenceCommand::Create {
        owned_by: None,
        declared_type: astersql_meta_model::PostgreSQLSequenceType::Bigint,
        schema: Some("seq_space".into()),
        name: "same".into(),
        options: "START WITH 10 INCREMENT BY 1 MINVALUE 1 MAXVALUE 11 CACHE 1 NO CYCLE".into(),
    };
    let next = PgSequenceCommand::Value {
        schema: Some("seq_space".into()),
        name: Some("same".into()),
        next: true,
    };
    let current = PgSequenceCommand::Value {
        schema: Some("seq_space".into()),
        name: Some("same".into()),
        next: false,
    };
    a.pg_sequence(&create).unwrap();
    assert_eq!(
        a.pg_sequence(&current)
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "55000"
    );
    assert_eq!(a.pg_sequence(&next).unwrap(), Some(10));
    assert_eq!(
        b.pg_sequence(&current)
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "55000"
    );
    assert_eq!(b.pg_sequence(&next).unwrap(), Some(11));
    assert_eq!(
        a.pg_sequence(&next)
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "2200H"
    );
    assert_eq!(a.pg_sequence(&current).unwrap(), Some(10));
    assert_eq!(b.pg_sequence(&current).unwrap(), Some(11));
    let drop = PgSequenceCommand::Drop {
        schema: Some("seq_space".into()),
        name: "same".into(),
    };
    a.pg_sequence(&drop).unwrap();
    a.pg_sequence(&create).unwrap();
    assert_eq!(
        a.pg_sequence(&current)
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "55000"
    );
    assert_eq!(a.pg_sequence(&next).unwrap(), Some(10));
    a.pg_sequence(&drop).unwrap();
    a.pg_schema_command(&PgSchemaCommand::Drop("seq_space".into()))
        .unwrap();
}

#[test]
fn sequence_set_value_identity_and_recovery() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let a = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    let b = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    let create = PgSequenceCommand::Create {
        owned_by: None,
        declared_type: astersql_meta_model::PostgreSQLSequenceType::Bigint,
        schema: None,
        name: "seq_set_local".into(),
        options: "START WITH 10 MINVALUE 1 MAXVALUE 1000 CACHE 1 NO CYCLE".into(),
    };
    let set = |value| PgSequenceCommand::Set {
        schema: None,
        name: Some("seq_set_local".into()),
        value,
        is_called: Some(true),
    };
    let next = PgSequenceCommand::Value {
        schema: None,
        name: Some("seq_set_local".into()),
        next: true,
    };
    let curr = PgSequenceCommand::Value {
        schema: None,
        name: Some("seq_set_local".into()),
        next: false,
    };
    let drop = PgSequenceCommand::Drop {
        schema: None,
        name: "seq_set_local".into(),
    };
    a.pg_sequence(&create).unwrap();
    let uncalled = |value| PgSequenceCommand::Set {
        schema: None,
        name: Some("seq_set_local".into()),
        value,
        is_called: Some(false),
    };
    assert_eq!(a.pg_sequence(&uncalled(Some(60))).unwrap(), Some(60));
    assert_eq!(
        a.pg_sequence(&curr)
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "55000"
    );
    assert_eq!(b.pg_sequence(&next).unwrap(), Some(60));
    assert_eq!(a.pg_sequence(&next).unwrap(), Some(61));
    assert_eq!(a.pg_sequence(&uncalled(Some(70))).unwrap(), Some(70));
    assert_eq!(a.pg_sequence(&curr).unwrap(), Some(61));
    assert_eq!(b.pg_sequence(&next).unwrap(), Some(70));
    assert_eq!(a.pg_sequence(&curr).unwrap(), Some(61));
    assert_eq!(a.pg_sequence(&set(Some(40))).unwrap(), Some(40));
    assert_eq!(a.pg_sequence(&curr).unwrap(), Some(40));
    assert_eq!(b.pg_sequence(&next).unwrap(), Some(41));
    assert_eq!(a.pg_sequence(&set(Some(20))).unwrap(), Some(20));
    assert_eq!(b.pg_sequence(&curr).unwrap(), Some(41));
    assert_eq!(b.pg_sequence(&next).unwrap(), Some(21));
    assert_eq!(a.pg_sequence(&set(None)).unwrap(), None);
    assert_eq!(
        a.pg_sequence(&set(Some(0)))
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "22003"
    );
    assert_eq!(a.pg_sequence(&curr).unwrap(), Some(20));
    assert_eq!(b.pg_sequence(&next).unwrap(), Some(22));
    assert_eq!(a.pg_sequence(&uncalled(Some(95))).unwrap(), Some(95));
    assert_eq!(a.pg_sequence(&uncalled(None)).unwrap(), None);
    assert!(a.pg_sequence(&uncalled(Some(0))).is_err());
    assert_eq!(a.pg_sequence(&curr).unwrap(), Some(20));
    assert_eq!(b.pg_sequence(&next).unwrap(), Some(95));
    a.pg_sequence(&drop).unwrap();
    a.pg_sequence(&create).unwrap();
    assert_eq!(
        a.pg_sequence(&curr)
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "55000"
    );
    assert_eq!(b.pg_sequence(&next).unwrap(), Some(10));
    a.pg_sequence(&drop).unwrap();
}

#[test]
fn sequence_cache_setval_preserves_other_connections() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let a = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    let b = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    let create = PgSequenceCommand::Create {
        owned_by: None,
        declared_type: astersql_meta_model::PostgreSQLSequenceType::Bigint,
        schema: None,
        name: "seq_cache_local21".into(),
        options: "START WITH 10 MINVALUE 1 MAXVALUE 1000 CACHE 3 NO CYCLE".into(),
    };
    let next = PgSequenceCommand::Value {
        schema: None,
        name: Some("seq_cache_local21".into()),
        next: true,
    };
    let set = |value, called| PgSequenceCommand::Set {
        schema: None,
        name: Some("seq_cache_local21".into()),
        value,
        is_called: Some(called),
    };
    a.pg_sequence(&create).unwrap();
    assert_eq!(a.pg_sequence(&next).unwrap(), Some(10));
    assert_eq!(b.pg_sequence(&set(Some(40), true)).unwrap(), Some(40));
    assert_eq!(a.pg_sequence(&next).unwrap(), Some(11));
    assert_eq!(a.pg_sequence(&next).unwrap(), Some(12));
    assert_eq!(a.pg_sequence(&next).unwrap(), Some(41));
    assert!(a.pg_sequence(&set(Some(1001), false)).is_err());
    assert_eq!(a.pg_sequence(&set(None, false)).unwrap(), None);
    assert_eq!(a.pg_sequence(&next).unwrap(), Some(42));
    assert_eq!(a.pg_sequence(&set(Some(60), false)).unwrap(), Some(60));
    assert_eq!(a.pg_sequence(&next).unwrap(), Some(60));
    assert_eq!(b.pg_sequence(&next).unwrap(), Some(63));
    a.pg_sequence(&PgSequenceCommand::Drop {
        schema: None,
        name: "seq_cache_local21".into(),
    })
    .unwrap();
}

#[test]
fn identity_defaults_use_owned_sequence_and_preserve_failed_null() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let a = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    let b = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    a.execute("CREATE TABLE identity_local22 (id integer GENERATED BY DEFAULT AS IDENTITY (START 10), note text)").unwrap();
    let table = domain
        .info_schema()
        .ModelTableInfoByExactName(
            &astersql_infoschema::CiString::new("test"),
            "identity_local22",
        )
        .unwrap();
    let identity = table.Columns[0].PostgreSQLIdentity.as_ref().unwrap();
    let sequence = domain
        .info_schema()
        .TableByID(identity.SequenceID)
        .unwrap()
        .ModelMeta()
        .unwrap();
    let current = PgSequenceCommand::Value {
        schema: None,
        name: Some(sequence.Name.O.clone()),
        next: false,
    };
    assert!(a.pg_sequence(&current).is_err());
    assert!(
        a.execute("INSERT INTO identity_local22(id) VALUES (NULL)")
            .is_err()
    );
    assert!(a.pg_sequence(&current).is_err());
    a.execute("INSERT INTO identity_local22(note) VALUES ('omit')")
        .unwrap();
    assert_eq!(a.pg_sequence(&current).unwrap(), Some(10));
    assert!(b.pg_sequence(&current).is_err());
    b.execute("INSERT INTO identity_local22(id) VALUES (DEFAULT)")
        .unwrap();
    assert_eq!(b.pg_sequence(&current).unwrap(), Some(11));
    assert_eq!(a.pg_sequence(&current).unwrap(), Some(10));
    a.execute("INSERT INTO identity_local22 DEFAULT VALUES")
        .unwrap();
    assert_eq!(a.pg_sequence(&current).unwrap(), Some(12));
    assert!(
        a.execute(&format!("DROP SEQUENCE {}", sequence.Name.O))
            .is_err()
    );
    assert!(
        domain
            .info_schema()
            .TableByID(identity.SequenceID)
            .is_some()
    );
    // Ownership lifecycle support already allows dropping an identity column.
    // The protected sequence disappears with the column, never on a direct drop.
    a.execute("ALTER TABLE identity_local22 DROP COLUMN id")
        .unwrap();
    assert!(
        domain
            .info_schema()
            .TableByID(identity.SequenceID)
            .is_none()
    );
    a.execute("DROP TABLE identity_local22").unwrap();
    assert!(
        domain
            .info_schema()
            .TableByID(identity.SequenceID)
            .is_none()
    );
    a.execute(
        "CREATE TABLE identity_local22 (id integer GENERATED BY DEFAULT AS IDENTITY (START 10))",
    )
    .unwrap();
    let fresh = domain
        .info_schema()
        .ModelTableInfoByExactName(
            &astersql_infoschema::CiString::new("test"),
            "identity_local22",
        )
        .unwrap();
    assert_ne!(
        fresh.Columns[0]
            .PostgreSQLIdentity
            .as_ref()
            .unwrap()
            .SequenceID,
        identity.SequenceID
    );
    a.execute("INSERT INTO identity_local22 DEFAULT VALUES")
        .unwrap();
    a.execute("DROP TABLE identity_local22").unwrap();
}

#[test]
fn identity_explicit_policy_preserves_owned_sequence() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    for (mode, name) in [
        ("ALWAYS", "policy_always23"),
        ("BY DEFAULT", "policy_default23"),
    ] {
        let session =
            ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
        session
            .execute(&format!(
                "CREATE TABLE {name} (id bigint GENERATED {mode} AS IDENTITY (START 10))"
            ))
            .unwrap();
        let table = domain
            .info_schema()
            .ModelTableInfoByExactName(&astersql_infoschema::CiString::new("test"), name)
            .unwrap();
        let identity = table.Columns[0].PostgreSQLIdentity.as_ref().unwrap();
        let sequence = domain
            .info_schema()
            .TableByID(identity.SequenceID)
            .unwrap()
            .ModelMeta()
            .unwrap();
        let current = PgSequenceCommand::Value {
            schema: None,
            name: Some(sequence.Name.O.clone()),
            next: false,
        };
        let result = session.execute(&format!("INSERT INTO {name} VALUES (900)"));
        if mode == "ALWAYS" {
            assert!(
                result
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("GENERATED ALWAYS")
            );
        } else {
            result.unwrap();
        }
        assert!(
            session.pg_sequence(&current).is_err(),
            "explicit input must not allocate"
        );
        assert!(
            session
                .execute(&format!("INSERT INTO {name} VALUES (NULL)"))
                .is_err()
        );
        assert!(
            session.pg_sequence(&current).is_err(),
            "NULL must not allocate"
        );
        session
            .execute(&format!("INSERT INTO {name} DEFAULT VALUES"))
            .unwrap();
        assert_eq!(session.pg_sequence(&current).unwrap(), Some(10));
        session.execute(&format!("DROP TABLE {name}")).unwrap();
        assert!(
            domain
                .info_schema()
                .TableByID(identity.SequenceID)
                .is_none()
        );
    }
}

#[test]
fn identity_system_override_preserves_owned_sequence() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    for mode in ["ALWAYS", "BY DEFAULT"] {
        let session =
            ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
        session
            .execute(&format!(
                "CREATE TABLE system24 (id bigint GENERATED {mode} AS IDENTITY (START 10))"
            ))
            .unwrap();
        let table = domain
            .info_schema()
            .ModelTableInfoByExactName(&astersql_infoschema::CiString::new("test"), "system24")
            .unwrap();
        let id = table.Columns[0]
            .PostgreSQLIdentity
            .as_ref()
            .unwrap()
            .SequenceID;
        let sequence = domain
            .info_schema()
            .TableByID(id)
            .unwrap()
            .ModelMeta()
            .unwrap();
        let current = PgSequenceCommand::Value {
            schema: None,
            name: Some(sequence.Name.O.clone()),
            next: false,
        };
        session
            .execute("INSERT INTO system24 OVERRIDING SYSTEM VALUE VALUES (900)")
            .unwrap();
        assert!(session.pg_sequence(&current).is_err());
        assert!(
            session
                .execute("INSERT INTO system24 OVERRIDING SYSTEM VALUE VALUES (NULL)")
                .is_err()
        );
        assert!(
            session
                .execute("INSERT INTO system24 OVERRIDING SYSTEM VALUE DEFAULT VALUES")
                .is_err()
        );
        assert!(session.pg_sequence(&current).is_err());
        session
            .execute("INSERT INTO system24 OVERRIDING SYSTEM VALUE VALUES (DEFAULT)")
            .unwrap();
        assert_eq!(session.pg_sequence(&current).unwrap(), Some(10));
        session
            .execute("INSERT INTO system24 DEFAULT VALUES")
            .unwrap();
        assert_eq!(session.pg_sequence(&current).unwrap(), Some(11));
        session.execute("DROP TABLE system24").unwrap();
        assert!(domain.info_schema().TableByID(id).is_none());
    }
}

#[test]
fn identity_user_override_preserves_owned_sequence() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    for mode in ["ALWAYS", "BY DEFAULT"] {
        let session =
            ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
        session
            .execute(&format!(
                "CREATE TABLE user25 (id bigint GENERATED {mode} AS IDENTITY (START 10))"
            ))
            .unwrap();
        let table = domain
            .info_schema()
            .ModelTableInfoByExactName(&astersql_infoschema::CiString::new("test"), "user25")
            .unwrap();
        let id = table.Columns[0]
            .PostgreSQLIdentity
            .as_ref()
            .unwrap()
            .SequenceID;
        let sequence = domain
            .info_schema()
            .TableByID(id)
            .unwrap()
            .ModelMeta()
            .unwrap();
        let current = PgSequenceCommand::Value {
            schema: None,
            name: Some(sequence.Name.O.clone()),
            next: false,
        };
        assert!(session.pg_sequence(&current).is_err());
        for (input, expected) in [("900", 10), ("NULL", 11), ("DEFAULT", 12), ("900+1", 13)] {
            session
                .execute(&format!(
                    "INSERT INTO user25 OVERRIDING USER VALUE VALUES ({input})"
                ))
                .unwrap();
            assert_eq!(session.pg_sequence(&current).unwrap(), Some(expected));
        }
        for input in ["'invalid'"] {
            assert!(
                session
                    .execute(&format!(
                        "INSERT INTO user25 OVERRIDING USER VALUE VALUES ({input})"
                    ))
                    .is_err()
            );
            assert_eq!(session.pg_sequence(&current).unwrap(), Some(13));
            session.execute("SELECT 1").unwrap();
        }
        session
            .execute("INSERT INTO user25 OVERRIDING USER VALUE VALUES (1/0)")
            .unwrap();
        assert_eq!(session.pg_sequence(&current).unwrap(), Some(14));
        session
            .execute("INSERT INTO user25 DEFAULT VALUES")
            .unwrap();
        assert_eq!(session.pg_sequence(&current).unwrap(), Some(15));
        session.execute("DROP TABLE user25").unwrap();
        assert!(domain.info_schema().TableByID(id).is_none());
    }
}

#[test]
fn pg11_30_missing_schema_relation_diagnostics() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (create, expected) in [(false, "42P01"), (true, "3F000")] {
        let error = pg
            .pg_bind_relation(Some("absent_schema"), "path_doc", create)
            .unwrap_err();
        assert_eq!(error.pg_function_error().unwrap().state, expected);
    }
}

#[test]
fn identity_declaration_bounds_sqlstate() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for option in ["START 32768", "MAXVALUE 32768"] {
        let error = session
            .execute(&format!(
                "CREATE TABLE bad_identity(id smallint GENERATED ALWAYS AS IDENTITY ({option}))"
            ))
            .err()
            .unwrap();
        assert_eq!(error.pg_array_error().unwrap().state, "22023");
        session.execute("SELECT 1").unwrap();
    }
    session
        .execute(
            "CREATE TABLE bad_identity(id smallint GENERATED ALWAYS AS IDENTITY (START 32767))",
        )
        .unwrap();
    session
        .execute("INSERT INTO bad_identity DEFAULT VALUES")
        .unwrap();
    session.execute("DROP TABLE bad_identity").unwrap();
}

#[test]
fn identity_prepare_validates_without_allocating() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    session
        .execute("CREATE TABLE identity_prepare(id bigint GENERATED ALWAYS AS IDENTITY (START 10))")
        .unwrap();
    for (sql, state) in [
        (
            "INSERT INTO identity_prepare VALUES(900) RETURNING id",
            "428C9",
        ),
        (
            "INSERT INTO identity_prepare OVERRIDING USER VALUE VALUES('invalid') RETURNING id",
            "22P02",
        ),
    ] {
        let error = session.prepare_protocol_statement(sql).err().unwrap();
        assert_eq!(
            error
                .pg_array_error()
                .map(|e| e.state)
                .or_else(|| error.pg_int4_error().map(|e| e.sqlstate())),
            Some(state)
        );
    }
    session
        .prepare_protocol_statement(
            "INSERT INTO identity_prepare OVERRIDING USER VALUE VALUES(1/0) RETURNING id",
        )
        .unwrap();
    session
        .execute("INSERT INTO identity_prepare DEFAULT VALUES")
        .unwrap();
    let mut rows = session
        .execute("SELECT id FROM identity_prepare")
        .unwrap()
        .remove(0);
    assert_eq!(rows.next_row().unwrap().unwrap(), vec!["10"]);
    session.execute("DROP TABLE identity_prepare").unwrap();
}

#[test]
fn namespace_rename_preserves_table_identity_and_reads_do_not_allocate() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    pg.pg_schema_command(&PgSchemaCommand::Create("ns_identity".into()))
        .unwrap();
    let first = pg
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find(|f| f.name == "ns_identity")
        .unwrap();
    let physical = pg.pg_bind_relation(Some("ns_identity"), "t", true).unwrap();
    pg.execute(&format!("CREATE TABLE \"{physical}\" (id integer)"))
        .unwrap();
    pg.pg_schema_command(&PgSchemaCommand::Rename {
        schema: "ns_identity".into(),
        name: "ns_after".into(),
    })
    .unwrap();
    let read_id = || {
        domain.storage_handle().with_storage(|store| {
            let ctx = kv::Context::default();
            let mut value = Vec::new();
            kv::RunInNewTxn(&ctx, store, false, |_, txn| {
                value = txn
                    .Get(
                        &ctx,
                        astersql_meta::transaction_meta_string_key(b"NextGlobalID"),
                        &[],
                    )?
                    .Value;
                Ok(())
            })
            .unwrap();
            value
        })
    };
    let before = read_id();
    let other = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    for _ in 0..3 {
        let facts = other.pg_namespace_facts().unwrap();
        let renamed = facts.iter().find(|f| f.name == "ns_after").unwrap();
        assert_eq!(renamed.id, first.id);
        assert_eq!(renamed.owner, first.owner);
        assert!(!facts.iter().any(|f| f.name == "ns_identity"));
        assert_eq!(
            other
                .pg_bind_relation(Some("ns_after"), "t", false)
                .unwrap(),
            physical
        );
    }
    assert_eq!(before, read_id());
    other
        .execute(&format!("DROP TABLE \"{physical}\""))
        .unwrap();
    other
        .pg_schema_command(&PgSchemaCommand::Drop("ns_after".into()))
        .unwrap();
}

#[test]
fn namespace_owner_revocation_preserves_identity_and_removes_effective_privileges() {
    let (domain, native) = CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE USER 'ns_acl_owner'@'%' IDENTIFIED WITH mysql_native_password BY ''")
        .unwrap();
    let admin = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    admin
        .pg_schema_command(&PgSchemaCommand::CreateOwned {
            schema: "owner_acl".into(),
            owner: "ns_acl_owner".into(),
        })
        .unwrap();
    let before = admin
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find(|f| f.name == "owner_acl")
        .unwrap();
    assert!(before.owner.is_some() && before.acl_available && before.acl.is_none());
    admin
        .pg_schema_command(&PgSchemaCommand::RevokeAll {
            schema: "owner_acl".into(),
            grantee: "ns_acl_owner".into(),
        })
        .unwrap();
    let after = admin
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find(|f| f.name == "owner_acl")
        .unwrap();
    assert_eq!((before.id, before.owner), (after.id, after.owner));
    assert_eq!(after.acl.as_deref(), Some("{}"));
    let mut owner = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    owner.login_user = Some("ns_acl_owner".into());
    let owned = owner
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find(|f| f.name == "owner_acl")
        .unwrap();
    assert!(!owned.usage && !owned.can_create);
    admin
        .pg_schema_command(&PgSchemaCommand::Usage {
            create: false,
            schema: "owner_acl".into(),
            grantee: "ns_acl_owner".into(),
            grant: true,
        })
        .unwrap();
    let owned = owner
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find(|f| f.name == "owner_acl")
        .unwrap();
    assert!(owned.usage && !owned.can_create);
    admin
        .pg_schema_command(&PgSchemaCommand::Drop("owner_acl".into()))
        .unwrap();
    native.execute("DROP USER 'ns_acl_owner'@'%'").unwrap();
}

#[test]
fn pg13_030_standard_schema_is_durable_and_reads_never_allocate() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    pg.pg_initialize_standard_schema().unwrap();
    let read = || {
        domain.storage_handle().with_storage(|store| {
            let ctx = kv::Context::default();
            let mut values = Vec::new();
            kv::RunInNewTxn(&ctx, store, false, |_, txn| {
                values.clear();
                for key in [b"NextGlobalID".as_slice(), b"PgStandardSchema".as_slice()] {
                    values.push(
                        txn.Get(&ctx, astersql_meta::transaction_meta_string_key(key), &[])?
                            .Value,
                    );
                }
                Ok(())
            })
            .unwrap();
            values
        })
    };
    let before = read();
    let first = pg
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find_map(|f| f.standard)
        .unwrap();
    let ids = [
        first.namespace,
        first.sql_identifier,
        first.character_data,
        first.schemata,
        first.tables,
        first.yes_or_no,
        first.views,
    ];
    assert_eq!(
        ids.into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        7
    );
    let other = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    other.pg_initialize_standard_schema().unwrap();
    for _ in 0..3 {
        let facts = other.pg_namespace_facts().unwrap();
        let standard = facts
            .iter()
            .find(|f| f.name == "information_schema")
            .unwrap();
        assert_eq!(standard.id, first.namespace);
        assert_eq!(
            standard.standard.as_ref().unwrap().sql_identifier,
            first.sql_identifier
        );
        assert!(standard.acl_available);
    }
    assert_eq!(read(), before);
    let mut corrupt = first.clone();
    corrupt.character_data = corrupt.sql_identifier;
    domain.storage_handle().with_storage(|store| {
        let ctx = kv::Context::default();
        kv::RunInNewTxn(&ctx, store, true, |_, txn| {
            txn.Set(
                astersql_meta::transaction_meta_string_key(b"PgStandardSchema"),
                serde_json::to_vec(&corrupt).unwrap(),
            )
        })
        .unwrap();
    });
    assert_eq!(
        other
            .pg_namespace_facts()
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "0A000"
    );
    println!(
        "pg13_030: durable_identity=7 second_session=1 no_allocation=1 corrupt_identity_rejected=1"
    );
}

#[test]
fn pg13_031_standard_schema_upgrade_is_explicit() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    pg.pg_initialize_standard_schema().unwrap();
    let original = pg
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find_map(|f| f.standard)
        .unwrap();
    domain.storage_handle().with_storage(|store| {
        let ctx = kv::Context::default();
        kv::RunInNewTxn(&ctx, store, true, |_, txn| {
            let mut legacy = serde_json::to_value(&original).unwrap();
            legacy.as_object_mut().unwrap().remove("tables");
            legacy.as_object_mut().unwrap().remove("yes_or_no");
            txn.Set(
                astersql_meta::transaction_meta_string_key(b"PgStandardSchema"),
                serde_json::to_vec(&legacy).unwrap(),
            )
        })
        .unwrap();
    });
    assert_eq!(
        pg.pg_namespace_facts()
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "0A000"
    );
    pg.pg_initialize_standard_schema().unwrap();
    let upgraded = pg
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find_map(|f| f.standard)
        .unwrap();
    assert_eq!(original.namespace, upgraded.namespace);
    assert_eq!(original.schemata, upgraded.schemata);
    assert_eq!(original.sql_identifier, upgraded.sql_identifier);
    assert!(upgraded.tables > original.tables && upgraded.yes_or_no > original.yes_or_no);
    println!("pg13_031 upgrade: legacy_read_refused=1 explicit_upgrade=1 old_ids_preserved=3");
}

#[test]
fn pg13_031_temporary_catalog_identities_are_private() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let first = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    let second = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    for session in [&first, &second] {
        session.pg_initialize_standard_schema().unwrap();
        session
            .execute("CREATE TEMPORARY TABLE pg13_031_local(id integer)")
            .unwrap();
    }
    let a = first.pg_local_temporary_models();
    let b = second.pg_local_temporary_models();
    assert_eq!(a.len(), 1);
    assert_eq!(b.len(), 1);
    assert_ne!(a[0].ID, b[0].ID);
    assert_ne!(a[0].PostgreSQLNamespace, b[0].PostgreSQLNamespace);
    assert!(a[0].ID > 0 && a[0].ID < (0x4000_0000 - 16384 - 1) / 2);
    let namespace = a[0].PostgreSQLNamespace.as_ref().unwrap().0;
    assert!(
        first
            .pg_namespace_facts()
            .unwrap()
            .iter()
            .any(|f| f.id == namespace && f.name.starts_with("pg_temp_"))
    );
    assert!(
        !second
            .pg_namespace_facts()
            .unwrap()
            .iter()
            .any(|f| f.id == namespace)
    );
    assert!(
        domain
            .info_schema()
            .TableByName(
                &astersql_infoschema::CiString::new("test"),
                &astersql_infoschema::CiString::new("pg13_031_local")
            )
            .is_err()
    );
    first.execute("DROP TABLE pg13_031_local").unwrap();
    assert!(first.pg_local_temporary_models().is_empty());
    assert_eq!(second.pg_local_temporary_models()[0].ID, b[0].ID);
    second.execute("DROP TABLE pg13_031_local").unwrap();
    println!(
        "pg13_031 temporary: private_relations=2 private_namespaces=2 bounded_ids=1 shared_domain_unchanged=1 cleanup=2"
    );
}

#[test]
fn pg13_032_standard_view_upgrade_is_explicit() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    pg.pg_initialize_standard_schema().unwrap();
    let original = pg
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find_map(|f| f.standard)
        .unwrap();
    domain.storage_handle().with_storage(|store| {
        let ctx = kv::Context::default();
        kv::RunInNewTxn(&ctx, store, true, |_, txn| {
            let mut legacy = serde_json::to_value(&original).unwrap();
            legacy.as_object_mut().unwrap().remove("views");
            txn.Set(
                astersql_meta::transaction_meta_string_key(b"PgStandardSchema"),
                serde_json::to_vec(&legacy).unwrap(),
            )
        })
        .unwrap();
    });
    assert_eq!(
        pg.pg_namespace_facts()
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "0A000"
    );
    pg.pg_initialize_standard_schema().unwrap();
    let upgraded = pg
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find_map(|f| f.standard)
        .unwrap();
    assert_eq!(original.namespace, upgraded.namespace);
    assert_eq!(original.schemata, upgraded.schemata);
    assert_eq!(original.sql_identifier, upgraded.sql_identifier);
    assert_eq!(upgraded.tables, original.tables);
    assert_eq!(upgraded.yes_or_no, original.yes_or_no);
    assert!(upgraded.views > original.views);
    println!("pg13_032 upgrade: legacy_read_refused=1 explicit_upgrade=1 old_ids_preserved=3");
}

#[test]
fn pg13_033_standard_columns_upgrade_is_explicit() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    pg.pg_initialize_standard_schema().unwrap();
    let original = pg
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find_map(|f| f.standard)
        .unwrap();
    domain.storage_handle().with_storage(|store| {
        let ctx = kv::Context::default();
        kv::RunInNewTxn(&ctx, store, true, |_, txn| {
            let mut legacy = serde_json::to_value(&original).unwrap();
            legacy.as_object_mut().unwrap().remove("columns");
            legacy.as_object_mut().unwrap().remove("cardinal_number");
            txn.Set(
                astersql_meta::transaction_meta_string_key(b"PgStandardSchema"),
                serde_json::to_vec(&legacy).unwrap(),
            )
        })
        .unwrap();
    });
    assert_eq!(
        pg.pg_namespace_facts()
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "0A000"
    );
    pg.pg_initialize_standard_schema().unwrap();
    let upgraded = pg
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find_map(|f| f.standard)
        .unwrap();
    assert_eq!(original.namespace, upgraded.namespace);
    assert_eq!(original.schemata, upgraded.schemata);
    assert_eq!(original.sql_identifier, upgraded.sql_identifier);
    assert_eq!(upgraded.tables, original.tables);
    assert_eq!(upgraded.yes_or_no, original.yes_or_no);
    assert_eq!(upgraded.views, original.views);
    assert!(
        upgraded.columns > original.columns && upgraded.cardinal_number > original.cardinal_number
    );
    let read = || {
        domain.storage_handle().with_storage(|store| {
            let ctx = kv::Context::default();
            let mut value = Vec::new();
            kv::RunInNewTxn(&ctx, store, false, |_, txn| {
                value = txn
                    .Get(
                        &ctx,
                        astersql_meta::transaction_meta_string_key(b"NextGlobalID"),
                        &[],
                    )?
                    .Value;
                Ok(())
            })
            .unwrap();
            value
        })
    };
    let before = read();
    for _ in 0..3 {
        let facts = pg.pg_namespace_facts().unwrap();
        assert_eq!(
            facts
                .iter()
                .find_map(|f| f.standard.as_ref())
                .unwrap()
                .columns,
            upgraded.columns
        );
    }
    assert_eq!(before, read());
    println!(
        "pg13_033 upgrade: legacy_read_refused=1 explicit_upgrade=1 old_ids_preserved=6 catalog_reads_never_allocate=3"
    );
}

#[test]
fn pg13_036_identity_alteration_runtime() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    pg.execute("CREATE TABLE identity_alter(id integer GENERATED BY DEFAULT AS IDENTITY (START 7 INCREMENT 2 MAXVALUE 99), plain integer)").unwrap();
    let alter = |column: &str, option: &str, value| PgSequenceCommand::AlterIdentity {
        schema: Some("public".into()),
        name: "identity_alter".into(),
        column: column.into(),
        option: option.into(),
        value,
    };
    pg.pg_sequence(&alter("id", "generated", 1)).unwrap();
    let table = domain
        .info_schema()
        .ModelTableInfoByExactName(
            &astersql_infoschema::CiString::new("test"),
            "identity_alter",
        )
        .unwrap();
    assert!(table.Columns[0].PostgreSQLIdentity.as_ref().unwrap().Always);
    for (column, option, value, state) in [
        ("plain", "generated", 1, "55000"),
        ("absent", "generated", 1, "42703"),
        ("id", "increment", 0, "22023"),
        ("id", "increment", -1, "0A000"),
    ] {
        assert_eq!(
            pg.pg_sequence(&alter(column, option, value))
                .unwrap_err()
                .pg_function_error()
                .unwrap()
                .state,
            state
        );
    }
    pg.execute("DROP TABLE identity_alter").unwrap();
    println!("pg13_036_identity_alteration_runtime: generation=1 errors=4 cleanup=1");
}

#[test]
fn pg13_037_table_constraints_upgrade_is_explicit() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    pg.pg_initialize_standard_schema().unwrap();
    let original = pg
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find_map(|f| f.standard)
        .unwrap();
    domain.storage_handle().with_storage(|store| {
        let ctx = kv::Context::default();
        kv::RunInNewTxn(&ctx, store, true, |_, txn| {
            let mut legacy = serde_json::to_value(&original).unwrap();
            legacy.as_object_mut().unwrap().remove("table_constraints");
            txn.Set(
                astersql_meta::transaction_meta_string_key(b"PgStandardSchema"),
                serde_json::to_vec(&legacy).unwrap(),
            )
        })
        .unwrap();
    });
    assert_eq!(
        pg.pg_namespace_facts()
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "0A000"
    );
    pg.pg_initialize_standard_schema().unwrap();
    let upgraded = pg
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find_map(|f| f.standard)
        .unwrap();
    assert_eq!(original.namespace, upgraded.namespace);
    assert_eq!(original.schemata, upgraded.schemata);
    assert_eq!(original.sql_identifier, upgraded.sql_identifier);
    assert_eq!(upgraded.tables, original.tables);
    assert_eq!(upgraded.yes_or_no, original.yes_or_no);
    assert_eq!(upgraded.views, original.views);
    assert_eq!(upgraded.columns, original.columns);
    assert_eq!(upgraded.cardinal_number, original.cardinal_number);
    assert!(upgraded.table_constraints > original.table_constraints);
    let read = || {
        domain.storage_handle().with_storage(|store| {
            let ctx = kv::Context::default();
            let mut value = Vec::new();
            kv::RunInNewTxn(&ctx, store, false, |_, txn| {
                value = txn
                    .Get(
                        &ctx,
                        astersql_meta::transaction_meta_string_key(b"NextGlobalID"),
                        &[],
                    )?
                    .Value;
                Ok(())
            })
            .unwrap();
            value
        })
    };
    let before = read();
    for _ in 0..3 {
        let facts = pg.pg_namespace_facts().unwrap();
        assert_eq!(
            facts
                .iter()
                .find_map(|f| f.standard.as_ref())
                .unwrap()
                .table_constraints,
            upgraded.table_constraints
        );
    }
    assert_eq!(before, read());
    println!(
        "pg13_037 upgrade: legacy_read_refused=1 explicit_upgrade=1 old_ids_preserved=8 catalog_reads_never_allocate=3"
    );
}

#[test]
fn pg13_038_key_column_usage_upgrade_is_explicit() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    pg.pg_initialize_standard_schema().unwrap();
    let original = pg
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find_map(|f| f.standard)
        .unwrap();
    domain.storage_handle().with_storage(|store| {
        let ctx = kv::Context::default();
        kv::RunInNewTxn(&ctx, store, true, |_, txn| {
            let mut legacy = serde_json::to_value(&original).unwrap();
            legacy.as_object_mut().unwrap().remove("key_column_usage");
            txn.Set(
                astersql_meta::transaction_meta_string_key(b"PgStandardSchema"),
                serde_json::to_vec(&legacy).unwrap(),
            )
        })
        .unwrap();
    });
    assert_eq!(
        pg.pg_namespace_facts()
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "0A000"
    );
    pg.pg_initialize_standard_schema().unwrap();
    let upgraded = pg
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find_map(|f| f.standard)
        .unwrap();
    assert_eq!(original.namespace, upgraded.namespace);
    assert_eq!(original.owner, upgraded.owner);
    assert_eq!(original.character_data, upgraded.character_data);
    assert_eq!(original.table_constraints, upgraded.table_constraints);
    assert_eq!(original.schemata, upgraded.schemata);
    assert_eq!(original.sql_identifier, upgraded.sql_identifier);
    assert_eq!(upgraded.tables, original.tables);
    assert_eq!(upgraded.yes_or_no, original.yes_or_no);
    assert_eq!(upgraded.views, original.views);
    assert_eq!(upgraded.columns, original.columns);
    assert_eq!(upgraded.cardinal_number, original.cardinal_number);
    assert!(upgraded.key_column_usage > original.key_column_usage);
    let read = || {
        domain.storage_handle().with_storage(|store| {
            let ctx = kv::Context::default();
            let mut value = Vec::new();
            kv::RunInNewTxn(&ctx, store, false, |_, txn| {
                value = txn
                    .Get(
                        &ctx,
                        astersql_meta::transaction_meta_string_key(b"NextGlobalID"),
                        &[],
                    )?
                    .Value;
                Ok(())
            })
            .unwrap();
            value
        })
    };
    let before = read();
    for _ in 0..3 {
        let facts = pg.pg_namespace_facts().unwrap();
        assert_eq!(
            facts
                .iter()
                .find_map(|f| f.standard.as_ref())
                .unwrap()
                .key_column_usage,
            upgraded.key_column_usage
        );
    }
    assert_eq!(before, read());
    println!(
        "pg13_038 upgrade: legacy_read_refused=1 explicit_upgrade=1 old_ids_preserved=10 catalog_reads_never_allocate=3"
    );
}

#[test]
fn pg13_039_referential_constraints_upgrade_is_explicit() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    pg.pg_initialize_standard_schema().unwrap();
    let original = pg
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find_map(|f| f.standard)
        .unwrap();
    domain.storage_handle().with_storage(|store| {
        let ctx = kv::Context::default();
        kv::RunInNewTxn(&ctx, store, true, |_, txn| {
            let mut legacy = serde_json::to_value(&original).unwrap();
            legacy
                .as_object_mut()
                .unwrap()
                .remove("referential_constraints");
            txn.Set(
                astersql_meta::transaction_meta_string_key(b"PgStandardSchema"),
                serde_json::to_vec(&legacy).unwrap(),
            )
        })
        .unwrap();
    });
    assert_eq!(
        pg.pg_namespace_facts()
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "0A000"
    );
    pg.pg_initialize_standard_schema().unwrap();
    let upgraded = pg
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find_map(|f| f.standard)
        .unwrap();
    assert_eq!(original.namespace, upgraded.namespace);
    assert_eq!(original.owner, upgraded.owner);
    assert_eq!(original.character_data, upgraded.character_data);
    assert_eq!(original.table_constraints, upgraded.table_constraints);
    assert_eq!(original.schemata, upgraded.schemata);
    assert_eq!(original.sql_identifier, upgraded.sql_identifier);
    assert_eq!(upgraded.tables, original.tables);
    assert_eq!(upgraded.yes_or_no, original.yes_or_no);
    assert_eq!(upgraded.views, original.views);
    assert_eq!(upgraded.columns, original.columns);
    assert_eq!(upgraded.cardinal_number, original.cardinal_number);
    assert_eq!(upgraded.key_column_usage, original.key_column_usage);
    assert!(upgraded.referential_constraints > original.referential_constraints);
    let read = || {
        domain.storage_handle().with_storage(|store| {
            let ctx = kv::Context::default();
            let mut value = Vec::new();
            kv::RunInNewTxn(&ctx, store, false, |_, txn| {
                value = txn
                    .Get(
                        &ctx,
                        astersql_meta::transaction_meta_string_key(b"NextGlobalID"),
                        &[],
                    )?
                    .Value;
                Ok(())
            })
            .unwrap();
            value
        })
    };
    let before = read();
    for _ in 0..3 {
        let facts = pg.pg_namespace_facts().unwrap();
        assert_eq!(
            facts
                .iter()
                .find_map(|f| f.standard.as_ref())
                .unwrap()
                .referential_constraints,
            upgraded.referential_constraints
        );
    }
    assert_eq!(before, read());
    println!(
        "pg13_039 upgrade: legacy_read_refused=1 explicit_upgrade=1 old_ids_preserved=11 catalog_reads_never_allocate=3"
    );
}

#[test]
fn pg13_040_constraint_column_usage_upgrade_is_explicit() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    pg.pg_initialize_standard_schema().unwrap();
    let original = pg
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find_map(|f| f.standard)
        .unwrap();
    domain.storage_handle().with_storage(|store| {
        let ctx = kv::Context::default();
        kv::RunInNewTxn(&ctx, store, true, |_, txn| {
            let mut legacy = serde_json::to_value(&original).unwrap();
            legacy
                .as_object_mut()
                .unwrap()
                .remove("constraint_column_usage");
            txn.Set(
                astersql_meta::transaction_meta_string_key(b"PgStandardSchema"),
                serde_json::to_vec(&legacy).unwrap(),
            )
        })
        .unwrap();
    });
    assert_eq!(
        pg.pg_namespace_facts()
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "0A000"
    );
    pg.pg_initialize_standard_schema().unwrap();
    let upgraded = pg
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find_map(|f| f.standard)
        .unwrap();
    assert_eq!(original.namespace, upgraded.namespace);
    assert_eq!(original.owner, upgraded.owner);
    assert_eq!(original.character_data, upgraded.character_data);
    assert_eq!(original.table_constraints, upgraded.table_constraints);
    assert_eq!(original.schemata, upgraded.schemata);
    assert_eq!(original.sql_identifier, upgraded.sql_identifier);
    assert_eq!(upgraded.tables, original.tables);
    assert_eq!(upgraded.yes_or_no, original.yes_or_no);
    assert_eq!(upgraded.views, original.views);
    assert_eq!(upgraded.columns, original.columns);
    assert_eq!(upgraded.cardinal_number, original.cardinal_number);
    assert_eq!(upgraded.key_column_usage, original.key_column_usage);
    assert_eq!(
        upgraded.referential_constraints,
        original.referential_constraints
    );
    assert!(upgraded.constraint_column_usage > original.constraint_column_usage);
    let read = || {
        domain.storage_handle().with_storage(|store| {
            let ctx = kv::Context::default();
            let mut value = Vec::new();
            kv::RunInNewTxn(&ctx, store, false, |_, txn| {
                value = txn
                    .Get(
                        &ctx,
                        astersql_meta::transaction_meta_string_key(b"NextGlobalID"),
                        &[],
                    )?
                    .Value;
                Ok(())
            })
            .unwrap();
            value
        })
    };
    let before = read();
    for _ in 0..3 {
        let facts = pg.pg_namespace_facts().unwrap();
        assert_eq!(
            facts
                .iter()
                .find_map(|f| f.standard.as_ref())
                .unwrap()
                .constraint_column_usage,
            upgraded.constraint_column_usage
        );
    }
    assert_eq!(before, read());
    println!(
        "pg13_040 upgrade: legacy_read_refused=1 explicit_upgrade=1 old_ids_preserved=12 catalog_reads_never_allocate=3"
    );
}

#[test]
fn cat14_13_builtin_owner_requires_persistence_and_survives_cleanup() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    let fact = || {
        pg.pg_namespace_facts()
            .unwrap()
            .into_iter()
            .find(|f| f.name == "public")
            .unwrap()
    };
    let before = fact();
    assert_eq!(
        before.owner, None,
        "an implicit root label is not a durable owner"
    );
    pg.pg_schema_command(&PgSchemaCommand::Owner {
        schema: "public".into(),
        owner: "root".into(),
    })
    .unwrap();
    let migrated = fact();
    assert_eq!(migrated.id, before.id);
    assert_eq!(
        migrated.owner,
        Some(pg.pg_database_role_id("root").unwrap())
    );
    pg.pg_schema_command(&PgSchemaCommand::Create("cat14_13_cleanup".into()))
        .unwrap();
    pg.pg_schema_command(&PgSchemaCommand::Drop("cat14_13_cleanup".into()))
        .unwrap();
    domain.force_full_reload_for_test().unwrap();
    let other = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    let reloaded = other
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find(|f| f.name == "public")
        .unwrap();
    assert_eq!((reloaded.id, reloaded.owner), (before.id, migrated.owner));
    domain.close();
}

#[test]
fn cat14_15_historical_type_owner_is_not_backfilled() {
    let (domain, pg) = CreateAnalyzeSession().unwrap();
    let types = [(23, "int4", 1007, false), (1007, "_int4", 0, true)];
    pg.pg_initialize_standard_schema_with_types(&types).unwrap();
    let standard = || {
        pg.pg_namespace_facts()
            .unwrap()
            .into_iter()
            .find_map(|f| f.standard)
            .unwrap()
    };
    let before = standard();
    assert_eq!(before.type_owners.len(), 4);
    assert_eq!(before.builtin_type_owners.len(), 2);
    let context = kv::Context::default();
    domain
        .storage_handle()
        .with_storage(|store| {
            kv::RunInNewTxn(&context, store, true, |_, tx| {
                let key = astersql_meta::transaction_meta_string_key(b"PgStandardSchema");
                let bytes = tx.Get(&context, key.clone(), &[])?.Value;
                let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                value.as_object_mut().unwrap().remove("type_owners");
                value.as_object_mut().unwrap().remove("builtin_type_owners");
                value.as_object_mut().unwrap().remove("builtin_types");
                tx.Set(key, serde_json::to_vec(&value).unwrap())
            })
        })
        .unwrap();
    pg.pg_initialize_standard_schema_with_types(&types).unwrap();
    let old = standard();
    assert_eq!(old.sql_identifier, before.sql_identifier);
    assert!(old.type_owners.is_empty());
    assert!(old.builtin_type_owners.is_empty());
    pg.pg_schema_command(&PgSchemaCommand::TypeOwner {
        domain: false,
        schema: Some("pg_catalog".into()),
        name: "int4".into(),
        owner: "root".into(),
    })
    .unwrap();
    let migrated = standard();
    assert_eq!(migrated.builtin_type_owners.len(), 2);
    assert_eq!(
        migrated.builtin_type_owners[&23],
        pg.pg_database_role_id("root").unwrap()
    );
    assert_eq!(
        migrated.builtin_type_owners[&23],
        migrated.builtin_type_owners[&1007]
    );
    pg.pg_schema_command(&PgSchemaCommand::TypeOwner {
        domain: false,
        schema: Some("information_schema".into()),
        name: "sql_identifier".into(),
        owner: "root".into(),
    })
    .unwrap();
    let after = standard();
    assert_eq!(after.type_owners.len(), 1);
    assert_eq!(
        after.type_owners[&before.sql_identifier],
        pg.pg_database_role_id("root").unwrap()
    );
    assert_eq!(
        (after.sql_identifier, after.owner),
        (before.sql_identifier, before.owner)
    );
    domain.close();
}

#[test]
fn cat14_16_historical_function_owner_is_not_backfilled() {
    let (domain, pg) = CreateAnalyzeSession().unwrap();
    pg.pg_initialize_standard_schema().unwrap();
    let standard = || {
        pg.pg_namespace_facts()
            .unwrap()
            .into_iter()
            .find_map(|fact| fact.standard)
            .unwrap()
    };
    let before = standard();
    let root = pg.pg_database_role_id("root").unwrap();
    assert_eq!(before.function_owners.len(), 3);
    for (oid, _) in PG_INTROSPECTION_FUNCTION_IDENTITIES {
        assert_eq!(before.function_owners[oid], root);
    }
    let context = kv::Context::default();
    domain
        .storage_handle()
        .with_storage(|store| {
            kv::RunInNewTxn(&context, store, true, |_, tx| {
                let key = astersql_meta::transaction_meta_string_key(b"PgStandardSchema");
                let bytes = tx.Get(&context, key.clone(), &[])?.Value;
                let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                // Verify that initial ownership was actually persisted, not derived
                // from schema/view ownership when the reader asked for it.
                assert_eq!(value["function_owners"]["16000"].as_i64(), Some(root));
                value.as_object_mut().unwrap().remove("function_owners");
                value
                    .as_object_mut()
                    .unwrap()
                    .remove("function_owner_overrides");
                tx.Set(key, serde_json::to_vec(&value).unwrap())
            })
        })
        .unwrap();
    pg.pg_initialize_standard_schema().unwrap();
    assert!(standard().function_owners.is_empty());
    assert!(standard().function_owner_overrides.is_empty());
    pg.pg_schema_command(&PgSchemaCommand::FunctionOwner {
        schema: Some("pg_catalog".into()),
        name: "pg_get_function_arguments".into(),
        owner: "root".into(),
    })
    .unwrap();
    pg.pg_initialize_standard_schema().unwrap();
    let after = standard();
    assert!(after.function_owners.is_empty());
    let overrides = after.function_owner_overrides.values().next().unwrap();
    assert_eq!(overrides.len(), 1);
    assert_eq!(overrides[&16000], root);
    assert_eq!(
        (after.sql_identifier, after.owner),
        (before.sql_identifier, before.owner)
    );
    domain.close();
}
