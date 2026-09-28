// Copyright 2026 AsterSQL.

use std::sync::{Arc, Mutex};

use crate::restore::RestoreConfig;
use crate::restore_data::{ReadBackupMetaData, RunResolveKvData};
use crate::stubs::{Glue, MemProgress, MemStorage, Progress, Result};

#[derive(Default)]
struct RecordingGlue {
    starts: Mutex<Vec<(String, i64, bool)>>,
}

impl Glue for RecordingGlue {
    fn GetVersion(&self) -> String {
        "test".to_string()
    }

    fn StartProgress(&self, cmd: &str, total: i64, log_progress: bool) -> Arc<dyn Progress> {
        self.starts
            .lock()
            .unwrap()
            .push((cmd.to_string(), total, log_progress));
        Arc::new(MemProgress::default())
    }

    fn Record(&self, _key: &str, _value: u64) {}

    fn ConsoleOutWrite(&self, _msg: &[u8]) -> Result<()> {
        Ok(())
    }
}

fn storage_with_meta(json: &str) -> Arc<MemStorage> {
    let storage = Arc::new(MemStorage::new());
    storage.put("backupmeta.json", json.as_bytes().to_vec());
    storage
}

#[test]
fn reads_go_ebs_cluster_metadata() {
    let storage = storage_with_meta(
        r#"{
            "cluster_info": {"full_backup_type":"aws-ebs","resolved_ts":4242},
            "tikv": {"replicas":3,"stores":[]}
        }"#,
    );

    assert_eq!(ReadBackupMetaData(storage.as_ref()).unwrap(), (4242, 3));
}

#[test]
fn rejects_metadata_without_ebs_backup_type() {
    let storage = storage_with_meta(r#"{"tikv":{"replicas":3,"stores":[]}}"#);

    let error = ReadBackupMetaData(storage.as_ref()).unwrap_err();
    assert_eq!(error.msg, "invalid meta file, only support aws-ebs now");
}

#[test]
fn resolve_progress_uses_go_store_formula() {
    let storage = storage_with_meta(
        r#"{
            "cluster_info": {"full_backup_type":"aws-ebs","resolved_ts":4242},
            "tikv": {"replicas":3,"stores":[]}
        }"#,
    );
    let glue = RecordingGlue::default();
    let mut cfg = RestoreConfig::default();
    cfg.Config.PD = vec!["127.0.0.1:2379".to_string()];

    RunResolveKvData(&glue, "Resolve KV Data", &mut cfg, storage).unwrap();

    assert_eq!(
        *glue.starts.lock().unwrap(),
        vec![("Resolve KV Data".to_string(), 11, true)]
    );
}
