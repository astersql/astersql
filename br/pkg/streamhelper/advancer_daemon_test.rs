// Copyright 2026 AsterSQL.

use crate::advancer::NewCheckpointAdvancer;
use crate::advancer_cliext::{EventType, TaskEvent};
use crate::advancer_daemon::{
    IsAdvancerOwner, OwnerManagerForLogBackupId, OwnerManagerPath, OwnerManagerPrompt,
};
use crate::basic_lib_for_test::{create_fake_cluster, new_test_env};
use crate::stubs::{KeyRange, StreamBackupTaskInfo};

#[test]
fn daemon_lifecycle_consumes_task_snapshot_and_cleans_owner_state() {
    let cluster = create_fake_cluster(1, false);
    let env = new_test_env(&cluster);
    *env.task.lock().unwrap() = TaskEvent {
        Type: EventType::EventAdd,
        Name: "daemon-task".into(),
        Info: Some(StreamBackupTaskInfo {
            Name: "daemon-task".into(),
            StartTs: 42,
            ..Default::default()
        }),
        Ranges: vec![KeyRange::default()],
        Err: None,
    };
    let adv = NewCheckpointAdvancer(env);

    assert!(!adv.HasTask());
    adv.OnStart();
    assert!(adv.HasTask());

    adv.OnBecomeOwner();
    assert!(IsAdvancerOwner());
    adv.OnStop();
    assert!(!IsAdvancerOwner());
    assert!(!adv.HasSubscriptions());
}

#[test]
fn owner_identity_contract_matches_go() {
    assert_eq!(OwnerManagerPrompt(), "log-backup");
    assert_eq!(OwnerManagerPath(), "/tidb/br-stream/owner");
    assert_eq!(
        OwnerManagerForLogBackupId().parse::<uuid::Uuid>().is_ok(),
        true
    );
    assert_ne!(OwnerManagerForLogBackupId(), OwnerManagerForLogBackupId());
}
