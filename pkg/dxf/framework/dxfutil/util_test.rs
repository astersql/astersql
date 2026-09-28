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

// dxfutil 工具函数的 mock 单元测试。
//
// 覆盖 AcquireTaskRuntime（同/跨 keyspace、Acquire 失败、session 失败）
// 与 CheckTaskRuntime（合法、store/session keyspace 不匹配）。

#![allow(non_snake_case)]

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::kv::*;
use crate::sessionctx;
use crate::sqlsvrapi::{KSRuntimeHandle, Runtime, Server, SqlSvrError};
use crate::util::{DestroyableSessionPool, NewSessionPool, PooledResource};
use crate::{AcquireTaskRuntime, CheckTaskRuntime, sessionProvider};
use sqlsvrapimock_dependency::domain::sqlsvrapi::mock::{
    MockKSRuntimeHandle, MockRuntime, MockServer,
};

/// 仅关心 GetKeyspace 的测试用 Storage。
struct StoreWithKeyspace {
    keyspace: String,
}

impl Storage for StoreWithKeyspace {
    fn Begin(&self, _options: &[tikv::TxnOption]) -> Result<Box<dyn Transaction>, Error> {
        panic!("unused in dxfutil util tests")
    }

    fn GetSnapshot(&self, _version: Version) -> Box<dyn Snapshot> {
        panic!("unused in dxfutil util tests")
    }

    fn GetClient(&self) -> &dyn Client {
        panic!("unused in dxfutil util tests")
    }

    fn GetMPPClient(&self) -> &dyn MPPClient {
        panic!("unused in dxfutil util tests")
    }

