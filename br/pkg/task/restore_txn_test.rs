// Copyright 2026 AsterSQL.

//! Go-equivalent contract tests for `br/pkg/task/restore_txn.go`.

use std::sync::{Arc, Mutex};

use crate::common::Config;
use crate::restore_txn::RunRestoreTxnWithStorage;
use crate::stubs::backuppb::{BackupMeta, File};
use crate::stubs::{Glue, MemProgress, MemStorage, MetaFile, Progress, RestoreDataSize, Result};

#[derive(Default)]
struct RecordingGlue {
    records: Mutex<Vec<(String, u64)>>,
    progresses: Mutex<Vec<(String, i64, Arc<MemProgress>)>>,
}

impl Glue for RecordingGlue {
    fn GetVersion(&self) -> String {
        String::new()
    }

    fn StartProgress(&self, cmd: &str, total: i64, _log_progress: bool) -> Arc<dyn Progress> {
        let progress = Arc::new(MemProgress::default());
        self.progresses
            .lock()
            .unwrap()
            .push((cmd.into(), total, progress.clone()));
        progress
    }

    fn Record(&self, key: &str, value: u64) {
        self.records.lock().unwrap().push((key.into(), value));
    }

    fn ConsoleOutWrite(&self, _msg: &[u8]) -> Result<()> {
        Ok(())
    }
}

fn config() -> Config {
    Config {
        PD: vec!["127.0.0.1:2379".into()],
        Storage: "local:///tmp".into(),
        ..Default::default()
    }
}

#[test]
fn txn_restore_reads_meta_merges_ranges_and_tracks_go_progress() {
    let storage = MemStorage::new();
    storage.put(
        MetaFile,
        serde_json::to_vec(&BackupMeta {
            IsTxnKv: true,
            Files: vec![
                File {
                    Name: "one".into(),
                    StartKey: b"a".to_vec(),
                    EndKey: b"c".to_vec(),
                    Size_: 3,
                    ..Default::default()
                },
                File {
                    Name: "two".into(),
                    StartKey: b"b".to_vec(),
                    EndKey: b"d".to_vec(),
                    Size_: 5,
                    ..Default::default()
                },
            ],
            ..Default::default()
        })
        .unwrap(),
    );
    let glue = RecordingGlue::default();

    RunRestoreTxnWithStorage(&glue, "Txn Restore", &mut config(), &storage).unwrap();

    assert_eq!(
        glue.records.lock().unwrap().as_slice(),
        &[(RestoreDataSize.into(), 8)]
    );
    let progresses = glue.progresses.lock().unwrap();
    assert_eq!(progresses.len(), 1);
    // Two overlapping files merge into one split range: one split + two ingests.
    assert_eq!(progresses[0].1, 3);
    assert_eq!(progresses[0].2.GetCurrent(), 3);
}

#[test]
fn txn_restore_rejects_raw_meta_before_starting_progress() {
    let storage = MemStorage::new();
    storage.put(
        MetaFile,
        serde_json::to_vec(&BackupMeta {
            IsRawKv: true,
            ..Default::default()
        })
        .unwrap(),
    );
    let glue = RecordingGlue::default();

    let err = RunRestoreTxnWithStorage(&glue, "Txn Restore", &mut config(), &storage).unwrap_err();

    assert!(
        err.msg
            .contains("cannot do transactional restore from raw data")
    );
    assert!(glue.progresses.lock().unwrap().is_empty());
}
