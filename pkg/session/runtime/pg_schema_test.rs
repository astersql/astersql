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
        a.execute("ALTER TABLE identity_local22 DROP COLUMN id")
            .is_err()
    );
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
