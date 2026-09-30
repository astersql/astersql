// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// Coprocessor 行为测试：逐项覆盖 Go 的 row hint、store batch、共享限流、
// runaway CoolDown、small-cop 并发和 Region 分裂后的 lite-worker 回退契约。

use astersql_store_copr::{
    BatchedCopTask, CopBackend, CopClient, CopProtocolResponse, CopRequest, CopSmallTaskRow,
    CopTask, CopWireRequest, KeyCodec, KeyRange, KeyRanges, LocatedKeyRanges, PagingOptions,
    PartitionKeyRanges, Peer, RateLimit, RateLimitAction, RegionVerId, ReplicaReadType,
    RequestType, RunawayAction, RunawayChecker, build_key_ranges, is_small_task,
    set_lite_worker_fallback_hook_for_test, small_task_concurrency,
};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

fn get_keyspace_aware_key(codec: &KeyCodec, key: &[u8]) -> Vec<u8> {
    codec.encode_key(key)
}

#[test]
fn keyspace_aware_keys_use_the_store_codec() {
    assert_eq!(get_keyspace_aware_key(&KeyCodec::v1(), b"g"), b"g");
    let next_gen = KeyCodec::v2("tenant".to_owned(), 0x01_02_03).unwrap();
    assert_eq!(
        get_keyspace_aware_key(&next_gen, b"g"),
        vec![b'x', 0x01, 0x02, 0x03, b'g']
    );
}

/// 按行数 hint 构造最小 CopTask，用于小任务分类断言。
fn task(row_count_hint: isize) -> CopTask {
    CopTask {
        row_count_hint,
        ..CopTask::default()
    }
}

/// 验证 row_count_hint 如何划分大小任务，以及小任务并发上限。
#[test]
fn row_count_hint_controls_small_task_classification_and_concurrency() {
    assert!(!is_small_task(&task(-1)));
    assert!(!is_small_task(&task(0)));
    assert!(is_small_task(&task(1)));
    assert!(is_small_task(&task(CopSmallTaskRow as isize)));
    assert!(!is_small_task(&task(CopSmallTaskRow as isize + 1)));

    let tasks = (0..32).map(|_| task(1)).collect::<Vec<_>>();
    let (count, concurrency) = small_task_concurrency(&tasks, 2);
    assert_eq!(32, count);
    assert!(concurrency > 0);
    assert!(concurrency <= 16);
}

/// 成对键组装为 KeyRange，奇数个键应报错。
#[test]
fn paired_key_ranges_preserve_order_and_reject_odd_input() {
    let ranges = build_key_ranges(&["a", "c", "d", "f"]).unwrap();
    assert_eq!(2, ranges.len());
    assert_eq!(b"a", ranges[0].start.as_slice());
    assert_eq!(b"f", ranges[1].end.as_slice());
    assert!(build_key_ranges(&["a", "b", "c"]).is_err());
}

/// runaway 限速：仅在超限周期后通过 destroy_token 归还令牌。
#[test]
fn runaway_rate_limit_consumes_token_only_after_an_exceeded_cycle() {
    let limiter = RateLimitAction::new(3);
    let returned = AtomicUsize::new(0);
    limiter.destroy_token_if_needed(|| {
        returned.fetch_add(1, Ordering::SeqCst);
    });
    assert_eq!(1, returned.load(Ordering::SeqCst));

    limiter.set_enabled(true);
    assert!(limiter.action());
    limiter.destroy_token_if_needed(|| {
        returned.fetch_add(1, Ordering::SeqCst);
    });
    assert_eq!(1, returned.load(Ordering::SeqCst));
    assert_eq!(3, limiter.total_tokens());
    limiter.close();
    assert!(!limiter.is_enabled());
}

#[derive(Default)]
struct TestBackend {
    locations: Mutex<Vec<LocatedKeyRanges>>,
    wires: Mutex<Vec<astersql_store_copr::CopWireRequest>>,
    allow_batch: bool,
}

impl TestBackend {
    fn with_locations(locations: Vec<LocatedKeyRanges>) -> Arc<Self> {
        Arc::new(Self {
            locations: Mutex::new(locations),
            ..Self::default()
        })
    }
}

impl CopBackend for TestBackend {
    fn split_key_ranges(
        &self,
        _ranges: &KeyRanges,
        _skip_buckets: bool,
    ) -> astersql_store_copr::BatchResult<Vec<LocatedKeyRanges>> {
        Ok(self.locations.lock().unwrap().clone())
    }

