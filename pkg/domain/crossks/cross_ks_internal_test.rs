// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 跨 KS Manager 内部行为测试：acquire/release、持有者跟踪、空闲驱逐与 GC 循环。
// 通过 CountingFactory 统计 create 次数，验证复用与再创建语义。

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::{
    CROSS_KEYSPACE_RUNTIME_IDLE_TIMEOUT, Cancellation, DdlClient, InfoCache, Lifecycle, Manager,
    ManagerError, RuntimeFactory, SYSTEM_KEYSPACE, SessionManager, SessionPool, Store, new_manager,
    new_schema_coordinator,
};

#[derive(Default)]
/// 测试 Store：记录 keyspace 与 close 次数。
struct TestStore {
    ks: String,
    close_count: AtomicUsize,
}

impl TestStore {
    fn new(ks: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            ks: ks.into(),
            close_count: AtomicUsize::new(0),
        })
    }
}

impl Store for TestStore {
    fn keyspace(&self) -> &str {
        &self.ks
    }

    fn close(&self) -> Result<(), ManagerError> {
        self.close_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Default)]
/// 测试 SessionPool：可挂 on_close 回调以验证锁外关闭。
struct TestSessPool {
    close_count: AtomicUsize,
    on_close: Mutex<Option<Box<dyn FnMut() + Send>>>,
}

impl TestSessPool {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

impl SessionPool for TestSessPool {
    fn close(&self) {
        self.close_count.fetch_add(1, Ordering::SeqCst);
        if let Some(on_close) = self.on_close.lock().unwrap().as_mut() {
            on_close();
        }
    }
}

#[derive(Default)]
/// 空 InfoCache 桩。
struct DummyInfoCache;
impl InfoCache for DummyInfoCache {}

/// 可计数/可预加载/可失败的 RuntimeFactory。
struct CountingFactory {
    create_count: AtomicUsize,
    managers: Mutex<HashMap<String, Arc<SessionManager>>>,
    fail: Mutex<Option<String>>,
}

impl CountingFactory {
    /// 构造空 CountingFactory。
    fn new() -> Arc<Self> {
        Arc::new(Self {
            create_count: AtomicUsize::new(0),
            managers: Mutex::new(HashMap::new()),
            fail: Mutex::new(None),
        })
    }

