// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 跨 KS Manager 与 Alter Table Mode 路径的行为测试（对齐 Go crossks 测试）。
// 覆盖经典内核拒绝、运行时复用、以及 submit-only 的表模式变更流程。

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::{
    AlterTableModeJob, AlterTableModeTarget, Cancellation, DdlBackend, DdlClient, Error,
    HistoryJobState, InfoCache, Lifecycle, ManagerError, RuntimeFactory, SYSTEM_KEYSPACE,
    SchemaCoordinator, SessionManager, SessionPool, SessionVariables, Store, TableMode,
    new_manager, new_schema_coordinator,
};

#[derive(Default)]
/// 测试 Store。
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
/// 测试 SessionPool。
struct TestSessPool {
    close_count: AtomicUsize,
}

impl TestSessPool {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

impl SessionPool for TestSessPool {
    fn close(&self) {
        self.close_count.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
/// 空 InfoCache 桩。
struct DummyInfoCache;
impl InfoCache for DummyInfoCache {}

/// 可预加载/可注入失败的 RuntimeFactory，并统计 create 次数。
struct MapFactory {
    create_count: AtomicUsize,
    fail: Mutex<Option<String>>,
    managers: Mutex<HashMap<String, Arc<SessionManager>>>,
}

impl MapFactory {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            create_count: AtomicUsize::new(0),
            fail: Mutex::new(None),
            managers: Mutex::new(HashMap::new()),
        })
    }

    /// 预置 SessionManager。
    fn preload(&self, ks: &str, manager: Arc<SessionManager>) {
        self.managers
            .lock()
            .unwrap()
            .insert(ks.to_string(), manager);
    }
}

impl RuntimeFactory for MapFactory {
    fn create(&self, keyspace: &str) -> Result<Arc<SessionManager>, ManagerError> {
        if let Some(message) = self.fail.lock().unwrap().clone() {
            return Err(ManagerError(message));
        }
        self.create_count.fetch_add(1, Ordering::SeqCst);
        if let Some(manager) = self.managers.lock().unwrap().remove(keyspace) {
            return Ok(manager);
        }
        Ok(new_session_manager(
            TestStore::new(keyspace),
            TestSessPool::new(),
            Arc::new(new_schema_coordinator()),
            Arc::new(DdlClient::new(Arc::new(RecordingDdlBackend::default()))),
        ))
    }
}

/// 构造带 RecordingDdlBackend 的测试 SessionManager。
fn new_session_manager(
    store: Arc<dyn Store>,
    pool: Arc<dyn SessionPool>,
    coordinator: Arc<SchemaCoordinator>,
    ddl_client: Arc<DdlClient>,
) -> Arc<SessionManager> {
    Arc::new(SessionManager::new(
        store,
        Arc::new(DummyInfoCache),
        pool,
        coordinator,
        ddl_client,
        Vec::<Arc<dyn Lifecycle>>::new(),
    ))
}

#[derive(Default)]
/// 可配置解析结果与历史状态，并计数 submit/refresh 的 DDL 后端。
struct RecordingDdlBackend {
    database: Mutex<Option<String>>,
    table: Mutex<Option<(String, TableMode)>>,
    history: Mutex<Option<HistoryJobState>>,
    submit_count: AtomicUsize,
    refresh_count: AtomicUsize,
}

