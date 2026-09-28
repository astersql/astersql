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

// dxfutil 迁移对齐单元测试：跨 keyspace 获取 Runtime、校验 keyspace 一致性、生成 holder ID。
//
// 使用自建 Recording* fixture 模拟 SQL Server / KSRuntimeHandle，覆盖与 Go 相同的分支语义。

use std::any::Any;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::kv::*;
use crate::meta::model::AlterTableModeTarget;
use crate::owner::Manager;
use crate::sessionctx;
use crate::sqlsvrapi::{Context as ApiContext, KSRuntimeHandle, Runtime, Server, SqlSvrError};
use crate::util::{DestroyableSessionPool, NewSessionPool, PooledResource};
use crate::{AcquireTaskRuntime, CheckTaskRuntime, GenHolderID, sessionProvider};

/// 仅实现 GetKeyspace 等必要方法的测试用 KV Storage。
struct TestStore {
    keyspace: String,
}

impl Storage for TestStore {
    fn Begin(&self, _options: &[tikv::TxnOption]) -> Result<Box<dyn Transaction>, Error> {
        panic!("unused in dxfutil focused test")
    }

    fn GetSnapshot(&self, _version: Version) -> Box<dyn Snapshot> {
        panic!("unused in dxfutil focused test")
    }

    fn GetClient(&self) -> &dyn Client {
        panic!("unused in dxfutil focused test")
    }

    fn GetMPPClient(&self) -> &dyn MPPClient {
        panic!("unused in dxfutil focused test")
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
        panic!("unused in dxfutil focused test")
    }

    fn SupportDeleteRange(&self) -> bool {
        false
    }

    fn Name(&self) -> String {
        "test-store".to_owned()
    }

    fn Describe(&self) -> String {
        "dxfutil focused test store".to_owned()
    }

    fn ShowStatus(&self, _ctx: &context::Context, _key: &str) -> Result<Box<dyn Any>, Error> {
        Ok(Box::new(()))
    }