    fn build_batch_task(
        &self,
        task: &CopTask,
        _replica_read: ReplicaReadType,
    ) -> astersql_store_copr::BatchResult<Option<BatchedCopTask>> {
        Ok(self.allow_batch.then(|| BatchedCopTask {
            task: Box::new(task.clone()),
            store_id: 1,
            peer: Some(Peer { id: 1, store_id: 1 }),
            load_based_replica_retry: false,
        }))
    }

    fn tidb_server_addresses(&self) -> astersql_store_copr::BatchResult<Vec<(u64, String)>> {
        Ok(Vec::new())
    }

    fn send(
        &self,
        _task: &CopTask,
        request: &astersql_store_copr::CopWireRequest,
    ) -> astersql_store_copr::BatchResult<CopProtocolResponse> {
        self.wires.lock().unwrap().push(request.clone());
        Ok(CopProtocolResponse::default())
    }

    fn invalidate_region(&self, _region: RegionVerId) {}

    fn update_buckets(&self, _region: RegionVerId, _old_version: u64, _new_version: u64) {}

    fn resolve_lock(&self, _lock: &[u8], _start_ts: u64) -> astersql_store_copr::BatchResult<()> {
        Ok(())
    }

    fn check_visibility(&self, _start_ts: u64) -> astersql_store_copr::BatchResult<()> {
        Ok(())
    }
}

fn key_range(start: &str, end: &str) -> KeyRange {
    KeyRange {
        start: start.as_bytes().to_vec(),
        end: end.as_bytes().to_vec(),
    }
}

fn located_region(id: u64, ranges: Vec<KeyRange>) -> LocatedKeyRanges {
    LocatedKeyRanges {
        region: RegionVerId::new(id, 1, 1),
        location_start: ranges
            .first()
            .map(|range| range.start.clone())
            .unwrap_or_default(),
        location_end: ranges
            .last()
            .map(|range| range.end.clone())
            .unwrap_or_default(),
        ranges: KeyRanges::new(ranges),
        store_id: 1,
        store_address: "store-1".to_owned(),
        ..LocatedKeyRanges::default()
    }
}

fn cop_request(ranges: Vec<KeyRange>, row_hints: Vec<usize>) -> CopRequest {
    CopRequest {
        key_ranges: vec![PartitionKeyRanges { ranges, row_hints }],
        concurrency: 15,
        ..CopRequest::default()
    }
}

#[test]
fn TestBuildCopIteratorWithRowCountHint() {
    let ranges = vec![
        key_range("a", "c"),
        key_range("d", "e"),
        key_range("h", "x"),
        key_range("y", "z"),
    ];
    let backend = TestBackend::with_locations(vec![
        located_region(1, vec![key_range("a", "c"), key_range("d", "e")]),
        located_region(2, vec![key_range("h", "n")]),
        located_region(3, vec![key_range("n", "t")]),
        located_region(4, vec![key_range("t", "x"), key_range("y", "z")]),
    ]);
    let client = CopClient::new(backend, None, 4);

    let mut request = cop_request(ranges.clone(), vec![1, 1, 3, CopSmallTaskRow]);
    let iterator = client.build_cop_iterator(&mut request).unwrap();
    assert_eq!(iterator.concurrency(), (1, 1));
    assert_eq!(iterator.send_rate().capacity(), 2);

    let mut request = cop_request(ranges, vec![1, 1, 3, 3]);
    let iterator = client.build_cop_iterator(&mut request).unwrap();
    assert_eq!(iterator.concurrency(), (1, 2));
    assert_eq!(iterator.send_rate().capacity(), 3);

    let ranges = vec![key_range("a", "z")];
    let backend = TestBackend::with_locations(vec![
        located_region(1, vec![key_range("a", "g")]),
        located_region(2, vec![key_range("g", "n")]),
        located_region(3, vec![key_range("n", "t")]),
        located_region(4, vec![key_range("t", "z")]),
    ]);
    let client = CopClient::new(backend, None, 4);
    let mut request = cop_request(ranges.clone(), vec![10]);
    let iterator = client.build_cop_iterator(&mut request).unwrap();
    assert_eq!(iterator.concurrency(), (1, 2));
    assert_eq!(iterator.send_rate().capacity(), 3);

    let mut request = cop_request(ranges, vec![CopSmallTaskRow + 1]);
    let iterator = client.build_cop_iterator(&mut request).unwrap();
    assert_eq!(iterator.concurrency(), (4, 0));
    assert_eq!(iterator.send_rate().capacity(), 4);
}

