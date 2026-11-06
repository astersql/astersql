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

#[test]
fn cat14_08_role_defaults_persist_and_old_fields_remain_unknown() {
    use super::pg_database::{PgRoleAttributes, PgRoleCommand};
    let (domain, native) = CreateAnalyzeSession().unwrap();
    native.pg_register_bootstrap_role().unwrap();
    native
        .pg_role_command(&PgRoleCommand::Create {
            name: "cat14_08_model".into(),
            can_login: true,
        })
        .unwrap();
    let oid = native.pg_database_role_id("cat14_08_model").unwrap();
    domain.force_full_reload_for_test().unwrap();
    let observer = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    let fact = observer
        .pg_role_facts()
        .unwrap()
        .into_iter()
        .find(|f| f.oid == oid)
        .unwrap();
    let attrs = fact.attributes.unwrap();
    assert_eq!(
        (attrs.inherit, attrs.create_role, attrs.connection_limit),
        (Some(true), Some(false), Some(-1))
    );
    let mut serialized = serde_json::to_value(&attrs).unwrap();
    for field in ["inherit", "create_role", "connection_limit"] {
        let saved = serialized.as_object_mut().unwrap().remove(field).unwrap();
        let old: PgRoleAttributes = serde_json::from_value(serialized.clone()).unwrap();
        assert_eq!(
            old.inherit,
            if field == "inherit" { None } else { Some(true) }
        );
        assert_eq!(
            old.create_role,
            if field == "create_role" {
                None
            } else {
                Some(false)
            }
        );
        assert_eq!(
            old.connection_limit,
            if field == "connection_limit" {
                None
            } else {
                Some(-1)
            }
        );
        serialized
            .as_object_mut()
            .unwrap()
            .insert(field.into(), saved);
    }
    native
        .pg_role_command(&PgRoleCommand::Drop {
            name: "cat14_08_model".into(),
        })
        .unwrap();
}

#[test]
fn cat14_09_shared_identity_rejects_ambiguous_native_roles() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let mut session =
        ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    assert_eq!(
        session
            .pg_session_identity()
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "28000"
    );
    session.SetAuthenticatedUser("reader@quoted".into(), false);
    assert_eq!(
        session.pg_session_identity().unwrap(),
        ("reader@quoted".into(), "reader@quoted".into())
    );
    *session.active_roles.borrow_mut() = vec![
        astersql_privilege_privileges::RoleIdentity::new("one", "%"),
        astersql_privilege_privileges::RoleIdentity::new("two", "%"),
    ];
    assert_eq!(
        session
            .pg_session_identity()
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "0A000"
    );
    // Never silently choose the first actor in a native multi-role context.
    assert!(session.pg_set_role(Some("one")).is_err());
    assert_eq!(session.active_roles.borrow().len(), 2);
    session.SetAuthenticatedIdentity(
        "reader@quoted".into(),
        "127.0.0.1".into(),
        "127.0.0.1".into(),
        false,
    );
    assert_eq!(
        session
            .pg_session_identity()
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "0A000"
    );
    domain.close();
}

#[test]
fn cat14_09_native_revoke_preserves_request_atomicity_and_defaults() {
    let (domain, native) = CreateAnalyzeSession().unwrap();
    native.execute("CREATE USER 'cat14_09_native'@'%'").unwrap();
    native.execute("CREATE ROLE 'cat14_09_role'@'%'").unwrap();
    native
        .execute("GRANT 'cat14_09_role'@'%' TO 'cat14_09_native'@'%'")
        .unwrap();
    native
        .execute("SET DEFAULT ROLE 'cat14_09_role'@'%' TO 'cat14_09_native'@'%'")
        .unwrap();
    let role = astersql_privilege_privileges::RoleIdentity::new("cat14_09_role", "%");
    let handle = runtime_privilege_handle(&domain);
    assert!(
        native
            .execute(
                "REVOKE 'cat14_09_role'@'%' FROM 'cat14_09_native'@'%', 'cat14_09_missing'@'%'"
            )
            .is_err()
    );
    assert!(handle.Get().FindRole("cat14_09_native", "%", &role));
    assert_eq!(
        handle.Get().getDefaultRoles("cat14_09_native", "%"),
        vec![role.clone()]
    );
    let mut reader = ConcreteSession::new(domain.clone());
    reader.SetAuthenticatedUser("cat14_09_native".into(), false);
    assert!(
        reader
            .execute("REVOKE 'cat14_09_role'@'%' FROM 'cat14_09_native'@'%'")
            .is_err()
    );
    assert!(handle.Get().FindRole("cat14_09_native", "%", &role));
    native
        .execute("REVOKE 'cat14_09_role'@'%' FROM 'cat14_09_native'@'%'")
        .unwrap();
    assert!(!handle.Get().FindRole("cat14_09_native", "%", &role));
    assert!(
        handle
            .Get()
            .getDefaultRoles("cat14_09_native", "%")
            .is_empty()
    );
    native.execute("DROP USER 'cat14_09_native'@'%'").unwrap();
    native.execute("DROP ROLE 'cat14_09_role'@'%'").unwrap();
    domain.close();
}

