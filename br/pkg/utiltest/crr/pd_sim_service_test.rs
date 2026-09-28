// Copyright 2026 AsterSQL.

use std::sync::atomic::Ordering;
use std::time::Duration;

use astersql_br_pkg_streamhelper::{
    GetLastFlushTSOfRegionRequest, LogBackupService, RegionIdentity,
};

use crate::{NewPDSimWithTestContext, NewTestContextWithSeed};

#[test]
fn log_backup_client_preserves_region_error_kind() {
    let pd =
        NewPDSimWithTestContext(Vec::new(), String::new(), &NewTestContextWithSeed(1970)).unwrap();
    let region_id = pd.RegionIDs()[0];
    let store = pd.cluster.EnsureStore(1, 1);
    store
        .LegacyRegionCheckpointRPCEnabled
        .store(1, Ordering::SeqCst);

    let client = pd.GetLogBackupClient(1).unwrap();
    let response = client
        .GetLastFlushTSOfRegion(&GetLastFlushTSOfRegionRequest {
            Regions: vec![RegionIdentity {
                Id: region_id,
                EpochVersion: 0,
            }],
        })
        .unwrap();
    let error = response.Checkpoints[0].Err.as_ref().unwrap();
    assert!(error.EpochNotMatch);
    assert!(!error.NotLeader);
}

#[test]
fn log_backup_client_forwards_flush_subscription() {
    let pd =
        NewPDSimWithTestContext(Vec::new(), String::new(), &NewTestContextWithSeed(1970)).unwrap();
    let store = pd.cluster.EnsureStore(1, 1);
    let client = pd.GetLogBackupClient(1).unwrap();

    let events = client.SubscribeFlushEvents().unwrap();
    store.Flush();
    let batch = events.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(batch.len(), 1);
    assert!(batch[0].StartKey.is_empty());
    assert!(batch[0].EndKey.is_empty());
    assert_eq!(batch[0].Checkpoint, pd.task_start());
}