#[test]
fn TestBuildCopIteratorWithBatchStoreCopr() {
    let ranges = vec![
        key_range("a", "c"),
        key_range("d", "e"),
        key_range("h", "x"),
        key_range("y", "z"),
    ];
    let backend = TestBackend {
        locations: Mutex::new(vec![
            located_region(1, vec![key_range("a", "c"), key_range("d", "e")]),
            located_region(2, vec![key_range("h", "n")]),
            located_region(3, vec![key_range("n", "t")]),
            located_region(4, vec![key_range("t", "x"), key_range("y", "z")]),
        ]),
        allow_batch: true,
        ..TestBackend::default()
    };
    let client = CopClient::new(Arc::new(backend), None, 4);

    let mut request = cop_request(ranges.clone(), vec![1, 1, 3, 3]);
    request.store_batch_size = 3;
    let iterator = client.build_cop_iterator(&mut request).unwrap();
    assert_eq!(iterator.tasks().len(), 1);
    assert_eq!(iterator.tasks()[0].to_pb_batch_tasks().len(), 3);
    assert_eq!(iterator.tasks()[0].row_count_hint, 14);

    let mut request = cop_request(ranges.clone(), vec![1, 1, 3, 3]);
    request.store_batch_size = 1;
    let iterator = client.build_cop_iterator(&mut request).unwrap();
    assert_eq!(iterator.tasks().len(), 2);
    assert_eq!(iterator.tasks()[0].to_pb_batch_tasks().len(), 1);
    assert_eq!(iterator.tasks()[0].row_count_hint, 5);
    assert_eq!(iterator.tasks()[1].to_pb_batch_tasks().len(), 1);
    assert_eq!(iterator.tasks()[1].row_count_hint, 9);

    let mut request = cop_request(ranges.clone(), vec![1, 1, 3, 3]);
    request.store_batch_size = 3;
    request.paging = PagingOptions {
        enabled: true,
        minimum_size: 1,
        maximum_size: 1024,
        size_bytes: 0,
    };
    let iterator = client.build_cop_iterator(&mut request).unwrap();
    assert_eq!(iterator.tasks().len(), 4);
    assert!(
        iterator
            .tasks()
            .iter()
            .all(|task| task.to_pb_batch_tasks().is_empty())
    );

    let mut request = cop_request(ranges, vec![1, 1, 3, 3]);
    request.store_batch_size = 3;
    request.paging.size_bytes = 4 * 1024 * 1024;
    let iterator = client.build_cop_iterator(&mut request).unwrap();
    assert_eq!(iterator.tasks().len(), 4);
    assert_eq!(request.paging.size_bytes, 4 * 1024 * 1024);
    assert_eq!(request.store_batch_size, 0);

    let mut request = cop_request(vec![key_range("a", "c")], vec![1]);
    request.request_type = RequestType::Analyze;
    request.paging.size_bytes = 4 * 1024 * 1024;
    let _ = client.build_cop_iterator(&mut request).unwrap();
    assert_eq!(request.paging.size_bytes, 0);

    let backend = TestBackend {
        locations: Mutex::new(vec![
            located_region(1, vec![key_range("a", "b")]),
            located_region(2, vec![key_range("h", "i")]),
            located_region(3, vec![key_range("o", "p")]),
        ]),
        allow_batch: true,
        ..TestBackend::default()
    };
    let client = CopClient::new(Arc::new(backend), None, 4);
    let mut request = cop_request(
        vec![
            key_range("a", "b"),
            key_range("h", "i"),
            key_range("o", "p"),
        ],
        vec![1, CopSmallTaskRow + 1, CopSmallTaskRow],
    );
    request.store_batch_size = 3;
    let iterator = client.build_cop_iterator(&mut request).unwrap();
    assert_eq!(iterator.tasks().len(), 2);
    assert_eq!(iterator.tasks()[0].to_pb_batch_tasks().len(), 1);
    assert!(iterator.tasks()[1].to_pb_batch_tasks().is_empty());
}

