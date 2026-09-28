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

// sqlsvrapi 迁移期单测：用不依赖 mockall 的 Recording 桩验证 Runtime/Server 契约。
//
// 覆盖：上下文取消标志与 AlterTableModeTarget 转发、错误透传、
// AcquireKSRuntime 成功/失败，以及 DDL owner Manager 暴露。

use std::any::Any;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use astersql_domain_sqlsvrapi::kv_test_support::*;
use astersql_domain_sqlsvrapi::meta::model::{AlterTableModeTarget, TableMode, ast};
use astersql_domain_sqlsvrapi::owner_test_support::{Manager, NewMockManager};
use astersql_domain_sqlsvrapi::server::{Context, KSRuntimeHandle, Runtime, Server, SqlSvrError};
use astersql_domain_sqlsvrapi::util_test_support::session_pool::{
    DestroyableSessionPool, NewSessionPool, PooledResource, Resource,
};

/// session pool 用的空资源实现。
struct TestResource;

impl Resource for TestResource {
    fn close(&self) {}

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 仅实现 GetKeyspace 等查询路径的测试 Storage。
struct TestStore {
    keyspace: String,
}

impl Storage for TestStore {
    fn Begin(&self, _options: &[tikv::TxnOption]) -> Result<Box<dyn Transaction>, Error> {
        panic!("unused in sqlsvrapi focused test")
    }

    fn GetSnapshot(&self, _version: Version) -> Box<dyn Snapshot> {
        panic!("unused in sqlsvrapi focused test")
    }

    fn GetClient(&self) -> &dyn Client {
        panic!("unused in sqlsvrapi focused test")
    }

    fn GetMPPClient(&self) -> &dyn MPPClient {
        panic!("unused in sqlsvrapi focused test")
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
        panic!("unused in sqlsvrapi focused test")
    }

    fn SupportDeleteRange(&self) -> bool {
        false
    }

    fn Name(&self) -> String {
        "test-store".to_owned()
    }

    fn Describe(&self) -> String {
        "sqlsvrapi focused test store".to_owned()
    }

    fn ShowStatus(
        &self,
        _ctx: &astersql_domain_sqlsvrapi::kv_test_support::context::Context,
        _key: &str,
    ) -> Result<Box<dyn Any>, Error> {
        Ok(Box::new(()))
    }

    fn GetMemCache(&self) -> &dyn MemManager {
        panic!("unused in sqlsvrapi focused test")
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

/// 记录 AlterTableMode 调用参数，并可注入失败信息的 Runtime 桩。
struct RecordingRuntime {
    store: Arc<dyn Storage + Send + Sync>,
    pool: Arc<dyn DestroyableSessionPool>,
    calls: Mutex<Vec<(bool, i64, i64, TableMode)>>,
    failure: Option<String>,
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
        ctx: Context,
        target: AlterTableModeTarget,
    ) -> Result<(), SqlSvrError> {
        self.calls.lock().unwrap().push((
            ctx.is_cancelled(),
            target.SchemaID,
            target.TableID,
            target.TargetMode,
        ));
        match &self.failure {
            Some(message) => Err(std::io::Error::other(message.clone()).into()),
            None => Ok(()),
        }
    }
}

/// 包装 RecordingRuntime，并统计 Release 次数的 KSRuntimeHandle 桩。
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
        ctx: Context,
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

/// 记录 AcquireKSRuntime 参数，并可对 missing keyspace 返回错误的 Server 桩。
struct RecordingServer {
    runtime: Arc<dyn Runtime>,
    handle: Arc<dyn KSRuntimeHandle>,
    owner: Arc<dyn Manager>,
    acquisitions: Mutex<Vec<(String, String)>>,
}

impl Server for RecordingServer {
    fn GetRuntime(&self) -> Arc<dyn Runtime> {
        Arc::clone(&self.runtime)
    }

    fn AcquireKSRuntime(
        &self,
        targetKS: String,
        holderID: String,
    ) -> Result<Arc<dyn KSRuntimeHandle>, SqlSvrError> {
        self.acquisitions
            .lock()
            .unwrap()
            .push((targetKS.clone(), holderID));
        if targetKS == "missing" {
            return Err(std::io::Error::other("keyspace missing").into());
        }
        Ok(Arc::clone(&self.handle))
    }

