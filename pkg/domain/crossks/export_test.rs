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

// 对应 Go `export_test.go`：导出测试用的 Manager.Get / Manager.CloseKS 行为验证。
// 使用一次性 RuntimeFactory 与桩 Store/SessionPool，确认关闭 KS 时资源被释放。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{
    DdlClient, InfoCache, Lifecycle, ManagerError, RuntimeFactory, SYSTEM_KEYSPACE, SessionManager,
    SessionPool, Store, new_manager, new_schema_coordinator,
};

#[derive(Default)]
/// 测试用 Store：记录所属 keyspace 与 close 调用次数。
struct TestStore {
    ks: String,
    close_count: AtomicUsize,
}

impl TestStore {
    /// 按 keyspace 名构造测试 Store。
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
/// 测试用 SessionPool：记录 close 次数。
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

#[derive(Default)]
/// 空 DDL 后端桩：所有操作成功且无副作用。
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

/// 只能 create 一次的 RuntimeFactory：取出预置 SessionManager 后即耗尽。
struct OneShotFactory {
    manager: std::sync::Mutex<Option<Arc<SessionManager>>>,
}

impl RuntimeFactory for OneShotFactory {
    fn create(&self, _keyspace: &str) -> Result<Arc<SessionManager>, ManagerError> {
        self.manager
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| ManagerError("factory exhausted".into()))
    }
}

/// 对应 Go `export_test.go` 的 `Manager.Get` / `Manager.CloseKS`。
/// Corresponds to Go `export_test.go` helpers `Manager.Get` / `Manager.CloseKS`.
#[test]
/// 验证：acquire 后 get 可见；close_ks 后不可见，且 pool/store 各 close 一次。
fn test_export_get_and_close_ks() {
    let store = TestStore::new("ks-export");
    let pool = TestSessPool::new();
    let manager = Arc::new(SessionManager::new(
        store.clone(),
        Arc::new(DummyInfoCache),
        pool.clone(),
        Arc::new(new_schema_coordinator()),
        Arc::new(DdlClient::new(Arc::new(DummyDdlBackend))),
        Vec::<Arc<dyn Lifecycle>>::new(),
    ));
    let factory = Arc::new(OneShotFactory {
        manager: std::sync::Mutex::new(Some(manager)),
    });
    let mgr = new_manager(false, SYSTEM_KEYSPACE, factory);

    assert!(mgr.get("ks-export").is_none());
    let handle = mgr.acquire("ks-export", "holder").unwrap();
    assert!(mgr.get("ks-export").is_some());
    handle.release();

    mgr.close_ks("ks-export");
    assert!(mgr.get("ks-export").is_none());
    assert_eq!(pool.close_count.load(Ordering::SeqCst), 1);
    assert_eq!(store.close_count.load(Ordering::SeqCst), 1);
}
