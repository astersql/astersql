// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// KV 存储驱动注册与打开入口。
//
// 按 URI scheme（如 tikv / unistore / mocktikv）查找已注册的 `Driver`，
// 打开失败时按可重试错误分类做退避重试。同时维护 nextgen 场景下的
// 全局 SYSTEM keyspace 存储引用。

use std::collections::HashMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::thread;
use std::time::Duration;

use astersql_store_driver::TiKVDriver;
pub use astersql_store_driver::{ReadAttempt, ReadOptions, ReadStats, TikvStore};
use config_dependency::{self as config, StoreType};
use keyspace_dependency::{self as keyspace, ApiVersion, BasicCodec, Codec};
use url::Url;

/// 打开存储时的默认最大重试次数。
const DEFAULT_MAX_RETRIES: usize = 30;
/// 线性退避的基础间隔（毫秒）；实际休眠为 interval * attempt。
const RETRY_INTERVAL: Duration = Duration::from_millis(500);
/// PD/集群尚未 bootstrap（初始化）时的错误子串。
const NOT_BOOTSTRAPPED: &str = "NOT_BOOTSTRAPPED";
/// keyspace 不存在时的错误子串（ENTRY_NOT_FOUND）。
const ENTRY_NOT_FOUND: &str = "ENTRY_NOT_FOUND";
/// “非 leader”错误子串； alone 不足以判定为 PD TSO leader 错误。
const NOT_LEADER: &str = "not leader";

/// The structured classifications that TiDB uses when deciding whether opening
/// a storage should be retried. PD leader errors deliberately remain distinct:
/// a plain error message containing "not leader" is not sufficient.
/// 打开存储时可重试错误的结构化分类。
/// PD leader 相关错误必须带明确 kind，仅凭消息含 "not leader" 不够。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreErrorKind {
    /// 普通/未分类错误。
    Other,
    /// 事务可重试错误（如锁冲突、临时不可用）。
    TxnRetryable,
    /// 向 PD 获取 TSO（Timestamp Oracle，全局时间戳）失败。
    PdClientGetTso,
    /// 向 PD 获取 Leader 信息失败。
    PdClientGetLeader,
}

/// 存储打开/注册路径上的错误类型，可嵌套 source 形成错误链。
#[derive(Debug)]
pub struct StoreError {
    /// 错误分类，决定是否可重试。
    kind: StoreErrorKind,
    /// 人类可读消息。
    message: String,
    /// 可选的内层错误，用于链式判定（如 PD not leader）。
    source: Option<Box<StoreError>>,
}

impl StoreError {
    /// 构造指定 kind 与消息的错误。
    pub fn new(kind: StoreErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }

    /// 构造 `Other` 类错误。
    pub fn other(message: impl Into<String>) -> Self {
        Self::new(StoreErrorKind::Other, message)
    }

    /// 用上下文消息包装内层错误，外层 kind 为 Other。
    pub fn wrap(context: impl Into<String>, source: StoreError) -> Self {
        Self {
            kind: StoreErrorKind::Other,
            message: context.into(),
            source: Some(Box::new(source)),
        }
    }

    /// 返回错误分类。
    pub fn kind(&self) -> StoreErrorKind {
        self.kind
    }
}

impl Display for StoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)?;
        if let Some(source) = &self.source {
            write!(formatter, ": {source}")?;
        }
        Ok(())
    }
}

impl Error for StoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

/// The store surface used by this package. It keeps the same three operations
/// consumed by the Go files while allowing concrete KV stores to expose their
/// complete API through their own types.
/// 本包使用的存储抽象：暴露 keyspace、编解码器，以及可选的 etcd 后端。
pub trait Storage: Send + Sync {
    /// 返回该存储绑定的 keyspace 名称。
    fn GetKeyspace(&self) -> &str;
    /// 返回 keyspace 编解码器（Codec）。
    fn GetCodec(&self) -> &dyn Codec;

    /// 若底层支持 etcd 后端则返回，否则 None。
    fn AsEtcdBackend(&self) -> Option<&dyn crate::EtcdBackend> {
        None
    }

    /// Close releases the concrete store lifecycle. Legacy/test stores keep the
    /// no-op default; the registered TiKV store closes client-rust resources.
    fn Close(&self) -> Result<(), StoreError> {
        Ok(())
    }

    /// Returns the real TiKV cluster identity when the concrete store has one.
    fn GetClusterID(&self) -> Option<u64> {
        None
    }

    /// Requests a TSO from the concrete store when supported.
    fn CurrentVersion(&self, _txn_scope: &str) -> Result<Option<u64>, StoreError> {
        Ok(None)
    }

