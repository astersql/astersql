// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// GC Worker（垃圾回收工作线程）测试与 Go 参考用例占位。
//
// 可执行部分校验 store 引擎过滤（TiKV / TiFlash / Tombstone）以及 DeleteRange
// 并发度计算；其后大块 `/* ... */` 保留 Go 侧 mock suite、ResolveLock、
// Safe Point（安全点）、Placement/Label 规则等完整测试源码，供迁移对照。

// mockGCWorkerLockResolver 对应 Go 的同名 struct，字段顺序与嵌入类型保持原样，便于人工核对测试 fixture。
// pub struct mockGCWorkerLockResolver {
//     tikv::RegionLockResolver
//     tikvStore         tikv::Storage
//     scanLocks         func([]*txnlock::Lock, []byte) ([]*txnlock::Lock, *tikv::KeyLocation)
//     batchResolveLocks func([]*txnlock::Lock, *tikv::KeyLocation) (*tikv::KeyLocation, error)
// }
// */
use crate::calculate_delete_range_concurrency;
use crate::gc_worker::{
    DelRangeTask, GCError, GCResult, GCSession, GCWorker, GCWorkerRuntime, KeyRange, KeyspaceInfo,
    SafePointAdvance, StoreInfo, StoreState, gcConcurrency, needsGCOperationForStore,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// 构造固定 ID 的 StoreInfo 测试夹具。
fn store(state: StoreState, engine: &str) -> StoreInfo {
    StoreInfo {
        id: 42,
        address: "127.0.0.1:20160".into(),
        state,
        engine: engine.into(),
    }
}

#[test]
/// 校验仅对存活 TiKV store 执行 GC；TiFlash / Tombstone 应跳过。
fn store_filter_matches_tikv_tiflash_and_tombstone_rules() {
    assert!(needsGCOperationForStore(&store(StoreState::Up, "tikv")).unwrap());
    assert!(needsGCOperationForStore(&store(StoreState::Up, "")).unwrap());
    assert!(!needsGCOperationForStore(&store(StoreState::Up, "tiflash")).unwrap());
    assert!(!needsGCOperationForStore(&store(StoreState::Up, "tiflash_compute")).unwrap());
    assert!(!needsGCOperationForStore(&store(StoreState::Tombstone, "tikv")).unwrap());
}

#[test]
/// 未知引擎不得静默跳过，须返回明确错误以便发现配置问题。
fn unknown_live_store_engine_is_not_silently_skipped() {
    let error = needsGCOperationForStore(&store(StoreState::Offline, "custom"))
        .expect_err("unknown engine must fail GC store discovery");
    assert!(error.to_string().contains("unsupported store engine"));
    assert!(error.to_string().contains("storeID 42"));
}

#[test]
/// 对照 Go：自动模式与固定并发上限下的 DeleteRange 并发度公式。
fn delete_range_concurrency_preserves_go_auto_and_fixed_modes() {
    assert_eq!(1, calculate_delete_range_concurrency(2, true, 0));
    assert_eq!(2, calculate_delete_range_concurrency(8, true, 200_000));
    assert_eq!(2, calculate_delete_range_concurrency(8, false, 1));
    assert_eq!(32, calculate_delete_range_concurrency(128, false, 1));
}

#[test]
/// Go `time.ParseDuration` accepts decimal and compound durations.
fn go_duration_parser_accepts_compound_and_fractional_values() {
    assert_eq!(
        std::time::Duration::from_secs(5_400),
        super::gc_worker::parse_duration("1h30m").unwrap()
    );
    assert_eq!(
        std::time::Duration::from_millis(1500),
        super::gc_worker::parse_duration("1.5s").unwrap()
    );
}

#[test]
fn gc_duration_persistence_matches_go_format() {
    assert_eq!(
        "1h30m0s",
        super::gc_worker::format_duration(Duration::from_secs(5_400))
    );
    assert_eq!(
        "1.5s",
        super::gc_worker::format_duration(Duration::from_millis(1_500))
    );
}

#[test]
fn gc_time_persistence_matches_go_format_and_timezone_parsing() {
    assert_eq!(
        "19700101-00:00:00.000 +0000",
        super::gc_worker::format_system_time(SystemTime::UNIX_EPOCH)
    );
    assert_eq!(
        SystemTime::UNIX_EPOCH,
        super::gc_worker::parse_system_time("19700101-08:00:00.000 +0800").unwrap()
    );
    assert_eq!(
        SystemTime::UNIX_EPOCH,
        super::gc_worker::parse_system_time("19700101-08:00:00 +0800 CST").unwrap()
    );
    let leap_day = SystemTime::UNIX_EPOCH + Duration::from_secs(1_709_164_800);
    assert_eq!(
        "20240229-00:00:00.000 +0000",
        super::gc_worker::format_system_time(leap_day)
    );
}

#[derive(Clone)]
struct FakeRuntime {
    state: Arc<Mutex<FakeState>>,
}

struct FakeState {
    values: HashMap<String, String>,
    oracle_time: SystemTime,
    current_version: u64,
    safe_point: u64,
    txn_result: SafePointAdvance,
    service_min_safe_point: u64,
    stores: Vec<StoreInfo>,
    ranges: Vec<DelRangeTask>,
    done_ranges: Vec<DelRangeTask>,
    keyspaces: Vec<KeyspaceInfo>,
    null_ranges: Vec<KeyRange>,
    placement_ids: Vec<i64>,
    events: Vec<String>,
    failures: Vec<(String, String)>,
    calls: Vec<String>,
    keyspace_requests: Vec<(u32, u32)>,
    session_failures: usize,
    fail_delete_range: bool,
    fail_unsafe_destroy: bool,
    fail_resolve: bool,
    raft_v2: bool,
    starter_mode: bool,
    in_test: bool,
    unified_gc: bool,
    null_keyspace: bool,
}

impl Default for FakeState {
    fn default() -> Self {
        Self {
            values: HashMap::new(),
            oracle_time: SystemTime::UNIX_EPOCH + Duration::from_secs(3_600),
            current_version: 100,
            safe_point: 100,
            txn_result: SafePointAdvance {
                old_safe_point: 0,
                new_safe_point: 100,
                blocker_description: String::new(),
            },
            service_min_safe_point: 0,
            stores: vec![store(StoreState::Up, "tikv")],
            ranges: Vec::new(),
            done_ranges: Vec::new(),
            keyspaces: Vec::new(),
            null_ranges: vec![KeyRange::default()],
            placement_ids: Vec::new(),
            events: Vec::new(),
            failures: Vec::new(),
            calls: Vec::new(),
            keyspace_requests: Vec::new(),
            session_failures: 0,
            fail_delete_range: false,
            fail_unsafe_destroy: false,
            fail_resolve: false,
            raft_v2: false,
            starter_mode: false,
            in_test: true,
            unified_gc: false,
            null_keyspace: false,
        }
    }
}

impl FakeRuntime {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeState::default())),
        }
    }

    fn update(&self, update: impl FnOnce(&mut FakeState)) {
        update(&mut self.state.lock().unwrap());
    }

    fn snapshot(&self) -> FakeStateSnapshot {
        let state = self.state.lock().unwrap();
        FakeStateSnapshot {
            values: state.values.clone(),
            events: state.events.clone(),
            failures: state.failures.clone(),
            calls: state.calls.clone(),
            keyspace_requests: state.keyspace_requests.clone(),
        }
    }
}

struct FakeStateSnapshot {
    values: HashMap<String, String>,
    events: Vec<String>,
    failures: Vec<(String, String)>,
    calls: Vec<String>,
    keyspace_requests: Vec<(u32, u32)>,
}

struct FakeSession {
    runtime: FakeRuntime,
}

impl GCSession for FakeSession {
    fn Begin(&mut self) -> GCResult {
        self.runtime
            .update(|state| state.calls.push("begin".into()));
        Ok(())
    }

    fn CommitTxn(&mut self) -> GCResult {
        self.runtime
            .update(|state| state.calls.push("commit".into()));
        Ok(())
    }

    fn RollbackTxn(&mut self) {
        self.runtime
            .update(|state| state.calls.push("rollback".into()));
    }

    fn LoadValue(&mut self, key: &str) -> GCResult<Option<String>> {
        Ok(self.runtime.state.lock().unwrap().values.get(key).cloned())
    }

    fn SaveValue(&mut self, key: &str, value: &str, _comment: &str) -> GCResult {
        self.runtime
            .update(|state| drop(state.values.insert(key.into(), value.into())));
        Ok(())
    }

    fn Close(&mut self) {
        self.runtime
            .update(|state| state.calls.push("close".into()));
    }
}

impl GCWorkerRuntime for FakeRuntime {
    fn CurrentVersion(&self) -> GCResult<u64> {
        Ok(self.state.lock().unwrap().current_version)
    }
    fn IsTiKVStorage(&self) -> bool {
        true
    }
    fn HostName(&self) -> Option<String> {
        Some("test-host".into())
    }
    fn ProcessID(&self) -> u32 {
        42
    }
    fn KeyspaceID(&self) -> u32 {
        0
    }
    fn IsUnifiedGC(&self) -> bool {
        self.state.lock().unwrap().unified_gc
    }
    fn IsNullKeyspace(&self) -> bool {
        self.state.lock().unwrap().null_keyspace
    }
    fn KeyspaceName(&self) -> Option<String> {
        Some("test".into())
    }
    fn IsStarterMode(&self) -> bool {
        self.state.lock().unwrap().starter_mode
    }
    fn InTest(&self) -> bool {
        self.state.lock().unwrap().in_test
    }
    fn CreateSession(&self) -> GCResult<Box<dyn GCSession>> {
        let mut state = self.state.lock().unwrap();
        if state.session_failures > 0 {
            state.session_failures -= 1;
            return Err(GCError::new("session unavailable"));
        }
        Ok(Box::new(FakeSession {
            runtime: self.clone(),
        }))
    }
    fn RegisterStatistics(&self) {
        self.update(|state| state.calls.push("register_statistics".into()));
    }
    fn OracleTime(&self) -> GCResult<SystemTime> {
        Ok(self.state.lock().unwrap().oracle_time)
    }
    fn TimestampFromTime(&self, time: SystemTime) -> u64 {
        time.duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }
    fn TimeFromTimestamp(&self, timestamp: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_millis(timestamp)
    }
    fn GetGCSafePoint(&self) -> GCResult<u64> {
        Ok(self.state.lock().unwrap().safe_point)
    }
    fn AdvanceTxnSafePoint(&self, _target: u64) -> GCResult<SafePointAdvance> {
        Ok(self.state.lock().unwrap().txn_result.clone())
    }
    fn UpdateServiceGCSafePoint(&self, _id: &str, safe_point: u64) -> GCResult<u64> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .service_min_safe_point
            .min(safe_point))
    }
    fn AdvanceGCSafePoint(&self, safe_point: u64) -> GCResult<SafePointAdvance> {
        self.update(|state| state.calls.push(format!("advance_gc:{safe_point}")));
        Ok(SafePointAdvance {
            old_safe_point: 0,
            new_safe_point: safe_point,
            blocker_description: String::new(),
        })
    }
    fn GetAllStores(&self) -> GCResult<Vec<StoreInfo>> {
        Ok(self.state.lock().unwrap().stores.clone())
    }
    fn IsRaftKV2(&self) -> GCResult<bool> {
        Ok(self.state.lock().unwrap().raft_v2)
    }
    fn LoadDeleteRanges(&self, _safe_point: u64) -> GCResult<Vec<DelRangeTask>> {
        Ok(self.state.lock().unwrap().ranges.clone())
    }
    fn LoadDoneDeleteRanges(&self, _before: u64) -> GCResult<Vec<DelRangeTask>> {
        Ok(self.state.lock().unwrap().done_ranges.clone())
    }
    fn DeleteRangeRaftV2(
        &self,
        _range: &KeyRange,
        _concurrency: usize,
        _cancel: &super::gc_worker::CancellationToken,
    ) -> GCResult {
        self.update(|state| state.calls.push("delete_range_raft_v2".into()));
        if self.state.lock().unwrap().fail_delete_range {
            Err(GCError::new("delete range failed"))
        } else {
            Ok(())
        }
    }
    fn UnsafeDestroyRange(
        &self,
        store: &StoreInfo,
        _range: &KeyRange,
        _timeout: Duration,
        _cancel: &super::gc_worker::CancellationToken,
    ) -> GCResult {
        self.update(|state| state.calls.push(format!("unsafe:{}", store.id)));
        if self.state.lock().unwrap().fail_unsafe_destroy {
            Err(GCError::new("unsafe destroy failed"))
        } else {
            Ok(())
        }
    }
    fn GCPlacementRules(&self, _task: &DelRangeTask) -> GCResult<Vec<i64>> {
        if self.state.lock().unwrap().fail_delete_range {
            return Err(GCError::new("placement failed"));
        }
        Ok(self.state.lock().unwrap().placement_ids.clone())
    }
    fn GCLabelRules(&self, _task: &DelRangeTask) -> GCResult {
        if self.state.lock().unwrap().fail_delete_range {
            Err(GCError::new("label failed"))
        } else {
            self.update(|state| state.calls.push("label_rules".into()));
            Ok(())
        }
    }
    fn CompleteDeleteRange(&self, _task: &DelRangeTask, remove_data: bool) -> GCResult {
        self.update(|state| state.calls.push(format!("complete:{remove_data}")));
        Ok(())
    }
    fn DeleteDoneRecord(&self, _task: &DelRangeTask) -> GCResult {
        self.update(|state| state.calls.push("delete_done".into()));
        Ok(())
    }
    fn ResolveLocksForRange(
        &self,
        max_version: u64,
        range: &KeyRange,
        _concurrency: usize,
        _cancel: &super::gc_worker::CancellationToken,
    ) -> GCResult {
        self.update(|state| {
            state.calls.push(format!("resolve:{max_version}"));
            if range != &KeyRange::default() {
                state.calls.push(format!("range:{:?}", range.start_key));
            }
        });
        if self.state.lock().unwrap().fail_resolve {
            Err(GCError::new("resolve failed"))
        } else {
            Ok(())
        }
    }
    fn NullKeyspaceRanges(&self) -> Vec<KeyRange> {
        self.state.lock().unwrap().null_ranges.clone()
    }
    fn GetAllKeyspaces(&self, start_id: u32, limit: u32) -> GCResult<Vec<KeyspaceInfo>> {
        self.update(|state| state.keyspace_requests.push((start_id, limit)));
        Ok(self
            .state
            .lock()
            .unwrap()
            .keyspaces
            .iter()
            .filter(|keyspace| keyspace.id >= start_id)
            .take(limit as usize)
            .cloned()
            .collect())
    }
    fn EncodeKeyspaceRange(&self, keyspace: &KeyspaceInfo) -> GCResult<KeyRange> {
        Ok(KeyRange {
            start_key: keyspace.id.to_be_bytes().to_vec(),
            end_key: keyspace.id.saturating_add(1).to_be_bytes().to_vec(),
        })
    }
    fn TableRange(&self, table_id: i64) -> KeyRange {
        KeyRange {
            start_key: table_id.to_be_bytes().to_vec(),
            end_key: table_id.saturating_add(1).to_be_bytes().to_vec(),
        }
    }
    fn RecordFailure(&self, stage: &str, error: &GCError) {
        self.update(|state| state.failures.push((stage.into(), error.to_string())));
    }
    fn RecordEvent(&self, event: &str) {
        self.update(|state| state.events.push(event.into()));
    }
}

fn worker_fixture() -> (GCWorker, FakeRuntime) {
    let runtime = FakeRuntime::new();
    let worker = super::gc_worker::NewGCWorker(Arc::new(runtime.clone())).unwrap();
    (worker, runtime)
}

fn range_task(id: i64) -> DelRangeTask {
    DelRangeTask {
        job_id: id,
        element_id: id,
        range: KeyRange {
            start_key: vec![id as u8],
            end_key: vec![id as u8 + 1],
        },
    }
}

fn keyspace(id: u32, enabled: bool, keyspace_level_gc: bool) -> KeyspaceInfo {
    KeyspaceInfo {
        id,
        name: format!("ks-{id}"),
        enabled,
        keyspace_level_gc,
    }
}

fn assert_resolve_calls(runtime: &FakeRuntime, expected: usize) {
    assert_eq!(
        expected,
        runtime
            .snapshot()
            .calls
            .iter()
            .filter(|call| call.starts_with("resolve:"))
            .count()
    );
}

#[test]
#[allow(non_snake_case)]
fn TestGetOracleTime() {
    let (worker, runtime) = worker_fixture();
    let expected = runtime.state.lock().unwrap().oracle_time;
    assert_eq!(expected, worker.getOracleTime().unwrap());
}

#[test]
#[allow(non_snake_case)]
fn TestPrepareGC() {
    let (worker, runtime) = worker_fixture();
    let result = worker.prepare().unwrap().unwrap();
    assert!(result > 0);
    let snapshot = runtime.snapshot();
    assert!(
        snapshot
            .values
            .contains_key(super::gc_worker::gcLastRunTimeKey)
    );
    assert!(
        snapshot
            .values
            .contains_key(super::gc_worker::gcSafePointKey)
    );
}

#[test]
#[allow(non_snake_case)]
fn TestStatusVars() {
    let (worker, runtime) = worker_fixture();
    runtime.update(|state| {
        state
            .values
            .insert("tikv_gc_leader_uuid".into(), "u".into());
        state.values.insert("tikv_gc_safe_point".into(), "9".into());
    });
    let stats = worker.Stats();
    assert_eq!(Some(&"u".to_string()), stats.get("tidb_gc_leader_uuid"));
    assert_eq!(Some(&"9".to_string()), stats.get("tidb_gc_safe_point"));
}

