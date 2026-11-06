// Copyright 2026 AsterSQL.
use super::*;

#[test]
fn database_reads_do_not_allocate_and_corrupt_owner_is_rejected() {
    let (domain, native) = CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_legacy_read").unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    pg.execute("CREATE DATABASE pg13_owned_read").unwrap();
    let next_id = || {
        domain
            .storage_handle()
            .with_storage(|store| {
                let mut txn = store.Begin(&[])?;
                let value = txn.Get(
                    &kv::Context::default(),
                    astersql_meta::transaction_meta_string_key(b"NextGlobalID"),
                    &[],
                )?;
                txn.Rollback()?;
                Ok::<_, kv::errors::SharedError>(value.Value)
            })
            .unwrap()
    };
    let before = next_id();
    let legacy = domain
        .info_schema()
        .SchemaByName(&astersql_infoschema::CiString::new("pg13_legacy_read"))
        .unwrap();
    for _ in 0..3 {
        let facts = pg.pg_database_facts(None).unwrap();
        assert!(
            facts
                .iter()
                .find(|f| f.id == legacy.id)
                .unwrap()
                .owner
                .is_none()
        );
    }
    assert_eq!(next_id(), before, "directory reads allocated identity");
    let owned = domain
        .info_schema()
        .SchemaByName(&astersql_infoschema::CiString::new("pg13_owned_read"))
        .unwrap();
    // Corrupt only the persisted model boundary, never provider output.
    domain
        .storage_handle()
        .with_storage(|store| {
            kv::RunInNewTxn(&kv::Context::default(), store, true, |_, txn| {
                txn.Set(
                    astersql_meta::transaction_meta_hash_key(
                        b"PgDatabaseOwners",
                        owned.id.to_string().as_bytes(),
                    ),
                    b"4294967296".to_vec(),
                )
            })
        })
        .unwrap();
    assert_eq!(
        pg.pg_database_facts(None)
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "0A000"
    );
    native.execute("DROP DATABASE pg13_owned_read").unwrap();
    native.execute("DROP DATABASE pg13_legacy_read").unwrap();
}

#[test]
fn database_policy_persists_without_changing_native_use_or_other_databases() {
    use super::pg_database::PgDatabaseCommand;
    let (domain, native) = CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_policy_a").unwrap();
    native.execute("CREATE DATABASE pg13_policy_b").unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    let id = domain
        .info_schema()
        .SchemaByName(&astersql_infoschema::CiString::new("pg13_policy_b"))
        .unwrap()
        .id;
    let before = pg.pg_database_facts(None).unwrap();
    let fact = before.iter().find(|f| f.id == id).unwrap();
    assert_eq!(
        (fact.is_template, fact.allow_connections),
        (Some(false), Some(true))
    );
    pg.pg_database_facts(Some(&PgDatabaseCommand::Policy {
        database: "pg13_policy_b".into(),
        is_template: Some(true),
        allow_connections: Some(false),
    }))
    .unwrap();
    domain.reload().unwrap();
    let observer = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    for _ in 0..3 {
        let facts = observer.pg_database_facts(None).unwrap();
        for current in &facts {
            if current.id == id {
                assert_eq!(
                    (current.is_template, current.allow_connections),
                    (Some(true), Some(false))
                );
            } else {
                let previous = before.iter().find(|f| f.id == current.id).unwrap();
                assert_eq!(
                    (current.is_template, current.allow_connections),
                    (previous.is_template, previous.allow_connections)
                );
            }
        }
    }
    assert_eq!(
        observer
            .execute("USE pg13_policy_b")
            .err()
            .expect("disabled database must reject PostgreSQL USE")
            .pg_function_error()
            .unwrap()
            .state,
        "55000"
    );
    native.execute("USE pg13_policy_b").unwrap();
    native.execute("CREATE TABLE health (id INT)").unwrap();
    native.execute("INSERT INTO health VALUES (2)").unwrap();
    assert!(native.execute("SELECT id FROM health").is_ok());
    native.execute("USE test").unwrap();
    pg.pg_database_facts(Some(&PgDatabaseCommand::Policy {
        database: "pg13_policy_b".into(),
        is_template: Some(false),
        allow_connections: Some(true),
    }))
    .unwrap();
    observer.execute("USE pg13_policy_b").unwrap();
    assert_eq!(
        observer
            .pg_database_facts(Some(&PgDatabaseCommand::Policy {
                database: "pg13_policy_b".into(),
                is_template: None,
                allow_connections: Some(false),
            }))
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "22023"
    );
    native.execute("DROP DATABASE pg13_policy_b").unwrap();
    native.execute("CREATE DATABASE pg13_policy_b").unwrap();
    let new_id = domain
        .info_schema()
        .SchemaByName(&astersql_infoschema::CiString::new("pg13_policy_b"))
        .unwrap()
        .id;
    assert_ne!(new_id, id);
    let facts = pg.pg_database_facts(None).unwrap();
    let recreated = facts.iter().find(|f| f.id == new_id).unwrap();
    assert_eq!(
        (recreated.is_template, recreated.allow_connections),
        (Some(false), Some(true))
    );
    native.execute("DROP DATABASE pg13_policy_b").unwrap();
    native.execute("DROP DATABASE pg13_policy_a").unwrap();
    println!(
        "pg13_002 model: persistence=3 isolation=1 native_health=1 current_db_reject=1 recreate=1 cleanup=2"
    );
}