    /// Returns a clone of the canonical client-rust-backed TiKV store.
    ///
    /// The default rejects local and test registry stores explicitly so callers
    /// cannot silently create or substitute a second TiKV client.
    fn CanonicalTiKVStore(&self) -> Result<TikvStore, StoreError> {
        Err(StoreError::other(
            "canonical TiKV store is unavailable for this storage",
        ))
    }
}

/// 共享所有权的存储句柄。
pub type StorageRef = Arc<dyn Storage>;

/// 按路径打开具体 KV 存储的驱动接口。
pub trait Driver: Send + Sync {
    /// 打开 `path` 指定的存储（通常为 scheme:// 形式 URI）。
    fn Open(&self, path: &str) -> Result<StorageRef, StoreError>;

    /// Concrete registered driver type, exposed for wiring verification.
    fn TypeName(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

/// 共享所有权的驱动句柄。
pub type DriverRef = Arc<dyn Driver>;

/// Mutex adapter that lets the synchronous registry own the canonical
/// `astersql_store_driver::TiKVDriver`, whose Open API applies mutable options.
pub struct TiKVStoreDriver {
    inner: Mutex<TiKVDriver>,
}

impl TiKVStoreDriver {
    /// Wraps the exact production driver instance configured by tidb-server.
    pub fn new(driver: TiKVDriver) -> Self {
        Self {
            inner: Mutex::new(driver),
        }
    }
}

impl Driver for TiKVStoreDriver {
    fn Open(&self, path: &str) -> Result<StorageRef, StoreError> {
        let store = self
            .inner
            .lock()
            .map_err(|error| StoreError::other(format!("TiKV driver lock poisoned: {error}")))?
            .Open(path)
            .map_err(|error| StoreError::other(error.to_string()))?;
        store
            .StartGCWorker()
            .map_err(|error| StoreError::other(error.to_string()))?;
        Ok(Arc::new(RegisteredTiKVStorage::new(store)))
    }

    fn TypeName(&self) -> &'static str {
        std::any::type_name::<TiKVDriver>()
    }
}

/// Store-registry view of the canonical client-rust-backed `TikvStore`.
struct RegisteredTiKVStorage {
    store: TikvStore,
    keyspace: String,
    codec: BasicCodec,
}

impl RegisteredTiKVStorage {
    fn new(store: TikvStore) -> Self {
        let keyspace = store.GetKeyspace();
        let codec = BasicCodec {
            api_version: if keyspace.is_empty() {
                ApiVersion::V1
            } else {
                ApiVersion::V2
            },
            // client-rust resolves a named API-v2 keyspace internally but does
            // not expose its numeric ID through the public 0.4 API.
            keyspace_id: 0,
        };
        Self {
            store,
            keyspace,
            codec,
        }
    }
}

impl Storage for RegisteredTiKVStorage {
    fn GetKeyspace(&self) -> &str {
        &self.keyspace
    }

    fn GetCodec(&self) -> &dyn Codec {
        &self.codec
    }

    fn Close(&self) -> Result<(), StoreError> {
        self.store
            .Close()
            .map_err(|error| StoreError::other(error.to_string()))
    }

    fn GetClusterID(&self) -> Option<u64> {
        Some(self.store.GetClusterID())
    }

    fn CurrentVersion(&self, txn_scope: &str) -> Result<Option<u64>, StoreError> {
        self.store
            .CurrentVersion(txn_scope)
            .map(|version| Some(version.0))
            .map_err(|error| StoreError::other(error.to_string()))
    }

    fn CanonicalTiKVStore(&self) -> Result<TikvStore, StoreError> {
        Ok(self.store.clone())
    }
}

/// Lightweight non-TiKV driver retained for the existing mock/unistore startup
/// paths. It is never registered for the TiKV store type.
#[derive(Default)]
pub struct LocalStoreDriver;

impl Driver for LocalStoreDriver {
    fn Open(&self, path: &str) -> Result<StorageRef, StoreError> {
        let url = Url::parse(path)
            .map_err(|error| StoreError::other(format!("invalid storage URL: {error}")))?;
        let keyspace = url
            .query_pairs()
            .find_map(|(key, value)| (key == "keyspaceName").then(|| value.into_owned()))
            .unwrap_or_default();
        Ok(Arc::new(LocalStorage {
            codec: BasicCodec {
                api_version: if keyspace.is_empty() {
                    ApiVersion::V1
                } else {
                    ApiVersion::V2
                },
                keyspace_id: 0,
            },
            keyspace,
        }))
    }
}

struct LocalStorage {
    keyspace: String,
    codec: BasicCodec,
}

impl Storage for LocalStorage {
    fn GetKeyspace(&self) -> &str {
        &self.keyspace
    }