#[test]
fn cat14_17_database_acl_defaults_legacy_and_identity_lifecycle() {
    use super::pg_database::{PgDatabaseCommand, PgRoleCommand};
    let (domain, native) = CreateAnalyzeSession().unwrap();
    native.pg_register_bootstrap_role().unwrap();
    native
        .execute("CREATE DATABASE cat14_17_model_legacy")
        .unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    pg.execute("CREATE DATABASE cat14_17_model_pg").unwrap();
    pg.pg_role_command(&PgRoleCommand::Create {
        name: "cat14_17_model_role".into(),
        can_login: true,
    })
    .unwrap();
    let facts = pg.pg_database_facts(None).unwrap();
    let database_id = |name: &str| {
        domain
            .info_schema()
            .SchemaByName(&astersql_infoschema::CiString::new(name))
            .unwrap()
            .id
    };
    let owned = facts
        .iter()
        .find(|f| f.id == database_id("cat14_17_model_pg"))
        .unwrap();
    assert!(owned.acl_default);
    assert_eq!(owned.acl_text().unwrap(), None);
    let legacy = facts
        .iter()
        .find(|f| f.id == database_id("cat14_17_model_legacy"))
        .unwrap();
    assert!(legacy.acl.is_none());
    assert_eq!(
        legacy
            .acl_text()
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "0A000"
    );
    pg.pg_database_facts(Some(&PgDatabaseCommand::Owner {
        database: "cat14_17_model_legacy".into(),
        role: "root".into(),
    }))
    .unwrap();
    assert!(
        pg.pg_database_facts(None)
            .unwrap()
            .iter()
            .find(|f| f.id == legacy.id)
            .unwrap()
            .acl
            .is_none(),
        "owner adoption must not fabricate historical ACL"
    );
    pg.pg_database_facts(Some(&PgDatabaseCommand::Privileges {
        database: "cat14_17_model_pg".into(),
        role: "cat14_17_model_role".into(),
        public: false,
        privileges: "CREATE".into(),
        grant: true,
        grant_option: true,
        cascade: false,
    }))
    .unwrap();
    let key = astersql_meta::transaction_meta_hash_key(
        b"PgDatabaseConnect",
        database_id("cat14_17_model_pg").to_string().as_bytes(),
    );
    let original = domain
        .storage_handle()
        .with_storage(|store| {
            let mut txn = store.Begin(&[])?;
            let original = txn.Get(&kv::Context::default(), key.clone(), &[])?.Value;
            let mut corrupt: serde_json::Value = serde_json::from_slice(&original).unwrap();
            corrupt["acl"][0]["grantor"] = serde_json::json!(0);
            txn.Set(key.clone(), serde_json::to_vec(&corrupt).unwrap())?;
            txn.Commit(&kv::Context::default())?;
            Ok::<_, kv::errors::SharedError>(original)
        })
        .unwrap();
    let failed = pg
        .pg_database_facts(Some(&PgDatabaseCommand::Privileges {
            database: "cat14_17_model_pg".into(),
            role: "cat14_17_model_role".into(),
            public: false,
            privileges: "CREATE".into(),
            grant: false,
            grant_option: false,
            cascade: false,
        }))
        .unwrap_err();
    assert_eq!(failed.pg_function_error().unwrap().state, "0A000");
    domain
        .storage_handle()
        .with_storage(|store| {
            let mut txn = store.Begin(&[])?;
            let unchanged: serde_json::Value =
                serde_json::from_slice(&txn.Get(&kv::Context::default(), key.clone(), &[])?.Value)
                    .unwrap();
            assert!(
                unchanged["acl"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|e| e["privileges"] == serde_json::json!(4)
                        && e["grant_options"] == serde_json::json!(4)),
                "failed revoke must not commit its writes"
            );
            txn.Set(key.clone(), original.clone())?;
            txn.Commit(&kv::Context::default())?;
            Ok::<_, kv::errors::SharedError>(())
        })
        .unwrap();
    assert!(
        pg.pg_database_drop_role("cat14_17_model_role", false)
            .is_err(),
        "a grant must retain its stable role identity"
    );
    domain.force_full_reload_for_test().unwrap();
    let facts = pg.pg_database_facts(None).unwrap();
    let owned = facts
        .iter()
        .find(|f| f.id == database_id("cat14_17_model_pg"))
        .unwrap();
    assert!(!owned.acl_default);
    assert!(
        owned
            .acl_text()
            .unwrap()
            .unwrap()
            .contains("cat14_17_model_role=C*/root")
    );
    pg.pg_database_facts(Some(&PgDatabaseCommand::Privileges {
        database: "cat14_17_model_pg".into(),
        role: "cat14_17_model_role".into(),
        public: false,
        privileges: "CREATE".into(),
        grant: false,
        grant_option: false,
        cascade: false,
    }))
    .unwrap();
    assert!(
        pg.pg_database_drop_role("cat14_17_model_role", false)
            .is_ok()
    );
    native.execute("DROP DATABASE cat14_17_model_pg").unwrap();
    native
        .execute("DROP DATABASE cat14_17_model_legacy")
        .unwrap();
    pg.pg_role_command(&PgRoleCommand::Drop {
        name: "cat14_17_model_role".into(),
    })
    .unwrap();
    domain.close();
}

