// Copyright 2026 AsterSQL.

use std::sync::Arc;

use crate::client::NewMetaDataClient;
use crate::models::{Pause, TaskInfo};
use crate::stubs::{EtcdKV, MemEtcd, StreamBackupTaskInfo};

#[test]
fn put_pausing_task_preserves_legacy_empty_pause_marker() {
    let kv = Arc::new(MemEtcd::new());
    let meta = NewMetaDataClient(kv.clone());
    let task_name = "legacy_pausing";
    let info = TaskInfo {
        PBInfo: StreamBackupTaskInfo {
            Name: task_name.into(),
            ..Default::default()
        },
        Ranges: Vec::new(),
        Pausing: true,
    };

    meta.PutTask(&info).unwrap();

    let pause = kv.GetWithRevision(&Pause(task_name)).unwrap();
    assert_eq!(pause.Value, Some(Vec::new()));
    assert!(meta.GetTaskWithPauseStatus(task_name).unwrap().1);

    let task = meta.GetTask(task_name).unwrap();
    assert!(task.IsPaused().unwrap());
    assert!(task.GetPauseV2().unwrap().is_none());
}
