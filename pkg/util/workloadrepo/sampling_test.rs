// Copyright 2026 AsterSQL.

use std::sync::{Arc, Mutex};

use crate::worker::{initializeWorker, repositoryTable};
use crate::*;

#[test]
fn change_sampling_interval_matches_go_hook_contract() {
    let worker = initializeWorker(Arc::new(NoopBackend), Vec::new());

    // Go's hook accepts every value that strconv.Atoi can parse. SQL sysvar
    // min/max clamping happens before the hook is called.
    worker.changeSamplingInterval("-1").unwrap();
    assert_eq!(worker.intervals().0, -1);
    worker.changeSamplingInterval("601").unwrap();
    assert_eq!(worker.intervals().0, 601);

    let error = worker.changeSamplingInterval("not-an-integer").unwrap_err();
    assert!(error.contains(repositorySamplingInterval));
    assert_eq!(worker.intervals().0, 601);
}

#[test]
fn sampling_round_attempts_all_sampling_tables_and_ignores_table_errors() {
    let backend = Arc::new(RecordingBackend::default());
    let worker = initializeWorker(
        backend.clone(),
        vec![
            repositoryTable {
                tableType: samplingTable,
                insertStmt: "FAIL".into(),
                ..Default::default()
            },
            repositoryTable {
                tableType: snapshotTable,
                insertStmt: "SKIP".into(),
                ..Default::default()
            },
            repositoryTable {
                tableType: samplingTable,
                insertStmt: "SUCCEED".into(),
                ..Default::default()
            },
        ],
    );

    worker.startSample()().unwrap();

    let mut calls = backend.calls.lock().unwrap().clone();
    calls.sort();
    assert_eq!(calls, ["FAIL", "SUCCEED"]);
}

struct NoopBackend;

impl RepositoryBackend for NoopBackend {
    fn execute(&self, _: &str, _: &[Value]) -> Result<Vec<Row>, String> {
        Ok(Vec::new())
    }

    fn source_columns(&self, _: &str, _: &str) -> Result<Vec<ColumnDefinition>, String> {
        Ok(Vec::new())
    }

    fn table_exists(&self, _: &str) -> bool {
        false
    }

    fn partitions(&self, _: &str) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }

    fn instance_id(&self) -> Result<String, String> {
        Ok(String::new())
    }

    fn is_owner(&self) -> bool {
        false
    }

    fn etcd_available(&self) -> bool {
        false
    }

    fn kv_create(&self, _: &str, _: &str) -> Result<bool, String> {
        Ok(false)
    }

    fn kv_get(&self, _: &str) -> Result<Option<String>, String> {
        Ok(None)
    }

    fn kv_cas(&self, _: &str, _: &str, _: &str) -> Result<bool, String> {
        Ok(false)
    }
}

#[derive(Default)]
struct RecordingBackend {
    calls: Mutex<Vec<String>>,
}

impl RepositoryBackend for RecordingBackend {
    fn execute(&self, sql: &str, _: &[Value]) -> Result<Vec<Row>, String> {
        self.calls.lock().unwrap().push(sql.into());
        if sql == "FAIL" {
            Err("injected sampling failure".into())
        } else {
            Ok(Vec::new())
        }
    }

    fn source_columns(&self, _: &str, _: &str) -> Result<Vec<ColumnDefinition>, String> {
        Ok(Vec::new())
    }

    fn table_exists(&self, _: &str) -> bool {
        false
    }

    fn partitions(&self, _: &str) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }

    fn instance_id(&self) -> Result<String, String> {
        Ok("node-1".into())
    }

    fn is_owner(&self) -> bool {
        false
    }

    fn etcd_available(&self) -> bool {
        false
    }

    fn kv_create(&self, _: &str, _: &str) -> Result<bool, String> {
        Ok(false)
    }

    fn kv_get(&self, _: &str) -> Result<Option<String>, String> {
        Ok(None)
    }

    fn kv_cas(&self, _: &str, _: &str, _: &str) -> Result<bool, String> {
        Ok(false)
    }
}