#[test]
fn go_merge_48_build_iterator_preserves_shared_request_limiter() {
    for keep_order in [true, false] {
        let shared = astersql_kv::NewCoprRequestLimiter(7).unwrap();
        let backend =
            TestBackend::with_locations(vec![located_region(1, vec![key_range("a", "z")])]);
        let client = CopClient::new(backend, None, 4);
        let mut request = cop_request(vec![key_range("a", "z")], Vec::new());
        request.keep_order = keep_order;
        request.copr_request_limiter = Some(Arc::clone(&shared));
        let iterator = client.build_cop_iterator(&mut request).unwrap();
        assert!(Arc::ptr_eq(&shared, iterator.request_limiter().unwrap()));
        assert_eq!(iterator.request_limiter().unwrap().Capacity(), 7);
    }
}

#[derive(Debug)]
struct CoolDownChecker {
    before_executor_calls: AtomicUsize,
    before_request_calls: AtomicUsize,
    reset_calls: AtomicUsize,
}

impl CoolDownChecker {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            before_executor_calls: AtomicUsize::new(0),
            before_request_calls: AtomicUsize::new(0),
            reset_calls: AtomicUsize::new(0),
        })
    }
}

impl RunawayChecker for CoolDownChecker {
    fn before_executor(&self) -> astersql_store_copr::BatchResult<()> {
        self.before_executor_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn before_cop_request(
        &self,
        request: &mut CopWireRequest,
    ) -> astersql_store_copr::BatchResult<()> {
        self.before_request_calls.fetch_add(1, Ordering::SeqCst);
        request.resource_group_name = "rg1".to_owned();
        Ok(())
    }

    fn reset_total_processed_keys(&self) {
        self.reset_calls.fetch_add(1, Ordering::SeqCst);
    }

    fn check_action(&self) -> RunawayAction {
        RunawayAction::CoolDown
    }
}

#[test]
fn TestBuildCopIteratorWithRunawayChecker() {
    let backend = TestBackend::with_locations(vec![
        located_region(1, vec![key_range("a", "c"), key_range("d", "e")]),
        located_region(2, vec![key_range("h", "n")]),
        located_region(3, vec![key_range("n", "t")]),
        located_region(4, vec![key_range("t", "x"), key_range("y", "z")]),
    ]);
    let client = CopClient::new(backend.clone(), None, 4);
    let checker = CoolDownChecker::new();
    checker.before_executor().unwrap();
    let mut request = cop_request(
        vec![
            key_range("a", "c"),
            key_range("d", "e"),
            key_range("h", "x"),
            key_range("y", "z"),
        ],
        vec![1, 1, 3, 3],
    );
    request.resource_group_name = "rg1".to_owned();
    request.runaway_checker = Some(checker.clone());
    let mut iterator = client.build_cop_iterator(&mut request).unwrap();
    assert_eq!(iterator.concurrency(), (1, 0));
    assert_eq!(checker.before_executor_calls.load(Ordering::SeqCst), 1);

    while iterator.next().unwrap().is_some() {}
    assert_eq!(checker.before_request_calls.load(Ordering::SeqCst), 4);
    assert_eq!(checker.reset_calls.load(Ordering::SeqCst), 1);
    assert!(
        backend
            .wires
            .lock()
            .unwrap()
            .iter()
            .all(|wire| wire.resource_group_name == "rg1")
    );
}

struct SlowBackend {
    locations: Mutex<Vec<LocatedKeyRanges>>,
    delay: Duration,
    active: AtomicUsize,
    peak: AtomicUsize,
}

impl SlowBackend {
    fn new(locations: Vec<LocatedKeyRanges>, delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            locations: Mutex::new(locations),
            delay,
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        })
    }
}

impl CopBackend for SlowBackend {
    fn split_key_ranges(
        &self,
        _ranges: &KeyRanges,
        _skip_buckets: bool,
    ) -> astersql_store_copr::BatchResult<Vec<LocatedKeyRanges>> {
        Ok(self.locations.lock().unwrap().clone())
    }

    fn build_batch_task(
        &self,
        _task: &CopTask,
        _replica_read: ReplicaReadType,
    ) -> astersql_store_copr::BatchResult<Option<BatchedCopTask>> {
        Ok(None)
    }

    fn tidb_server_addresses(&self) -> astersql_store_copr::BatchResult<Vec<(u64, String)>> {
        Ok(Vec::new())
    }

    fn send(
        &self,
        _task: &CopTask,
        _request: &CopWireRequest,
    ) -> astersql_store_copr::BatchResult<CopProtocolResponse> {
        let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
        self.peak.fetch_max(active, Ordering::AcqRel);
        thread::sleep(self.delay);
        self.active.fetch_sub(1, Ordering::AcqRel);
        Ok(CopProtocolResponse::default())
    }