    fn GetCodec(&self) -> &dyn Codec {
        &self.codec
    }
}

/// 全局已注册驱动表（按 StoreType 索引）。
fn store_drivers() -> &'static RwLock<HashMap<StoreType, DriverRef>> {
    static STORE_DRIVERS: OnceLock<RwLock<HashMap<StoreType, DriverRef>>> = OnceLock::new();
    STORE_DRIVERS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// 全局 SYSTEM keyspace 存储（nextgen 场景使用）。
fn system_store() -> &'static RwLock<Option<StorageRef>> {
    static SYSTEM_STORE: OnceLock<RwLock<Option<StorageRef>>> = OnceLock::new();
    SYSTEM_STORE.get_or_init(|| RwLock::new(None))
}

/// Register registers a KV storage driver under one of TiDB's valid store
/// types. The write lock covers both duplicate detection and insertion.
/// 按有效 StoreType 注册驱动；写锁同时覆盖重复检测与插入。
pub fn Register(tp: StoreType, driver: DriverRef) -> Result<(), StoreError> {
    if !tp.Valid() {
        return Err(StoreError::other(format!(
            "invalid storage type {}",
            tp.String()
        )));
    }

    let mut drivers = store_drivers().write().unwrap();
    if drivers.contains_key(&tp) {
        return Err(StoreError::other(format!(
            "{} is already registered",
            tp.String()
        )));
    }
    drivers.insert(tp, driver);
    Ok(())
}

/// 使用默认重试次数打开存储。
pub fn New(path: &str) -> Result<StorageRef, StoreError> {
    newStoreWithRetry(path, DEFAULT_MAX_RETRIES)?
        .ok_or_else(|| StoreError::other("default retry count unexpectedly produced no storage"))
}

/// 使用默认退避间隔打开存储。
pub fn newStoreWithRetry(path: &str, maxRetries: usize) -> Result<Option<StorageRef>, StoreError> {
    newStoreWithRetryAndInterval(path, maxRetries, RETRY_INTERVAL)
}

/// Testable form of Go's `newStoreWithRetry`. Production callers use the same
/// 500ms linear backoff; focused tests pass zero to avoid weakening the retry
/// branches or sleeping.
/// 可测版本：生产路径用 500ms 线性退避；测试可传入 0 间隔避免真实休眠。
pub fn newStoreWithRetryAndInterval(
    path: &str,
    maxRetries: usize,
    retryInterval: Duration,
) -> Result<Option<StorageRef>, StoreError> {
    // Registry only owns the scheme. The concrete TiKV driver parses the
    // authority because client-go-compatible comma-separated PD endpoints are
    // not representable by the generic URL parser.
    let (scheme, rest) = path
        .split_once("://")
        .ok_or_else(|| StoreError::other("invalid storage URL: missing ://"))?;
    if scheme.is_empty() || rest.is_empty() {
        return Err(StoreError::other(
            "invalid storage URL: scheme and path are required",
        ));
    }
    let name = scheme.to_ascii_lowercase();
    let driver = loadDriver(StoreType::from(name.clone())).ok_or_else(|| {
        StoreError::other(format!(
            "invalid uri format, storage {name} is not registered"
        ))
    })?;

    if maxRetries == 0 {
        return Ok(None);
    }

    // 可重试错误则按 attempt 倍数休眠后继续；不可重试则立即返回。
    let mut last_error = None;
    for attempt in 1..=maxRetries {
        match driver.Open(path) {
            Ok(storage) => return Ok(Some(storage)),
            Err(error) => {
                if !isNewStoreRetryableError(Some(&error)) {
                    return Err(error);
                }
                last_error = Some(error);
                thread::sleep(retryInterval.saturating_mul(attempt as u32));
            }
        }
    }

    Err(last_error.expect("a positive retry count records an open error"))
}

/// 按类型加载已注册驱动。
pub fn loadDriver(tp: StoreType) -> Option<DriverRef> {
    store_drivers().read().unwrap().get(&tp).cloned()
}

/// Returns sorted registered store type names for process-wiring tests.
pub fn RegisteredStoreTypes() -> Vec<String> {
    let mut types: Vec<_> = store_drivers()
        .read()
        .unwrap()
        .keys()
        .map(|store_type| store_type.String().to_owned())
        .collect();
    types.sort();
    types
}

