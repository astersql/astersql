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

// sqlsvrapi mock 迁移期单测：用 mockall 期望验证 Runtime/KSHandle/Server。
//
// 检查参数匹配、返回值转发、Release 回调，以及对 missing keyspace 的错误路径。

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::domain::sqlsvrapi::mock::{NewMockKSRuntimeHandle, NewMockRuntime, NewMockServer};
use crate::domain::sqlsvrapi::{Context, KSRuntimeHandle, Runtime, Server};
use crate::kv_test_support::*;
use crate::meta::model::{AlterTableModeTarget, TableMode, ast};
use crate::owner_test_support::NewMockManager;
use crate::util_test_support::session_pool::{
    DestroyableSessionPool, NewSessionPool, PooledResource, Resource,
};

/// session pool 空资源。
struct TestResource;

impl Resource for TestResource {
    fn close(&self) {}

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 测试用 Storage，主要暴露 GetKeyspace。
struct TestStore {
    keyspace: String,
}

impl Storage for TestStore {
    fn Begin(&self, _options: &[tikv::TxnOption]) -> Result<Box<dyn Transaction>, Error> {
        panic!("unused in sqlsvrapi mock focused test")
    }

    fn GetSnapshot(&self, _version: Version) -> Box<dyn Snapshot> {
        panic!("unused in sqlsvrapi mock focused test")
    }

    fn GetClient(&self) -> &dyn Client {
        panic!("unused in sqlsvrapi mock focused test")
    }

    fn GetMPPClient(&self) -> &dyn MPPClient {
        panic!("unused in sqlsvrapi mock focused test")
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
        panic!("unused in sqlsvrapi mock focused test")
    }

    fn SupportDeleteRange(&self) -> bool {
        false
    }

    fn Name(&self) -> String {
        "test-store".to_owned()
    }

    fn Describe(&self) -> String {
        "sqlsvrapi mock focused test store".to_owned()
    }

    fn ShowStatus(
        &self,
        _ctx: &crate::kv_test_support::context::Context,
        _key: &str,
    ) -> Result<Box<dyn Any>, Error> {
        Ok(Box::new(()))
    }

    fn GetMemCache(&self) -> &dyn MemManager {
        panic!("unused in sqlsvrapi mock focused test")
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

/// 构造指定 keyspace 的测试 Storage。
fn test_store(keyspace: &str) -> Arc<dyn Storage + Send + Sync> {
    Arc::new(TestStore {
        keyspace: keyspace.to_owned(),
    })
}

/// 构造测试 session pool。
fn test_pool() -> Arc<dyn DestroyableSessionPool> {
    NewSessionPool(
        1,
        Arc::new(|| Ok(Arc::new(TestResource) as PooledResource)),
        None,
        None,
        None,
    )
}

/// 固定 AlterTableMode 目标（Normal → Import）。
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

/// 验证 MockRuntime 按期望返回 Store/Pool，并转发 AlterTableMode 错误。
#[test]
fn runtime_records_expectations_and_forwards_all_results() {
    let mut runtime = NewMockRuntime(&());
    let store = test_store("current");
    let pool = test_pool();

    runtime
        .expect_Store()
        .times(1)
        .return_const(Arc::clone(&store));
    runtime
        .expect_SysSessionPool()
        .times(2)
        .return_const(Arc::clone(&pool));
    runtime
        .expect_AlterTableMode()
        .withf(|ctx, target| {
            ctx.is_cancelled()
                && target.SchemaID == 11
                && target.TableID == 22
                && target.TargetMode == TableMode::TableModeImport
        })
        .times(1)
        .returning(|_, _| Err(std::io::Error::other("ddl rejected").into()));

    assert_eq!(runtime.Store().GetKeyspace(), "current");
    let resource = runtime.SysSessionPool().Get().unwrap();
    runtime.SysSessionPool().Put(resource);

    let ctx = Context::new();
    ctx.cancel();
    let error = runtime.AlterTableMode(ctx, target()).unwrap_err();
    assert_eq!(error.to_string(), "ddl rejected");
    runtime.checkpoint();
}

/// 验证 MockKSRuntimeHandle 的 Runtime 方法与 Release 计数。
#[test]
fn keyspace_runtime_records_release_and_runtime_methods() {
    let mut runtime = NewMockKSRuntimeHandle(&());
    let store = test_store("analytics");
    let pool = test_pool();
    let releases = Arc::new(AtomicUsize::new(0));

    runtime
        .expect_Store()
        .times(1)
        .return_const(Arc::clone(&store));
    runtime
        .expect_SysSessionPool()
        .times(2)
        .return_const(Arc::clone(&pool));
    runtime
        .expect_AlterTableMode()
        .withf(|_, target| target.TableID == 22)
        .times(1)
        .returning(|_, _| Ok(()));
    let release_count = Arc::clone(&releases);
    runtime.expect_Release().times(1).returning(move || {
        release_count.fetch_add(1, Ordering::SeqCst);
    });

    assert_eq!(runtime.Store().GetKeyspace(), "analytics");
    let resource = runtime.SysSessionPool().Get().unwrap();
    runtime.SysSessionPool().Destroy(resource);
    runtime.AlterTableMode(Context::new(), target()).unwrap();
    runtime.Release();
    assert_eq!(releases.load(Ordering::SeqCst), 1);
    runtime.checkpoint();
}

/// 验证 MockServer 返回 runtime/handle/owner，并对 missing 返回错误。
#[test]
fn server_records_arguments_and_returns_runtime_handle_owner_and_error() {
    let mut current = NewMockRuntime(&());
    current
        .expect_Store()
        .times(1)
        .return_const(test_store("current"));
    let current: Arc<dyn Runtime> = Arc::new(current);

    let mut handle = NewMockKSRuntimeHandle(&());
    handle
        .expect_Store()
        .times(1)
        .return_const(test_store("analytics"));
    let handle: Arc<dyn KSRuntimeHandle> = Arc::new(handle);

    let owner = NewMockManager(Context::new(), "ddl-owner", None, "/ddl/owner");
    let mut server = NewMockServer(&());
    let expected_runtime = Arc::clone(&current);
    server
        .expect_GetRuntime()
        .times(1)
        .returning(move || Arc::clone(&expected_runtime));
    let expected_handle = Arc::clone(&handle);
    server
        .expect_AcquireKSRuntime()
        .with(
            mockall::predicate::eq("analytics".to_owned()),
            mockall::predicate::eq("task-572".to_owned()),
        )
        .times(1)
        .returning(move |_, _| Ok(Arc::clone(&expected_handle)));
    server
        .expect_AcquireKSRuntime()
        .with(
            mockall::predicate::eq("missing".to_owned()),
            mockall::predicate::eq("task-572".to_owned()),
        )
        .times(1)
        .returning(|_, _| Err(std::io::Error::other("keyspace missing").into()));
    let expected_owner = Arc::clone(&owner);
    server
        .expect_GetDDLOwnerMgr()
        .times(1)
        .returning(move || Arc::clone(&expected_owner));

    assert_eq!(server.GetRuntime().Store().GetKeyspace(), "current");
    assert_eq!(
        server
            .AcquireKSRuntime("analytics".to_owned(), "task-572".to_owned())
            .unwrap()
            .Store()
            .GetKeyspace(),
        "analytics"
    );
    assert_eq!(server.GetDDLOwnerMgr().ID(), "ddl-owner");
    let error = server
        .AcquireKSRuntime("missing".to_owned(), "task-572".to_owned())
        .err()
        .expect("missing keyspace should fail");
    assert_eq!(error.to_string(), "keyspace missing");
    server.checkpoint();
}