#[test]
fn cat14_07_role_reads_do_not_allocate_or_infer_login_from_locks() {
    let (domain, native) = CreateAnalyzeSession().unwrap();
    native.pg_register_bootstrap_role().unwrap();
    native.execute("CREATE USER 'cat14_07_native_locked'@'%' IDENTIFIED WITH mysql_native_password BY '' ACCOUNT LOCK").unwrap();
    native
        .execute("CREATE ROLE 'cat14_07_native_group'@'%'")
        .unwrap();
    let next = || {
        domain
            .storage_handle()
            .with_storage(|store| {
                let mut txn = store.Begin(&[])?;
                let value = txn.Get(
                    &kv::Context::default(),
                    astersql_meta::transaction_meta_string_key(b"NextGlobalID"),
                    &[],
                )?;
                txn.Rollback()?;
                Ok::<_, kv::errors::SharedError>(value.Value)
            })
            .unwrap()
    };
    let before = next();
    let facts = native.pg_role_facts().unwrap();
    let locked = facts
        .iter()
        .find(|f| f.name == "cat14_07_native_locked")
        .unwrap();
    let group = facts
        .iter()
        .find(|f| f.name == "cat14_07_native_group")
        .unwrap();
    assert!(
        locked.can_login == Some(true),
        "a locked LOGIN account remains a LOGIN role"
    );
    assert!(
        group.can_login == Some(false),
        "CREATE ROLE carries a separate NOLOGIN declaration"
    );
    assert!(
        locked.attributes.is_none(),
        "native privileges do not invent PG policies"
    );
    let stable = locked.oid;
    for _ in 0..3 {
        domain.force_full_reload_for_test().unwrap();
        assert_eq!(
            native
                .pg_role_facts()
                .unwrap()
                .iter()
                .find(|f| f.name == locked.name)
                .unwrap()
                .oid,
            stable
        );
    }
    assert_eq!(next(), before);
    // Preserve a known identity while leaving a missing declaration unknown:
    // no read-time OID allocation or guessing LOGIN from the account lock.
    domain
        .storage_handle()
        .with_storage(|store| {
            kv::RunInNewTxn(&kv::Context::default(), store, true, |_, txn| {
                txn.Delete(astersql_meta::transaction_meta_hash_key(
                    b"PgDatabaseRoleProperties",
                    stable.to_string().as_bytes(),
                ))
            })
        })
        .unwrap();
    let legacy = native
        .pg_role_facts()
        .unwrap()
        .into_iter()
        .find(|f| f.oid == stable)
        .unwrap();
    assert_eq!(legacy.name, "cat14_07_native_locked");
    assert_eq!(
        legacy.can_login, None,
        "unknown declaration must not be inferred from locking"
    );
    assert!(legacy.attributes.is_none());
    assert_eq!(next(), before);
    native
        .execute("DROP USER 'cat14_07_native_locked'@'%'")
        .unwrap();
    native
        .execute("DROP ROLE 'cat14_07_native_group'@'%'")
        .unwrap();
}