/// Returns the concrete driver type registered for a store type.
pub fn RegisteredDriverTypeName(tp: StoreType) -> Option<&'static str> {
    store_drivers()
        .read()
        .unwrap()
        .get(&tp)
        .map(|driver| driver.TypeName())
}

/// Stable identity of the shared storage handle passed through bootstrap.
pub fn StorageIdentity(storage: &StorageRef) -> usize {
    Arc::as_ptr(storage) as *const () as usize
}

/// 判断打开存储时的错误是否应触发重试。
pub fn isNewStoreRetryableError(err: Option<&StoreError>) -> bool {
    let Some(err) = err else {
        return false;
    };
    error_chain(err).any(|inner| inner.kind == StoreErrorKind::TxnRetryable)
        || IsNotBootstrappedError(Some(err))
        || IsKeyspaceNotExistError(Some(err))
        || IsNotTSOLeaderError(Some(err))
}

/// 错误消息是否包含集群未 bootstrap 标记。
pub fn IsNotBootstrappedError(err: Option<&StoreError>) -> bool {
    err.is_some_and(|error| error.to_string().contains(NOT_BOOTSTRAPPED))
}

/// 错误消息是否包含 keyspace 条目不存在标记。
pub fn IsKeyspaceNotExistError(err: Option<&StoreError>) -> bool {
    err.is_some_and(|error| error.to_string().contains(ENTRY_NOT_FOUND))
}

/// 是否为 PD TSO/Leader 相关的 "not leader" 错误。
/// 消息含 "not leader" 且错误链上存在 PdClientGetTso/GetLeader kind。
pub fn IsNotTSOLeaderError(err: Option<&StoreError>) -> bool {
    let Some(err) = err else {
        return false;
    };
    if !err.to_string().contains(NOT_LEADER) {
        return false;
    }
    error_chain(err).any(|inner| {
        matches!(
            inner.kind,
            StoreErrorKind::PdClientGetTso | StoreErrorKind::PdClientGetLeader
        )
    })
}

/// 遍历错误链（自身及嵌套 source）。
fn error_chain(error: &StoreError) -> impl Iterator<Item = &StoreError> {
    std::iter::successors(Some(error), |current| current.source.as_deref())
}

/// 初始化默认存储；nextgen 下额外确保 SYSTEM keyspace 存储可用。
pub fn MustInitStorage(keyspaceName: &str) -> StorageRef {
    let default_store = mustInitStorage(keyspaceName);
    if kerneltype::IsNextGen() {
        // nextgen：用户 keyspace 与 SYSTEM keyspace 可能是两套存储。
        if default_store.GetKeyspace() != keyspace::System {
            *system_store().write().unwrap() = Some(mustInitStorage(keyspace::System));
        } else {
            *system_store().write().unwrap() = Some(default_store.clone());
        }
    }
    default_store
}

/// 获取全局 SYSTEM keyspace 存储（若已初始化）。
pub fn GetSystemStorage() -> Option<StorageRef> {
    system_store().read().unwrap().clone()
}

/// 设置全局 SYSTEM keyspace 存储；非空时必须绑定 SYSTEM keyspace。
pub fn SetSystemStorage(storage: Option<StorageRef>) {
    if let Some(storage) = &storage {
        assert_eq!(
            storage.GetKeyspace(),
            keyspace::System,
            "systemStore should be set with SYSTEM keyspace"
        );
    }
    *system_store().write().unwrap() = storage;
}

/// 初始化失败则 panic。
fn mustInitStorage(keyspaceName: &str) -> StorageRef {
    InitStorage(keyspaceName).unwrap_or_else(|error| panic!("initialize storage: {error}"))
}

/// 拼装存储 URI：`store://path`，可选附带 `keyspaceName` 查询参数。
pub fn BuildStoragePath(store: &str, path: &str, keyspaceName: &str) -> String {
    if keyspaceName.is_empty() {
        format!("{store}://{path}")
    } else {
        format!("{store}://{path}?keyspaceName={keyspaceName}")
    }
}

/// 按全局配置构造路径并打开存储。
pub fn InitStorage(keyspaceName: &str) -> Result<StorageRef, StoreError> {
    let cfg = config::get_global_config();
    New(&BuildStoragePath(&cfg.store, &cfg.path, keyspaceName))
}

/// Clears package globals between integration tests. Production code never
/// calls this; it mirrors the cleanup that Go tests perform around globals.
/// 测试用：清空驱动表与 SYSTEM 存储全局状态。
#[doc(hidden)]
pub fn ResetStoreStateForTest() {
    store_drivers().write().unwrap().clear();
    *system_store().write().unwrap() = None;
}
