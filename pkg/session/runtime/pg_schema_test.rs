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
fn legacy_empty_namespace_migrates_once_without_reusing_global_ids() {
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
    let identity = first
        .pg_namespace_facts()
        .unwrap()
        .into_iter()
        .find(|f| f.name == "legacy")
        .unwrap()
        .id;
    let second = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    assert_eq!(
        second
            .pg_namespace_facts()
            .unwrap()
            .into_iter()
            .find(|f| f.name == "legacy")
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
    for schema in ["legacy", "new_namespace"] {
        second
            .pg_schema_command(&PgSchemaCommand::Drop(schema.into()))
            .unwrap();
    }
}
