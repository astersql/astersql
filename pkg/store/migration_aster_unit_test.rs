// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// store 包迁移对照单元测试。
//
// 校验 Driver 注册、`New` 打开存储、可重试错误分类、存储路径拼装、
// 系统 Keyspace（键空间）限制，以及 etcd 地址/命名空间默认值，与 Go 行为对齐。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use config_dependency::{StoreType, StoreTypeMockTiKV, StoreTypeTiKV};
use keyspace_dependency::{ApiVersion, BasicCodec, Codec};
use serial_test::serial;
use astersql_store_driver::{InMemoryBackend, TiKVDriver};
use crate::{
    BuildStoragePath, Driver, EtcdBackend, EtcdClientSettings, GetEtcdAddrs,
    IsKeyspaceNotExistError, IsNotBootstrappedError, IsNotTSOLeaderError, New, NewEtcdCli,
    Register, RegisteredDriverTypeName, ResetStoreStateForTest, SetSystemStorage, Storage,
    StorageRef, StoreError, StoreErrorKind, TiKVStoreDriver, isNewStoreRetryableError,
    newStoreWithRetryAndInterval,
};

/// 测试用 Storage：可模拟经典（无 Keyspace）或带元数据地址的 Keyspace 存储。
struct TestStorage {
    keyspace: String,
    codec: BasicCodec,
    meta_addrs: Option<Vec<String>>,
}

impl TestStorage {
    /// 构造无 Keyspace 的经典存储（API V1）。
    fn classic() -> Self {
        Self {
            keyspace: String::new(),
            codec: BasicCodec {
                api_version: ApiVersion::V1,
                keyspace_id: 0,
            },
            meta_addrs: None,
        }
    }

    /// 构造指定 Keyspace 的存储（API V2），并可附带 etcd 元数据地址。
    fn keyspace(id: u32, name: &str, meta_addrs: Vec<String>) -> Self {
        Self {
            keyspace: name.to_owned(),
            codec: BasicCodec {
                api_version: ApiVersion::V2,
                keyspace_id: id,
            },
            meta_addrs: Some(meta_addrs),
        }
    }
}

impl Storage for TestStorage {
    fn GetKeyspace(&self) -> &str {
        &self.keyspace
    }

    fn GetCodec(&self) -> &dyn Codec {
        &self.codec
    }

    fn AsEtcdBackend(&self) -> Option<&dyn EtcdBackend> {
        self.meta_addrs.as_ref().map(|_| self as &dyn EtcdBackend)
    }
}

impl EtcdBackend for TestStorage {
    fn EtcdAddrs(&self) -> Result<Vec<String>, StoreError> {
        Ok(self.meta_addrs.clone().unwrap_or_default())
    }

    fn GetPDAddrs(&self) -> Result<Vec<String>, StoreError> {
        Ok(vec!["localhost:2379".to_owned()])
    }

    fn TLSConfig(&self) -> Option<etcd_client::TlsOptions> {
        None
    }

    fn StartGCWorker(&self) -> Result<(), StoreError> {
        Ok(())
    }
}

/// 脚本化 Driver：按预设结果队列依次返回 Open 结果，并记录调用路径。
struct ScriptedDriver {
    calls: Mutex<Vec<String>>,
    results: Mutex<VecDeque<Result<StorageRef, StoreError>>>,
}

impl ScriptedDriver {
    /// 用预设的 Open 结果序列构造 Driver。
    fn new(results: Vec<Result<StorageRef, StoreError>>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            results: Mutex::new(results.into()),
        }
    }
}

impl Driver for ScriptedDriver {
    fn Open(&self, path: &str) -> Result<StorageRef, StoreError> {
        self.calls.lock().unwrap().push(path.to_owned());
        self.results
            .lock()
            .unwrap()
            .pop_front()
            .expect("test driver result")
    }
}

/// 包装经典 TestStorage 为 `StorageRef`。
fn classic_storage() -> StorageRef {
    Arc::new(TestStorage::classic())
}

/// 校验 Register 仅允许合法 StoreType，拒绝重复注册，且 New 会调用已注册 Driver。
#[test]
#[serial]
fn register_and_new_match_go_validation_scheme_and_duplicate_behavior() {
    ResetStoreStateForTest();
    let invalid = Register(
        StoreType::from("retry"),
        Arc::new(ScriptedDriver::new(vec![])),
    )
    .expect_err("only TiKV, UniStore, and MockTiKV are valid");
    assert!(invalid.to_string().contains("invalid storage type retry"));

    let driver = Arc::new(ScriptedDriver::new(vec![Ok(classic_storage())]));
    Register(StoreTypeTiKV, driver.clone()).unwrap();
    let duplicate = Register(
        StoreTypeTiKV,
        Arc::new(ScriptedDriver::new(vec![Ok(classic_storage())])),
    )
    .expect_err("duplicate registration must fail");
    assert!(duplicate.to_string().contains("tikv is already registered"));

    let storage = New("TiKV://127.0.0.1:2379").unwrap();
    assert_eq!(storage.GetCodec().api_version(), ApiVersion::V1);
    assert_eq!(
        driver.calls.lock().unwrap().as_slice(),
        ["TiKV://127.0.0.1:2379"]
    );
}

/// The registry adapter owns the canonical TiKVDriver/TikvStore and preserves
/// multi-PD URI, cluster identity, TSO, keyspace and idempotent close behavior.
#[test]
#[serial]
fn registered_tikv_driver_opens_canonical_store() {
    ResetStoreStateForTest();
    let driver = TiKVDriver::with_backend(Arc::new(InMemoryBackend::default()));
    Register(StoreTypeTiKV, Arc::new(TiKVStoreDriver::new(driver))).unwrap();
    assert_eq!(
        RegisteredDriverTypeName(StoreTypeTiKV),
        Some(std::any::type_name::<TiKVDriver>())
    );

    let storage = New("tikv://pd-a:2379,pd-b:2379?keyspaceName=analytics").unwrap();
    assert_eq!(storage.GetKeyspace(), "analytics");
    assert_ne!(storage.GetClusterID().expect("TiKV cluster identity"), 0);
    assert_eq!(storage.CurrentVersion("global").unwrap(), Some(1));
    storage.Close().unwrap();
    storage.Close().unwrap();
}