    fn invalidate_region(&self, _region: RegionVerId) {}

    fn update_buckets(&self, _region: RegionVerId, _old_version: u64, _new_version: u64) {}

    fn resolve_lock(&self, _lock: &[u8], _start_ts: u64) -> astersql_store_copr::BatchResult<()> {
        Ok(())
    }

    fn check_visibility(&self, _start_ts: u64) -> astersql_store_copr::BatchResult<()> {
        Ok(())
    }
}

fn ten_small_ranges() -> (Vec<KeyRange>, Vec<LocatedKeyRanges>) {
    let ranges = (0..10)
        .map(|index| key_range(&format!("k{index:02}"), &format!("k{:02}", index + 1)))
        .collect::<Vec<_>>();
    let locations = ranges
        .iter()
        .enumerate()
        .map(|(index, range)| located_region(index as u64 + 1, vec![range.clone()]))
        .collect();
    (ranges, locations)
}

#[test]
fn TestQueryWithConcurrentSmallCop() {
    let (ranges, locations) = ten_small_ranges();
    let backend = SlowBackend::new(locations, Duration::from_millis(100));
    let client = CopClient::new(backend.clone(), None, 4);
    let mut request = cop_request(ranges, vec![1; 10]);
    let mut iterator = client.build_cop_iterator(&mut request).unwrap();
    assert_eq!(iterator.concurrency(), (1, 4));

    let started = Instant::now();
    let mut responses = 0;
    while iterator.next().unwrap().is_some() {
        responses += 1;
    }
    assert_eq!(responses, 10);
    assert!(backend.peak.load(Ordering::Acquire) > 1);
    assert!(started.elapsed() < Duration::from_millis(500));
}

#[test]
fn shared_request_rate_limit_caps_in_flight_send_attempts() {
    let (ranges, locations) = ten_small_ranges();
    let backend = SlowBackend::new(locations, Duration::from_millis(20));
    let client = CopClient::new(backend.clone(), None, 4);
    let limiter = Arc::new(RateLimit::new(2));
    let mut request = cop_request(ranges, vec![CopSmallTaskRow + 1; 10]);
    request.concurrency = 10;
    request.copr_request_rate_limit = Some(limiter);
    let mut iterator = client.build_cop_iterator(&mut request).unwrap();
    while iterator.next().unwrap().is_some() {}
    assert_eq!(backend.peak.load(Ordering::Acquire), 2);
}

#[derive(Default)]
struct LiteGate {
    entered: bool,
    released: bool,
}

struct SplitDuringLiteBackend {
    locations: Mutex<Vec<LocatedKeyRanges>>,
    gate: (Mutex<LiteGate>, Condvar),
    sends: AtomicUsize,
}

impl SplitDuringLiteBackend {
    fn new(initial: Vec<LocatedKeyRanges>) -> Arc<Self> {
        Arc::new(Self {
            locations: Mutex::new(initial),
            gate: (Mutex::new(LiteGate::default()), Condvar::new()),
            sends: AtomicUsize::new(0),
        })
    }

    fn wait_until_first_send(&self) {
        let gate = self.gate.0.lock().unwrap();
        let (gate, _) = self
            .gate
            .1
            .wait_timeout_while(gate, Duration::from_secs(5), |gate| !gate.entered)
            .unwrap();
        assert!(gate.entered, "timeout waiting for cop request");
    }

    fn split_and_release(&self, locations: Vec<LocatedKeyRanges>) {
        *self.locations.lock().unwrap() = locations;
        let mut gate = self.gate.0.lock().unwrap();
        gate.released = true;
        self.gate.1.notify_all();
    }
}

impl CopBackend for SplitDuringLiteBackend {
    fn split_key_ranges(
        &self,
        _ranges: &KeyRanges,
        _skip_buckets: bool,
    ) -> astersql_store_copr::BatchResult<Vec<LocatedKeyRanges>> {
        Ok(self.locations.lock().unwrap().clone())
    }

    fn build_batch_task(
        &self,
        _task: &CopTask,
        _replica_read: ReplicaReadType,
    ) -> astersql_store_copr::BatchResult<Option<BatchedCopTask>> {
        Ok(None)
    }

    fn tidb_server_addresses(&self) -> astersql_store_copr::BatchResult<Vec<(u64, String)>> {
        Ok(Vec::new())
    }