#[test]
fn cat14_18_database_acl_admin_ignores_unrelated_legacy_roles() {
    use super::pg_database::PgDatabaseCommand;
    let (domain, native) = CreateAnalyzeSession().unwrap();
    native.pg_register_bootstrap_role().unwrap();
    native
        .execute(
            "CREATE USER 'cat14_18_unregistered'@'%' IDENTIFIED WITH mysql_native_password BY ''",
        )
        .unwrap();
    // Reproduce a historical native account that has not acquired a PG identity.
    let ctx = kv::Context::default();
    domain
        .storage_handle()
        .with_storage(|store| {
            kv::RunInNewTxn(&ctx, store, true, |_, txn| {
                let key = astersql_meta::transaction_meta_string_key(b"PgDatabaseRoleIds");
                let mut ids: std::collections::BTreeMap<String, i64> =
                    serde_json::from_slice(&txn.Get(&ctx, key.clone(), &[])?.Value).unwrap();
                assert!(ids.remove("cat14_18_unregistered").is_some());
                txn.Set(key, serde_json::to_vec(&ids).unwrap())
            })
        })
        .unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    pg.execute("CREATE DATABASE cat14_18_admin_acl").unwrap();
    let facts = pg
        .pg_database_facts(Some(&PgDatabaseCommand::Privileges {
            database: "cat14_18_admin_acl".into(),
            role: "PUBLIC".into(),
            public: true,
            privileges: "CREATE".into(),
            grant: true,
            grant_option: false,
            cascade: false,
        }))
        .unwrap();
    let id = domain
        .info_schema()
        .SchemaByName(&astersql_infoschema::CiString::new("cat14_18_admin_acl"))
        .unwrap()
        .id;
    let database = facts.iter().find(|f| f.id == id).unwrap();
    assert!(database.acl_text().unwrap().unwrap().contains("=CTc/root"));
    native.execute("DROP DATABASE cat14_18_admin_acl").unwrap();
    native
        .execute("DROP USER 'cat14_18_unregistered'@'%'")
        .unwrap();
    domain.close();
}
