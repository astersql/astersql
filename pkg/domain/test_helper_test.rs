// Copyright 2026 AsterSQL.

use std::sync::Arc;

use astersql_infoschema::{self as infoschema, SchemaRef};
use astersql_kv as kv;

use crate::canonical_domain_test::TestStorage;
use crate::test_helper::DomainTestHelper;
use crate::{Domain, InfoSchemaLoader, LoadedInfoSchema};

struct StaticSchemaLoader(SchemaRef);

impl InfoSchemaLoader for StaticSchemaLoader {
    fn load_info_schema(
        &self,
        _store: &dyn kv::Storage,
        _keyspace: &str,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError> {
        Ok(LoadedInfoSchema::new(Arc::clone(&self.0), 1))
    }

    fn load_snapshot_info_schema(
        &self,
        _store: &dyn kv::Storage,
        _keyspace: &str,
        timestamp: u64,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError> {
        Ok(LoadedInfoSchema::new(Arc::clone(&self.0), timestamp))
    }

    fn keyspace_exists(
        &self,
        _store: &dyn kv::Storage,
        keyspace: &str,
    ) -> Result<bool, kv::errors::SharedError> {
        Ok(keyspace == "SYSTEM")
    }
}

fn domain_with_partitioned_table() -> Domain {
    let table = astersql_meta_model::TableInfo {
        ID: 42,
        DBID: 7,
        Name: astersql_parser_ast::NewCIStr("Orders"),
        Partition: Some(astersql_meta_model::PartitionInfo {
            Definitions: vec![
                astersql_meta_model::PartitionDefinition {
                    ID: 101,
                    Name: astersql_parser_ast::NewCIStr("p0"),
                    ..Default::default()
                },
                astersql_meta_model::PartitionDefinition {
                    ID: 102,
                    Name: astersql_parser_ast::NewCIStr("p1"),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }),
        ..Default::default()
    };
    let schema = infoschema::infoschema::MockInfoSchema(vec![
        infoschema::Table::from_model(table).Meta().clone(),
    ]);
    let domain = Domain::new_mock(TestStorage::new(), Arc::new(StaticSchemaLoader(schema)));
    domain.init().expect("initialize domain test fixture");
    domain
}

#[test]
fn table_info_id_partition_and_schema_enumeration_match_go_helpers() {
    let domain = domain_with_partitioned_table();

    let table = domain.must_get_table_info("TEST", "orders");
    assert_eq!(table.id, 42);
    assert_eq!(domain.must_get_table_id("test", "ORDERS"), 42);
    assert_eq!(domain.must_get_partition_at("test", "orders", 0), 101);
    assert_eq!(domain.must_get_partition_at("test", "orders", 1), 102);
    let schemas = domain.fetch_all_schemas_with_tables();
    assert!(schemas.contains(&("test".to_owned(), vec![("Orders".to_owned(), 42)])));
    assert!(schemas.contains(&("mysql".to_owned(), vec![("stats_meta".to_owned(), 9999)])));
}

#[test]
#[should_panic(expected = "table missing.unknown does not exist")]
fn missing_table_fails_the_must_contract() {
    domain_with_partitioned_table().must_get_table_info("missing", "unknown");
}

#[test]
#[should_panic]
fn partition_index_is_not_silently_clamped() {
    domain_with_partitioned_table().must_get_partition_at("test", "orders", 2);
}