    fn Close(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn UUID(&self) -> String {
        format!("{}-store", self.keyspace)
    }

    fn CurrentVersion(&self, _txn_scope: &str) -> Result<Version, Error> {
        Ok(Version { Ver: 0 })
    }

    fn GetOracle(&self) -> &dyn oracle::Oracle {
        panic!("unused in dxfutil util tests")
    }

    fn SupportDeleteRange(&self) -> bool {
        false
    }

    fn Name(&self) -> String {
        "store-with-keyspace".to_owned()
    }

    fn Describe(&self) -> String {
        "dxfutil util test store".to_owned()
    }

    fn ShowStatus(&self, _ctx: &context::Context, _key: &str) -> Result<Box<dyn Any>, Error> {
        Ok(Box::new(()))
    }

    fn GetMemCache(&self) -> &dyn MemManager {
        panic!("unused in dxfutil util tests")
    }

    fn GetMinSafeTS(&self, _txn_scope: &str) -> u64 {
        0
    }

    fn GetLockWaits(&self) -> Result<Vec<deadlockpb::WaitForEntry>, Error> {
        Ok(Vec::new())
    }

    fn GetCodec(&self) -> tikv::Codec {
        tikv::Codec
    }

    fn SetOption(&self, _key: Box<dyn Any>, _value: Box<dyn Any>) {}

    fn GetOption(&self, _key: &dyn Any) -> Option<&dyn Any> {
        None
    }

    fn GetClusterID(&self) -> u64 {
        42
    }

    fn GetKeyspace(&self) -> String {
        self.keyspace.clone()
    }
}

/// 构造绑定指定 keyspace 的测试存储。
fn store_with_keyspace(keyspace: &str) -> Arc<dyn Storage + Send + Sync> {
    Arc::new(StoreWithKeyspace {
        keyspace: keyspace.to_owned(),
    })
}

/// 为 CheckTaskRuntime 构造只含一个会话的会话池。
fn new_check_task_runtime_session_pool(
    session_store: Arc<dyn Storage + Send + Sync>,
) -> Arc<dyn DestroyableSessionPool> {
    let session = sessionctx::Context::new(session_store, None);
    NewSessionPool(
        1,
        Arc::new(move || Ok(Arc::new(session.clone()) as PooledResource)),
        None,
        None,
        None,
    )
}

/// 构造 MockRuntime：固定 Store，可选配置 SysSessionPool。
fn new_check_task_runtime_mock_runtime(
    store: Arc<dyn Storage + Send + Sync>,
    session_pool: Option<Arc<dyn DestroyableSessionPool>>,
) -> Arc<dyn Runtime> {
    let mut runtime = MockRuntime::new();
    runtime.expect_Store().returning(move || Arc::clone(&store));
    if let Some(session_pool) = session_pool {
        runtime
            .expect_SysSessionPool()
            .returning(move || Arc::clone(&session_pool));
    }
    Arc::new(runtime)
}

/// 可注入会话或错误的 sessionProvider。
struct TaskSessionProvider {
    session: Option<sessionctx::Context>,
    error: Option<String>,
}

impl sessionProvider for TaskSessionProvider {
    fn WithNewSession<F>(&self, callback: F) -> Result<(), SqlSvrError>
    where
        F: FnOnce(sessionctx::Context) -> Result<(), SqlSvrError>,
    {
        if let Some(message) = &self.error {
            return Err(std::io::Error::other(message.clone()).into());
        }
        callback(self.session.clone().expect("provider session"))
    }
}

/// 用当前 keyspace 的会话与 Server 组装 TaskSessionProvider。
fn new_task_session_provider(
    server: Arc<dyn Server>,
    current_keyspace: &str,
) -> TaskSessionProvider {
    TaskSessionProvider {
        session: Some(sessionctx::Context::new(
            store_with_keyspace(current_keyspace),
            Some(server),
        )),
        error: None,
    }
}

#[test]
/// 四分支：同 KS 用 GetRuntime、异 KS 需 Release、Acquire 错误、session 错误。
fn test_acquire_task_runtime() {
    // current keyspace uses server runtime
    {
        let runtime = new_check_task_runtime_mock_runtime(store_with_keyspace("task_ks"), None);
        let expected_runtime = Arc::clone(&runtime);
        let mut server = MockServer::new();
        server
            .expect_GetRuntime()
            .times(1)
            .returning(move || Arc::clone(&expected_runtime));
        let server: Arc<dyn Server> = Arc::new(server);

        let (got_runtime, release_runtime) = AcquireTaskRuntime(
            new_task_session_provider(server, "task_ks"),
            "task_ks".to_owned(),
            "holder".to_owned(),
        )
        .expect("current keyspace should use the server runtime");
        assert!(Arc::ptr_eq(&runtime, &got_runtime));
        assert!(catch_unwind(AssertUnwindSafe(release_runtime)).is_ok());
    }

    // different keyspace acquires and releases handle
    {
        let release_count = Arc::new(AtomicUsize::new(0));
        let release_count_for_mock = Arc::clone(&release_count);
        let mut runtime_handle = MockKSRuntimeHandle::new();
        runtime_handle.expect_Release().times(1).returning(move || {
            release_count_for_mock.fetch_add(1, Ordering::SeqCst);
        });
        let runtime_handle: Arc<dyn KSRuntimeHandle> = Arc::new(runtime_handle);
        let expected_runtime: Arc<dyn Runtime> = runtime_handle.clone();

        let handle_for_server = Arc::clone(&runtime_handle);
        let mut server = MockServer::new();
        server
            .expect_AcquireKSRuntime()
            .withf(|task_ks, holder| task_ks == "task_ks" && holder == "holder")
            .times(1)
            .returning(move |_, _| Ok(Arc::clone(&handle_for_server)));
        let server: Arc<dyn Server> = Arc::new(server);

        let (got_runtime, release_runtime) = AcquireTaskRuntime(
            new_task_session_provider(server, "current_ks"),
            "task_ks".to_owned(),
            "holder".to_owned(),
        )
        .expect("different keyspace should acquire a runtime handle");
        assert!(Arc::ptr_eq(&expected_runtime, &got_runtime));
        assert_eq!(release_count.load(Ordering::SeqCst), 0);
        release_runtime();
        assert_eq!(release_count.load(Ordering::SeqCst), 1);
    }

    // acquire error
    {
        let mut server = MockServer::new();
        server
            .expect_AcquireKSRuntime()
            .withf(|task_ks, holder| task_ks == "task_ks" && holder == "holder")
            .times(1)
            .returning(|_, _| Err(std::io::Error::other("ks runtime not found").into()));
        let server: Arc<dyn Server> = Arc::new(server);

        let error = AcquireTaskRuntime(
            new_task_session_provider(server, "current_ks"),
            "task_ks".to_owned(),
            "holder".to_owned(),
        )
        .err()
        .expect("acquire error should propagate");
        assert_eq!(error.to_string(), "ks runtime not found");
    }

    // session error
    {
        let error = AcquireTaskRuntime(
            TaskSessionProvider {
                session: None,
                error: Some("session error".to_owned()),
            },
            "task_ks".to_owned(),
            "holder".to_owned(),
        )
        .err()
        .expect("session error should propagate");
        assert_eq!(error.to_string(), "session error");
    }
}

#[test]
/// 合法 Runtime、store keyspace 不匹配、session keyspace 不匹配。
fn test_check_task_runtime() {
    // valid runtime
    {
        let store = store_with_keyspace("task_ks");
        let runtime = new_check_task_runtime_mock_runtime(
            Arc::clone(&store),
            Some(new_check_task_runtime_session_pool(store)),
        );
        CheckTaskRuntime(runtime, "task_ks".to_owned())
            .expect("matching store and session keyspaces should be valid");
    }

    // store keyspace mismatch
    {
        let runtime = new_check_task_runtime_mock_runtime(store_with_keyspace("store_ks"), None);
        let error = CheckTaskRuntime(runtime, "task_ks".to_owned())
            .expect_err("store keyspace mismatch should fail");
        assert!(
            error
                .to_string()
                .contains("store keyspace mismatch with task: store_ks vs task_ks")
        );
    }

    // session keyspace mismatch
    {
        let runtime = new_check_task_runtime_mock_runtime(
            store_with_keyspace("task_ks"),
            Some(new_check_task_runtime_session_pool(store_with_keyspace(
                "session_ks",
            ))),
        );
        let error = CheckTaskRuntime(runtime, "task_ks".to_owned())
            .expect_err("session keyspace mismatch should fail");
        assert!(
            error
                .to_string()
                .contains("invalid task runtime with mismatched keyspace: task_ks vs session_ks")
        );
    }
}