    fn send(
        &self,
        _task: &CopTask,
        _request: &CopWireRequest,
    ) -> astersql_store_copr::BatchResult<CopProtocolResponse> {
        if self.sends.fetch_add(1, Ordering::AcqRel) == 0 {
            let mut gate = self.gate.0.lock().unwrap();
            gate.entered = true;
            self.gate.1.notify_all();
            while !gate.released {
                gate = self.gate.1.wait(gate).unwrap();
            }
            return Ok(CopProtocolResponse {
                region_error: Some("epoch not match after split".to_owned()),
                ..CopProtocolResponse::default()
            });
        }
        Ok(CopProtocolResponse::default())
    }

    fn invalidate_region(&self, _region: RegionVerId) {}

    fn update_buckets(&self, _region: RegionVerId, _old_version: u64, _new_version: u64) {}

    fn resolve_lock(&self, _lock: &[u8], _start_ts: u64) -> astersql_store_copr::BatchResult<()> {
        Ok(())
    }

    fn check_visibility(&self, _start_ts: u64) -> astersql_store_copr::BatchResult<()> {
        Ok(())
    }
}

struct LiteHookReset;

impl Drop for LiteHookReset {
    fn drop(&mut self) {
        set_lite_worker_fallback_hook_for_test(None);
    }
}

fn lite_hook_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
}

#[test]
fn TestDMLWithLiteCopWorker() {
    let _lock = lite_hook_test_lock();
    let _reset = LiteHookReset;
    let fallback_triggered = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&fallback_triggered);
    set_lite_worker_fallback_hook_for_test(Some(Arc::new(move || {
        observed.store(true, Ordering::Release);
    })));

    // A single-region DML completes in the lite worker without falling back.
    let no_split_backend =
        TestBackend::with_locations(vec![located_region(1, vec![key_range("a", "z")])]);
    let no_split_client = CopClient::new(no_split_backend, None, 4);
    let mut no_split_request = cop_request(vec![key_range("a", "z")], Vec::new());
    let mut no_split_iterator = no_split_client
        .build_cop_iterator(&mut no_split_request)
        .unwrap();
    let no_split_try_state = Arc::new(AtomicU32::new(0));
    no_split_iterator.open_with_lite_worker(Arc::clone(&no_split_try_state));
    let mut no_split_responses = 0;
    while no_split_iterator.next().unwrap().is_some() {
        no_split_responses += 1;
    }
    assert_eq!(no_split_responses, 1);
    assert!(!fallback_triggered.load(Ordering::Acquire));
    assert_eq!(no_split_try_state.load(Ordering::Acquire), 0);

    let backend = SplitDuringLiteBackend::new(vec![located_region(1, vec![key_range("a", "z")])]);
    let client = CopClient::new(backend.clone(), None, 4);
    let mut request = cop_request(vec![key_range("a", "z")], Vec::new());
    let mut iterator = client.build_cop_iterator(&mut request).unwrap();
    let try_lite_worker = Arc::new(AtomicU32::new(0));
    iterator.open_with_lite_worker(Arc::clone(&try_lite_worker));

    let handle = thread::spawn(move || {
        let mut responses = 0;
        while iterator.next().unwrap().is_some() {
            responses += 1;
        }
        responses
    });
    backend.wait_until_first_send();
    backend.split_and_release(vec![
        located_region(2, vec![key_range("a", "m")]),
        located_region(3, vec![key_range("m", "z")]),
    ]);

    assert_eq!(handle.join().unwrap(), 2);
    assert!(fallback_triggered.load(Ordering::Acquire));
    assert_eq!(try_lite_worker.load(Ordering::Acquire), 0);
    assert_eq!(backend.sends.load(Ordering::Acquire), 3);

    // A fresh read after the split sees both regions and does not enter lite mode.
    fallback_triggered.store(false, Ordering::Release);
    let client = CopClient::new(backend, None, 4);
    let mut request = cop_request(vec![key_range("a", "z")], Vec::new());
    let mut iterator = client.build_cop_iterator(&mut request).unwrap();
    iterator.open_with_lite_worker(Arc::clone(&try_lite_worker));
    let mut responses = 0;
    while iterator.next().unwrap().is_some() {
        responses += 1;
    }
    assert_eq!(responses, 2);
    assert!(!fallback_triggered.load(Ordering::Acquire));
    assert_eq!(try_lite_worker.load(Ordering::Acquire), 0);
}
