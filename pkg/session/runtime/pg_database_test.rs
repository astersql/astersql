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
