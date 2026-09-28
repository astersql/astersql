// Copyright 2026 AsterSQL.

use crate::log_restore::{CheckpointProgress, RestoreProgress};

#[test]
fn test_restore_progress_uses_go_compatible_numeric_json() {
    let snapshot = CheckpointProgress {
        Progress: RestoreProgress::InSnapshotRestore,
    };
    assert_eq!(
        serde_json::to_string(&snapshot).unwrap(),
        r#"{"progress":0}"#
    );

    let log = CheckpointProgress {
        Progress: RestoreProgress::InLogRestoreAndIdMapPersisted,
    };
    assert_eq!(serde_json::to_string(&log).unwrap(), r#"{"progress":1}"#);

    let decoded: CheckpointProgress = serde_json::from_str(r#"{"progress":1}"#).unwrap();
    assert_eq!(
        decoded.Progress,
        RestoreProgress::InLogRestoreAndIdMapPersisted
    );
}