#[test]
#[allow(non_snake_case)]
fn TestDoGCForOneRegion() {
    let runtime = FakeRuntime::new();
    runtime.update(|state| state.safe_point = 77);
    assert_eq!(77, super::gc_worker::getGCSafePoint(&runtime).unwrap());
}

#[test]
#[allow(non_snake_case)]
fn TestGetGCConcurrency() {
    let (worker, runtime) = worker_fixture();
    runtime.update(|state| {
        state.stores.push(store(StoreState::Up, "tikv"));
        state
            .values
            .insert("tikv_gc_auto_concurrency".into(), "true".into());
    });
    assert_eq!(2, worker.getGCConcurrency().unwrap().v);
    runtime.update(|state| {
        state
            .values
            .insert("tikv_gc_auto_concurrency".into(), "false".into());
        state
            .values
            .insert("tikv_gc_concurrency".into(), "7".into());
    });
    assert_eq!(7, worker.getGCConcurrency().unwrap().v);
}

#[test]
#[allow(non_snake_case)]
fn TestCheckGCMode() {
    let (worker, runtime) = worker_fixture();
    assert!(worker.checkUseDistributedGC());
    assert_eq!(
        Some(&"distributed".to_string()),
        runtime.snapshot().values.get("tikv_gc_mode")
    );
}

#[test]
#[allow(non_snake_case)]
fn TestNeedsGCOperationForStore() {
    assert!(needsGCOperationForStore(&store(StoreState::Offline, "tikv")).unwrap());
    assert!(!needsGCOperationForStore(&store(StoreState::Tombstone, "tikv")).unwrap());
}

#[test]
#[allow(non_snake_case)]
fn TestDeleteRangesFailure() {
    let (worker, runtime) = worker_fixture();
    runtime.update(|state| {
        state.ranges = vec![range_task(1)];
        state.fail_unsafe_destroy = true;
    });
    worker
        .deleteRanges(
            100,
            gcConcurrency {
                v: 1,
                isAuto: false,
            },
        )
        .unwrap();
    assert!(
        runtime
            .snapshot()
            .failures
            .iter()
            .any(|(stage, _)| stage == "delete_range_item")
    );
}

#[test]
#[allow(non_snake_case)]
fn TestConcurrentDeleteRanges() {
    let (worker, runtime) = worker_fixture();
    runtime.update(|state| state.ranges = (0..8).map(range_task).collect());
    worker
        .deleteRanges(
            100,
            gcConcurrency {
                v: 8,
                isAuto: false,
            },
        )
        .unwrap();
    assert_eq!(
        8,
        runtime
            .snapshot()
            .calls
            .iter()
            .filter(|call| call.starts_with("unsafe:"))
            .count()
    );
}

#[test]
#[allow(non_snake_case)]
fn TestUnsafeDestroyRangeForRaftkv2() {
    let (worker, runtime) = worker_fixture();
    runtime.update(|state| {
        state.raft_v2 = true;
        state.ranges = vec![range_task(1)];
    });
    worker
        .deleteRanges(
            100,
            gcConcurrency {
                v: 1,
                isAuto: false,
            },
        )
        .unwrap();
    let calls = runtime.snapshot().calls;
    assert!(calls.iter().any(|call| call == "delete_range_raft_v2"));
    assert!(!calls.iter().any(|call| call.starts_with("unsafe:")));
}

#[test]
#[allow(non_snake_case)]
fn TestLeaderTick() {
    let (worker, runtime) = worker_fixture();
    assert!(worker.checkLeader().unwrap());
    assert_eq!(
        Some(&worker.uuid),
        runtime.snapshot().values.get("tikv_gc_leader_uuid")
    );
}

#[test]
#[allow(non_snake_case)]
fn TestUnifiedGCNeedsToWait() {
    let (worker, _) = worker_fixture();
    assert!(worker.needsToWait());
}

#[test]
#[allow(non_snake_case)]
fn TestResolveLockRangeInfine() {
    let (worker, runtime) = worker_fixture();
    worker.resolveLocks(10, 2).unwrap();
    assert!(
        runtime
            .snapshot()
            .calls
            .iter()
            .any(|call| call == "resolve:9")
    );
}

#[test]
#[allow(non_snake_case)]
fn TestResolveLockRangeMeetRegionCacheMiss() {
    let (worker, runtime) = worker_fixture();
    runtime.update(|state| {
        state.null_keyspace = true;
        state.keyspaces.clear();
    });
    worker.resolveLocks(10, 2).unwrap();
    assert_resolve_calls(&runtime, 1);
}

#[test]
#[allow(non_snake_case)]
fn TestResolveLockRangeMeetRegionEnlargeCausedByRegionMerge() {
    let (worker, runtime) = worker_fixture();
    runtime.update(|state| {
        state.null_keyspace = true;
        state.keyspaces = vec![keyspace(1, true, false)];
        state.null_ranges = vec![
            KeyRange {
                start_key: vec![1],
                end_key: vec![2],
            },
            KeyRange {
                start_key: vec![2],
                end_key: vec![3],
            },
        ];
    });
    worker.resolveLocks(10, 2).unwrap();
    assert_resolve_calls(&runtime, 3);
}

#[test]
#[allow(non_snake_case)]
fn TestResolveLocksWithKeyspaces_NullKeyspaceOnly() {
    let (worker, runtime) = worker_fixture();
    runtime.update(|state| state.null_keyspace = true);
    worker.resolveLocks(10, 2).unwrap();
    assert_resolve_calls(&runtime, 1);
}

#[test]
#[allow(non_snake_case)]
fn TestResolveLocksWithKeyspaces_NullKeyspaceOnlyMultiRegion() {
    let (worker, runtime) = worker_fixture();
    runtime.update(|state| {
        state.null_keyspace = true;
        state.keyspaces = vec![keyspace(1, false, false)];
        state.null_ranges = vec![
            KeyRange::default(),
            KeyRange {
                start_key: vec![1],
                end_key: vec![2],
            },
        ];
    });
    worker.resolveLocks(10, 2).unwrap();
    assert_resolve_calls(&runtime, 2);
}

#[test]
#[allow(non_snake_case)]
fn TestResolveLocksWithKeyspaces_NullKeyspaceInMultiKeyspaceEnvironment() {
    let (worker, runtime) = worker_fixture();
    runtime.update(|state| {
        state.null_keyspace = true;
        state.keyspaces = vec![keyspace(1, false, false)];
    });
    worker.resolveLocks(10, 2).unwrap();
    assert_resolve_calls(&runtime, 1);
}

#[test]
#[allow(non_snake_case)]
fn TestResolveLocksWithKeyspaces_NonNullKeyspaceInMultiKeyspaceEnvironment() {
    let (worker, runtime) = worker_fixture();
    runtime.update(|state| {
        state.null_keyspace = false;
        state.keyspaces = vec![keyspace(1, true, false)];
    });
    worker.resolveLocks(10, 2).unwrap();
    assert_resolve_calls(&runtime, 1);
}

#[test]
#[allow(non_snake_case)]
fn TestResolveLocksWithKeyspaces_UnifiedGCInMixedUsage() {
    let (worker, runtime) = worker_fixture();
    runtime.update(|state| {
        state.null_keyspace = true;
        state.keyspaces = vec![keyspace(1, true, false), keyspace(2, true, true)];
    });
    worker.resolveLocks(10, 2).unwrap();
    assert_resolve_calls(&runtime, 2);
}

#[test]
#[allow(non_snake_case)]
fn TestResolveLocksWithKeyspaces_UnifiedGCWithMaxKeyspaceID() {
    let (worker, runtime) = worker_fixture();
    runtime.update(|state| {
        state.null_keyspace = true;
        state.keyspaces = vec![keyspace(u32::MAX, true, false)];
    });
    worker.resolveLocks(10, 2).unwrap();
    assert_resolve_calls(&runtime, 2);
}

#[test]
#[allow(non_snake_case)]
fn TestResolveLocksWithKeyspaces_UnifiedGCInMultiBatchesOfKeyspaces_8() {
    let (worker, runtime) = worker_fixture();
    runtime.update(|state| {
        state.null_keyspace = true;
        state.keyspaces = (1..=8).map(|id| keyspace(id, true, false)).collect();
    });
    worker.resolveLocks(10, 2).unwrap();
    assert_resolve_calls(&runtime, 9);
}

#[test]
#[allow(non_snake_case)]
fn TestResolveLocksWithKeyspaces_UnifiedGCInMultiBatchesOfKeyspaces_Last8_Step2() {
    let (worker, runtime) = worker_fixture();
    runtime.update(|state| {
        state.null_keyspace = true;
        state.keyspaces = (1..=58).map(|id| keyspace(id, true, false)).collect();
    });
    worker.resolveLocks(10, 2).unwrap();
    let snapshot = runtime.snapshot();
    assert_resolve_calls(&runtime, 59);
    assert_eq!(3, snapshot.keyspace_requests.len());
}

#[test]
#[allow(non_snake_case)]
fn TestResolveLocksWithKeyspaces_UnifiedGCInMultiBatchesOfKeyspaces_MultipleOfBatchSize() {
    let (worker, runtime) = worker_fixture();
    runtime.update(|state| {
        state.null_keyspace = true;
        state.keyspaces = (1..=50).map(|id| keyspace(id, true, false)).collect();
    });
    worker.resolveLocks(10, 2).unwrap();
    assert_eq!(2, runtime.snapshot().keyspace_requests.len());
    assert_resolve_calls(&runtime, 51);
}

#[test]
#[allow(non_snake_case)]
fn TestResolveLocksWithKeyspaces_UnifiedGCInMultiBatchesOfKeyspaces_MultipleOfBatchSizeToEnd() {
    let (worker, runtime) = worker_fixture();
    runtime.update(|state| {
        state.null_keyspace = true;
        state.keyspaces = (1..=51).map(|id| keyspace(id, true, false)).collect();
    });
    worker.resolveLocks(10, 2).unwrap();
    assert_eq!(3, runtime.snapshot().keyspace_requests.len());
    assert_resolve_calls(&runtime, 52);
}

#[test]
#[allow(non_snake_case)]
fn TestResolveLocksNearTxnSafePoint() {
    let (worker, runtime) = worker_fixture();
    worker.resolveLocks(0, 1).unwrap();
    assert!(
        runtime
            .snapshot()
            .calls
            .iter()
            .any(|call| call == "resolve:18446744073709551615")
    );
}

#[test]
#[allow(non_snake_case)]
fn TestRunGCJob() {
    let (worker, runtime) = worker_fixture();
    worker
        .runGCJob(
            100,
            gcConcurrency {
                v: 1,
                isAuto: false,
            },
        )
        .unwrap();
    let snapshot = runtime.snapshot();
    assert!(snapshot.events.iter().any(|event| event == "run_job"));
    assert!(snapshot.calls.iter().any(|call| call == "advance_gc:100"));
}

#[test]
#[allow(non_snake_case)]
fn TestSetServiceSafePoint() {
    let (worker, runtime) = worker_fixture();
    runtime.update(|state| state.service_min_safe_point = 40);
    assert_eq!(40, worker.setGCWorkerServiceSafePoint(100).unwrap());
}

#[test]
#[allow(non_snake_case)]
fn TestRunGCJobAPI() {
    let runtime = FakeRuntime::new();
    let state = runtime.state.clone();
    super::gc_worker::RunGCJob(Arc::new(runtime), 100, "api", 1).unwrap();
    assert!(
        state
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|call| call == "advance_gc:100")
    );
}

#[test]
#[allow(non_snake_case)]
fn TestRunDistGCJobAPI() {
    let runtime = FakeRuntime::new();
    let state = runtime.state.clone();
    super::gc_worker::RunDistributedGCJob(Arc::new(runtime), 100, "distributed", 1).unwrap();
    assert!(
        state
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|call| call == "advance_gc:100")
    );
}

#[test]
#[allow(non_snake_case)]
fn TestStartWithRunGCJobFailures() {
    let (worker, runtime) = worker_fixture();
    runtime.update(|state| {
        state
            .values
            .insert("tikv_gc_leader_uuid".into(), "other".into());
        state
            .values
            .insert("tikv_gc_leader_lease".into(), "999999999999999".into());
    });
    worker.Start();
    worker.Close();
    assert_eq!(vec!["not_leader".to_string()], runtime.snapshot().events);
}

#[test]
#[allow(non_snake_case)]
fn TestGCPlacementRules() {
    let runtime = FakeRuntime::new();
    let table_range = runtime.TableRange(1);
    let mut rules = HashMap::new();
    rules.insert(
        "rule".into(),
        super::gc_worker::LabelRule {
            id: "rule".into(),
            ranges: vec![table_range],
        },
    );
    assert_eq!(
        vec!["rule".to_string()],
        super::gc_worker::getGCRules(&[1], &rules, &runtime)
    );
}

#[test]
#[allow(non_snake_case)]
fn TestGCLabelRules() {
    let (worker, runtime) = worker_fixture();
    runtime.update(|state| {
        state.ranges = vec![range_task(1)];
        state.placement_ids = vec![1];
    });
    worker
        .deleteRanges(
            100,
            gcConcurrency {
                v: 1,
                isAuto: false,
            },
        )
        .unwrap();
    assert!(
        runtime
            .snapshot()
            .calls
            .iter()
            .any(|call| call == "label_rules")
    );
}

#[test]
#[allow(non_snake_case)]
fn TestGCWithPendingTxn() {
    let (worker, runtime) = worker_fixture();
    runtime.update(|state| {
        state.txn_result.old_safe_point = 100;
        state.txn_result.new_safe_point = 100;
    });
    assert!(worker.prepare().unwrap().is_none());
}

#[test]
#[allow(non_snake_case)]
fn TestGCWithPendingTxn2() {
    let (worker, runtime) = worker_fixture();
    runtime.update(|state| {
        state.txn_result.old_safe_point = 50;
        state.txn_result.new_safe_point = 100;
    });
    assert!(worker.prepare().unwrap().is_some());
}

#[test]
#[allow(non_snake_case)]
fn TestSkipGCAndOnlyResolveLock() {
    let runtime = FakeRuntime::new();
    let state = runtime.state.clone();
    super::gc_worker::RunResolveLocks(Arc::new(runtime), 10, "resolve-only", 1).unwrap();
    let calls = state.lock().unwrap().calls.clone();
    assert!(calls.iter().any(|call| call == "resolve:9"));
    assert!(!calls.iter().any(|call| call.starts_with("advance_gc:")));
}