    fn GetDDLOwnerMgr(&self) -> Arc<dyn Manager> {
        Arc::clone(&self.owner)
    }
}

/// 构造容量为 1 的测试 session pool。
fn test_pool() -> Arc<dyn DestroyableSessionPool> {
    NewSessionPool(
        1,
        Arc::new(|| Ok(Arc::new(TestResource) as PooledResource)),
        None,
        None,
        None,
    )
}

/// 构造指定 keyspace 与可选失败消息的 RecordingRuntime。
fn test_runtime(keyspace: &str, failure: Option<&str>) -> RecordingRuntime {
    RecordingRuntime {
        store: Arc::new(TestStore {
            keyspace: keyspace.to_owned(),
        }),
        pool: test_pool(),
        calls: Mutex::new(Vec::new()),
        failure: failure.map(str::to_owned),
    }
}

/// 固定的 AlterTableMode 目标：Normal → Import（导入模式切换）。
fn target() -> AlterTableModeTarget {
    AlterTableModeTarget {
        SchemaID: 11,
        SchemaName: ast::CIStr {
            O: "app".to_owned(),
            L: "app".to_owned(),
        },
        TableID: 22,
        TableName: ast::CIStr {
            O: "orders".to_owned(),
            L: "orders".to_owned(),
        },
        CurrentMode: TableMode::TableModeNormal,
        TargetMode: TableMode::TableModeImport,
    }
}

/// 验证 Store/Pool 可用，AlterTableMode 保留取消态与目标，并透传错误。
#[test]
fn runtime_uses_real_dependencies_and_preserves_context_target_and_error() {
    let runtime = test_runtime("current", None);
    assert_eq!(runtime.Store().GetKeyspace(), "current");

    let resource = runtime.SysSessionPool().Get().unwrap();
    runtime.SysSessionPool().Put(resource);

    // 取消后再调用，应记录 is_cancelled=true。
    let ctx = Context::new();
    let cancelled_view = ctx.clone();
    ctx.cancel();
    runtime.AlterTableMode(cancelled_view, target()).unwrap();
    assert_eq!(
        runtime.calls.lock().unwrap().as_slice(),
        &[(true, 11, 22, TableMode::TableModeImport)]
    );

    let failing = test_runtime("current", Some("ddl rejected"));
    let error = failing
        .AlterTableMode(Context::new(), target())
        .unwrap_err();
    assert_eq!(error.to_string(), "ddl rejected");
}

/// 验证获取 KS handle、Release、owner ID，以及对 missing keyspace 报错。
#[test]
fn server_acquires_keyspace_handle_reports_errors_and_exposes_owner() {
    let releases = Arc::new(AtomicUsize::new(0));
    let handle: Arc<dyn KSRuntimeHandle> = Arc::new(RecordingHandle {
        runtime: test_runtime("analytics", None),
        releases: Arc::clone(&releases),
    });
    let runtime: Arc<dyn Runtime> = Arc::new(test_runtime("current", None));
    let owner = NewMockManager(Context::new(), "ddl-owner", None, "/ddl/owner");
    let server = RecordingServer {
        runtime,
        handle,
        owner,
        acquisitions: Mutex::new(Vec::new()),
    };

    assert_eq!(server.GetRuntime().Store().GetKeyspace(), "current");
    let acquired = server
        .AcquireKSRuntime("analytics".to_owned(), "task-557".to_owned())
        .unwrap();
    assert_eq!(acquired.Store().GetKeyspace(), "analytics");
    acquired.Release();
    assert_eq!(releases.load(Ordering::SeqCst), 1);
    assert_eq!(server.GetDDLOwnerMgr().ID(), "ddl-owner");
    assert_eq!(
        server.acquisitions.lock().unwrap().as_slice(),
        &[("analytics".to_owned(), "task-557".to_owned())]
    );

    let error = server
        .AcquireKSRuntime("missing".to_owned(), "task-557".to_owned())
        .err()
        .expect("missing keyspace should fail");
    assert_eq!(error.to_string(), "keyspace missing");
}
