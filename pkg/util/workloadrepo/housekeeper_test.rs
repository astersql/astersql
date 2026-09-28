// Copyright 2026 AsterSQL.

use std::sync::Mutex;

use chrono::{Local, TimeZone};

use crate::worker::repositoryTable;
use crate::{ColumnDefinition, RepositoryBackend, Row, Value, dropOldPartition};

#[derive(Default)]
struct RecordingBackend {
    statements: Mutex<Vec<String>>,
}

impl RepositoryBackend for RecordingBackend {
    fn execute(&self, sql: &str, _args: &[Value]) -> Result<Vec<Row>, String> {
        self.statements.lock().unwrap().push(sql.to_owned());
        Ok(Vec::new())
    }

    fn source_columns(&self, _schema: &str, _table: &str) -> Result<Vec<ColumnDefinition>, String> {
        Ok(Vec::new())
    }

    fn table_exists(&self, _table: &str) -> bool {
        true
    }

    fn partitions(&self, _table: &str) -> Result<Vec<String>, String> {
        Ok(vec!["p20260101".to_owned()])
    }

    fn instance_id(&self) -> Result<String, String> {
        Ok(String::new())
    }

    fn is_owner(&self) -> bool {
        true
    }

    fn etcd_available(&self) -> bool {
        true
    }

    fn kv_create(&self, _key: &str, _value: &str) -> Result<bool, String> {
        Ok(true)
    }

    fn kv_get(&self, _key: &str) -> Result<Option<String>, String> {
        Ok(None)
    }

    fn kv_cas(&self, _key: &str, _old: &str, _new: &str) -> Result<bool, String> {
        Ok(true)
    }
}

#[test]
fn drop_partition_quotes_partition_identifier_like_go() {
    let backend = RecordingBackend::default();
    let table = repositoryTable {
        destTable: "HIST_T".to_owned(),
        ..Default::default()
    };
    let now = Local.with_ymd_and_hms(2026, 1, 10, 12, 0, 0).unwrap();

    dropOldPartition(&backend, &table, now, 7).unwrap();

    assert_eq!(
        backend.statements.lock().unwrap().as_slice(),
        ["ALTER TABLE `WORKLOAD_SCHEMA`.`HIST_T` DROP PARTITION `p20260101`"]
    );
}