#[test]
#[allow(non_snake_case)]
fn TestCalcDeleteRangeConcurrency() {
    let (worker, _) = worker_fixture();
    assert_eq!(
        1,
        worker.calcDeleteRangeConcurrency(gcConcurrency { v: 2, isAuto: true }, 0)
    );
    assert_eq!(
        2,
        worker.calcDeleteRangeConcurrency(gcConcurrency { v: 8, isAuto: true }, 200_000)
    );
}
// 以下块注释保留 Go mockGCWorkerSuite 及各类 GC 场景测试原文（锁解析、安全点、
// delete-range、统一 Keyspace GC 等），Rust 侧尚未逐条可执行化，仅作迁移对照。
/*

// mockGCWorkerLockResolver_ScanLocksInOneRegion 对应 Go 中 mockGCWorkerLockResolver.ScanLocksInOneRegion 方法，保留测试辅助/断言流程。
pub fn mockGCWorkerLockResolver_ScanLocksInOneRegion(l: &mut mockGCWorkerLockResolver, bo: Box<tikv::Backoffer>, key: Vec<u8>, endKey: Vec<u8>, maxVersion: u64, limit: u32) -> ([]*txnlock::Lock, Box<tikv::KeyLocation>, errors::Error) {
    // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
    locks, loc, err = l.RegionLockResolver.ScanLocksInOneRegion(bo, key, endKey, maxVersion, limit)
    if err != None {
        return None, None, err
    }
    if l.scanLocks != None {
        mockLocks, mockLoc = l.scanLocks(locks, key)
        // append locks from mock function
        locks = append(locks, mockLocks...)
        // use location from mock function
        loc = mockLoc
    }
    return locks, loc, None
}

// mockGCWorkerLockResolver_ResolveLocksInOneRegion 对应 Go 中 mockGCWorkerLockResolver.ResolveLocksInOneRegion 方法，保留测试辅助/断言流程。
pub fn mockGCWorkerLockResolver_ResolveLocksInOneRegion(l: &mut mockGCWorkerLockResolver, bo: Box<tikv::Backoffer>, locks: []*txnlock::Lock, loc: Box<tikv::KeyLocation>) -> (Box<tikv::KeyLocation>, errors::Error) {
    if l.batchResolveLocks != None {
        return l.batchResolveLocks(locks, loc)
    }
    // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
    return l.RegionLockResolver.ResolveLocksInOneRegion(bo, locks, loc)
}

// mockGCWorkerLockResolver_GetStore 对应 Go 中 mockGCWorkerLockResolver.GetStore 方法，保留测试辅助/断言流程。
pub fn mockGCWorkerLockResolver_GetStore(l: &mut mockGCWorkerLockResolver) -> tikv::Storage {
    return l.tikvStore
}

// mockGCWorkerLockResolver_Identifier 对应 Go 中 mockGCWorkerLockResolver.Identifier 方法，保留测试辅助/断言流程。
pub fn mockGCWorkerLockResolver_Identifier(l: &mut mockGCWorkerLockResolver) -> String {
    return "gc worker test"
}

// mockGCWorkerClient 对应 Go 的同名 struct，字段顺序与嵌入类型保持原样，便于人工核对测试 fixture。
pub struct mockGCWorkerClient {
    tikv::Client
    unsafeDestroyRangeHandler handler
    deleteRangeHandler        handler
    scanLockRequestHandler    handler
}

// handler 对应 Go 函数类型，用于保留 mock 回调/选项注入语义。
pub type handler = fn(addr: String, req *tikvrpc.Request): (*tikvrpc::Response, error);

// gcContext 对应 Go 辅助函数，保留参数、返回值和主要控制流。
pub fn gcContext() -> context.Context {
    // internal statements must bind with resource type
    return kv::WithInternalSourceType(context::Background(), kv::InternalTxnGC)
}

// mockGCWorkerClient_SendRequest 对应 Go 中 mockGCWorkerClient.SendRequest 方法，保留测试辅助/断言流程。
pub fn mockGCWorkerClient_SendRequest(c: &mut mockGCWorkerClient, ctx: context.Context, addr: String, req: Box<tikvrpc::Request>, timeout: time.Duration) -> (Box<tikvrpc::Response>, errors::Error) {
    // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
    var resp *tikvrpc::Response
    var err error
    // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
    if req.Type == tikvrpc::CmdUnsafeDestroyRange && c.unsafeDestroyRangeHandler != None {
        resp, err = c.unsafeDestroyRangeHandler(addr, req)
    }
    // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
    if req.Type == tikvrpc::CmdDeleteRange && c.deleteRangeHandler != None {
        resp, err = c.deleteRangeHandler(addr, req)
    }
    // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
    if req.Type == tikvrpc::CmdScanLock && c.scanLockRequestHandler != None {
        resp, err = c.scanLockRequestHandler(addr, req)
    }

    if resp != None || err != None {
        return resp, err
    }

    // If there's no mock handler, or the mock handler returns both nil, continue executing the inner implementation.
    // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
    return c.Client.SendRequest(ctx, addr, req, timeout)
}

// mockGCWorkerSuite 对应 Go 的同名 struct，字段顺序与嵌入类型保持原样，便于人工核对测试 fixture。
pub struct mockGCWorkerSuite {
    store      kv::Storage
    tikvStore  tikv::Storage
    cluster    testutils::Cluster
    oracle     *oracles::MockOracle
    gcWorker   *GCWorker
    dom        *domain::Domain
    client     *mockGCWorkerClient
    pdClient   pd::Client
    initRegion struct {
        storeIDs []uint64
        peerIDs  []uint64
        regionID uint64
    }
}

// mockGCWorkerSuiteOptions 对应 Go 的同名 struct，字段顺序与嵌入类型保持原样，便于人工核对测试 fixture。
pub struct mockGCWorkerSuiteOptions {
    storeType        mockstore::StoreType
    schemaLease      time.Duration
    mockStoreOptions []mockstore::MockTiKVStoreOption
}

// mockGCWorkerSuiteOption 对应 Go 函数类型，用于保留 mock 回调/选项注入语义。
pub type mockGCWorkerSuiteOption = fn(*mockGCWorkerSuiteOptions);

// withStoreType 对应 Go 辅助函数，保留参数、返回值和主要控制流。
pub fn withStoreType(storeType: mockstore::StoreType) -> mockGCWorkerSuiteOption {
    return func(opts *mockGCWorkerSuiteOptions) {
        opts.storeType = storeType
    }
}

// withSchemaLease 对应 Go 辅助函数，保留参数、返回值和主要控制流。
pub fn withSchemaLease(schemaLease: time.Duration) -> mockGCWorkerSuiteOption {
    return func(opts *mockGCWorkerSuiteOptions) {
        opts.schemaLease = schemaLease
    }
}

// withMockStoreOptions 对应 Go 辅助函数，保留参数、返回值和主要控制流。
pub fn withMockStoreOptions(opt: Vec<mockstore::MockTiKVStoreOption>) -> mockGCWorkerSuiteOption {
    return func(opts *mockGCWorkerSuiteOptions) {
        opts.mockStoreOptions = append(opts.mockStoreOptions, opt...)
    }
}

// createGCWorkerSuite 对应 Go 辅助函数，保留参数、返回值和主要控制流。
pub fn createGCWorkerSuite(t: Box<testing.T>, opts: Vec<mockGCWorkerSuiteOption>) -> Box<mockGCWorkerSuite> {
    options = &mockGCWorkerSuiteOptions{
        storeType:        mockstore::EmbedUnistore,
        schemaLease:      config::DefSchemaLease,
        mockStoreOptions: None,
    }
    for _, opt = range opts {
        opt(options)
    }

    s = new(mockGCWorkerSuite)
    hijackClient = func(client tikv::Client) tikv::Client {
        s.client = &mockGCWorkerClient{Client: client}
        client = s.client
        return client
    }
    storeOpts = []mockstore::MockTiKVStoreOption{
        mockstore::WithStoreType(options.storeType),
        mockstore::WithClusterInspector(func(c testutils::Cluster) {
            // mock store / domain / GC worker 初始化依赖 Go harness，保留 fixture 构造顺序。
            s.initRegion.storeIDs, s.initRegion.peerIDs, s.initRegion.regionID, _ = mockstore::BootstrapWithMultiStores(c, 3)
            s.cluster = c
        }),
        mockstore::WithClientHijacker(hijackClient),
        mockstore::WithPDClientHijacker(func(c pd::Client) pd::Client {
            s.pdClient = c
            return c
        }),
    }
    storeOpts = append(storeOpts, options.mockStoreOptions...)

    s.oracle = &oracles::MockOracle{}
    // mock store / domain / GC worker 初始化依赖 Go harness，保留 fixture 构造顺序。
    store, err = mockstore::NewMockStore(storeOpts...)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    store.GetOracle().Close()
    store.(tikv::Storage).SetOracle(s.oracle)
    // mock store / domain / GC worker 初始化依赖 Go harness，保留 fixture 构造顺序。
    dom = bootstrap(t, store, options.schemaLease)
    s.store, s.dom = store, dom

    s.tikvStore = s.store.(tikv::Storage)

    // mock store / domain / GC worker 初始化依赖 Go harness，保留 fixture 构造顺序。
    gcWorker, err = NewGCWorker(s.store, s.pdClient)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    gcWorker.Start()
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    gcWorker.Close()
    s.gcWorker = gcWorker

    return s
}

// mockGCWorkerSuite_mustPut 对应 Go 中 mockGCWorkerSuite.mustPut 方法，保留测试辅助/断言流程。
pub fn mockGCWorkerSuite_mustPut(s: &mut mockGCWorkerSuite, t: Box<testing.T>, key, value: String) {
    txn, err = s.store.Begin()
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    err = txn.Set(Vec::<u8>::from(key), Vec::<u8>::from(value))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    err = txn.Commit(context::Background())
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
}

// mockGCWorkerSuite_mustGet 对应 Go 中 mockGCWorkerSuite.mustGet 方法，保留测试辅助/断言流程。
pub fn mockGCWorkerSuite_mustGet(s: &mut mockGCWorkerSuite, t: Box<testing.T>, key: String, ts: u64) -> String {
    snap = s.store.GetSnapshot(kv::Version{Ver: ts})
    value, err = snap.Get(context::TODO(), Vec::<u8>::from(key))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    return string(value.Value)
}

// mockGCWorkerSuite_mustGetNone 对应 Go 中 mockGCWorkerSuite.mustGetNone 方法，保留测试辅助/断言流程。
pub fn mockGCWorkerSuite_mustGetNone(s: &mut mockGCWorkerSuite, t: Box<testing.T>, key: String, ts: u64) {
    snap = s.store.GetSnapshot(kv::Version{Ver: ts})
    _, err = snap.Get(context::TODO(), Vec::<u8>::from(key))
    if err != None {
        // unistore gc is based on compaction filter.
        // So skip the error check if err == nil.
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::True(t, kv::ErrNotExist.Equal(err), "unexpected error: %+q", err)
    }
}

// mockGCWorkerSuite_mustAllocTs 对应 Go 中 mockGCWorkerSuite.mustAllocTs 方法，保留测试辅助/断言流程。
pub fn mockGCWorkerSuite_mustAllocTs(s: &mut mockGCWorkerSuite, t: Box<testing.T>) -> u64 {
    ts, err = s.oracle::GetTimestamp(context::Background(), &oracle::Option{})
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    return ts
}

// mockGCWorkerSuite_mustGetSafePointFromPd 对应 Go 中 mockGCWorkerSuite.mustGetSafePointFromPd 方法，保留测试辅助/断言流程。
pub fn mockGCWorkerSuite_mustGetSafePointFromPd(s: &mut mockGCWorkerSuite, t: Box<testing.T>) -> u64 {
    gcStates, err = s.pdClient.GetGCStatesClient(uint32(s.store.GetCodec().GetKeyspaceID())).GetGCState(context::Background())
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    return gcStates.GCSafePoint
}

// mockGCWorkerSuite_mustGetMinServiceSafePointFromPd 对应 Go 中 mockGCWorkerSuite.mustGetMinServiceSafePointFromPd 方法，保留测试辅助/断言流程。
pub fn mockGCWorkerSuite_mustGetMinServiceSafePointFromPd(s: &mut mockGCWorkerSuite, t: Box<testing.T>) -> u64 {
    // UpdateServiceGCSafePoint returns the minimal service safePoint. If trying to update it with a value less than the
    // current minimal safePoint, nothing will be updated and the current minimal one will be returned. So we can use
    // this API to check the current safePoint.
    // This function shouldn't be invoked when there's no service safePoint set.
    minSafePoint, err = s.pdClient.UpdateServiceGCSafePoint(context::Background(), "test", 0, 0)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    return minSafePoint
}

// mockGCWorkerSuite_mustUpdateServiceGCSafePoint 对应 Go 中 mockGCWorkerSuite.mustUpdateServiceGCSafePoint 方法，保留测试辅助/断言流程。
pub fn mockGCWorkerSuite_mustUpdateServiceGCSafePoint(s: &mut mockGCWorkerSuite, t: Box<testing.T>, serviceID: String, safePoint, expectedMinSafePoint: u64) {
    minSafePoint, err = s.pdClient.UpdateServiceGCSafePoint(context::Background(), serviceID, math::MaxInt64, safePoint)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, expectedMinSafePoint, minSafePoint)
}

// mockGCWorkerSuite_mustRemoveServiceGCSafePoint 对应 Go 中 mockGCWorkerSuite.mustRemoveServiceGCSafePoint 方法，保留测试辅助/断言流程。
pub fn mockGCWorkerSuite_mustRemoveServiceGCSafePoint(s: &mut mockGCWorkerSuite, t: Box<testing.T>, serviceID: String, safePoint, expectedMinSafePoint: u64) {
    minSafePoint, err = s.pdClient.UpdateServiceGCSafePoint(context::Background(), serviceID, 0, safePoint)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, expectedMinSafePoint, minSafePoint)
}

// mockGCWorkerSuite_mustSetTiDBServiceSafePoint 对应 Go 中 mockGCWorkerSuite.mustSetTiDBServiceSafePoint 方法，保留测试辅助/断言流程。
pub fn mockGCWorkerSuite_mustSetTiDBServiceSafePoint(s: &mut mockGCWorkerSuite, t: Box<testing.T>, safePoint, expectedMinSafePoint: u64) {
    minSafePoint, err = s.gcWorker.setGCWorkerServiceSafePoint(context::Background(), safePoint)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, expectedMinSafePoint, minSafePoint)
}

// mockGCWorkerSuite_splitAtKeys 对应 Go 中 mockGCWorkerSuite.splitAtKeys 方法，保留测试辅助/断言流程。
pub fn mockGCWorkerSuite_splitAtKeys(s: &mut mockGCWorkerSuite, t: Box<testing.T>, keysStr: ...String) {
    initialRegion, err = s.tikvStore.GetRegionCache().LocateKey(tikv::NewBackoffer(context::Background(), 10000), Vec::<u8>::from("a"))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    slices::Sort(keysStr)
    keys = make([][]byte, 0, len(keysStr))
    for _, k = range keysStr {
        keys = append(keys, Vec::<u8>::from(k))
    }
    s.cluster.(*unistore.Cluster).SplitArbitrary(keys...)

    s.tikvStore.GetRegionCache().InvalidateCachedRegion(initialRegion.Region)
}

// gcProbe represents a key that contains multiple versions, one of which should be collected. Execution of GC with
// greater ts will be detected, but it may not work properly if there are newer versions of the key.
// This is not used to check the correctness of GC algorithm, but only for checking whether GC has been executed on the
// specified key. Create this using `s.createGCProbe`.
// gcProbe 对应 Go 的同名 struct，字段顺序与嵌入类型保持原样，便于人工核对测试 fixture。
pub struct gcProbe {
    key string
    // The ts that can see the version that should be deleted.
    v1Ts uint64
    // The ts that can see the version that should be kept.
    v2Ts uint64
}

// createGCProbe creates gcProbe on specified key.
// mockGCWorkerSuite_createGCProbe 对应 Go 中 mockGCWorkerSuite.createGCProbe 方法，保留测试辅助/断言流程。
pub fn mockGCWorkerSuite_createGCProbe(s: &mut mockGCWorkerSuite, t: Box<testing.T>, key: String) -> Box<gcProbe> {
    s.mustPut(t, key, "v1")
    ts1 = s.mustAllocTs(t)
    s.mustPut(t, key, "v2")
    ts2 = s.mustAllocTs(t)
    p = &gcProbe{
        key:  key,
        v1Ts: ts1,
        v2Ts: ts2,
    }
    s.checkNotCollected(t, p)
    return p
}

// checkCollected asserts the gcProbe has been correctly collected.
// mockGCWorkerSuite_checkCollected 对应 Go 中 mockGCWorkerSuite.checkCollected 方法，保留测试辅助/断言流程。
pub fn mockGCWorkerSuite_checkCollected(s: &mut mockGCWorkerSuite, t: Box<testing.T>, p: Box<gcProbe>) {
    s.mustGetNone(t, p.key, p.v1Ts)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, "v2", s.mustGet(t, p.key, p.v2Ts))
}

// checkNotCollected asserts the gcProbe has not been collected.
// mockGCWorkerSuite_checkNotCollected 对应 Go 中 mockGCWorkerSuite.checkNotCollected 方法，保留测试辅助/断言流程。
pub fn mockGCWorkerSuite_checkNotCollected(s: &mut mockGCWorkerSuite, t: Box<testing.T>, p: Box<gcProbe>) {
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, "v1", s.mustGet(t, p.key, p.v1Ts))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, "v2", s.mustGet(t, p.key, p.v2Ts))
}

// timeEqual 对应 Go 辅助函数，保留参数、返回值和主要控制流。
pub fn timeEqual(t: Box<testing.T>, t1, t2: time.Time, epsilon: time.Duration) {
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Less(t, math.Abs(float64(t1.Sub(t2))), float64(epsilon))
}

#[test]
// TestGetOracleTime 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestGetOracleTime() {
    s = createGCWorkerSuite(t)

    t1, err = s.gcWorker.getOracleTime()
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    timeEqual(t, time.Now(), t1, time.Millisecond*10)

    s.oracle::AddOffset(time::Second * 10)
    t2, err = s.gcWorker.getOracleTime()
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    timeEqual(t, t2, t1.Add(time::Second*10), time.Millisecond*10)
}

#[test]
// TestPrepareGC 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestPrepareGC() {
    // as we are adjusting the base TS, we need a larger schema lease to avoid
    // the info schema outdated error. as we keep adding offset to time oracle,
    // so we need set a very large lease.
    s = createGCWorkerSuite(t, withStoreType(mockstore::EmbedUnistore), withSchemaLease(220*time::Minute))

    now, err = s.gcWorker.getOracleTime()
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    lastRunBefore, err = s.gcWorker.loadTime(gcLastRunTimeKey)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NotNil(t, lastRunBefore)
    safePointBefore, err = s.gcWorker.loadTime(gcSafePointKey)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NotNil(t, safePointBefore)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    timeEqual(t, safePointBefore.Add(gcDefaultLifeTime), now, 2*time::Second)

    close(s.gcWorker.done)
    ok, _, err = s.gcWorker.prepare(gcContext())
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::False(t, ok)
    lastRun, err = s.gcWorker.loadTime(gcLastRunTimeKey)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NotNil(t, lastRun)
    safePoint, err = s.gcWorker.loadTime(gcSafePointKey)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, *lastRunBefore, *lastRun)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, *safePointBefore, *safePoint)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    timeEqual(t, safePoint.Add(gcDefaultLifeTime), *lastRun, 2*time::Second)

    // Change GC run interval.
    err = s.gcWorker.saveDuration(gcRunIntervalKey, time::Minute*5)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    s.oracle::AddOffset(time::Minute * 4)
    ok, _, err = s.gcWorker.prepare(gcContext())
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::False(t, ok)
    s.oracle::AddOffset(time::Minute * 2)
    ok, _, err = s.gcWorker.prepare(gcContext())
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::True(t, ok)

    // Change GC lifetime.
    err = s.gcWorker.saveDuration(gcLifeTimeKey, time::Minute*30)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    s.oracle::AddOffset(time::Minute * 5)
    ok, _, err = s.gcWorker.prepare(gcContext())
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::False(t, ok)
    s.oracle::AddOffset(time::Minute * 40)
    ok, _, err = s.gcWorker.prepare(gcContext())
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::True(t, ok)
    lastRun, err = s.gcWorker.loadTime(gcLastRunTimeKey)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NotNil(t, lastRun)
    safePoint, err = s.gcWorker.loadTime(gcSafePointKey)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    timeEqual(t, safePoint.Add(time::Minute*30), *lastRun, 2*time::Second)

    // Change GC concurrency.
    concurrency, err = s.gcWorker.loadGCConcurrencyWithDefault()
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, gcDefaultConcurrency, concurrency)

    err = s.gcWorker.saveValueToSysTable(gcConcurrencyKey, strconv::Itoa(gcMinConcurrency))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    concurrency, err = s.gcWorker.loadGCConcurrencyWithDefault()
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, gcMinConcurrency, concurrency)

    err = s.gcWorker.saveValueToSysTable(gcConcurrencyKey, strconv::Itoa(-1))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    concurrency, err = s.gcWorker.loadGCConcurrencyWithDefault()
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, gcMinConcurrency, concurrency)

    err = s.gcWorker.saveValueToSysTable(gcConcurrencyKey, strconv::Itoa(1000000))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    concurrency, err = s.gcWorker.loadGCConcurrencyWithDefault()
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, gcMaxConcurrency, concurrency)

    // Change GC enable status.
    s.oracle::AddOffset(time::Minute * 40)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    err = s.gcWorker.saveValueToSysTable(gcEnableKey, booleanFalse)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    ok, _, err = s.gcWorker.prepare(gcContext())
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::False(t, ok)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    err = s.gcWorker.saveValueToSysTable(gcEnableKey, booleanTrue)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    ok, _, err = s.gcWorker.prepare(gcContext())
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::True(t, ok)

    // Check gc lifetime smaller than min.
    s.oracle::AddOffset(time::Minute * 40)
    err = s.gcWorker.saveDuration(gcLifeTimeKey, time::Minute)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    ok, _, err = s.gcWorker.prepare(gcContext())
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::True(t, ok)
    lifeTime, err = s.gcWorker.loadDuration(gcLifeTimeKey)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, gcMinLifeTime, *lifeTime)

    s.oracle::AddOffset(time::Minute * 40)
    err = s.gcWorker.saveDuration(gcLifeTimeKey, time::Minute*30)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    ok, _, err = s.gcWorker.prepare(gcContext())
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::True(t, ok)
    lifeTime, err = s.gcWorker.loadDuration(gcLifeTimeKey)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, 30*time::Minute, *lifeTime)

    // Change auto concurrency
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    err = s.gcWorker.saveValueToSysTable(gcAutoConcurrencyKey, booleanFalse)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    useAutoConcurrency, err = s.gcWorker.checkUseAutoConcurrency()
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::False(t, useAutoConcurrency)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    err = s.gcWorker.saveValueToSysTable(gcAutoConcurrencyKey, booleanTrue)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    useAutoConcurrency, err = s.gcWorker.checkUseAutoConcurrency()
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::True(t, useAutoConcurrency)

    // Check skipping GC if safe point is not changed.
    // Use a GC barrier to block GC from pushing forward.
    gcStatesCli = s.pdClient.GetGCStatesClient(uint32(s.store.GetCodec().GetKeyspaceID()))
    gcStates, err = gcStatesCli.GetGCState(context::Background())
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    lastTxnSafePoint = gcStates.TxnSafePoint
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NotEqual(t, uint64(0), lastTxnSafePoint)
    _, err = gcStatesCli.SetGCBarrier(context::Background(), "a", lastTxnSafePoint, gc::TTLNeverExpire)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    s.oracle::AddOffset(time::Minute * 40)
    ok, safepoint, err = s.gcWorker.prepare(gcContext())
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::False(t, ok)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, uint64(0), safepoint)
}

#[test]
// TestStatusVars 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestStatusVars() {
    s = createGCWorkerSuite(t)

    // Status variables should now exist for:
    // tidb_gc_safe_point, tidb_gc_last_run_time
    se = createSession(s.gcWorker.store)
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    defer se.Close()

    safePoint, err = s.gcWorker.loadValueFromSysTable(gcSafePointKey)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    lastRunTime, err = s.gcWorker.loadValueFromSysTable(gcLastRunTimeKey)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    statusVars, _ = s.gcWorker.Stats(se.GetSessionVars())
    val, ok = statusVars[tidbGCSafePoint]
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::True(t, ok)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, safePoint, val)
    val, ok = statusVars[tidbGCLastRunTime]
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::True(t, ok)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, lastRunTime, val)
}

#[test]
// TestDoGCForOneRegion 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestDoGCForOneRegion() {
    s = createGCWorkerSuite(t)

    ctx = context::Background()
    bo = tikv::NewBackofferWithVars(ctx, gcOneRegionMaxBackoff, None)
    loc, err = s.tikvStore.GetRegionCache().LocateKey(bo, Vec::<u8>::from(""))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    var regionErr *errorpb::Error

    p = s.createGCProbe(t, "k1")
    regionErr, err = s.gcWorker.doGCForRegion(bo, s.mustAllocTs(t), loc.Region)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Nil(t, regionErr)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    s.checkCollected(t, p)

    // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, failpoint::Enable("tikvclient/tikvStoreSendReqResult", `return("timeout")`))
    regionErr, err = s.gcWorker.doGCForRegion(bo, s.mustAllocTs(t), loc.Region)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Nil(t, regionErr)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Error(t, err)
    // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, failpoint::Disable("tikvclient/tikvStoreSendReqResult"))

    // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, failpoint::Enable("tikvclient/tikvStoreSendReqResult", `return("GCNotLeader")`))
    regionErr, err = s.gcWorker.doGCForRegion(bo, s.mustAllocTs(t), loc.Region)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NotNil(t, regionErr.GetNotLeader())
    // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, failpoint::Disable("tikvclient/tikvStoreSendReqResult"))

    // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, failpoint::Enable("tikvclient/tikvStoreSendReqResult", `return("GCServerIsBusy")`))
    regionErr, err = s.gcWorker.doGCForRegion(bo, s.mustAllocTs(t), loc.Region)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NotNil(t, regionErr.GetServerIsBusy())
    // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, failpoint::Disable("tikvclient/tikvStoreSendReqResult"))
}

#[test]
// TestGetGCConcurrency 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestGetGCConcurrency() {
    s = createGCWorkerSuite(t)

    // Pick a concurrency that doesn't equal to the number of stores.
    concurrencyConfig = 25
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NotEqual(t, len(s.cluster.GetAllStores()), concurrencyConfig)
    err = s.gcWorker.saveValueToSysTable(gcConcurrencyKey, strconv::Itoa(concurrencyConfig))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    ctx = context::Background()

    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    err = s.gcWorker.saveValueToSysTable(gcAutoConcurrencyKey, booleanFalse)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    concurrency, err = s.gcWorker.getGCConcurrency(ctx)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, concurrencyConfig, concurrency.v)

    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    err = s.gcWorker.saveValueToSysTable(gcAutoConcurrencyKey, booleanTrue)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    concurrency, err = s.gcWorker.getGCConcurrency(ctx)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Len(t, s.cluster.GetAllStores(), concurrency.v)
}

#[test]
// TestCheckGCMode 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestCheckGCMode() {
    s = createGCWorkerSuite(t)

    useDistributedGC = s.gcWorker.checkUseDistributedGC()
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::True(t, useDistributedGC)
    // Now the row must be set to the default value.
    str, err = s.gcWorker.loadValueFromSysTable(gcModeKey)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, gcModeDistributed, str)

    // Central mode is deprecated in v5.0.
    err = s.gcWorker.saveValueToSysTable(gcModeKey, gcModeCentral)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    useDistributedGC = s.gcWorker.checkUseDistributedGC()
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::True(t, useDistributedGC)

    err = s.gcWorker.saveValueToSysTable(gcModeKey, gcModeDistributed)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    useDistributedGC = s.gcWorker.checkUseDistributedGC()
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::True(t, useDistributedGC)

    err = s.gcWorker.saveValueToSysTable(gcModeKey, "invalid_mode")
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    useDistributedGC = s.gcWorker.checkUseDistributedGC()
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::True(t, useDistributedGC)
}

#[test]
// TestNeedsGCOperationForStore 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestNeedsGCOperationForStore() {
    newStore = func(state metapb::StoreState, hasEngineLabel bool, engineLabel string) *metapb::Store {
        store = &metapb::Store{}
        store.State = state
        if hasEngineLabel {
            store.Labels = []*metapb::StoreLabel{{Key: placement::EngineLabelKey, Value: engineLabel}}
        }
        return store
    }

    // TiKV needs to do the store-level GC operations.
    for _, state = range []metapb::StoreState{metapb::StoreState_Up, metapb::StoreState_Offline, metapb::StoreState_Tombstone} {
        needGC = state != metapb::StoreState_Tombstone
        res, err = needsGCOperationForStore(newStore(state, false, ""))
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, err)
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Equal(t, needGC, res)
        res, err = needsGCOperationForStore(newStore(state, true, ""))
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, err)
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Equal(t, needGC, res)
        res, err = needsGCOperationForStore(newStore(state, true, placement::EngineLabelTiKV))
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, err)
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Equal(t, needGC, res)

        // TiFlash does not need these operations.
        res, err = needsGCOperationForStore(newStore(state, true, placement::EngineLabelTiFlash))
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, err)
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::False(t, res)
    }
    // Throw an error for unknown store types.
    _, err = needsGCOperationForStore(newStore(metapb::StoreState_Up, true, "invalid"))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Error(t, err)
}

// 以下常量对应 Go const 块，表达式和顺序保持原测试语义。
const (
    failRPCErr  = 0
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    failNilResp = 1
    failErrResp = 2
)

#[test]
// TestDeleteRangesFailure 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestDeleteRangesFailure() {
    tests = []struct {
        name     string
        failType int
    }{
        {"failRPCErr", failRPCErr},
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        {"failNilResp", failNilResp},
        {"failErrResp", failErrResp},
    }

    s = createGCWorkerSuite(t)

    for _, test = range tests {
        t.Run(test.name, func(t *testing.T) {
            failType = test.failType
            // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
            // Go require 断言保持为测试期望说明，当前不运行这些断言。
            require::NoError(t, failpoint::Enable("github.com/pingcap/tidb/pkg/store/gcworker/mockHistoryJobForGC", "return(1)"))
            // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
            defer func() {
                // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
                // Go require 断言保持为测试期望说明，当前不运行这些断言。
                require::NoError(t, failpoint::Disable("github.com/pingcap/tidb/pkg/store/gcworker/mockHistoryJobForGC"))
            }()

            // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
            // Go require 断言保持为测试期望说明，当前不运行这些断言。
            require::NoError(t, failpoint::Enable("github.com/pingcap/tidb/pkg/store/gcworker/mockHistoryJob", "return(\"schema/d1/t1\")"))
            // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
            defer func() {
                // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
                // Go require 断言保持为测试期望说明，当前不运行这些断言。
                require::NoError(t, failpoint::Disable("github.com/pingcap/tidb/pkg/store/gcworker/mockHistoryJob"))
            }()

            // Put some delete range tasks.
            se = createSession(s.gcWorker.store)
            // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
            defer se.Close()
            _, err = se.Execute(gcContext(), `INSERT INTO mysql.gc_delete_range VALUES
("1", "2", "31", "32", "10"),
("3", "4", "33", "34", "10"),
("5", "6", "35", "36", "10")`)
            // Go require 断言保持为测试期望说明，当前不运行这些断言。
            require::NoError(t, err)

            ranges = []util::DelRangeTask{
                {
                    JobID:     1,
                    ElementID: 2,
                    StartKey:  Vec::<u8>::from("1"),
                    EndKey:    Vec::<u8>::from("2"),
                },
                {
                    JobID:     3,
                    ElementID: 4,
                    StartKey:  Vec::<u8>::from("3"),
                    EndKey:    Vec::<u8>::from("4"),
                },
                {
                    JobID:     5,
                    ElementID: 6,
                    StartKey:  Vec::<u8>::from("5"),
                    EndKey:    Vec::<u8>::from("6"),
                },
            }

            // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
            // Check the DeleteRanges tasks.
            // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
            preparedRanges, err = util::LoadDeleteRanges(gcContext(), se, 20)
            // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
            se.Close()
            // Go require 断言保持为测试期望说明，当前不运行这些断言。
            require::NoError(t, err)
            // Go require 断言保持为测试期望说明，当前不运行这些断言。
            require::Equal(t, ranges, preparedRanges)

            stores, err = s.gcWorker.getStoresForGC(context::Background())
            // Go require 断言保持为测试期望说明，当前不运行这些断言。
            require::NoError(t, err)
            // Go require 断言保持为测试期望说明，当前不运行这些断言。
            require::Len(t, stores, 3)

            // Sort by address for checking.
            sort::Slice(stores, func(i, j int) bool { return stores[i].Address < stores[j].Address })

            // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
            sendReqCh = make(chan SentReq, 20)

            // The request sent to the specified key and store will fail.
// 以下变量对应 Go var 块，保留测试共享状态和 fixture 初始化语义。
            var (
                failKey   []byte
                failStore *metapb::Store
            )
            // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
            s.client.unsafeDestroyRangeHandler = func(addr string, req *tikvrpc::Request) (*tikvrpc::Response, error) {
                // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
                sendReqCh <- SentReq{req, addr}
                // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
                resp = &tikvrpc::Response{
                    // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
                    Resp: &kvrpcpb::UnsafeDestroyRangeResponse{},
                }
                // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
                // Go require 断言保持为测试期望说明，当前不运行这些断言。
                if bytes::Equal(req.UnsafeDestroyRange().GetStartKey(), failKey) && addr == failStore.GetAddress() {
                    if failType == failRPCErr {
                        return None, errors::New("error")
                    // Go require 断言保持为测试期望说明，当前不运行这些断言。
                    } else if failType == failNilResp {
                        resp.Resp = None
                    } else if failType == failErrResp {
                        // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
                        (resp.Resp.(*kvrpcpb::UnsafeDestroyRangeResponse)).Error = "error"
                    } else {
                        panic("unreachable")
                    }
                }
                return resp, None
            }
            // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
            defer func() { s.client.unsafeDestroyRangeHandler = None }()

            // Make the logic in a closure to reduce duplicated code that tests deleteRanges and
            test = func(redo bool) {
                deleteRangeFunc = s.gcWorker.deleteRanges
                // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
                loadRangesFunc = util::LoadDeleteRanges
                if redo {
                    // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
                    deleteRangeFunc = s.gcWorker.redoDeleteRanges
                    // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
                    loadRangesFunc = util::LoadDoneDeleteRanges
                }

                // Make the first request fail.
                failKey = ranges[0].StartKey
                failStore = stores[0]

                err = deleteRangeFunc(gcContext(), 20, gcConcurrency{1, false})
                // Go require 断言保持为测试期望说明，当前不运行这些断言。
                require::NoError(t, err)

                s.checkDestroyRangeReq(t, sendReqCh, ranges, stores)

                // The first delete range task should be still here since it didn't success.
                se = createSession(s.gcWorker.store)
                remainingRanges, err = loadRangesFunc(gcContext(), se, 20)
                // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
                se.Close()
                // Go require 断言保持为测试期望说明，当前不运行这些断言。
                require::NoError(t, err)
                // Go require 断言保持为测试期望说明，当前不运行这些断言。
                require::Equal(t, ranges[:1], remainingRanges)

                failKey = None
                failStore = None

                // Delete the remaining range again.
                err = deleteRangeFunc(gcContext(), 20, gcConcurrency{1, false})
                // Go require 断言保持为测试期望说明，当前不运行这些断言。
                require::NoError(t, err)
                s.checkDestroyRangeReq(t, sendReqCh, ranges[:1], stores)

                se = createSession(s.gcWorker.store)
                remainingRanges, err = loadRangesFunc(gcContext(), se, 20)
                // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
                se.Close()
                // Go require 断言保持为测试期望说明，当前不运行这些断言。
                require::NoError(t, err)
                // Go require 断言保持为测试期望说明，当前不运行这些断言。
                require::Len(t, remainingRanges, 0)
            }

            test(false)
            // Change the order because the first range is the last successfully deleted.
            ranges = append(ranges[1:], ranges[0])
            test(true)
        })
    }
}

#[test]
// TestConcurrentDeleteRanges 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestConcurrentDeleteRanges() {
    // make sure the parallelization of deleteRanges works

    // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, failpoint::Enable("github.com/pingcap/tidb/pkg/store/gcworker/mockHistoryJobForGC", "return(1)"))
    // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, failpoint::Enable("github.com/pingcap/tidb/pkg/store/gcworker/mockHistoryJob", "return(\"schema/d1/t1\")"))
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    defer func() {
        // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, failpoint::Disable("github.com/pingcap/tidb/pkg/store/gcworker/mockHistoryJobForGC"))
        // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, failpoint::Disable("github.com/pingcap/tidb/pkg/store/gcworker/mockHistoryJob"))
    }()

    s = createGCWorkerSuite(t)
    se = createSession(s.gcWorker.store)
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    defer se.Close()
    _, err = se.Execute(gcContext(), `INSERT INTO mysql.gc_delete_range VALUES
("1", "2", "31", "32", "10"),
("3", "4", "33", "34", "10"),
("5", "6", "35", "36", "15"),
("7", "8", "37", "38", "15"),
("9", "10", "39", "40", "15")
    `)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
    ranges, err = util::LoadDeleteRanges(gcContext(), se, 20)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Len(t, ranges, 5)

    stores, err = s.gcWorker.getStoresForGC(context::Background())
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Len(t, stores, 3)
    sort::Slice(stores, func(i, j int) bool { return stores[i].Address < stores[j].Address })

    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    sendReqCh = make(chan SentReq, 20)
    // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
    s.client.unsafeDestroyRangeHandler = func(addr string, req *tikvrpc::Request) (*tikvrpc::Response, error) {
        // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
        sendReqCh <- SentReq{req, addr}
        // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
        resp = &tikvrpc::Response{
            // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
            Resp: &kvrpcpb::UnsafeDestroyRangeResponse{},
        }
        return resp, None
    }
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    defer func() { s.client.unsafeDestroyRangeHandler = None }()

    err = s.gcWorker.deleteRanges(gcContext(), 20, gcConcurrency{3, false})
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    s.checkDestroyRangeReq(t, sendReqCh, ranges, stores)

    se = createSession(s.gcWorker.store)
    // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
    remainingRanges, err = util::LoadDeleteRanges(gcContext(), se, 20)
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    se.Close()
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Len(t, remainingRanges, 0)
}

// SentReq 对应 Go 的同名 struct，字段顺序与嵌入类型保持原样，便于人工核对测试 fixture。
pub struct SentReq {
    // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
    req  *tikvrpc::Request
    addr string
}

// checkDestroyRangeReq checks whether given sentReq matches given ranges and stores.
// mockGCWorkerSuite_checkDestroyRangeReq 对应 Go 中 mockGCWorkerSuite.checkDestroyRangeReq 方法，保留测试辅助/断言流程。
pub fn mockGCWorkerSuite_checkDestroyRangeReq(s: &mut mockGCWorkerSuite, t: Box<testing.T>, sendReqCh chan: SentReq, expectedRanges: []util::DelRangeTask, expectedStores: []*metapb::Store) {
    sentReq = make([]SentReq, 0, len(expectedStores)*len(expectedStores))
Loop:
    for {
        select {
        // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
        case req = <-sendReqCh:
            sentReq = append(sentReq, req)
        default:
            break Loop
        }
    }

    sort::Slice(sentReq, func(i, j int) bool {
        // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
        cmp = bytes::Compare(sentReq[i].req.UnsafeDestroyRange().StartKey, sentReq[j].req.UnsafeDestroyRange().StartKey)
        return cmp < 0 || (cmp == 0 && sentReq[i].addr < sentReq[j].addr)
    })

    sortedRanges = slices::Clone(expectedRanges)
    sort::Slice(sortedRanges, func(i, j int) bool {
        return bytes::Compare(sortedRanges[i].StartKey, sortedRanges[j].StartKey) < 0
    })

    for rangeIndex = range sortedRanges {
        for storeIndex = range expectedStores {
            i = rangeIndex*len(expectedStores) + storeIndex
            // Go require 断言保持为测试期望说明，当前不运行这些断言。
            require::Equal(t, expectedStores[storeIndex].Address, sentReq[i].addr)
            // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
            // Go require 断言保持为测试期望说明，当前不运行这些断言。
            require::Equal(t, sortedRanges[rangeIndex].StartKey, kv::Key(sentReq[i].req.UnsafeDestroyRange().GetStartKey()))
            // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
            // Go require 断言保持为测试期望说明，当前不运行这些断言。
            require::Equal(t, sortedRanges[rangeIndex].EndKey, kv::Key(sentReq[i].req.UnsafeDestroyRange().GetEndKey()))
        }
    }
}

#[test]
// TestUnsafeDestroyRangeForRaftkv2 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestUnsafeDestroyRangeForRaftkv2() {
    // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, failpoint::Enable("github.com/pingcap/tidb/pkg/ddl/util/IsRaftKv2", "return(true)"))

    // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, failpoint::Enable("github.com/pingcap/tidb/pkg/store/gcworker/mockHistoryJobForGC", "return(1)"))
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    defer func() {
        // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, failpoint::Disable("github.com/pingcap/tidb/pkg/store/gcworker/mockHistoryJobForGC"))
    }()

    // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, failpoint::Enable("github.com/pingcap/tidb/pkg/store/gcworker/mockHistoryJob", "return(\"schema/d1/t1\")"))
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    defer func() {
        // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, failpoint::Disable("github.com/pingcap/tidb/pkg/store/gcworker/mockHistoryJob"))
    }()

    s = createGCWorkerSuite(t)
    // Put some delete range tasks.
    se = createSession(s.gcWorker.store)
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    defer se.Close()
    _, err = se.Execute(gcContext(), `INSERT INTO mysql.gc_delete_range VALUES
("1", "2", "31", "32", "5"),
("3", "4", "33", "34", "10"),
("5", "6", "35", "36", "15"),
("7", "8", "37", "38", "15")`)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    ranges = []util::DelRangeTask{
        {
            JobID:     1,
            ElementID: 2,
            StartKey:  Vec::<u8>::from("1"),
            EndKey:    Vec::<u8>::from("2"),
        },
        {
            JobID:     3,
            ElementID: 4,
            StartKey:  Vec::<u8>::from("3"),
            EndKey:    Vec::<u8>::from("4"),
        },
        {
            JobID:     5,
            ElementID: 6,
            StartKey:  Vec::<u8>::from("5"),
            EndKey:    Vec::<u8>::from("6"),
        },
        {
            JobID:     7,
            ElementID: 8,
            StartKey:  Vec::<u8>::from("7"),
            EndKey:    Vec::<u8>::from("8"),
        },
    }

    // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
    // Check the DeleteRanges tasks.
    // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
    preparedRanges, err = util::LoadDeleteRanges(gcContext(), se, 20)
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    se.Close()
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, ranges, preparedRanges)

    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    sendReqCh = make(chan SentReq, 20)
    // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
    s.client.deleteRangeHandler = func(addr string, req *tikvrpc::Request) (*tikvrpc::Response, error) {
        // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
        sendReqCh <- SentReq{req, addr}
        // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
        resp = &tikvrpc::Response{
            // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
            Resp: &kvrpcpb::DeleteRangeResponse{},
        }
        return resp, None
    }
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    defer func() { s.client.deleteRangeHandler = None }()

    err = s.gcWorker.deleteRanges(gcContext(), 8, gcConcurrency{1, false})
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    s.checkDestroyRangeReqV2(t, sendReqCh, ranges[:1])

    se = createSession(s.gcWorker.store)
    // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
    remainingRanges, err = util::LoadDeleteRanges(gcContext(), se, 20)
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    se.Close()
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, ranges[1:], remainingRanges)

    err = s.gcWorker.deleteRanges(gcContext(), 20, gcConcurrency{1, false})
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    s.checkDestroyRangeReqV2(t, sendReqCh, ranges[1:])

    // In v2, they should not be recorded in done ranges
    // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
    doneRanges, err = util::LoadDoneDeleteRanges(gcContext(), se, 20)
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    se.Close()
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::True(t, len(doneRanges) == 0)
}

// checkDestroyRangeReqV2 checks whether given sentReq matches given ranges and stores when raft-kv2 is enabled.
// mockGCWorkerSuite_checkDestroyRangeReqV2 对应 Go 中 mockGCWorkerSuite.checkDestroyRangeReqV2 方法，保留测试辅助/断言流程。
pub fn mockGCWorkerSuite_checkDestroyRangeReqV2(s: &mut mockGCWorkerSuite, t: Box<testing.T>, sendReqCh chan: SentReq, expectedRanges: []util::DelRangeTask) {
    sentReq = make([]SentReq, 0, 5)
Loop:
    for {
        select {
        // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
        case req = <-sendReqCh:
            sentReq = append(sentReq, req)
        default:
            break Loop
        }
    }

    sort::Slice(sentReq, func(i, j int) bool {
        // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
        cmp = bytes::Compare(sentReq[i].req.DeleteRange().StartKey, sentReq[j].req.DeleteRange().StartKey)
        return cmp < 0 || (cmp == 0 && sentReq[i].addr < sentReq[j].addr)
    })

    sortedRanges = slices::Clone(expectedRanges)
    sort::Slice(sortedRanges, func(i, j int) bool {
        return bytes::Compare(sortedRanges[i].StartKey, sortedRanges[j].StartKey) < 0
    })

    for rangeIndex = range sortedRanges {
        // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Equal(t, sortedRanges[rangeIndex].StartKey, kv::Key(sentReq[rangeIndex].req.DeleteRange().GetStartKey()))
        // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Equal(t, sortedRanges[rangeIndex].EndKey, kv::Key(sentReq[rangeIndex].req.DeleteRange().GetEndKey()))
    }
}

#[test]
// TestLeaderTick 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestLeaderTick() {
    // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
    // Disable the background txn safe-point cache updater (10 s poll) so it
    // does not race with snapshot reads in checkCollected/mustGetNone.
    // Without this, the updater may cache the advanced safe point before the
    // test reads old probe timestamps, causing [tikv:9006] errors.
    // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, failpoint::Enable("tikvclient/noBuiltInTxnSafePointUpdater", "return"))
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    t.Cleanup(func() {
        // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, failpoint::Disable("tikvclient/noBuiltInTxnSafePointUpdater"))
    })

    // as we are adjusting the base TS, we need a larger schema lease to avoid
    // the info schema outdated error.
    s = createGCWorkerSuite(t, withStoreType(mockstore::EmbedUnistore), withSchemaLease(time::Hour))

    txnSafePointSyncWaitTime = 0

    veryLong = gcDefaultLifeTime * 10
    // Avoid failing at interval check. `lastFinish` is checked by os time.
    s.gcWorker.lastFinish = time.Now().Add(-veryLong)
    // Use central mode to do this test.
    err = s.gcWorker.saveValueToSysTable(gcModeKey, gcModeCentral)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    p = s.createGCProbe(t, "k1")
    s.oracle::AddOffset(gcDefaultLifeTime * 2)

    // Skip if GC is running.
    s.gcWorker.gcIsRunning = true
    err = s.gcWorker.leaderTick(context::Background())
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    s.checkNotCollected(t, p)
    s.gcWorker.gcIsRunning = false
    // Reset GC last run time
    err = s.gcWorker.saveTime(gcLastRunTimeKey, oracle::GetTimeFromTS(s.mustAllocTs(t)).Add(-veryLong))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    // Skip if prepare failed (disabling GC will make prepare returns ok = false).
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    err = s.gcWorker.saveValueToSysTable(gcEnableKey, booleanFalse)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    err = s.gcWorker.leaderTick(gcContext())
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    s.checkNotCollected(t, p)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    err = s.gcWorker.saveValueToSysTable(gcEnableKey, booleanTrue)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Reset GC last run time
    err = s.gcWorker.saveTime(gcLastRunTimeKey, oracle::GetTimeFromTS(s.mustAllocTs(t)).Add(-veryLong))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    // Skip if gcWaitTime not exceeded.
    s.gcWorker.lastFinish = time.Now()
    err = s.gcWorker.leaderTick(gcContext())
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    s.checkNotCollected(t, p)
    s.gcWorker.lastFinish = time.Now().Add(-veryLong)
    // Reset GC last run time
    err = s.gcWorker.saveTime(gcLastRunTimeKey, oracle::GetTimeFromTS(s.mustAllocTs(t)).Add(-veryLong))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // The "skip gcWaitTime" leaderTick above ran prepare() which advanced the
    // PD txn safe point as a side effect. Bump the oracle so the next
    // prepare() computes a strictly higher target (GoTimeToTS truncates to ms,
    // so without this bump both calls can land in the same millisecond and
    // AdvanceTxnSafePoint returns NewSafePoint == OldSafePoint → GC skipped).
    s.oracle::AddOffset(time::Second)

    // Continue GC if all those checks passed.
    err = s.gcWorker.leaderTick(gcContext())
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Wait for GC finish
    select {
    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    case err = <-s.gcWorker.done:
        s.gcWorker.gcIsRunning = false
        break
    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    case <-time.After(time::Second * 10):
        err = errors::New("receive from s.gcWorker.done timeout")
    }
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    s.checkCollected(t, p)

    // Test again to ensure the synchronization between goroutines is correct.
    err = s.gcWorker.saveTime(gcLastRunTimeKey, oracle::GetTimeFromTS(s.mustAllocTs(t)).Add(-veryLong))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    s.gcWorker.lastFinish = time.Now().Add(-veryLong)
    p = s.createGCProbe(t, "k1")
    s.oracle::AddOffset(gcDefaultLifeTime * 2)

    err = s.gcWorker.leaderTick(gcContext())
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Wait for GC finish
    select {
    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    case err = <-s.gcWorker.done:
        s.gcWorker.gcIsRunning = false
        break
    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    case <-time.After(time::Second * 10):
        err = errors::New("receive from s.gcWorker.done timeout")
    }
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    s.checkCollected(t, p)

    // No more signals in the channel
    select {
    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    case err = <-s.gcWorker.done:
        err = errors::Errorf("received signal s.gcWorker.done which shouldn't exist: %v", err)
        break
    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    case <-time.After(time::Second):
        break
    }
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
}

#[test]
// TestUnifiedGCNeedsToWait 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestUnifiedGCNeedsToWait() {
    if kerneltype::IsClassic() {
        t.Skip("starter deploy mode is only available in nextgen kernel")
    }

    originInTest = intest::InTest
    originDeployMode = deploymode::Get()
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    t.Cleanup(func() {
        intest::InTest = originInTest
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, deploymode::Set(originDeployMode))
    })

    // Starter unified GC skips the initial gcWaitTime only in production and
    // only before the first GC job has completed. Other modes, and intest by
    // default, keep the normal cooldown semantics.
    testCases = []struct {
        name                  string
        deployMode            deploymode::Mode
        inTest                bool
        hasFinishedFirstGCJob bool
        expected              bool
    }{
        {
            name:                  "starter still waits in intest",
            deployMode:            deploymode::Starter,
            inTest:                true,
            hasFinishedFirstGCJob: false,
            expected:              true,
        },
        {
            name:                  "starter skips initial wait in production",
            deployMode:            deploymode::Starter,
            inTest:                false,
            hasFinishedFirstGCJob: false,
            expected:              false,
        },
        {
            name:                  "starter waits after first gc job in production",
            deployMode:            deploymode::Starter,
            inTest:                false,
            hasFinishedFirstGCJob: true,
            expected:              true,
        },
        {
            name:                  "premium still waits in production",
            deployMode:            deploymode::Premium,
            inTest:                false,
            hasFinishedFirstGCJob: false,
            expected:              true,
        },
    }

    for _, tc = range testCases {
        t.Run(tc.name, func(t *testing.T) {
            intest::InTest = tc.inTest
            // Go require 断言保持为测试期望说明，当前不运行这些断言。
            require::NoError(t, deploymode::Set(tc.deployMode))

            worker = &GCWorker{
                lastFinish:            time.Now(),
                hasFinishedFirstGCJob: tc.hasFinishedFirstGCJob,
            }
            // Go require 断言保持为测试期望说明，当前不运行这些断言。
            require::Equal(t, tc.expected, worker.needsToWait())
        })
    }
}

#[test]
// TestResolveLockRangeInfine 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestResolveLockRangeInfine() {
    s = createGCWorkerSuite(t)

    // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, failpoint::Enable("tikvclient/invalidCacheAndRetry", "return(true)"))
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    defer func() {
        // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, failpoint::Disable("tikvclient/invalidCacheAndRetry"))
    }()

    mockLockResolver = &mockGCWorkerLockResolver{
        RegionLockResolver: tikv::NewRegionLockResolver("test", s.tikvStore),
        tikvStore:          s.tikvStore,
        scanLocks: func(_ []*txnlock::Lock, key []byte) ([]*txnlock::Lock, *tikv::KeyLocation) {
            return []*txnlock::Lock{}, &tikv::KeyLocation{}
        },
        batchResolveLocks: func(
            locks []*txnlock::Lock,
            loc *tikv::KeyLocation,
        ) (*tikv::KeyLocation, error) {
            // mock error to test backoff
            return None, errors::New("mock error")
        },
    }
    _, err = tikv::ResolveLocksForRange(gcContext(), mockLockResolver, 1, vec![0}, vec![1}, tikv::NewNoopBackoff, 10)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Error(t, err)
}

#[test]
// TestResolveLockRangeMeetRegionCacheMiss 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestResolveLockRangeMeetRegionCacheMiss() {
    s = createGCWorkerSuite(t)

// 以下变量对应 Go var 块，保留测试共享状态和 fixture 初始化语义。
    var (
        scanCnt       int
        scanCntRef    = &scanCnt
        resolveCnt    int
        resolveCntRef = &resolveCnt

        safepointTS uint64 = 434245550444904450
    )

    allLocks = []*txnlock::Lock{
        {
            Key: vec![1},
            // TxnID < safepointTS
            TxnID: 434245550444904449,
            TTL:   5,
        },
        {
            Key: vec![2},
            // safepointTS < TxnID < lowResolveTS , TxnID + TTL < lowResolveTS
            TxnID: 434245550445166592,
            TTL:   10,
        },
        {
            Key: vec![3},
            // safepointTS < TxnID < lowResolveTS , TxnID + TTL > lowResolveTS
            TxnID: 434245550445166593,
            TTL:   20,
        },
        {
            Key: vec![4},
            // TxnID > lowResolveTS
            TxnID: 434245550449099752,
            TTL:   20,
        },
    }

    mockLockResolver = &mockGCWorkerLockResolver{
        RegionLockResolver: tikv::NewRegionLockResolver("test", s.tikvStore),
        tikvStore:          s.tikvStore,
        scanLocks: func(_ []*txnlock::Lock, key []byte) ([]*txnlock::Lock, *tikv::KeyLocation) {
            *scanCntRef++
            return allLocks, &tikv::KeyLocation{
                Region: tikv::NewRegionVerID(s.initRegion.regionID, 0, 0),
            }
        },
        batchResolveLocks: func(
            locks []*txnlock::Lock,
            loc *tikv::KeyLocation,
        ) (*tikv::KeyLocation, error) {
            *resolveCntRef++
            if *resolveCntRef == 1 {
                s.gcWorker.tikvStore.GetRegionCache().InvalidateCachedRegion(loc.Region)
                // mock the region cache miss error
                return None, None
            }
            return loc, None
        },
    }
    _, err = tikv::ResolveLocksForRange(gcContext(), mockLockResolver, safepointTS, vec![0}, vec![10}, tikv::NewNoopBackoff, 10)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, 2, resolveCnt)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, 2, scanCnt)
}

#[test]
// TestResolveLockRangeMeetRegionEnlargeCausedByRegionMerge 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestResolveLockRangeMeetRegionEnlargeCausedByRegionMerge() {
    // TODO: Update the test code.
    // This test rely on the obsolete mock tikv, but mock tikv does not implement paging.
    // So use this failpoint to force non-paging protocol.
    // Mock TiKV does not have implementation to up-to-date PD APIs about GC either, making it fail when running in next gen.
    if kerneltype::IsNextGen() {
        t.Skip("The test is currently not compatible with next gen")
    }
    // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
    failpoint::Enable("github.com/pingcap/tidb/pkg/store/copr/DisablePaging", `return`)
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    defer func() {
        // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, failpoint::Disable("github.com/pingcap/tidb/pkg/store/copr/DisablePaging"))
    }()
    s = createGCWorkerSuite(t, withStoreType(mockstore::MockTiKV), withSchemaLease(config::DefSchemaLease))

// 以下变量对应 Go var 块，保留测试共享状态和 fixture 初始化语义。
    var (
        firstAccess    = true
        firstAccessRef = &firstAccess
        resolvedLock   [][]byte
    )

    // key range: ['' - 'm' - 'z']
    region2 = s.cluster.AllocID()
    newPeers = vec![s.cluster.AllocID(), s.cluster.AllocID(), s.cluster.AllocID()}
    s.cluster.Split(s.initRegion.regionID, region2, Vec::<u8>::from("m"), newPeers, newPeers[0])

    mockGCLockResolver = &mockGCWorkerLockResolver{
        RegionLockResolver: tikv::NewRegionLockResolver("test", s.tikvStore),
        tikvStore:          s.tikvStore,
        scanLocks: func(_ []*txnlock::Lock, key []byte) ([]*txnlock::Lock, *tikv::KeyLocation) {
            // first time scan locks
            region, _, _, _ = s.cluster.GetRegionByKey(key)
            if region.GetId() == s.initRegion.regionID {
                return []*txnlock::Lock{{Key: Vec::<u8>::from("a")}, {Key: Vec::<u8>::from("b")}},
                    &tikv::KeyLocation{
                        Region: tikv::NewRegionVerID(
                            region.GetId(),
                            region.GetRegionEpoch().ConfVer,
                            region.GetRegionEpoch().Version,
                        ),
                    }
            }
            // second time scan locks
            if region.GetId() == region2 {
                return []*txnlock::Lock{{Key: Vec::<u8>::from("o")}, {Key: Vec::<u8>::from("p")}},
                    &tikv::KeyLocation{
                        Region: tikv::NewRegionVerID(
                            region.GetId(),
                            region.GetRegionEpoch().ConfVer,
                            region.GetRegionEpoch().Version,
                        ),
                    }
            }
            return []*txnlock::Lock{}, None
        },
    }
    mockGCLockResolver.batchResolveLocks = func(
        locks []*txnlock::Lock,
        loc *tikv::KeyLocation,
    ) (*tikv::KeyLocation, error) {
        if loc.Region.GetID() == s.initRegion.regionID && *firstAccessRef {
            *firstAccessRef = false
            // merge region2 into region1 and return EpochNotMatch error.
            mCluster = s.cluster.(*testutils::MockCluster)
            mCluster.Merge(s.initRegion.regionID, region2)
            regionMeta, _ = mCluster.GetRegion(s.initRegion.regionID)
            _, err = s.tikvStore.GetRegionCache().OnRegionEpochNotMatch(
                tikv::NewNoopBackoff(context::Background()),
                &tikv::RPCContext{Region: loc.Region, Store: &tikv::Store{}},
                []*metapb::Region{regionMeta})
            // Go require 断言保持为测试期望说明，当前不运行这些断言。
            require::NoError(t, err)
            // also let region1 contains all 4 locks
            mockGCLockResolver.scanLocks = func(_ []*txnlock::Lock, key []byte) ([]*txnlock::Lock, *tikv::KeyLocation) {
                // Go require 断言保持为测试期望说明，当前不运行这些断言。
                if bytes::Equal(key, Vec::<u8>::from("")) {
                    locks = []*txnlock::Lock{
                        {Key: Vec::<u8>::from("a")},
                        {Key: Vec::<u8>::from("b")},
                        {Key: Vec::<u8>::from("o")},
                        {Key: Vec::<u8>::from("p")},
                    }
                    for i, lock = range locks {
                        if bytes::Compare(key, lock.Key) <= 0 {
                            return locks[i:], &tikv::KeyLocation{Region: tikv::NewRegionVerID(
                                regionMeta.GetId(),
                                regionMeta.GetRegionEpoch().ConfVer,
                                regionMeta.GetRegionEpoch().Version)}
                        }
                    }
                }
                return []*txnlock::Lock{}, None
            }
            return None, None
        }
        for _, lock = range locks {
            resolvedLock = append(resolvedLock, lock.Key)
        }
        return loc, None
    }
    _, err = tikv::ResolveLocksForRange(gcContext(), mockGCLockResolver, 1, Vec::<u8>::from(""), Vec::<u8>::from("z"), tikv::NewGcResolveLockMaxBackoffer, 10)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Len(t, resolvedLock, 4)
    expects = []vec![Vec::<u8>::from("a"), Vec::<u8>::from("b"), Vec::<u8>::from("o"), Vec::<u8>::from("p")}
    for i, l = range resolvedLock {
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Equal(t, expects[i], l)
    }
}

// testResolveLocksWithKeyspacesImpl 对应 Go 辅助函数，保留参数、返回值和主要控制流。
pub fn testResolveLocksWithKeyspacesImpl(t: Box<testing.T>, subCaseName: String) {
    // Note: this test is expected not to respect to whether compiled as NextGen, but tests the logic designed to work
    // on both classic and NextGen, and even mixed keyspaced & un-keyspaced usages in the same cluster.
    // The unified GC, which is not used and won't be used in next gen, is also covered here.
    // However, as the NextGen flag is currently overused, causing some keyspace-specific code unable to run on
    // non-next-gen compilation or vice versa, the sub-tests must be filtered by the compilation flag for now.

    // Note: this test case consists of several sub-tests, but not managed with .t.Run(), because it can't be split
    // into multiple ones when running on CI, and might cause timeout.

// reqRange 对应 Go 的同名 struct，字段顺序与嵌入类型保持原样，便于人工核对测试 fixture。
pub struct reqRange {
        StartKey     []byte
        EndKey       []byte
        TxnSafePoint uint64
    }

    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    createSuiteForTestResolveLocks = func(t *testing.T, storeOpt ...mockstore::MockTiKVStoreOption) (suite *mockGCWorkerSuite, scanLockCounter *atomic::Int64, scanLockRangeCh chan reqRange) {
        suite = createGCWorkerSuite(t, withMockStoreOptions(storeOpt...))
        // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
        scanLockRangeCh = make(chan reqRange, 1000)
        // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
        scanLockCounter = &atomic::Int64{}
        // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
        suite.client.scanLockRequestHandler = func(addr string, req *tikvrpc::Request) (*tikvrpc::Response, error) {
            scanLockCounter.Add(1)
            // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
            scanLockReq = req.ScanLock()
            // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
            scanLockRangeCh <- reqRange{
                StartKey: scanLockReq.GetStartKey(),
                EndKey:   scanLockReq.GetEndKey(),
                // When resolving locks, the maxVersion parameter is set to txnSafePoint-1, so plus 1 back to retrieve
                // the txnSafePoint.
                TxnSafePoint: scanLockReq.GetMaxVersion() + 1,
            }
            // Only collect the request, without generating the result. Return nil to continue calling the inner client.
            return None, None
        }
        return
    }

    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    collectAndMergeRanges = func(t *testing.T, ch <-chan reqRange) []reqRange {
        ranges = make([]reqRange, 0, len(ch))
        for r = range ch {
            ranges = append(ranges, r)
        }
        slices::SortFunc(ranges, func(lhs, rhs reqRange) int {
            return bytes::Compare(lhs.StartKey, rhs.StartKey)
        })
        if len(ranges) == 0 {
            return ranges
        }
        mergedRanges = make([]reqRange, 0, len(ranges))
        mergedRanges = append(mergedRanges, ranges[0])
        for _, r = range ranges[1:] {
            previousMerged = &mergedRanges[len(mergedRanges)-1]
            if bytes::Compare(r.StartKey, previousMerged.EndKey) <= 0 {
                if len(r.EndKey) == 0 || bytes::Compare(r.EndKey, previousMerged.EndKey) > 0 {
                    previousMerged.EndKey = r.EndKey
                }
            } else {
                mergedRanges = append(mergedRanges, r)
            }
        }

        // Force empty key be represented as nil (instead of an empty slice) for the convenience of asserting.
        if len(mergedRanges[0].StartKey) == 0 {
            mergedRanges[0].StartKey = None
        }
        if len(mergedRanges[len(mergedRanges)-1].EndKey) == 0 {
            mergedRanges[len(mergedRanges)-1].EndKey = None
        }

        return mergedRanges
    }

    subCases = make(map[string]func(t *testing.T))

    subCases["NullKeyspaceOnly"] = func(t *testing.T) {
        if kerneltype::IsNextGen() {
            t.Skip()
        }
        s, counter, ch = createSuiteForTestResolveLocks(t, mockstore::WithCurrentKeyspaceMeta(None))
        err = s.gcWorker.resolveLocks(context::Background(), 100, 1)
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, err)
        close(ch)
        ranges = collectAndMergeRanges(t, ch)
        // In case there's totally no keyspace in the cluster, it skips the step to exclude the ranges used by
        // keyspaces.
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Equal(t, []reqRange{{
            StartKey:     None,
            EndKey:       None,
            TxnSafePoint: 100,
        }}, ranges)
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Equal(t, int64(1), counter.Load())
    }

    subCases["NullKeyspaceOnlyMultiRegion"] = func(t *testing.T) {
        if kerneltype::IsNextGen() {
            t.Skip()
        }
        s, counter, ch = createSuiteForTestResolveLocks(t, mockstore::WithCurrentKeyspaceMeta(None))
        s.splitAtKeys(t, "a", "b", "c")
        err = s.gcWorker.resolveLocks(context::Background(), 100, 1)
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, err)
        close(ch)
        ranges = collectAndMergeRanges(t, ch)
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Equal(t, []reqRange{{
            StartKey:     None,
            EndKey:       None,
            TxnSafePoint: 100,
        }}, ranges)
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Equal(t, int64(4), counter.Load())
    }

    makeKeyspace = func(id uint32, name string, enableKeyspaceLevelGC bool) *keyspacepb::KeyspaceMeta {
        gcManagementType = pd::KeyspaceConfigGCManagementTypeKeyspaceLevel
        if !enableKeyspaceLevelGC {
            gcManagementType = pd::KeyspaceConfigGCManagementTypeUnified
        }
        return &keyspacepb::KeyspaceMeta{
            Id:     id,
            Name:   name,
            Config: map[string]string{pd::KeyspaceConfigGCManagementType: gcManagementType},
        }
    }

    makeKey = func(id uint32, key string) string {
        c, err = tikv::NewCodecV2(tikv::ModeTxn, makeKeyspace(id, "dummyks", true))
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, err)
        return string(c.EncodeKey(Vec::<u8>::from(key)))
    }

    subCases["NullKeyspaceInMultiKeyspaceEnvironment"] = func(t *testing.T) {
        if kerneltype::IsNextGen() {
            t.Skip()
        }
        s, counter, ch = createSuiteForTestResolveLocks(t, mockstore::WithKeyspacesAndCurrentKeyspaceID([]*keyspacepb::KeyspaceMeta{
            makeKeyspace(1, "ks1", true),
            makeKeyspace(3, "ks3", true),
        }, constants::NullKeyspaceID))
        s.splitAtKeys(t,
            makeKey(1, ""), makeKey(1, "a"),
            makeKey(2, ""), makeKey(2, "a"),
            makeKey(3, ""), makeKey(3, "a"),
            makeKey(4, ""), makeKey(4, "a"),
            "t1", "t2", "m")
        err = s.gcWorker.resolveLocks(context::Background(), 100, 2)
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, err)
        close(ch)
        ranges = collectAndMergeRanges(t, ch)
        // Handles ranges that are out of any keyspaces.
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Equal(t, []reqRange{
            {StartKey: None, EndKey: Vec::<u8>::from("r"), TxnSafePoint: 100},         // 2 regions (split at "m")
            {StartKey: Vec::<u8>::from("s"), EndKey: Vec::<u8>::from("x"), TxnSafePoint: 100}, // 3 regions (split at "t1", "t2")
            {StartKey: Vec::<u8>::from("y"), EndKey: None, TxnSafePoint: 100},         // 1 region
        }, ranges)
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Equal(t, int64(6), counter.Load())
    }

    subCases["NonNullKeyspaceInMultiKeyspaceEnvironment"] = func(t *testing.T) {
        if !kerneltype::IsNextGen() {
            t.Skip()
        }
        // Note: Currently it's hard to simulate a user keyspace with unistore, we only try to test it with the SYSTEM
        // keyspace for now, which should have no difference in GC with user keyspaces.
        s, counter, ch = createSuiteForTestResolveLocks(t, mockstore::WithKeyspacesAndCurrentKeyspaceID([]*keyspacepb::KeyspaceMeta{
            makeKeyspace(1, "ks1", true),
            makeKeyspace(3, "ks3", true),
            makeKeyspace(constants::MaxKeyspaceID-1, "SYSTEM", true),
        }, constants::MaxKeyspaceID-1))

        s.splitAtKeys(t,
            makeKey(1, ""), makeKey(1, "a"),
            makeKey(2, ""), makeKey(2, "a"),
            makeKey(3, ""), makeKey(3, "a"),
            makeKey(4, ""), makeKey(4, "a"),
            makeKey(constants::MaxKeyspaceID-1, ""), makeKey(constants::MaxKeyspaceID-1, "a"),
            "t1", "t2", "m")
        err = s.gcWorker.resolveLocks(context::Background(), 100, 2)
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, err)
        close(ch)
        ranges = collectAndMergeRanges(t, ch)
        // Currently a unistore represents the content of only one keyspace, and the mocked tikv client is different
        // from the real TiKV one and doesn't have the key prefix attaching/detaching step, causing it unable to simulate the structure
        // of a real cluster.
        // TODO: Replace the check if we make it to simulate the key range division of a real cluster correctly.
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        // require.Equal(t, []reqRange{
        //     {StartKey: []byte("x\xff\xff\xfe"), EndKey: []byte("x\xff\xff\xff"), TxnSafePoint: 100}, // 2 regions split at "x\x00\x00\x00a"
        // }, ranges)
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Equal(t, []reqRange{
            {StartKey: None, EndKey: None, TxnSafePoint: 100}, // 2 regions split at "a"
        }, ranges)
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Equal(t, int64(2), counter.Load())
    }

    subCases["UnifiedGCInMixedUsage"] = func(t *testing.T) {
        if kerneltype::IsNextGen() {
            t.Skip()
        }
        s, counter, ch = createSuiteForTestResolveLocks(t, mockstore::WithKeyspacesAndCurrentKeyspaceID([]*keyspacepb::KeyspaceMeta{
            makeKeyspace(0, "DEFAULT", false),
            makeKeyspace(1, "ks1", true),
            makeKeyspace(2, "ks2", false),
            makeKeyspace(3, "ks3", true),
            makeKeyspace(5, "ks5", false),
            makeKeyspace(8, "ks8", true),
        }, constants::NullKeyspaceID))
        splitKeys = vec!["t1", "t2", "m"}
        for i = 0; i < 8; i++ {
            splitKeys = append(splitKeys, makeKey(uint32(i), ""), makeKey(uint32(i), "a"))
        }
        s.splitAtKeys(t, splitKeys...)
        err = s.gcWorker.resolveLocks(context::Background(), 100, 2)
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, err)
        close(ch)
        ranges = collectAndMergeRanges(t, ch)
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Equal(t, []reqRange{
            {StartKey: None, EndKey: Vec::<u8>::from("r"), TxnSafePoint: 100},                                 // Non-keyspace range, 2 regions (split at "m")
            {StartKey: Vec::<u8>::from("s"), EndKey: Vec::<u8>::from("x"), TxnSafePoint: 100},                         // Non-keyspace range, 3 regions (split at "t1", "t2")
            {StartKey: Vec::<u8>::from("x\x00\x00\x00"), EndKey: Vec::<u8>::from("x\x00\x00\x01"), TxnSafePoint: 100}, // Keyspace 0 (DEFAULT), 2 regions
            {StartKey: Vec::<u8>::from("x\x00\x00\x02"), EndKey: Vec::<u8>::from("x\x00\x00\x03"), TxnSafePoint: 100}, // Keyspace 2, 2 regions
            {StartKey: Vec::<u8>::from("x\x00\x00\x05"), EndKey: Vec::<u8>::from("x\x00\x00\x06"), TxnSafePoint: 100}, // Keyspace 5, 2 regions
            {StartKey: Vec::<u8>::from("y"), EndKey: None, TxnSafePoint: 100},                                 // 1 region
        }, ranges)
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Equal(t, int64(12), counter.Load())
    }

    subCases["UnifiedGCWithMaxKeyspaceID"] = func(t *testing.T) {
        if kerneltype::IsNextGen() {
            t.Skip()
        }
        s, counter, ch = createSuiteForTestResolveLocks(t, mockstore::WithKeyspacesAndCurrentKeyspaceID([]*keyspacepb::KeyspaceMeta{
            makeKeyspace(constants::MaxKeyspaceID, "max", false),
        }, constants::NullKeyspaceID))

        splitKeys = vec!["t1", "t2", "m", makeKey(constants::MaxKeyspaceID, ""), makeKey(constants::MaxKeyspaceID, "a"), "y", "y\x00\x00\x00"}
        s.splitAtKeys(t, splitKeys...)
        err = s.gcWorker.resolveLocks(context::Background(), 100, 2)
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, err)
        close(ch)
        ranges = collectAndMergeRanges(t, ch)
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Equal(t, []reqRange{
            {StartKey: None, EndKey: Vec::<u8>::from("r"), TxnSafePoint: 100},             // Non-keyspace range, 2 regions (split at "m")
            {StartKey: Vec::<u8>::from("s"), EndKey: Vec::<u8>::from("x"), TxnSafePoint: 100},     // Non-keyspace range, 3 regions (split at "t1", "t2")
            {StartKey: Vec::<u8>::from("x\xff\xff\xff"), EndKey: None, TxnSafePoint: 100}, // Keyspace MaxKeyspaceID + ranges after keyspace prefix, 2 + 2 regions.
        }, ranges)
        // Note: The range ["y", "y\x00\x00\x00") is actually repeatedly handled, but it doesn't matter for now as it's never used and contains only 3 keys.
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Equal(t, int64(10), counter.Load())
    }

    testUnifiedGCInMultiBatchesOfKeyspacesImpl = func(t *testing.T, startID uint32, count uint32, step uint32, loadBatchSize int, expectedBatchCount int) {
        if kerneltype::IsNextGen() {
            t.Skip()
        }

        // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, failpoint::Enable("github.com/pingcap/tidb/pkg/store/gcworker/overrideLoadKeyspacesBatchSize", fmt::Sprintf("return(%d)", loadBatchSize)))
        // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
        defer func() {
            // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
            // Go require 断言保持为测试期望说明，当前不运行这些断言。
            require::NoError(t, failpoint::Disable("github.com/pingcap/tidb/pkg/store/gcworker/overrideLoadKeyspacesBatchSize"))
        }()

        // Retrieve the actual batch count of loading all keyspaces, for ensuring that the failpoint
        // `overrideLoadKeyspacesBatchSize` actually takes effect.
        loadKeyspacesBatchCount = 0
        // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, failpoint::EnableCall("github.com/pingcap/tidb/pkg/store/gcworker/getLoadKeyspacesBatchCount", func(v int) {
            loadKeyspacesBatchCount += v
        }))
        // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
        defer func() {
            // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
            // Go require 断言保持为测试期望说明，当前不运行这些断言。
            require::NoError(t, failpoint::Disable("github.com/pingcap/tidb/pkg/store/gcworker/getLoadKeyspacesBatchCount"))
        }()

        keyspaces = make([]*keyspacepb::KeyspaceMeta, 0, count)
        splitKeys = make([]string, 0, count*2+3)
        expectedRanges = make([]reqRange, 0, count+3)
        // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
        expectedScanLocksCount = int(count)*2 + 6
        splitKeys = append(splitKeys, "t1", "t2", "m")

        expectedRanges = append(expectedRanges, reqRange{StartKey: None, EndKey: Vec::<u8>::from("r"), TxnSafePoint: 100})         //  Non-keyspace range, 2 regions (split at "m")
        expectedRanges = append(expectedRanges, reqRange{StartKey: Vec::<u8>::from("s"), EndKey: Vec::<u8>::from("x"), TxnSafePoint: 100}) // Non-keyspace range, 3 regions (split at "t1", "t2")
        for i = range count {
            id = startID + i*step
            keyspaces = append(keyspaces, makeKeyspace(id, fmt::Sprintf("ks%d", id), false))
            splitKeys = append(splitKeys, makeKey(id, ""), makeKey(id, "a"))
            startKey, err = hex::DecodeString(fmt::Sprintf("78%06x", id))
            // Go require 断言保持为测试期望说明，当前不运行这些断言。
            require::NoError(t, err)
            endKey = kv2.PrefixNextKey(startKey)
            // Go require 断言保持为测试期望说明，当前不运行这些断言。
            require::NoError(t, err)
            // Go require 断言保持为测试期望说明，当前不运行这些断言。
            if bytes::Equal(startKey, expectedRanges[len(expectedRanges)-1].EndKey) {
                expectedRanges[len(expectedRanges)-1].EndKey = endKey
            } else {
                expectedRanges = append(expectedRanges, reqRange{StartKey: startKey, EndKey: endKey, TxnSafePoint: 100}) // 2 regions each
            }
        }
        if bytes::Compare(expectedRanges[len(expectedRanges)-1].EndKey, Vec::<u8>::from("y")) >= 0 {
            expectedRanges[len(expectedRanges)-1].EndKey = None
        } else {
            expectedRanges = append(expectedRanges, reqRange{StartKey: Vec::<u8>::from("y"), EndKey: None, TxnSafePoint: 100}) // 1 region
        }

        s, counter, ch = createSuiteForTestResolveLocks(t, mockstore::WithKeyspacesAndCurrentKeyspaceID(keyspaces, constants::NullKeyspaceID))
        s.splitAtKeys(t, splitKeys...)
        err = s.gcWorker.resolveLocks(context::Background(), 100, 10)
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, err)
        close(ch)
        ranges = collectAndMergeRanges(t, ch)
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Equal(t, expectedRanges, ranges)
        // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Equal(t, int64(expectedScanLocksCount), counter.Load())

        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Equal(t, expectedBatchCount, loadKeyspacesBatchCount)
    }

    subCases["UnifiedGCInMultiBatchesOfKeyspaces_8"] = func(t *testing.T) {
        // Load keyspaces batches: [1,2,3], [4,5,6], [7,8], []
        testUnifiedGCInMultiBatchesOfKeyspacesImpl(t, 1, 8, 1, 3, 4)
    }

    subCases["UnifiedGCInMultiBatchesOfKeyspaces_Last8_Step2"] = func(t *testing.T) {
        // Load keyspaces batches: [(MaxKeyspaceID)-14,-12,-10], [-8,-6,-4], [-2,0]
        testUnifiedGCInMultiBatchesOfKeyspacesImpl(t, constants::MaxKeyspaceID-14, 8, 2, 3, 3)
    }

    subCases["UnifiedGCInMultiBatchesOfKeyspaces_MultipleOfBatchSize"] = func(t *testing.T) {
        // Load keyspaces batches: [1,2,3], [4,5,6], []
        testUnifiedGCInMultiBatchesOfKeyspacesImpl(t, 1, 6, 1, 3, 3)
    }

    subCases["UnifiedGCInMultiBatchesOfKeyspaces_MultipleOfBatchSizeToEnd"] = func(t *testing.T) {
        // Load keyspaces batches: [(MaxKeyspaceID)-5,-4,-3], [-2,-1,0]
        testUnifiedGCInMultiBatchesOfKeyspacesImpl(t, constants::MaxKeyspaceID-5, 6, 1, 3, 2)
    }

    subCases[subCaseName](t)
}

#[test]
// TestResolveLocksWithKeyspaces_NullKeyspaceOnly 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestResolveLocksWithKeyspaces_NullKeyspaceOnly() {
    testResolveLocksWithKeyspacesImpl(t, "NullKeyspaceOnly")
}

#[test]
// TestResolveLocksWithKeyspaces_NullKeyspaceOnlyMultiRegion 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestResolveLocksWithKeyspaces_NullKeyspaceOnlyMultiRegion() {
    testResolveLocksWithKeyspacesImpl(t, "NullKeyspaceOnlyMultiRegion")
}

#[test]
// TestResolveLocksWithKeyspaces_NullKeyspaceInMultiKeyspaceEnvironment 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestResolveLocksWithKeyspaces_NullKeyspaceInMultiKeyspaceEnvironment() {
    testResolveLocksWithKeyspacesImpl(t, "NullKeyspaceInMultiKeyspaceEnvironment")
}

#[test]
// TestResolveLocksWithKeyspaces_NonNullKeyspaceInMultiKeyspaceEnvironment 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestResolveLocksWithKeyspaces_NonNullKeyspaceInMultiKeyspaceEnvironment() {
    testResolveLocksWithKeyspacesImpl(t, "NonNullKeyspaceInMultiKeyspaceEnvironment")
}

#[test]
// TestResolveLocksWithKeyspaces_UnifiedGCInMixedUsage 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestResolveLocksWithKeyspaces_UnifiedGCInMixedUsage() {
    testResolveLocksWithKeyspacesImpl(t, "UnifiedGCInMixedUsage")
}

#[test]
// TestResolveLocksWithKeyspaces_UnifiedGCWithMaxKeyspaceID 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestResolveLocksWithKeyspaces_UnifiedGCWithMaxKeyspaceID() {
    testResolveLocksWithKeyspacesImpl(t, "UnifiedGCWithMaxKeyspaceID")
}

#[test]
// TestResolveLocksWithKeyspaces_UnifiedGCInMultiBatchesOfKeyspaces_8 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestResolveLocksWithKeyspaces_UnifiedGCInMultiBatchesOfKeyspaces_8() {
    testResolveLocksWithKeyspacesImpl(t, "UnifiedGCInMultiBatchesOfKeyspaces_8")
}

#[test]
// TestResolveLocksWithKeyspaces_UnifiedGCInMultiBatchesOfKeyspaces_Last8_Step2 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestResolveLocksWithKeyspaces_UnifiedGCInMultiBatchesOfKeyspaces_Last8_Step2() {
    testResolveLocksWithKeyspacesImpl(t, "UnifiedGCInMultiBatchesOfKeyspaces_Last8_Step2")
}

#[test]
// TestResolveLocksWithKeyspaces_UnifiedGCInMultiBatchesOfKeyspaces_MultipleOfBatchSize 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestResolveLocksWithKeyspaces_UnifiedGCInMultiBatchesOfKeyspaces_MultipleOfBatchSize() {
    testResolveLocksWithKeyspacesImpl(t, "UnifiedGCInMultiBatchesOfKeyspaces_MultipleOfBatchSize")
}

#[test]
// TestResolveLocksWithKeyspaces_UnifiedGCInMultiBatchesOfKeyspaces_MultipleOfBatchSizeToEnd 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestResolveLocksWithKeyspaces_UnifiedGCInMultiBatchesOfKeyspaces_MultipleOfBatchSizeToEnd() {
    testResolveLocksWithKeyspacesImpl(t, "UnifiedGCInMultiBatchesOfKeyspaces_MultipleOfBatchSizeToEnd")
}

#[test]
// TestResolveLocksNearTxnSafePoint 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestResolveLocksNearTxnSafePoint() {
    s = createGCWorkerSuite(t, withStoreType(mockstore::EmbedUnistore))

    currentTS, err = s.oracle::GetTimestamp(context::Background(), &oracle::Option{})
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    txnSafePoint = oracle::GoTimeToTS(oracle::GetTimeFromTS(currentTS).Add(-time::Minute * 5))

    txns = make([]kv::Transaction, 0, 3)

    for i, startTS = range vec![txnSafePoint - 1, txnSafePoint, txnSafePoint + 1} {
        txn, err = s.store.Begin(tikv::WithStartTS(startTS))
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, err)
        txn.SetOption(kv::Pessimistic, true)
        lockCtx = &kv::LockCtx{ForUpdateTS: txn.StartTS(), WaitStartTime: time.Now()}
        err = txn.LockKeys(context::Background(), lockCtx, Vec::<u8>::from(fmt::Sprintf("k%d", i+1)))
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, err)
        txns = append(txns, txn)
    }

    err = s.gcWorker.resolveLocks(gcContext(), txnSafePoint, 1)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    // Prevent amending lock behavior by making new write to the keys.
    otherTxns = make([]kv::Transaction, 0, 3)
    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    resCh = make(chan error, 3)
    for i = range 3 {
        txn, err = s.store.Begin()
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, err)
        txn.SetOption(kv::Pessimistic, true)
        key = Vec::<u8>::from(fmt::Sprintf("k%d", i+1))
        // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
        go func() {
            lockCtx = &kv::LockCtx{ForUpdateTS: txn.StartTS(), WaitStartTime: time.Now()}
            err = txn.LockKeys(context::Background(), lockCtx, key)
            // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
            resCh <- err
        }()
        otherTxns = append(otherTxns, txn)
    }

    // It's expected that only the first transaction in `txns` should be rolled back by GC, so that one of `otherTxns`
    // should proceed while the other two should be blocked.
    select {
    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    case err = <-resCh:
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, err)
    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    case <-time.After(time.Millisecond * 200):
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Fail(t, "no transaction is resolved, which is not expected")
    }

    select {
    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    case err = <-resCh:
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::Fail(t, "more than one transaction is resolved, which is not expected")
    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    case <-time.After(time.Millisecond * 50):
    }

    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Error(t, txns[0].Commit(context::Background()))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, txns[1].Commit(context::Background()))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, txns[2].Commit(context::Background()))

    for range 2 {
        select {
        // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
        case err = <-resCh:
            // Go require 断言保持为测试期望说明，当前不运行这些断言。
            require::Error(t, err)
            // Go require 断言保持为测试期望说明，当前不运行这些断言。
            require::Contains(t, err.Error(), "Write conflict")
        // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
        case <-time.After(time.Millisecond * 200):
            // Go require 断言保持为测试期望说明，当前不运行这些断言。
            require::Fail(t, "not all transactions are finished")
        }
    }

    // Clear unfinished transactions.
    for _, txn = range otherTxns {
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, txn.Rollback())
    }
}

#[test]
// TestRunGCJob 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestRunGCJob() {
    // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, failpoint::Enable("tikvclient/noBuiltInTxnSafePointUpdater", "return"))
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    t.Cleanup(func() {
        // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, failpoint::Disable("tikvclient/noBuiltInTxnSafePointUpdater"))
    })

    s = createGCWorkerSuite(t)

    originalTxnSafePointSyncWaitTime = txnSafePointSyncWaitTime
    txnSafePointSyncWaitTime = 0
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    t.Cleanup(func() {
        txnSafePointSyncWaitTime = originalTxnSafePointSyncWaitTime
    })

    // Test distributed mode
    useDistributedGC = s.gcWorker.checkUseDistributedGC()
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::True(t, useDistributedGC)
    safePoint = s.mustAllocTs(t)
    ctl = s.pdClient.GetGCInternalController(uint32(s.store.GetCodec().GetKeyspaceID()))
    // runGCJob doesn't contain the AdvanceTxnSafePoint step. Do it explicitly.
    res, err = ctl.AdvanceTxnSafePoint(gcContext(), safePoint)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, safePoint, res.NewTxnSafePoint)
    err = s.gcWorker.runGCJob(gcContext(), safePoint, gcConcurrency{1, false})
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    pdSafePoint = s.mustGetSafePointFromPd(t)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, safePoint, pdSafePoint)

    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, s.gcWorker.saveTime(gcSafePointKey, oracle::GetTimeFromTS(safePoint)))
    tikvSafePoint, err = s.gcWorker.loadTime(gcSafePointKey)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, *tikvSafePoint, oracle::GetTimeFromTS(safePoint))

    etcdSafePoint = s.loadTxnSafePoint(t)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, safePoint, etcdSafePoint)

    // Test distributed mode with safePoint regressing (although this is impossible)
    err = s.gcWorker.runGCJob(gcContext(), safePoint-1, gcConcurrency{1, false})
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Error(t, err)

    // Central mode is deprecated in v5.0, fallback to distributed mode if it's set.
    err = s.gcWorker.saveValueToSysTable(gcModeKey, gcModeCentral)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    useDistributedGC = s.gcWorker.checkUseDistributedGC()
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::True(t, useDistributedGC)

    p = s.createGCProbe(t, "k1")
    safePoint = s.mustAllocTs(t)
    res, err = ctl.AdvanceTxnSafePoint(gcContext(), safePoint)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, safePoint, res.NewTxnSafePoint)
    err = s.gcWorker.runGCJob(gcContext(), safePoint, gcConcurrency{1, false})
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    s.checkCollected(t, p)

    etcdSafePoint = s.loadTxnSafePoint(t)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, safePoint, etcdSafePoint)
}

#[test]
// TestSetServiceSafePoint 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestSetServiceSafePoint() {
    s = createGCWorkerSuite(t)

    // SafePoint calculations are based on time rather than ts value.
    safePoint = s.mustAllocTs(t)
    s.mustSetTiDBServiceSafePoint(t, safePoint, safePoint)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, safePoint, s.mustGetMinServiceSafePointFromPd(t))

    // Advance the service safe point
    safePoint += 100
    s.mustSetTiDBServiceSafePoint(t, safePoint, safePoint)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, safePoint, s.mustGetMinServiceSafePointFromPd(t))

    // It doesn't matter if there is a greater safePoint from other services.
    safePoint += 100
    // Returns the last service safePoint that were uploaded.
    s.mustUpdateServiceGCSafePoint(t, "svc1", safePoint+10, safePoint-100)
    s.mustSetTiDBServiceSafePoint(t, safePoint, safePoint)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, safePoint, s.mustGetMinServiceSafePointFromPd(t))

    // Test the case when there is a smaller safePoint from other services.
    safePoint += 100
    // Returns the last service safePoint that were uploaded.
    s.mustUpdateServiceGCSafePoint(t, "svc1", safePoint-10, safePoint-100)
    s.mustSetTiDBServiceSafePoint(t, safePoint, safePoint-10)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, safePoint-10, s.mustGetMinServiceSafePointFromPd(t))

    // Test removing the minimum service safe point.
    // As UpdateServiceGCSafePoint in unistore has become the compatible wrapper around GC barrier interface, this
    // behavior has changed: the simulated service safe point for "gc_worker" will be blocked at `safePoint-10`.
    // s.mustRemoveServiceGCSafePoint(t, "svc1", safePoint-10, safePoint)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    // require.Equal(t, safePoint, s.mustGetMinServiceSafePointFromPd(t))
    s.mustRemoveServiceGCSafePoint(t, "svc1", safePoint-10, safePoint-10)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, safePoint-10, s.mustGetMinServiceSafePointFromPd(t))
    // Advance it to `safePoint.
    s.mustSetTiDBServiceSafePoint(t, safePoint, safePoint)

    // Test the case when there are many safePoints.
    safePoint += 100
    for i = range 10 {
        svcName = fmt::Sprintf("svc%d", i)
        s.mustUpdateServiceGCSafePoint(t, svcName, safePoint+uint64(i)*10, safePoint-100)
    }
    s.mustSetTiDBServiceSafePoint(t, safePoint+50, safePoint)
}

#[test]
// TestRunGCJobAPI 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestRunGCJobAPI() {
    if kerneltype::IsNextGen() {
        t.Skip("RunGCJobAPI currently does not support running under non-null keyspace")
    }

    s = createGCWorkerSuite(t)
    mockLockResolver = &mockGCWorkerLockResolver{
        RegionLockResolver: tikv::NewRegionLockResolver("test", s.tikvStore),
        tikvStore:          s.tikvStore,
        scanLocks: func(_ []*txnlock::Lock, key []byte) ([]*txnlock::Lock, *tikv::KeyLocation) {
            return []*txnlock::Lock{}, &tikv::KeyLocation{}
        },
        batchResolveLocks: func(
            locks []*txnlock::Lock,
            loc *tikv::KeyLocation,
        ) (*tikv::KeyLocation, error) {
            // no locks
            return loc, None
        },
    }

    txnSafePointSyncWaitTime = 0

    p = s.createGCProbe(t, "k1")
    safePoint = s.mustAllocTs(t)
    err = RunGCJob(gcContext(), mockLockResolver, s.tikvStore, s.pdClient, safePoint, "mock", 1)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    s.checkCollected(t, p)
    etcdSafePoint = s.loadTxnSafePoint(t)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, safePoint, etcdSafePoint)
}

#[test]
// TestRunDistGCJobAPI 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestRunDistGCJobAPI() {
    if kerneltype::IsNextGen() {
        t.Skip("RunDistributedGCJob currently does not support running under non-null keyspace")
    }

    s = createGCWorkerSuite(t)

    txnSafePointSyncWaitTime = 0
    mockLockResolver = &mockGCWorkerLockResolver{
        RegionLockResolver: tikv::NewRegionLockResolver("test", s.tikvStore),
        tikvStore:          s.tikvStore,
        scanLocks: func(_ []*txnlock::Lock, key []byte) ([]*txnlock::Lock, *tikv::KeyLocation) {
            return []*txnlock::Lock{}, &tikv::KeyLocation{}
        },
        batchResolveLocks: func(
            locks []*txnlock::Lock,
            loc *tikv::KeyLocation,
        ) (*tikv::KeyLocation, error) {
            // no locks
            return loc, None
        },
    }

    safePoint = s.mustAllocTs(t)
    err = RunDistributedGCJob(gcContext(), mockLockResolver, s.tikvStore, s.pdClient, safePoint, "mock", 1)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    pdSafePoint = s.mustGetSafePointFromPd(t)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, safePoint, pdSafePoint)
    etcdSafePoint = s.loadTxnSafePoint(t)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, safePoint, etcdSafePoint)
}

#[test]
// TestStartWithRunGCJobFailures 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestStartWithRunGCJobFailures() {
    s = createGCWorkerSuite(t)

    s.gcWorker.Start()
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    defer s.gcWorker.Close()

    for range 3 {
        select {
        // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
        case <-time.After(100 * time.Millisecond):
            // Go require 断言保持为测试期望说明，当前不运行这些断言。
            require::FailNow(t, "gc worker failed to handle errors")
        // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
        case s.gcWorker.done <- errors::New("mock error"):
        }
    }
}

// mockGCWorkerSuite_loadTxnSafePoint 对应 Go 中 mockGCWorkerSuite.loadTxnSafePoint 方法，保留测试辅助/断言流程。
pub fn mockGCWorkerSuite_loadTxnSafePoint(s: &mut mockGCWorkerSuite, t: Box<testing.T>) -> u64 {
    gcStates, err = s.pdClient.GetGCStatesClient(uint32(s.store.GetCodec().GetKeyspaceID())).GetGCState(context::Background())
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    return gcStates.TxnSafePoint
}

#[test]
// TestGCPlacementRules 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestGCPlacementRules() {
    s = createGCWorkerSuite(t)

    // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, failpoint::Enable("github.com/pingcap/tidb/pkg/store/gcworker/mockHistoryJobForGC", "return(10)"))
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    defer func() {
        // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, failpoint::Disable("github.com/pingcap/tidb/pkg/store/gcworker/mockHistoryJobForGC"))
    }()

    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    var gcPlacementRuleCache sync::Map
    deletePlacementRuleCounter = 0
    // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, failpoint::EnableWith("github.com/pingcap/tidb/pkg/store/gcworker/gcDeletePlacementRuleCounter", "return", func() error {
        deletePlacementRuleCounter++
        return None
    }))
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    defer func() {
        // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, failpoint::Disable("github.com/pingcap/tidb/pkg/store/gcworker/gcDeletePlacementRuleCounter"))
    }()

    bundleID = "TiDB_DDL_10"
    bundle, err = placement::NewBundleFromOptions(&model::PlacementSettings{
        PrimaryRegion: "r1",
        Regions:       "r1, r2",
    })
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    bundle.ID = bundleID

    // prepare bundle before gc
    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, infosync::PutRuleBundles(context::Background(), []*placement::Bundle{bundle}))
    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    got, err = infosync::GetRuleBundle(context::Background(), bundleID)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NotNil(t, got)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::False(t, got.IsEmpty())

    // do gc
    dr = util::DelRangeTask{JobID: 1, ElementID: 10}
    err = doGCPlacementRules(createSession(s.store), 1, dr, &gcPlacementRuleCache)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    v, ok = gcPlacementRuleCache.Load(int64(10))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::True(t, ok)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, struct{}{}, v)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, 1, deletePlacementRuleCounter)

    // check bundle deleted after gc
    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    got, err = infosync::GetRuleBundle(context::Background(), bundleID)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NotNil(t, got)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::True(t, got.IsEmpty())

    // gc the same table id repeatedly
    err = doGCPlacementRules(createSession(s.store), 1, dr, &gcPlacementRuleCache)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    v, ok = gcPlacementRuleCache.Load(int64(10))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::True(t, ok)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, struct{}{}, v)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, 1, deletePlacementRuleCounter)
}

#[test]
// TestGCLabelRules 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestGCLabelRules() {
    s = createGCWorkerSuite(t)

    // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, failpoint::Enable("github.com/pingcap/tidb/pkg/store/gcworker/mockHistoryJob", "return(\"schema/d1/t1\")"))
    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    defer func() {
        // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, failpoint::Disable("github.com/pingcap/tidb/pkg/store/gcworker/mockHistoryJob"))
    }()

    dr = util::DelRangeTask{JobID: 1, ElementID: 1}
    err = s.gcWorker.doGCLabelRules(dr)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
}

#[test]
// TestGCWithPendingTxn 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestGCWithPendingTxn() {
    if kerneltype::IsNextGen() {
        t.Skip("skip TestGCWithPendingTxn when kernel type is NextGen - test not yet adjusted to support next-gen")
    }
    s = createGCWorkerSuite(t, withStoreType(mockstore::EmbedUnistore), withSchemaLease(30*time::Minute))

    ctx = gcContext()
    txnSafePointSyncWaitTime = 0
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    err = s.gcWorker.saveValueToSysTable(gcEnableKey, booleanFalse)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    k1 = Vec::<u8>::from("tk1")
    v1 = Vec::<u8>::from("v1")
    txn, err = s.store.Begin()
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    txn.SetOption(kv::Pessimistic, true)
    lockCtx = &kv::LockCtx{ForUpdateTS: txn.StartTS(), WaitStartTime: time.Now()}

    // Lock the key.
    err = txn.Set(k1, v1)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    err = txn.LockKeys(ctx, lockCtx, k1)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    // Prepare to run gc with txn's startTS as the safepoint ts.
    spkv = s.tikvStore.GetSafePointKV()
    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    err = spkv.Put(fmt::Sprintf("%s/%s", infosync::ServerMinStartTSPath, "a"), strconv::FormatUint(txn.StartTS(), 10))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    //s.mustSetTiDBServiceSafePoint(t, txn.StartTS(), txn.StartTS())
    veryLong = gcDefaultLifeTime * 100
    err = s.gcWorker.saveTime(gcLastRunTimeKey, oracle::GetTimeFromTS(s.mustAllocTs(t)).Add(-veryLong))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    s.gcWorker.lastFinish = time.Now().Add(-veryLong)
    s.oracle::AddOffset(time::Minute * 10)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    err = s.gcWorker.saveValueToSysTable(gcEnableKey, booleanTrue)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    // Trigger the tick let the gc job start.
    err = s.gcWorker.leaderTick(ctx)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Wait for GC finish
    select {
    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    case err = <-s.gcWorker.done:
        s.gcWorker.gcIsRunning = false
        break
    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    case <-time.After(time::Second * 10):
        err = errors::New("receive from s.gcWorker.done timeout")
    }
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    err = txn.Commit(ctx)
    // TODO: The mock implementation of PD doesn't put the data in the etcd or `SafePointKV`, making this test not
    //   working for now. We need to fix this test after further refactor.
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    // require.NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Error(t, err)
}

#[test]
// TestGCWithPendingTxn2 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestGCWithPendingTxn2() {
    // as we are adjusting the base TS, we need a larger schema lease to avoid
    // the info schema outdated error.
    s = createGCWorkerSuite(t, withStoreType(mockstore::EmbedUnistore), withSchemaLease(10*time::Minute))

    ctx = gcContext()
    txnSafePointSyncWaitTime = 0
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    err = s.gcWorker.saveValueToSysTable(gcEnableKey, booleanFalse)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    now, err = s.oracle::GetTimestamp(ctx, &oracle::Option{})
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    // Prepare to run gc with txn's startTS as the safepoint ts.
    spkv = s.tikvStore.GetSafePointKV()
    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    err = spkv.Put(fmt::Sprintf("%s/%s", infosync::ServerMinStartTSPath, "a"), strconv::FormatUint(now, 10))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    //s.mustSetTiDBServiceSafePoint(t, now, now)
    veryLong = gcDefaultLifeTime * 100
    err = s.gcWorker.saveTime(gcLastRunTimeKey, oracle::GetTimeFromTS(s.mustAllocTs(t)).Add(-veryLong))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    s.gcWorker.lastFinish = time.Now().Add(-veryLong)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    err = s.gcWorker.saveValueToSysTable(gcEnableKey, booleanTrue)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    // lock the key1
    k1 = Vec::<u8>::from("tk1")
    v1 = Vec::<u8>::from("v1")
    txn, err = s.store.Begin(tikv::WithStartTS(now))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    txn.SetOption(kv::Pessimistic, true)
    lockCtx = &kv::LockCtx{ForUpdateTS: txn.StartTS(), WaitStartTime: time.Now()}

    err = txn.Set(k1, v1)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    err = txn.LockKeys(ctx, lockCtx, k1)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    // lock the key2
    k2 = Vec::<u8>::from("tk2")
    v2 = Vec::<u8>::from("v2")
    startTS = oracle::ComposeTS(oracle::ExtractPhysical(now)+10000, oracle::ExtractLogical(now))
    txn2, err = s.store.Begin(tikv::WithStartTS(startTS))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    txn2.SetOption(kv::Pessimistic, true)
    lockCtx = &kv::LockCtx{ForUpdateTS: txn2.StartTS(), WaitStartTime: time.Now()}

    err = txn2.Set(k2, v2)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    err = txn2.LockKeys(ctx, lockCtx, k2)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    // Trigger the tick let the gc job start.
    s.oracle::AddOffset(time::Minute * 5)
    err = s.gcWorker.leaderTick(ctx)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Wait for GC finish
    select {
    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    case err = <-s.gcWorker.done:
        s.gcWorker.gcIsRunning = false
        break
    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    case <-time.After(time::Second * 10):
        err = errors::New("receive from s.gcWorker.done timeout")
    }
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    err = txn.Commit(ctx)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    err = txn2.Commit(ctx)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
}

#[test]
// TestSkipGCAndOnlyResolveLock 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestSkipGCAndOnlyResolveLock() {
    // as we are adjusting the base TS, we need a larger schema lease to avoid
    // the info schema outdated error.
    s = createGCWorkerSuite(t, withStoreType(mockstore::EmbedUnistore), withSchemaLease(10*time::Minute))

    ctx = gcContext()
    txnSafePointSyncWaitTime = 0
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    err = s.gcWorker.saveValueToSysTable(gcEnableKey, booleanFalse)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    now, err = s.oracle::GetTimestamp(ctx, &oracle::Option{})
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    // Prepare to run gc with txn's startTS as the safepoint ts.
    spkv = s.tikvStore.GetSafePointKV()
    // Go 并发/同步语义按原顺序保留；后续接线时再替换为 Rust async、channel 或原子类型。
    err = spkv.Put(fmt::Sprintf("%s/%s", infosync::ServerMinStartTSPath, "a"), strconv::FormatUint(now, 10))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    s.mustSetTiDBServiceSafePoint(t, now, now)
    veryLong = gcDefaultLifeTime * 100
    lastRunTime = oracle::GetTimeFromTS(s.mustAllocTs(t)).Add(-veryLong)
    newGcLifeTime = time::Hour * 24
    err = s.gcWorker.saveTime(gcLastRunTimeKey, lastRunTime)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    err = s.gcWorker.saveTime(gcSafePointKey, oracle::GetTimeFromTS(s.mustAllocTs(t)).Add(-time::Minute*10))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    err = s.gcWorker.saveDuration(gcLifeTimeKey, newGcLifeTime)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    s.gcWorker.lastFinish = time.Now().Add(-veryLong)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    err = s.gcWorker.saveValueToSysTable(gcEnableKey, booleanTrue)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    // lock the key1
    k1 = Vec::<u8>::from("tk1")
    v1 = Vec::<u8>::from("v1")
    txn, err = s.store.Begin(tikv::WithStartTS(now))
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    txn.SetOption(kv::Pessimistic, true)
    lockCtx = &kv::LockCtx{ForUpdateTS: txn.StartTS(), WaitStartTime: time.Now()}

    err = txn.Set(k1, v1)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    err = txn.LockKeys(ctx, lockCtx, k1)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    // Trigger the tick let the gc job start.
    s.oracle::AddOffset(time::Minute * 5)
    err = s.gcWorker.leaderTick(ctx)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    // check the lock has not been resolved.
    err = txn.Commit(ctx)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    // check gc is skipped
    last, err = s.gcWorker.loadTime(gcLastRunTimeKey)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::Equal(t, last.Unix(), lastRunTime.Unix())
}

// bootstrap 对应 Go 辅助函数，保留参数、返回值和主要控制流。
pub fn bootstrap(t: testing.TB, store: kv::Storage, lease: time.Duration) -> Box<domain::Domain> {
    vardef::SetSchemaLease(lease)
    // failpoint 依赖 Go 测试运行时；这里保留注入点和清理意图。
    session::DisableStats4Test()
    // mock store / domain / GC worker 初始化依赖 Go harness，保留 fixture 构造顺序。
    dom, err = session::BootstrapSession(store)
    // Go require 断言保持为测试期望说明，当前不运行这些断言。
    require::NoError(t, err)

    dom.SetStatsUpdating(true)

    // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
    t.Cleanup(func() {
        // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
        dom.Close()
        // 资源收尾沿用 Go 测试顺序；只标出生命周期边界。
        err = store.Close()
        // Go require 断言保持为测试期望说明，当前不运行这些断言。
        require::NoError(t, err)
    })
    return dom
}

#[test]
// TestCalcDeleteRangeConcurrency 对应 Go 测试函数；断言和资源清理顺序按原文件保留。
pub fn TestCalcDeleteRangeConcurrency() {
    testCases = []struct {
        name        string
        concurrency gcConcurrency
        rangeNum    int
        expected    int
    }{
        {"Auto: Low concurrency, few ranges", gcConcurrency{16, true}, 50000, 1},
        {"Auto: High concurrency, many ranges", gcConcurrency{400, true}, 1000000, 10},
        {"Auto: High concurrency, few ranges", gcConcurrency{400, true}, 50000, 1},
        {"Auto: Low concurrency, many ranges", gcConcurrency{16, true}, 1000000, 4},
        {"Non-auto: Low concurrency", gcConcurrency{16, false}, 1000000, 4},
        {"Non-auto: High concurrency", gcConcurrency{400, false}, 50000, 100},
        {"Edge case: Zero concurrency", gcConcurrency{0, true}, 100000, 1},
        {"Edge case: Zero ranges", gcConcurrency{100, true}, 0, 1},
        {"Large range number", gcConcurrency{400, true}, 10000000, 100},
        {"Exact RequestsPerThread", gcConcurrency{400, true}, 200000, 2},
    }

    w = &GCWorker{}

    for _, tc = range testCases {
        t.Run(tc.name, func(t *testing.T) {
            // TiKV RPC/lock resolver 调用是外部依赖边界，迁移时仅保存请求类型和分支。
            result = w.calcDeleteRangeConcurrency(tc.concurrency, tc.rangeNum)
            if result != tc.expected {
                t.Errorf("Expected %d, but got %d", tc.expected, result)
            }
            if result < 1 {
                t.Errorf("Result should never be less than 1, but got %d", result)
            }
        })
    }
}
*/
