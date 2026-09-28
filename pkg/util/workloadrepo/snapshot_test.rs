// Copyright 2026 AsterSQL.

use std::sync::{Arc, Mutex};

use crate::worker::{repositoryTable, worker};
use crate::*;

#[derive(Default)]
struct SnapshotBackend {
    executed: Mutex<Vec<(String, Vec<Value>)>>,
    execute_error: Mutex<Option<String>>,
    columns_error: Mutex<Option<String>>,
}

impl RepositoryBackend for SnapshotBackend {
    fn execute(&self, sql: &str, args: &[Value]) -> Result<Vec<Row>, String> {
        if let Some(error) = self.execute_error.lock().unwrap().clone() {
            return Err(error);
        }
        self.executed
            .lock()
            .unwrap()
            .push((sql.to_string(), args.to_vec()));
        Ok(Vec::new())
    }

    fn source_columns(&self, _: &str, _: &str) -> Result<Vec<ColumnDefinition>, String> {
        if let Some(error) = self.columns_error.lock().unwrap().clone() {
            return Err(error);
        }
        Ok(Vec::new())
    }

    fn table_exists(&self, _: &str) -> bool {
        true
    }
    fn partitions(&self, _: &str) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }
    fn instance_id(&self) -> Result<String, String> {
        Ok("node-1".into())
    }
    fn is_owner(&self) -> bool {
        true
    }
    fn etcd_available(&self) -> bool {
        true
    }
    fn kv_create(&self, _: &str, _: &str) -> Result<bool, String> {
        Ok(true)
    }
    fn kv_get(&self, _: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn kv_cas(&self, _: &str, _: &str, _: &str) -> Result<bool, String> {
        Ok(true)
    }
}

fn snapshot_worker(backend: Arc<SnapshotBackend>, table: repositoryTable) -> Arc<worker> {
    initializeWorker(backend, vec![table])
}

#[test]
fn update_hist_snapshot_appends_go_joined_errors() {
    let backend = Arc::new(SnapshotBackend::default());
    let worker = snapshot_worker(backend.clone(), repositoryTable::default());

    worker
        .updateHistSnapshot(7, &["first".into(), "second".into()])
        .unwrap();

    let executed = backend.executed.lock().unwrap();
    let (sql, args) = executed.last().unwrap();
    assert!(sql.contains("COALESCE(CONCAT(ERROR, %?), ERROR, %?)"));
    assert_eq!(
        args,
        &[
            Value::String("first\nsecond".into()),
            Value::String("first\nsecond".into()),
            Value::UInt(7),
        ]
    );
}

#[test]
fn snapshot_table_wraps_build_error_with_destination_table() {
    let backend = Arc::new(SnapshotBackend::default());
    *backend.columns_error.lock().unwrap() = Some("columns unavailable".into());
    let worker = snapshot_worker(
        backend,
        repositoryTable {
            schema: "INFORMATION_SCHEMA".into(),
            table: "SOURCE".into(),
            destTable: "HIST_SOURCE".into(),
            tableType: snapshotTable,
            ..Default::default()
        },
    );

    assert_eq!(
        worker.snapshotTable(1, 0).unwrap_err(),
        "could not generate insert statement for `HIST_SOURCE`: columns unavailable"
    );
}

#[test]
fn snapshot_table_wraps_execution_error_with_destination_table() {
    let backend = Arc::new(SnapshotBackend::default());
    *backend.execute_error.lock().unwrap() = Some("write failed".into());
    let worker = snapshot_worker(
        backend,
        repositoryTable {
            destTable: "HIST_SOURCE".into(),
            insertStmt: "INSERT snapshot".into(),
            tableType: snapshotTable,
            ..Default::default()
        },
    );

    assert_eq!(
        worker.snapshotTable(1, 0).unwrap_err(),
        "could not run insert statement for `HIST_SOURCE`: write failed"
    );
}

#[test]
fn change_snapshot_interval_matches_go_hook_without_extra_clamping() {
    let backend = Arc::new(SnapshotBackend::default());
    let worker = snapshot_worker(backend, repositoryTable::default());

    worker.changeSnapshotInterval("2").unwrap();
    assert_eq!(worker.intervals().1, 2);
}
