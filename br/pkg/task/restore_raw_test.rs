// Copyright 2026 AsterSQL.

//! Go-equivalent contract tests for `br/pkg/task/restore_raw.go`.

use std::sync::{Arc, Mutex};

use crate::restore_raw::{RestoreRawConfig, RunRestoreRaw};
use crate::stubs::backuppb::{BackupMeta, File, RawRange};
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

fn config_with_meta(meta: BackupMeta) -> RestoreRawConfig {
    let storage = MemStorage::new();
    storage.put(MetaFile, serde_json::to_vec(&meta).unwrap());
    let mut cfg = RestoreRawConfig::default();
    cfg.RawKvConfig.Config.PD = vec!["127.0.0.1:2379".into()];
    cfg.RawKvConfig.Config.Storage = "local:///tmp".into();
    cfg.RawKvConfig.StartKey = b"b".to_vec();
    cfg.RawKvConfig.EndKey = b"d".to_vec();
    cfg.RawKvConfig.CF = "default".into();
    cfg.RestoreStorage = Some(storage);
    cfg
}

#[test]
fn raw_restore_reads_meta_filters_requested_range_and_tracks_go_progress() {
    let meta = BackupMeta {
        IsRawKv: true,
        RawRanges: vec![RawRange {
            StartKey: b"a".to_vec(),
            EndKey: b"e".to_vec(),
            Cf: "default".into(),
        }],
        Files: vec![
            File {
                Name: "before".into(),
                StartKey: b"a".to_vec(),
                EndKey: b"b".to_vec(),
                Size_: 2,
                Cf: "default".into(),
            },
            File {
                Name: "inside-1".into(),
                StartKey: b"b".to_vec(),
                EndKey: b"c".to_vec(),
                Size_: 3,
                Cf: "default".into(),
            },
            File {
                Name: "inside-2".into(),
                StartKey: b"c".to_vec(),
                EndKey: b"d".to_vec(),
                Size_: 5,
                Cf: "default".into(),
            },
            File {
                Name: "wrong-cf".into(),
                StartKey: b"b".to_vec(),
                EndKey: b"d".to_vec(),
                Size_: 11,
                Cf: "write".into(),
            },
        ],
        ..Default::default()
    };
    let mut cfg = config_with_meta(meta);
    let glue = RecordingGlue::default();

    RunRestoreRaw(
        &crate::restore_lifecycle_test::fixture_glue(&glue),
        "Raw Restore",
        &mut cfg,
    )
    .unwrap();

    // Go GetFilesInRawRange includes a file whose EndKey equals the requested StartKey.
    assert_eq!(
        glue.records.lock().unwrap().as_slice(),
        &[(RestoreDataSize.into(), 10)]
    );
    let progresses = glue.progresses.lock().unwrap();
    assert_eq!(progresses.len(), 1);
    assert_eq!(
        (&progresses[0].0, progresses[0].1),
        (&"Raw Restore".to_string(), 4)
    );
    assert_eq!(progresses[0].2.GetCurrent(), 4);
}

#[test]
fn raw_restore_rejects_transactional_meta_and_uncovered_range() {
    let glue = RecordingGlue::default();
    let mut transactional = config_with_meta(BackupMeta::default());
    let err = RunRestoreRaw(
        &crate::restore_lifecycle_test::fixture_glue(&glue),
        "Raw Restore",
        &mut transactional,
    )
    .unwrap_err();
    assert!(
        err.msg
            .contains("cannot do raw restore from transactional data")
    );

    let mut uncovered = config_with_meta(BackupMeta {
        IsRawKv: true,
        RawRanges: vec![RawRange {
            StartKey: b"c".to_vec(),
            EndKey: b"e".to_vec(),
            Cf: "default".into(),
        }],
        ..Default::default()
    });
    let err = RunRestoreRaw(
        &crate::restore_lifecycle_test::fixture_glue(&glue),
        "Raw Restore",
        &mut uncovered,
    )
    .unwrap_err();
    assert!(err.msg.contains("only partially covered"));
}
