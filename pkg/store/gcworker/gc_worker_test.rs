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
