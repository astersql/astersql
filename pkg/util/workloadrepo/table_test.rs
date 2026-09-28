// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

use super::*;
use crate::worker::repositoryTable;
use std::sync::Mutex;

struct TableBackend {
    source_result: Result<Vec<ColumnDefinition>, String>,
    source_calls: Mutex<usize>,
}

impl TableBackend {
    fn failing_source(error: &str) -> Self {
        Self {
            source_result: Err(error.into()),
            source_calls: Mutex::new(0),
        }
    }
}

impl RepositoryBackend for TableBackend {
    fn execute(&self, _sql: &str, _args: &[Value]) -> Result<Vec<Row>, String> {
        unreachable!()
    }

    fn source_columns(&self, _schema: &str, _table: &str) -> Result<Vec<ColumnDefinition>, String> {
        *self.source_calls.lock().unwrap() += 1;
        self.source_result.clone()
    }

    fn table_exists(&self, _table: &str) -> bool {
        unreachable!()
    }

    fn partitions(&self, _table: &str) -> Result<Vec<String>, String> {
        unreachable!()
    }

    fn instance_id(&self) -> Result<String, String> {
        unreachable!()
    }

    fn is_owner(&self) -> bool {
        unreachable!()
    }

    fn etcd_available(&self) -> bool {
        unreachable!()
    }

    fn kv_create(&self, _key: &str, _value: &str) -> Result<bool, String> {
        unreachable!()
    }

    fn kv_get(&self, _key: &str) -> Result<Option<String>, String> {
        unreachable!()
    }

    fn kv_cas(&self, _key: &str, _old: &str, _new: &str) -> Result<bool, String> {
        unreachable!()
    }
}

#[test]
fn metadata_create_propagates_source_lookup_error_before_type_error_like_go() {
    let backend = TableBackend::failing_source("source table missing");
    let table = repositoryTable {
        tableType: metadataTable,
        ..Default::default()
    };

    assert_eq!(
        buildCreateQuery(&backend, &table),
        Err("source table missing".into())
    );
    assert_eq!(*backend.source_calls.lock().unwrap(), 1);
}

#[test]
fn metadata_insert_propagates_source_lookup_error_before_type_error_like_go() {
    let backend = TableBackend::failing_source("source table missing");
    let mut table = repositoryTable {
        tableType: metadataTable,
        ..Default::default()
    };

    assert_eq!(
        buildInsertQuery(&backend, &mut table),
        Err("source table missing".into())
    );
    assert_eq!(*backend.source_calls.lock().unwrap(), 1);
}