    /// 预置某 keyspace 的 SessionManager，供下一次 create 取用。
    fn preload(&self, ks: &str, manager: Arc<SessionManager>) {
        self.managers
            .lock()
            .unwrap()
            .insert(ks.to_string(), manager);
    }
}

impl RuntimeFactory for CountingFactory {
    fn create(&self, keyspace: &str) -> Result<Arc<SessionManager>, ManagerError> {
        if let Some(message) = self.fail.lock().unwrap().clone() {
            return Err(ManagerError(message));
        }
        self.create_count.fetch_add(1, Ordering::SeqCst);
        self.managers
            .lock()
            .unwrap()
            .remove(keyspace)
            .or_else(|| {
                Some(new_session_manager(
                    TestStore::new(keyspace),
                    TestSessPool::new(),
                ))
            })
            .ok_or_else(|| ManagerError("factory exhausted".into()))
    }
}

/// 组装带空 DDL 后端的测试 SessionManager。
fn new_session_manager(store: Arc<dyn Store>, pool: Arc<dyn SessionPool>) -> Arc<SessionManager> {
    Arc::new(SessionManager::new(
        store,
        Arc::new(DummyInfoCache),
        pool,
        Arc::new(new_schema_coordinator()),
        Arc::new(DdlClient::new(Arc::new(DummyDdlBackend))),
        Vec::<Arc<dyn Lifecycle>>::new(),
    ))
}

#[derive(Default)]
/// 空 DDL 后端桩。
struct DummyDdlBackend;
impl crate::DdlBackend for DummyDdlBackend {
    fn resolve_database(&self, _schema_id: i64) -> Result<Option<String>, crate::Error> {
        Ok(None)
    }
    fn resolve_table(
        &self,
        _schema_id: i64,
        _table_id: i64,
    ) -> Result<Option<(String, crate::TableMode)>, crate::Error> {
        Ok(None)
    }
    fn session_variables(&self) -> Result<crate::SessionVariables, crate::Error> {
        Ok(crate::SessionVariables::default())
    }
    fn refresh_server_state(&self) -> Result<(), crate::Error> {
        Ok(())
    }
    fn submit(&self, _job: &mut crate::AlterTableModeJob) -> Result<(), crate::Error> {
        Ok(())
    }
    fn notify_owner(&self) -> Result<(), crate::Error> {
        Ok(())
    }
    fn history_job(&self, _job_id: i64) -> Result<Option<crate::HistoryJobState>, crate::Error> {
        Ok(None)
    }
}

/// 构造测试 Manager，并在 nextgen 路径下 seed acquire 一次以预创建运行时。
fn new_test_manager(
    classic: bool,
    current_ks: &str,
    target_ks: &str,
) -> (
    Arc<Manager>,
    Arc<CountingFactory>,
    Arc<TestStore>,
    Arc<TestSessPool>,
) {
    let factory = CountingFactory::new();
    let store = TestStore::new(target_ks);
    let pool = TestSessPool::new();
    factory.preload(target_ks, new_session_manager(store.clone(), pool.clone()));
    let mgr = new_manager(classic, current_ks, factory.clone());
    // 非经典内核且目标非当前 KS 时，先 acquire/release 以强制创建预加载运行时。
    // Force-create the preloaded runtime by acquiring once when nextgen.
    if !classic && current_ks != target_ks {
        let handle = mgr.acquire(target_ks, "__seed__").unwrap();
        handle.release();
        // 重置 last_release_at，避免 seed 影响空闲超时断言。
        // Clear the seed holder side effects for idle-timeout tests.
        mgr.set_last_release_at(target_ks, Instant::now());
    }
    (mgr, factory, store, pool)
}

#[test]
/// 空 holder_id 必须被拒绝。
fn test_acquire_runtime_handle_rejects_empty_holder_id() {
    let factory = CountingFactory::new();
    let mgr = new_manager(false, SYSTEM_KEYSPACE, factory);
    let err = mgr
        .acquire("ks-runtime-empty-holderID", "")
        .err()
        .expect("empty holderID must fail");
    assert!(err.0.contains("holderID"));
}

#[test]
/// 经典内核路径禁止跨 KS acquire。
fn test_acquire_runtime_handle_rejects_classic_kernel() {
    let (mgr, _, _, _) = new_test_manager(true, SYSTEM_KEYSPACE, "ks-runtime-classic");
    let err = mgr
        .acquire("ks-runtime-classic", "test/holderID")
        .err()
        .expect("classic kernel must reject");
    assert!(
        err.0
            .contains("cross keyspace is not available in classic kernel or current keyspace")
    );
}

#[test]
/// 验证 holder 登记、重复 acquire 失败、release 后可空闲驱逐。
fn test_acquire_runtime_handle_tracks_holder_ids() {
    let target_ks = "ks-runtime-holderID";
    let factory = CountingFactory::new();
    let store = TestStore::new(target_ks);
    let pool = TestSessPool::new();
    factory.preload(target_ks, new_session_manager(store.clone(), pool.clone()));
    let mgr = new_manager(false, SYSTEM_KEYSPACE, factory);

    let first = mgr.acquire(target_ks, "holder-1").unwrap();
    assert!(Arc::ptr_eq(
        &first.store(),
        &(store.clone() as Arc<dyn Store>)
    ));
    assert!(Arc::ptr_eq(
        &first.system_session_pool(),
        &(pool.clone() as Arc<dyn SessionPool>)
    ));
    assert!(mgr.has_active_holder(target_ks, "holder-1"));

    let duplicate = mgr.acquire(target_ks, "holder-1");
    assert!(duplicate.is_err());
    let err = match duplicate {
        Err(err) => err,
        Ok(_) => panic!("duplicate holder must fail"),
    };
    assert!(err.0.contains("already acquired"));

    let second = mgr.acquire(target_ks, "holder-2").unwrap();
    assert!(mgr.has_active_holder(target_ks, "holder-1"));
    assert!(mgr.has_active_holder(target_ks, "holder-2"));

    first.release();
    first.release();
    assert!(!mgr.has_active_holder(target_ks, "holder-1"));
    assert!(mgr.has_active_holder(target_ks, "holder-2"));
    assert!(!mgr.last_release_at_is_some(target_ks));
    assert_eq!(pool.close_count.load(Ordering::SeqCst), 0);

    second.release();
    second.release();
    assert_eq!(mgr.active_holder_len(target_ks), Some(0));
    assert!(mgr.last_release_at_is_some(target_ks));
    assert_eq!(pool.close_count.load(Ordering::SeqCst), 0);

    let reacquired = mgr.acquire(target_ks, "holder-1").unwrap();
    assert!(mgr.has_active_holder(target_ks, "holder-1"));
    assert_eq!(pool.close_count.load(Ordering::SeqCst), 0);
    reacquired.release();
}

#[test]
/// 并发多 holder acquire/release，最终持有者集合应为空。
fn test_acquire_runtime_handle_concurrently_tracks_holder_ids() {
    let target_ks = "ks-runtime-concurrent-holderID";
    let factory = CountingFactory::new();
    let store = TestStore::new(target_ks);
    let pool = TestSessPool::new();
    factory.preload(target_ks, new_session_manager(store.clone(), pool.clone()));
    let mgr = new_manager(false, SYSTEM_KEYSPACE, factory.clone());

    let unique_holder_ids: Vec<String> = (0..15).map(|i| format!("holderID-{i}")).collect();
    const DUPLICATE_ATTEMPTS: usize = 8;
    const DUPLICATE_HOLDER: &str = "holder-duplicate";

    let start = Arc::new(std::sync::Barrier::new(
        unique_holder_ids.len() + DUPLICATE_ATTEMPTS,
    ));
    let mut handles = Vec::new();
    for holder_id in &unique_holder_ids {
        let mgr = Arc::clone(&mgr);
        let start = Arc::clone(&start);
        let holder_id = holder_id.clone();
        let target_ks = target_ks.to_string();
        handles.push(thread::spawn(move || {
            start.wait();
            let result = mgr.acquire(&target_ks, &holder_id);
            (holder_id, result)
        }));
    }
    for _ in 0..DUPLICATE_ATTEMPTS {
        let mgr = Arc::clone(&mgr);
        let start = Arc::clone(&start);
        let target_ks = target_ks.to_string();
        handles.push(thread::spawn(move || {
            start.wait();
            let result = mgr.acquire(&target_ks, DUPLICATE_HOLDER);
            (DUPLICATE_HOLDER.to_string(), result)
        }));
    }

    let mut success_by_holder: HashMap<String, usize> = HashMap::new();
    let mut successful_handles = Vec::new();
    let mut duplicate_error_count = 0usize;
    for handle in handles {
        let (holder_id, result) = handle.join().unwrap();
        match result {
            Ok(runtime) => {
                *success_by_holder.entry(holder_id).or_default() += 1;
                successful_handles.push(runtime);
            }
            Err(err) => {
                assert_eq!(holder_id, DUPLICATE_HOLDER);
                assert!(err.0.contains("already acquired"));
                duplicate_error_count += 1;
            }
        }
    }

    assert_eq!(duplicate_error_count, DUPLICATE_ATTEMPTS - 1);
    assert_eq!(successful_handles.len(), unique_holder_ids.len() + 1);
    assert_eq!(factory.create_count.load(Ordering::SeqCst), 1);
    assert_eq!(mgr.all_keyspaces().len(), 1);
    assert!(mgr.get(target_ks).is_some());
    assert_eq!(
        mgr.active_holder_len(target_ks),
        Some(unique_holder_ids.len() + 1)
    );
    assert_eq!(success_by_holder.get(DUPLICATE_HOLDER), Some(&1));
    assert!(mgr.has_active_holder(target_ks, DUPLICATE_HOLDER));
    for holder_id in &unique_holder_ids {
        assert_eq!(success_by_holder.get(holder_id), Some(&1));
        assert!(mgr.has_active_holder(target_ks, holder_id));
    }

    let release_start = Arc::new(std::sync::Barrier::new(successful_handles.len()));
    let mut release_handles = Vec::new();
    for runtime in successful_handles {
        let release_start = Arc::clone(&release_start);
        release_handles.push(thread::spawn(move || {
            release_start.wait();
            runtime.release();
            runtime.release();
        }));
    }
    for handle in release_handles {
        handle.join().unwrap();
    }
    assert_eq!(mgr.active_holder_len(target_ks), Some(0));
    assert!(mgr.last_release_at_is_some(target_ks));
    assert_eq!(pool.close_count.load(Ordering::SeqCst), 0);
}

#[test]
/// 仍有活跃 holder 时，sweep 不得驱逐该运行时。
fn test_evict_runtime_skips_active_holders() {
    let target_ks = "ks-evict-active";
    let factory = CountingFactory::new();
    let store = TestStore::new(target_ks);
    let pool = TestSessPool::new();
    factory.preload(target_ks, new_session_manager(store, pool.clone()));
    let mgr = new_manager(false, SYSTEM_KEYSPACE, factory);

    let first = mgr.acquire(target_ks, "holder-1").unwrap();
    let second = mgr.acquire(target_ks, "holder-2").unwrap();
    first.release();
    mgr.set_last_release_at(
        target_ks,
        Instant::now() - CROSS_KEYSPACE_RUNTIME_IDLE_TIMEOUT - Duration::from_secs(1),
    );
    mgr.sweep_idle_runtimes(CROSS_KEYSPACE_RUNTIME_IDLE_TIMEOUT);

    assert!(mgr.get(target_ks).is_some());
    assert!(mgr.has_active_holder(target_ks, "holder-2"));
    assert_eq!(pool.close_count.load(Ordering::SeqCst), 0);
    second.release();
}

#[test]
/// 空闲驱逐时 SessionPool.close 在 Manager 锁外执行（通过回调检测）。
fn test_evict_runtime_closes_idle_entry_outside_manager_lock() {
    let target_ks = "ks-evict-idle";
    let factory = CountingFactory::new();
    let store = TestStore::new(target_ks);
    let pool = TestSessPool::new();
    factory.preload(target_ks, new_session_manager(store.clone(), pool.clone()));
    let mgr = new_manager(false, SYSTEM_KEYSPACE, factory);
    {
        let mgr_for_close = Arc::clone(&mgr);
        *pool.on_close.lock().unwrap() = Some(Box::new(move || {
            // Would deadlock if SessionManager::close ran while holding Manager.state.
            let _ = mgr_for_close.all_keyspaces();
        }));
    }

    let handle = mgr.acquire(target_ks, "holder-1").unwrap();
    handle.release();
    mgr.set_last_release_at(
        target_ks,
        Instant::now() - CROSS_KEYSPACE_RUNTIME_IDLE_TIMEOUT - Duration::from_secs(1),
    );
    mgr.sweep_idle_runtimes(CROSS_KEYSPACE_RUNTIME_IDLE_TIMEOUT);

    assert!(mgr.get(target_ks).is_none());
    assert_eq!(pool.close_count.load(Ordering::SeqCst), 1);
    assert_eq!(store.close_count.load(Ordering::SeqCst), 1);
}

#[test]
/// 驱逐后再 acquire 应触发 factory 重新 create。
fn test_evict_runtime_reacquire_creates_new_runtime() {
    let target_ks = "ks-evict-reacquire";
    let factory = CountingFactory::new();
    let old_store = TestStore::new(target_ks);
    let old_pool = TestSessPool::new();
    factory.preload(
        target_ks,
        new_session_manager(old_store.clone(), old_pool.clone()),
    );
    let mgr = new_manager(false, SYSTEM_KEYSPACE, factory.clone());

    let handle = mgr.acquire(target_ks, "holder-1").unwrap();
    handle.release();
    mgr.set_last_release_at(
        target_ks,
        Instant::now() - CROSS_KEYSPACE_RUNTIME_IDLE_TIMEOUT - Duration::from_secs(1),
    );
    mgr.sweep_idle_runtimes(CROSS_KEYSPACE_RUNTIME_IDLE_TIMEOUT);
    assert_eq!(old_pool.close_count.load(Ordering::SeqCst), 1);
    assert_eq!(old_store.close_count.load(Ordering::SeqCst), 1);

    let create_before = factory.create_count.load(Ordering::SeqCst);
    let reacquired = mgr.acquire(target_ks, "holder-2").unwrap();
    assert!(!Arc::ptr_eq(
        &reacquired.store(),
        &(old_store.clone() as Arc<dyn Store>)
    ));
    assert_eq!(
        factory.create_count.load(Ordering::SeqCst),
        create_before + 1
    );
    reacquired.release();
}

#[test]
/// Manager.close 立即关闭全部运行时，无视空闲超时。
fn test_runtime_handle_manager_close_closes_all_entries_regardless_of_idle_timeout() {
    let first_ks = "ks-close-runtime-1";
    let second_ks = "ks-close-runtime-2";
    let factory = CountingFactory::new();
    let first_store = TestStore::new(first_ks);
    let first_pool = TestSessPool::new();
    let second_store = TestStore::new(second_ks);
    let second_pool = TestSessPool::new();
    factory.preload(
        first_ks,
        new_session_manager(first_store.clone(), first_pool.clone()),
    );
    factory.preload(
        second_ks,
        new_session_manager(second_store.clone(), second_pool.clone()),
    );
    let mgr = new_manager(false, SYSTEM_KEYSPACE, factory);

    let first = mgr.acquire(first_ks, "holder-1").unwrap();
    first.release();
    let second = mgr.acquire(second_ks, "holder-2").unwrap();
    second.release();
    mgr.set_last_release_at(first_ks, Instant::now());

    mgr.close();

    assert!(mgr.all_keyspaces().is_empty());
    assert_eq!(first_pool.close_count.load(Ordering::SeqCst), 1);
    assert_eq!(first_store.close_count.load(Ordering::SeqCst), 1);
    assert_eq!(second_pool.close_count.load(Ordering::SeqCst), 1);
    assert_eq!(second_store.close_count.load(Ordering::SeqCst), 1);
}

#[test]
/// GC 循环在 Cancellation 触发后退出。
fn test_gc_loop_exits_when_context_cancelled() {
    let factory = CountingFactory::new();
    let mgr = new_manager(false, SYSTEM_KEYSPACE, factory);
    let cancellation = Arc::new(Cancellation::default());
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let finished_thread = Arc::clone(&finished);
    let mgr_thread = Arc::clone(&mgr);
    let cancellation_thread = Arc::clone(&cancellation);
    thread::spawn(move || {
        mgr_thread.run_system_keyspace_gc_loop(cancellation_thread.as_ref());
        finished_thread.store(true, Ordering::SeqCst);
    });

    cancellation.cancel();

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if finished.load(Ordering::SeqCst) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("GC loop did not exit after cancellation");
}
