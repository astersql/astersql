// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use astersql_infoschema::{self as infoschema, SchemaRef};
use astersql_kv as kv;

use crate::{Domain, DomainError, InfoSchemaLoader, LoadedInfoSchema};

/// Exercises the schema-fetch success/error boundary used by the Go tests.
struct SchemaLoader {
    schema: SchemaRef,
    fail: AtomicBool,
}

impl SchemaLoader {
    fn new() -> Self {
        let mut schema = infoschema::infoschema::infoSchema::new(1);
        for (database_id, database_name, table_names) in [
            (1, "mysql", &["user"][..]),
            (2, "information_schema", &[][..]),
            (3, "performance_schema", &[][..]),
            (4, "test1", &["t1", "t2"][..]),
            (5, "test2", &[][..]),
        ] {
            let tables = table_names
                .iter()
                .enumerate()
                .map(|(offset, name)| {
                    infoschema::Table::from_model(astersql_meta_model::TableInfo {
                        ID: database_id * 100 + offset as i64,
                        DBID: database_id,
                        Name: astersql_parser_ast::NewCIStr(name),
                        State: astersql_meta_model::StatePublic,
                        ..astersql_meta_model::TableInfo::default()
                    })
                })
                .collect();
            schema.add_schema(
                infoschema::DBInfo {
                    id: database_id,
                    name: infoschema::CiString::new(database_name),
                    ..infoschema::DBInfo::default()
                },
                tables,
            );
        }
        Self {
            schema: Arc::new(schema),
            fail: AtomicBool::new(false),
        }
    }

    fn fail_next_load(&self) {
        self.fail.store(true, Ordering::Release);
    }

    fn load(&self) -> Result<LoadedInfoSchema, kv::errors::SharedError> {
        if self.fail.swap(false, Ordering::AcqRel) {
            return Err(kv::errors::New(
                "failpoint: failed to fetch schemas with tables",
            ));
        }
        Ok(LoadedInfoSchema::new(Arc::clone(&self.schema), 1))
    }
}

impl InfoSchemaLoader for SchemaLoader {
    fn load_info_schema(
        &self,
        _store: &dyn kv::Storage,
        _keyspace: &str,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError> {
        self.load()
    }

    fn load_snapshot_info_schema(
        &self,
        _store: &dyn kv::Storage,
        _keyspace: &str,
        _timestamp: u64,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError> {
        self.load()
    }

    fn keyspace_exists(
        &self,
        _store: &dyn kv::Storage,
        keyspace: &str,
    ) -> Result<bool, kv::errors::SharedError> {
        Ok(keyspace == "SYSTEM")
    }
}

#[test]
fn go_db_test_inventory_has_executable_rust_mappings() {
    let go = include_str!("db_test.go");
    let go_tests: Vec<_> = go
        .lines()
        .filter_map(|line| line.strip_prefix("func Test"))
        .filter_map(|line| line.split_once('(').map(|(name, _)| name))
        .collect();
    assert_eq!(
        go_tests,
        [
            "DomainSession",
            "NormalSessionPool",
            "AbnormalSessionPool",
            "TetchAllSchemasWithTables",
            "FetchAllSchemasWithTablesWithFailpoint",
        ]
    );

    let domain_tests = include_str!("canonical_domain_test.rs");
    assert!(domain_tests.contains("domain_uses_canonical_storage_and_infoschema_lifecycle"));
    let pool_tests = include_str!("../session/syssession/pool_test.rs");
    assert!(pool_tests.contains("test_session_pool_with_session"));
    assert!(pool_tests.contains("test_session_pool_put_rejects_unclean_or_unstorable_sessions"));
}

#[test]
fn schema_enumeration_failure_and_domain_cleanup_match_go() {
    let loader = Arc::new(SchemaLoader::new());
    let domain = Domain::new_mock(
        super::canonical_domain_test::TestStorage::new(),
        loader.clone(),
    );
    domain.init().expect("bootstrap domain");

    let mut schemas: Vec<_> = domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .map(|database| {
            let mut tables: Vec<_> = database
                .tables
                .iter()
                .map(|table| table.name.original.clone())
                .collect();
            tables.sort_unstable();
            (database.name.original.clone(), tables)
        })
        .collect();
    schemas.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(schemas.len(), 5);
    assert_eq!(
        schemas
            .iter()
            .find(|(name, _)| name == "test1")
            .expect("test1 schema")
            .1,
        ["t1".to_owned(), "t2".to_owned()]
    );

    loader.fail_next_load();
    let error = domain.reload().expect_err("injected schema load failure");
    assert_eq!(
        error,
        DomainError::Store("failpoint: failed to fetch schemas with tables".to_owned())
    );

    domain.close();
    assert!(domain.is_closed());
    assert_eq!(domain.reload().unwrap_err(), DomainError::Closed);
}
