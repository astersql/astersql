// Copyright 2026 AsterSQL.

use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::restore::RestoreConfig;
use crate::restore_ebs_meta::{
    RestoreEBSController, RunRestoreEBSMeta, RunRestoreEBSMetaWithController,
};
use crate::stubs::{Glue, MemProgress, MemStorage, Progress, Result, Storage};

#[derive(Default)]
struct RecordingGlue {
    starts: Mutex<Vec<(String, i64, bool)>>,
}

#[derive(Default)]
struct RecordingController {
    events: Mutex<Vec<String>>,
}

impl RestoreEBSController for RecordingController {
    fn MarkRecovering(&self) -> Result<()> {
        self.events.lock().unwrap().push("mark".into());
        Ok(())
    }

    fn ResetTS(&self, resolved_ts: u64) -> Result<()> {
        self.events
            .lock()
            .unwrap()
            .push(format!("reset:{resolved_ts}"));
        Ok(())
    }

    fn Close(&self) {
        self.events.lock().unwrap().push("close".into());
    }
}

impl Glue for RecordingGlue {
    fn GetVersion(&self) -> String {
        String::new()
    }

    fn StartProgress(&self, command: &str, total: i64, log_progress: bool) -> Arc<dyn Progress> {
        self.starts
            .lock()
            .unwrap()
            .push((command.to_owned(), total, log_progress));
        Arc::new(MemProgress::default())
    }

    fn Record(&self, _key: &str, _value: u64) {}

    fn ConsoleOutWrite(&self, _msg: &[u8]) -> Result<()> {
        Ok(())
    }
}

fn valid_meta() -> serde_json::Value {
    json!({
        "cluster_info": {
            "cluster_version": "v8.5.0",
            "full_backup_type": "aws-ebs",
            "resolved_ts": 42
        },
        "tikv": {
            "replicas": 1,
            "stores": [{
                "store_id": 1,
                "volumes": [
                    {"volume_id": "vol-1", "snapshot_id": "snap-1", "volume_az": "az-1"},
                    {"volume_id": "vol-2", "snapshot_id": "snap-2", "volume_az": "az-1"}
                ]
            }]
        },
        "region": "us-east-1",
        "unknown_future_field": {"preserved": true}
    })
}

fn config() -> RestoreConfig {
    let mut cfg = RestoreConfig::default();
    cfg.Config.PD = vec!["127.0.0.1:2379".into()];
    cfg.SkipAWS = true;
    cfg.OutputMetaFile = "restored.json".into();
    cfg
}

#[test]
fn rejects_missing_and_non_ebs_backup_meta() {
    let glue = RecordingGlue::default();
    let storage = Arc::new(MemStorage::new());
    let error = RunRestoreEBSMeta(&glue, "restore", &mut config(), storage.clone()).unwrap_err();
    assert!(error.to_string().contains("file not found: backupmeta"));

    storage.put(
        "backupmeta",
        serde_json::to_vec(&json!({
            "cluster_info": {
                "cluster_version": "v8.5.0",
                "full_backup_type": "kv",
                "resolved_ts": 42
            },
            "tikv": {"stores": [{"store_id": 1, "volumes": []}]}
        }))
        .unwrap(),
    );
    let error = RunRestoreEBSMeta(&glue, "restore", &mut config(), storage).unwrap_err();
    assert_eq!(
        error.to_string(),
        "invalid meta file, only support aws-ebs now"
    );
}

#[test]
fn skip_aws_uses_meta_volume_count_and_always_writes_preserved_output() {
    let glue = RecordingGlue::default();
    let storage = Arc::new(MemStorage::new());
    storage.put("backupmeta", serde_json::to_vec(&valid_meta()).unwrap());

    RunRestoreEBSMeta(&glue, "restore-ebs", &mut config(), storage.clone()).unwrap();

    assert_eq!(
        glue.starts.lock().unwrap().as_slice(),
        &[("restore-ebs".into(), 2, true)]
    );
    let output: serde_json::Value =
        serde_json::from_slice(&storage.ReadFile("restored.json").unwrap()).unwrap();
    assert_eq!(output, valid_meta());
}

#[test]
fn rejects_empty_pd_before_starting_progress() {
    let glue = RecordingGlue::default();
    let storage = Arc::new(MemStorage::new());
    storage.put("backupmeta", serde_json::to_vec(&valid_meta()).unwrap());
    let mut cfg = config();
    cfg.Config.PD.clear();

    let error = RunRestoreEBSMeta(&glue, "restore", &mut cfg, storage).unwrap_err();
    assert_eq!(
        error.to_string(),
        "pd address can not be empty: invalid argument"
    );
    assert!(glue.starts.lock().unwrap().is_empty());
}

#[test]
fn marks_recovering_resets_ts_and_closes_controller_in_go_order() {
    let glue = RecordingGlue::default();
    let storage = Arc::new(MemStorage::new());
    storage.put("backupmeta", serde_json::to_vec(&valid_meta()).unwrap());
    let controller = Arc::new(RecordingController::default());

    RunRestoreEBSMetaWithController(&glue, "restore", &mut config(), storage, controller.clone())
        .unwrap();

    assert_eq!(
        controller.events.lock().unwrap().as_slice(),
        &["mark", "reset:42", "close"]
    );
}