/// 校验新建存储重试：仅对可重试错误重试，不可重试错误立即失败并保留尝试次数语义。
#[test]
#[serial]
fn retry_loop_retries_only_classified_errors_and_preserves_attempt_count() {
    ResetStoreStateForTest();
    let retryable = StoreError::new(StoreErrorKind::TxnRetryable, "write conflict");
    let driver = Arc::new(ScriptedDriver::new(vec![
        Err(retryable),
        Ok(classic_storage()),
    ]));
    Register(StoreTypeMockTiKV, driver.clone()).unwrap();

    let storage =
        newStoreWithRetryAndInterval("mocktikv://dummy-store", 3, Duration::ZERO)
            .unwrap()
            .expect("successful open returns storage");
    assert_eq!(storage.GetKeyspace(), "");
    assert_eq!(driver.calls.lock().unwrap().len(), 2);

    ResetStoreStateForTest();
    let driver = Arc::new(ScriptedDriver::new(vec![
        Err(StoreError::other("permission denied")),
        Ok(classic_storage()),
    ]));
    Register(StoreTypeMockTiKV, driver.clone()).unwrap();
    assert!(newStoreWithRetryAndInterval("mocktikv://dummy-store", 3, Duration::ZERO).is_err());
    assert_eq!(driver.calls.lock().unwrap().len(), 1);
}

/// 校验未 bootstrap、Keyspace 不存在、TSO 非 Leader 等错误文本/类型判定与 Go 一致。
#[test]
fn retry_error_classification_matches_go_text_and_rfc_chain_rules() {
    let not_bootstrapped = StoreError::other("rpc: NOT_BOOTSTRAPPED from PD");
    let missing_keyspace = StoreError::other("PD returned ENTRY_NOT_FOUND");
    assert!(IsNotBootstrappedError(Some(&not_bootstrapped)));
    assert!(IsKeyspaceNotExistError(Some(&missing_keyspace)));
    assert!(!IsNotBootstrappedError(None));
    assert!(!IsKeyspaceNotExistError(None));

    let tso = StoreError::new(StoreErrorKind::PdClientGetTso, "not leader");
    let wrapped = StoreError::wrap("open storage", tso);
    assert!(IsNotTSOLeaderError(Some(&wrapped)));
    assert!(isNewStoreRetryableError(Some(&wrapped)));

    let leader = StoreError::new(StoreErrorKind::PdClientGetLeader, "not leader");
    assert!(IsNotTSOLeaderError(Some(&leader)));
    let uppercase = StoreError::new(StoreErrorKind::PdClientGetLeader, "NOT LEADER");
    assert!(!IsNotTSOLeaderError(Some(&uppercase)));
    assert!(!IsNotTSOLeaderError(Some(&StoreError::other(
        "br: not leader"
    ))));
    assert!(!isNewStoreRetryableError(None));
}

/// 校验存储路径拼装：空 Keyspace 无查询串，命名 Keyspace 追加 `keyspaceName`。
#[test]
fn storage_path_matches_go_empty_and_named_keyspace_format() {
    assert_eq!(
        BuildStoragePath("tikv", "127.0.0.1:2379", ""),
        "tikv://127.0.0.1:2379"
    );
    assert_eq!(
        BuildStoragePath("tikv", "127.0.0.1:2379", "analytics"),
        "tikv://127.0.0.1:2379?keyspaceName=analytics"
    );
}

/// 校验系统存储仅接受 SYSTEM Keyspace，用户 Keyspace 会触发 panic。
#[test]
#[serial]
fn system_storage_accepts_only_the_system_keyspace() {
    ResetStoreStateForTest();
    let system: StorageRef = Arc::new(TestStorage::keyspace(1, "SYSTEM", vec![]));
    SetSystemStorage(Some(system));

    let user: StorageRef = Arc::new(TestStorage::keyspace(2, "user", vec![]));
    let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        SetSystemStorage(Some(user));
    }));
    assert!(rejected.is_err());
    SetSystemStorage(None);
}

/// 无真实网络下校验 etcd 地址解析、Keyspace 命名空间路径与客户端默认超时参数。
#[tokio::test]
async fn etcd_address_namespace_and_defaults_match_go_behavior_without_network() {
    let (backend, addrs) = GetEtcdAddrs(None).unwrap();
    assert!(backend.is_none());
    assert!(addrs.is_empty());
    assert!(NewEtcdCli(None).await.unwrap().is_none());

    let store = TestStorage::keyspace(42, "analytics", vec!["localhost:2389".to_owned()]);
    let (backend, addrs) = GetEtcdAddrs(Some(&store)).unwrap();
    assert!(backend.is_some());
    assert_eq!(addrs, ["localhost:2389"]);
    assert_eq!(crate::EtcdNamespace(&store), "/keyspaces/tidb/42");

    let defaults = EtcdClientSettings::default();
    assert_eq!(defaults.auto_sync_interval, Duration::from_secs(30));
    assert_eq!(defaults.dial_timeout, Duration::from_secs(5));
    assert_eq!(defaults.backoff_max_delay, Duration::from_secs(3));
    assert_eq!(defaults.keep_alive_interval, Duration::from_secs(10));
    assert_eq!(defaults.keep_alive_timeout, Duration::from_secs(3));
}