    fn GetMemCache(&self) -> &dyn MemManager {
        panic!("unused in dxfutil focused test")
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
fn store(keyspace: &str) -> Arc<dyn Storage + Send + Sync> {
    Arc::new(TestStore {
        keyspace: keyspace.to_owned(),
    })
}

/// 构造容量为 1 的会话池，内部会话绑定给定 keyspace 与可选 Server。
fn session_pool(
    keyspace: &str,
    server: Option<Arc<dyn Server>>,
) -> Arc<dyn DestroyableSessionPool> {
    let session = sessionctx::Context::new(store(keyspace), server);
    NewSessionPool(
        1,
        Arc::new(move || Ok(Arc::new(session.clone()) as PooledResource)),
        None,
        None,
        None,
    )
}

/// 可记录的 Runtime：暴露 Store 与 SysSessionPool，AlterTableMode 未使用。
struct RecordingRuntime {
    store: Arc<dyn Storage + Send + Sync>,
    pool: Arc<dyn DestroyableSessionPool>,
}

impl Runtime for RecordingRuntime {
    fn Store(&self) -> Arc<dyn Storage + Send + Sync> {
        Arc::clone(&self.store)
    }

    fn SysSessionPool(&self) -> Arc<dyn DestroyableSessionPool> {
        Arc::clone(&self.pool)
    }

    fn AlterTableMode(
        &self,
        _ctx: ApiContext,
        _target: AlterTableModeTarget,
    ) -> Result<(), SqlSvrError> {
        panic!("unused in dxfutil focused test")
    }
}

/// KSRuntimeHandle 包装：Release 时递增计数，用于断言跨 keyspace 释放语义。
struct RecordingHandle {
    runtime: RecordingRuntime,
    releases: Arc<AtomicUsize>,
}

impl Runtime for RecordingHandle {
    fn Store(&self) -> Arc<dyn Storage + Send + Sync> {
        self.runtime.Store()
    }

    fn SysSessionPool(&self) -> Arc<dyn DestroyableSessionPool> {
        self.runtime.SysSessionPool()
    }

    fn AlterTableMode(
        &self,
        ctx: ApiContext,
        target: AlterTableModeTarget,
    ) -> Result<(), SqlSvrError> {
        self.runtime.AlterTableMode(ctx, target)
    }
}

impl KSRuntimeHandle for RecordingHandle {
    fn Release(&self) {
        self.releases.fetch_add(1, Ordering::SeqCst);
    }
}

/// 可记录 AcquireKSRuntime 参数的 Server；target_ks=="missing" 时返回错误。
struct RecordingServer {
    runtime: Arc<dyn Runtime>,
    handle: Arc<dyn KSRuntimeHandle>,
    acquisitions: Mutex<Vec<(String, String)>>,
}

impl Server for RecordingServer {
    fn GetRuntime(&self) -> Arc<dyn Runtime> {
        Arc::clone(&self.runtime)
    }

    fn AcquireKSRuntime(
        &self,
        target_ks: String,
        holder_id: String,
    ) -> Result<Arc<dyn KSRuntimeHandle>, SqlSvrError> {
        self.acquisitions
            .lock()
            .unwrap()
            .push((target_ks.clone(), holder_id));
        if target_ks == "missing" {
            return Err(std::io::Error::other("ks runtime not found").into());
        }
        Ok(Arc::clone(&self.handle))
    }

    fn GetDDLOwnerMgr(&self) -> Arc<dyn Manager> {
        panic!("unused in dxfutil focused test")
    }
}

/// 测试用 sessionProvider：可注入固定会话或失败信息。
struct TaskSessionProvider {
    session: Option<sessionctx::Context>,
    failure: Option<String>,
}

impl sessionProvider for TaskSessionProvider {
    fn WithNewSession<F>(&self, callback: F) -> Result<(), SqlSvrError>
    where
        F: FnOnce(sessionctx::Context) -> Result<(), SqlSvrError>,
    {
        if let Some(message) = &self.failure {
            return Err(std::io::Error::other(message.clone()).into());
        }
        callback(self.session.clone().expect("provider session"))
    }
}

/// 构造 RecordingRuntime；store 与 session 的 keyspace 可故意不一致以触发校验错误。
fn runtime(keyspace: &str, session_keyspace: &str) -> Arc<dyn Runtime> {
    Arc::new(RecordingRuntime {
        store: store(keyspace),
        pool: session_pool(session_keyspace, None),
    })
}

/// 组装 RecordingServer 与共享 Release 计数器。
fn server_fixture() -> (Arc<RecordingServer>, Arc<AtomicUsize>) {
    let releases = Arc::new(AtomicUsize::new(0));
    let handle: Arc<dyn KSRuntimeHandle> = Arc::new(RecordingHandle {
        runtime: RecordingRuntime {
            store: store("task_ks"),
            pool: session_pool("task_ks", None),
        },
        releases: Arc::clone(&releases),
    });
    let server = Arc::new(RecordingServer {
        runtime: runtime("current_ks", "current_ks"),
        handle,
        acquisitions: Mutex::new(Vec::new()),
    });
    (server, releases)
}

/// 用当前 keyspace 的会话包装 Server，供 AcquireTaskRuntime 调用。
fn provider(server: Arc<RecordingServer>, current_keyspace: &str) -> TaskSessionProvider {
    let server: Arc<dyn Server> = server;
    TaskSessionProvider {
        session: Some(sessionctx::Context::new(
            store(current_keyspace),
            Some(server),
        )),
        failure: None,
    }
}

#[test]
/// 覆盖：同 keyspace 不 Acquire、跨 keyspace 需 Release、missing 失败、session 错误传播。
fn acquire_task_runtime_matches_all_go_branches_and_release_semantics() {
    let (server, releases) = server_fixture();

    let (current, release_current) = AcquireTaskRuntime(
        provider(Arc::clone(&server), "current_ks"),
        "current_ks".to_owned(),
        "holder-current".to_owned(),
    )
    .unwrap();
    assert_eq!(current.Store().GetKeyspace(), "current_ks");
    release_current();
    assert_eq!(releases.load(Ordering::SeqCst), 0);

    let (acquired, release_acquired) = AcquireTaskRuntime(
        provider(Arc::clone(&server), "current_ks"),
        "task_ks".to_owned(),
        "holder-cross".to_owned(),
    )
    .unwrap();
    assert_eq!(acquired.Store().GetKeyspace(), "task_ks");
    assert_eq!(releases.load(Ordering::SeqCst), 0);
    release_acquired();
    assert_eq!(releases.load(Ordering::SeqCst), 1);
    assert_eq!(
        server.acquisitions.lock().unwrap().as_slice(),
        &[("task_ks".to_owned(), "holder-cross".to_owned())]
    );

    let acquire_error = AcquireTaskRuntime(
        provider(Arc::clone(&server), "current_ks"),
        "missing".to_owned(),
        "holder-missing".to_owned(),
    )
    .err()
    .expect("missing keyspace must fail");
    assert_eq!(acquire_error.to_string(), "ks runtime not found");

    let session_error = AcquireTaskRuntime(
        TaskSessionProvider {
            session: None,
            failure: Some("session error".to_owned()),
        },
        "task_ks".to_owned(),
        "holder".to_owned(),
    )
    .err()
    .expect("provider error must propagate");
    assert_eq!(session_error.to_string(), "session error");
}

#[test]
/// 覆盖：合法 Runtime、store/session keyspace 不匹配、会话池已关闭三类错误。
fn check_task_runtime_matches_store_session_and_pool_error_paths() {
    CheckTaskRuntime(runtime("task_ks", "task_ks"), "task_ks".to_owned()).unwrap();

    let store_error =
        CheckTaskRuntime(runtime("store_ks", "store_ks"), "task_ks".to_owned()).unwrap_err();
    assert_eq!(
        store_error.to_string(),
        "store keyspace mismatch with task: store_ks vs task_ks"
    );

    let session_error =
        CheckTaskRuntime(runtime("task_ks", "session_ks"), "task_ks".to_owned()).unwrap_err();
    assert_eq!(
        session_error.to_string(),
        "invalid task runtime with mismatched keyspace: task_ks vs session_ks"
    );

    let closed_pool = session_pool("task_ks", None);
    closed_pool.Close();
    let pool_error = CheckTaskRuntime(
        Arc::new(RecordingRuntime {
            store: store("task_ks"),
            pool: closed_pool,
        }),
        "task_ks".to_owned(),
    )
    .unwrap_err();
    assert_eq!(pool_error.to_string(), "session pool closed");
}

#[test]
/// GenHolderID 格式为 `DXF/{component}/{taskID}`，负 task ID 原样保留符号。
fn gen_holder_id_preserves_go_format_for_signed_task_ids() {
    assert_eq!(GenHolderID("scheduler".to_owned(), 42), "DXF/scheduler/42");
    assert_eq!(GenHolderID("executor".to_owned(), -7), "DXF/executor/-7");
}