impl DdlBackend for RecordingDdlBackend {
    fn resolve_database(&self, _schema_id: i64) -> Result<Option<String>, Error> {
        Ok(self.database.lock().unwrap().clone())
    }
    fn resolve_table(
        &self,
        _schema_id: i64,
        _table_id: i64,
    ) -> Result<Option<(String, TableMode)>, Error> {
        Ok(self.table.lock().unwrap().clone())
    }
    fn session_variables(&self) -> Result<SessionVariables, Error> {
        Ok(SessionVariables::default())
    }
    fn refresh_server_state(&self) -> Result<(), Error> {
        self.refresh_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn submit(&self, job: &mut AlterTableModeJob) -> Result<(), Error> {
        self.submit_count.fetch_add(1, Ordering::SeqCst);
        if job.id == 0 {
            job.id = 1;
        }
        Ok(())
    }
    fn notify_owner(&self) -> Result<(), Error> {
        Ok(())
    }
    fn history_job(&self, _job_id: i64) -> Result<Option<HistoryJobState>, Error> {
        Ok(self.history.lock().unwrap().clone())
    }
}

#[derive(Default)]
/// 测试内部会话：记录 remove_lock_ddl_jobs 调用次数。
struct TestSession {
    id: u64,
    seen: Mutex<usize>,
}

impl crate::InternalSession for TestSession {
    fn id(&self) -> u64 {
        self.id
    }
    fn remove_lock_ddl_jobs(&self, _jobs: &HashMap<i64, crate::JobMdl>, _print_log: bool) {
        *self.seen.lock().unwrap() += 1;
    }
}

/// 对应 Go `TestManagerInClassical`：经典内核拒绝 get_or_create。
/// Corresponds to Go `TestManagerInClassical`.
#[test]
/// 经典内核下跨 KS get_or_create 应失败。
fn test_manager_in_classical() {
    let factory = MapFactory::new();
    let mgr = new_manager(true, SYSTEM_KEYSPACE, factory);
    let err = mgr
        .get_or_create("aaa")
        .err()
        .expect("classic kernel rejects cross keyspace");
    assert!(
        err.0
            .contains("cross keyspace is not available in classic kernel or current keyspace")
    );
}

/// Corresponds to Go `TestManager` subcases that do not require etcd/unistore.
#[test]
/// 验证 get_or_create 复用、当前 KS 拒绝、关闭后拒绝、以及 factory 失败传播。
fn test_manager() {
    let factory = MapFactory::new();
    let mgr = new_manager(false, SYSTEM_KEYSPACE, factory.clone());

    // same keyspace access
    let err = match mgr.get_or_create(SYSTEM_KEYSPACE) {
        Err(err) => err,
        Ok(_) => panic!("same keyspace must be rejected"),
    };
    assert!(
        err.0
            .contains("cross keyspace is not available in classic kernel or current keyspace")
    );

    // failed to get store / create session manager
    *factory.fail.lock().unwrap() = Some("failed to get store".into());
    let err = match mgr.get_or_create("ks1") {
        Err(err) => err,
        Ok(_) => panic!("factory failure must propagate"),
    };
    assert!(err.0.contains("failed to get store"));
    *factory.fail.lock().unwrap() = None;

    // cross keyspace session works: acquire runtime and keep coordinator bookkeeping
    for ks in ["ks1", "ks2", "ks3"] {
        let coordinator = Arc::new(new_schema_coordinator());
        let backend = Arc::new(RecordingDdlBackend::default());
        *backend.database.lock().unwrap() = Some("test".into());
        *backend.table.lock().unwrap() = Some(("t".into(), TableMode::Normal));
        *backend.history.lock().unwrap() = Some(HistoryJobState::Synced);
        let store = TestStore::new(ks);
        let pool = TestSessPool::new();
        factory.preload(
            ks,
            new_session_manager(
                store.clone(),
                pool.clone(),
                Arc::clone(&coordinator),
                Arc::new(DdlClient::new(backend)),
            ),
        );

        let handle = mgr.acquire(ks, &format!("holder-{ks}")).unwrap();
        assert_eq!(handle.store().keyspace(), ks);
        assert!(mgr.get(ks).is_some());

        let session = Arc::new(TestSession {
            id: 42,
            seen: Mutex::new(0),
        });
        coordinator.store_internal_session(session.clone());
        assert!(coordinator.contains_internal_session(42));
        assert!(coordinator.internal_session_count() >= 1);
        coordinator.delete_internal_session(42);
        assert_eq!(coordinator.internal_session_count(), 0);

        handle.release();
        mgr.close_ks(ks);
        assert!(mgr.get(ks).is_none());
        assert_eq!(pool.close_count.load(Ordering::SeqCst), 1);
        assert_eq!(store.close_count.load(Ordering::SeqCst), 1);
    }
}

/// Corresponds to Go `TestDomainAcquireKSRuntimeHandle`.
#[test]
/// 验证 RuntimeHandle 持有 Store/会话池，release 后可被空闲逻辑观察。
fn test_domain_acquire_ks_runtime_handle() {
    let target_ks = "ks-runtime-domain";
    let factory = MapFactory::new();
    let store = TestStore::new(target_ks);
    let pool = TestSessPool::new();
    factory.preload(
        target_ks,
        new_session_manager(
            store.clone(),
            pool.clone(),
            Arc::new(new_schema_coordinator()),
            Arc::new(DdlClient::new(Arc::new(RecordingDdlBackend::default()))),
        ),
    );
    let mgr = new_manager(false, SYSTEM_KEYSPACE, factory);

    let handle = mgr
        .acquire(target_ks, "test/domain-runtime-handle")
        .unwrap();
    assert!(Arc::ptr_eq(
        &handle.store(),
        &(store.clone() as Arc<dyn Store>)
    ));
    let sess_mgr = mgr.get(target_ks).expect("runtime must exist");
    assert!(Arc::ptr_eq(&sess_mgr.store(), &handle.store()));
    assert!(Arc::ptr_eq(
        &sess_mgr.system_session_pool(),
        &handle.system_session_pool()
    ));
    handle.release();
    mgr.close_ks(target_ks);
}

/// Corresponds to Go `TestDomainAlterTableModeInKeyspaceSubmitOnly` resolve/submit paths.
#[test]
/// 验证跨 KS Alter Table Mode：解析、提交、refresh/notify，以及历史状态轮询。
fn test_domain_alter_table_mode_in_keyspace_submit_only() {
    let target_ks = "ks-ddl-submit";
    let factory = MapFactory::new();
    let backend = Arc::new(RecordingDdlBackend::default());
    *backend.database.lock().unwrap() = Some("test".into());
    *backend.table.lock().unwrap() = Some(("t_mode".into(), TableMode::Normal));
    *backend.history.lock().unwrap() = Some(HistoryJobState::Synced);
    let store = TestStore::new(target_ks);
    let pool = TestSessPool::new();
    factory.preload(
        target_ks,
        new_session_manager(
            store,
            pool,
            Arc::new(new_schema_coordinator()),
            Arc::new(DdlClient::new(backend.clone())),
        ),
    );
    let mgr = new_manager(false, SYSTEM_KEYSPACE, factory);

    let mut req = AlterTableModeTarget {
        schema_id: 1,
        schema_name: "test".into(),
        table_id: 10,
        table_name: "t_mode".into(),
        current_mode: TableMode::Normal,
        target_mode: TableMode::Import,
    };

    {
        let handle = mgr
            .acquire(target_ks, "test/domain-alter-table-mode")
            .unwrap();
        handle
            .alter_table_mode(&Cancellation::default(), req.clone())
            .unwrap();
        handle.release();
    }
    assert_eq!(backend.submit_count.load(Ordering::SeqCst), 1);
    assert_eq!(backend.refresh_count.load(Ordering::SeqCst), 1);

    // Idempotent retry when already Import: resolve sees current Import == target.
    *backend.table.lock().unwrap() = Some(("t_mode".into(), TableMode::Import));
    {
        let handle = mgr
            .acquire(target_ks, "test/domain-alter-table-mode-retry")
            .unwrap();
        handle
            .alter_table_mode(&Cancellation::default(), req.clone())
            .unwrap();
        handle.release();
    }
    assert_eq!(backend.submit_count.load(Ordering::SeqCst), 1);

    req.schema_name = "renamed_test".into();
    {
        let handle = mgr
            .acquire(target_ks, "test/domain-alter-table-mode-schema-mismatch")
            .unwrap();
        let err = handle
            .alter_table_mode(&Cancellation::default(), req.clone())
            .unwrap_err();
        assert!(err.0.contains("expected schema name"));
        handle.release();
    }

    req.schema_name = "test".into();
    req.table_name = "renamed_t_mode".into();
    {
        let handle = mgr
            .acquire(target_ks, "test/domain-alter-table-mode-mismatch")
            .unwrap();
        let err = handle
            .alter_table_mode(&Cancellation::default(), req.clone())
            .unwrap_err();
        assert!(err.0.contains("expected table name"));
        handle.release();
    }

    req.table_name = "t_mode".into();
    req.target_mode = TableMode::Normal;
    *backend.table.lock().unwrap() = Some(("t_mode".into(), TableMode::Import));
    {
        let handle = mgr
            .acquire(target_ks, "test/domain-alter-table-mode")
            .unwrap();
        handle
            .alter_table_mode(&Cancellation::default(), req)
            .unwrap();
        handle.release();
    }
    assert_eq!(backend.submit_count.load(Ordering::SeqCst), 2);

    // Cancellation while waiting for history (upgrading / blocked DDL path).
    *backend.table.lock().unwrap() = Some(("t_mode_upgrade".into(), TableMode::Normal));
    *backend.history.lock().unwrap() = None;
    let cancellation = Arc::new(Cancellation::default());
    let mgr_thread = Arc::clone(&mgr);
    let cancellation_thread = Arc::clone(&cancellation);
    let join = thread::spawn(move || {
        let handle = mgr_thread
            .acquire(target_ks, "test/domain-alter-table-mode-upgrading")
            .unwrap();
        let result = handle.alter_table_mode(
            cancellation_thread.as_ref(),
            AlterTableModeTarget {
                schema_id: 1,
                schema_name: "test".into(),
                table_id: 11,
                table_name: "t_mode_upgrade".into(),
                current_mode: TableMode::Normal,
                target_mode: TableMode::Import,
            },
        );
        handle.release();
        result
    });
    thread::sleep(Duration::from_millis(150));
    cancellation.cancel();
    let err = join.join().unwrap().unwrap_err();
    assert!(err.0.contains("context cancelled"));

    mgr.close_ks(target_ks);
}
