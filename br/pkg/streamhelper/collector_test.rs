// Copyright 2026 AsterSQL.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::advancer_cliext::TaskEvent;
use crate::advancer_env::{LogBackupFlushIntervalGetter, RegionLockResolver, StreamMeta};
use crate::collector::{NewClusterCollector, defaultBatchSize};
use crate::regioniter::{RegionWithLeader, Store, TiKVClusterMeta};
use crate::stubs::{
    GetLastFlushTSOfRegionRequest, GetLastFlushTSOfRegionResponse, KeyRange, LogBackupClient,
    LogBackupService, Peer, Region, RegionCheckpoint, RegionEpoch, RegionError,
};

struct RecordingClient {
    calls: AtomicUsize,
    responses: Mutex<Vec<GetLastFlushTSOfRegionResponse>>,
}

impl LogBackupClient for RecordingClient {
    fn GetLastFlushTSOfRegion(
        &self,
        _req: &GetLastFlushTSOfRegionRequest,
    ) -> Result<GetLastFlushTSOfRegionResponse, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.responses.lock().unwrap().remove(0))
    }
}

struct TestEnv(Arc<RecordingClient>);

impl TiKVClusterMeta for TestEnv {
    fn RegionScan(&self, _: &[u8], _: &[u8], _: i32) -> Result<Vec<RegionWithLeader>, String> {
        Ok(vec![])
    }
    fn Stores(&self) -> Result<Vec<Store>, String> {
        Ok(vec![])
    }
    fn BlockGCUntil(&self, at: u64) -> Result<u64, String> {
        Ok(at)
    }
    fn UnblockGC(&self) -> Result<(), String> {
        Ok(())
    }
    fn FetchCurrentTS(&self) -> Result<u64, String> {
        Ok(0)
    }
}

impl LogBackupService for TestEnv {
    fn GetLogBackupClient(&self, _: u64) -> Result<Arc<dyn LogBackupClient>, String> {
        Ok(self.0.clone())
    }
    fn ClearCache(&self, _: u64) -> Result<(), String> {
        Ok(())
    }
}

impl StreamMeta for TestEnv {
    fn Begin(&self, _: &mut Vec<TaskEvent>) -> Result<(), String> {
        Ok(())
    }
    fn UploadV3GlobalCheckpointForTask(&self, _: &str, _: u64) -> Result<(), String> {
        Ok(())
    }
    fn GetGlobalCheckpointForTask(&self, _: &str) -> Result<u64, String> {
        Ok(0)
    }
    fn ClearV3GlobalCheckpointForTask(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn PauseTask(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

impl RegionLockResolver for TestEnv {
    fn ResolveLocksForRange(&self, _: u64, _: &[u8], _: &[u8]) -> Result<(), String> {
        Ok(())
    }
}

impl LogBackupFlushIntervalGetter for TestEnv {
    fn GetLogBackupFlushInterval(&self) -> Result<Duration, String> {
        Ok(Duration::from_secs(1))
    }
}

fn region(id: u64) -> RegionWithLeader {
    let start = id.to_be_bytes().to_vec();
    let end = id.saturating_add(1).to_be_bytes().to_vec();
    RegionWithLeader {
        Region: Region {
            Id: id,
            StartKey: start,
            EndKey: end,
            RegionEpoch: RegionEpoch {
                Version: 1,
                ConfVer: 0,
            },
        },
        Leader: Peer { Id: 1, StoreId: 1 },
    }
}

#[test]
fn full_batch_still_sends_go_equivalent_empty_tail_request() {
    let client = Arc::new(RecordingClient {
        calls: AtomicUsize::new(0),
        responses: Mutex::new(vec![
            GetLastFlushTSOfRegionResponse::default(),
            GetLastFlushTSOfRegionResponse::default(),
        ]),
    });
    let mut collector = NewClusterCollector(Arc::new(TestEnv(client.clone())));
    for id in 1..=defaultBatchSize as u64 {
        collector.CollectRegion(region(id)).unwrap();
    }
    collector.Finish().unwrap();
    assert_eq!(client.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn unknown_failed_region_preserves_go_zero_value_range() {
    let client = Arc::new(RecordingClient {
        calls: AtomicUsize::new(0),
        responses: Mutex::new(vec![GetLastFlushTSOfRegionResponse {
            Checkpoints: vec![RegionCheckpoint {
                Region: crate::stubs::RegionIdentity {
                    Id: 999,
                    EpochVersion: 1,
                },
                Checkpoint: 0,
                Err: Some(RegionError {
                    EpochNotMatch: true,
                    NotLeader: false,
                }),
            }],
        }]),
    });
    let mut collector = NewClusterCollector(Arc::new(TestEnv(client)));
    collector.CollectRegion(region(1)).unwrap();
    let result = collector.Finish().unwrap();
    assert_eq!(result.FailureSubRanges, vec![KeyRange::default()]);
}
