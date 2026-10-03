// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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
// TiKV store driver：打开集群、缓存 Store、配置与请求追踪注入。
//
// PD（Placement Driver）负责调度与时间戳分配；本模块解析 `tikv://` 路径、
// 管理 TLS / gRPC keepalive / 本地锁存（local latches），并通过 `DriverBackend`
// 抽象真实或内存后端。GC（垃圾回收）清理过期 MVCC 版本；Safe Point 标记
// 可安全回收的水位。

use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
use std::time::Duration;

use thiserror::Error;

use crate::client_runtime::{ClientConfig, ClientRuntime, KeyspaceConfig};

/// Driver 层错误：路径、TLS、后端与未实现操作。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DriverError {
    #[error("invalid TiKV path: {0}")]
    InvalidPath(String),
    #[error("invalid TLS configuration: {0}")]
    InvalidTls(String),
    #[error("backend error: {0}")]
    Backend(String),
    #[error("operation is not implemented")]
    NotImplemented,
}

/// 集群 TLS 材料路径（CA / 证书 / 私钥）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Security {
    /// CA 证书路径。
    pub cluster_ssl_ca: String,
    /// 客户端证书路径。
    pub cluster_ssl_cert: String,
    /// 客户端私钥路径。
    pub cluster_ssl_key: String,
}

impl Security {
    /// 校验证书与私钥成对配置，并组装可选的 `TlsConfig`。
    pub fn to_tls_config(&self) -> Result<Option<TlsConfig>, DriverError> {
        if self.cluster_ssl_cert.is_empty() != self.cluster_ssl_key.is_empty() {
            return Err(DriverError::InvalidTls(
                "certificate and private key must be configured together".into(),
            ));
        }
        if self.cluster_ssl_ca.is_empty()
            && self.cluster_ssl_cert.is_empty()
            && self.cluster_ssl_key.is_empty()
        {
            return Ok(None);
        }
        Ok(Some(TlsConfig {
            ca_path: self.cluster_ssl_ca.clone(),
            cert_path: self.cluster_ssl_cert.clone(),
            key_path: self.cluster_ssl_key.clone(),
        }))
    }
}

/// 已解析的 TLS 文件路径三元组。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TlsConfig {
    /// CA 路径。
    pub ca_path: String,
    /// 证书路径。
    pub cert_path: String,
    /// 私钥路径。
    pub key_path: String,
}

/// TiKV 客户端 gRPC keepalive 配置（秒）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TiKvClientConfig {
    /// keepalive 探测间隔（秒）。
    pub grpc_keep_alive_time: u64,
    /// keepalive 超时（秒）。
    pub grpc_keep_alive_timeout: u64,
}

impl Default for TiKvClientConfig {
    fn default() -> Self {
        Self {
            grpc_keep_alive_time: 10,
            grpc_keep_alive_timeout: 3,
        }
    }
}

/// 事务本地锁存（local latches）开关与容量，用于提交前本地串行化冲突键。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TxnLocalLatches {
    /// 是否启用本地锁存。
    pub enabled: bool,
    /// 锁存槽容量。
    pub capacity: u64,
}

/// PD 客户端超时配置（秒）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdClientConfig {
    /// PD 服务端请求超时（秒）。
    pub pd_server_timeout: u64,
}

impl Default for PdClientConfig {
    fn default() -> Self {
        Self {
            pd_server_timeout: 3,
        }
    }
}

/// 进程级全局配置：路径、转发、安全与各客户端参数。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlobalConfig {
    /// 逗号分隔的地址路径（部分 ldflag 场景读取）。
    pub path: String,
    /// 是否启用请求转发（forwarding）。
    pub enable_forwarding: bool,
    /// TLS 安全配置。
    pub security: Security,
    /// TiKV 客户端配置。
    pub tikv_client: TiKvClientConfig,
    /// 本地锁存配置。
    pub txn_local_latches: TxnLocalLatches,
    /// PD 客户端配置。
    pub pd_client: PdClientConfig,
}

/// 全局配置的 `OnceLock` 单元。
fn global_config_cell() -> &'static RwLock<GlobalConfig> {
    static CONFIG: OnceLock<RwLock<GlobalConfig>> = OnceLock::new();
    CONFIG.get_or_init(|| RwLock::new(GlobalConfig::default()))
}

/// 读取全局配置快照。
pub fn get_global_config() -> GlobalConfig {
    global_config_cell().read().unwrap().clone()
}

/// Go package initialization installs resource-controller hooks.  The Rust
/// driver keeps those hooks behind its backend trait, so construction is the
/// initialization boundary and this function intentionally has no side effect.
///
/// Go 包初始化会安装资源控制器钩子；Rust 侧钩子在 backend trait 中，本函数无副作用。
pub fn init() {}

/// 覆盖写入全局配置。
pub fn set_global_config(config: GlobalConfig) {
    *global_config_cell().write().unwrap() = config;
}

/// 指标常量标签的全局存储。
fn metrics_labels_cell() -> &'static RwLock<HashMap<String, String>> {
    static LABELS: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
    LABELS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// 设置指标常量标签。
pub fn set_const_labels(labels: impl IntoIterator<Item = (String, String)>) {
    *metrics_labels_cell().write().unwrap() = labels.into_iter().collect();
}

/// 读取指标常量标签。
pub fn get_const_labels() -> HashMap<String, String> {
    metrics_labels_cell().read().unwrap().clone()
}

/// 打开 PD 客户端时使用的选项集合。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdClientOptions {
    /// gRPC 最大接收消息大小。
    pub max_receive_message_size: i32,
    /// keepalive 间隔。
    pub keep_alive_time: Duration,
    /// keepalive 超时。
    pub keep_alive_timeout: Duration,
    /// 服务端超时。
    pub server_timeout: Duration,
    /// 是否启用转发。
    pub enable_forwarding: bool,
    /// 附加到指标的常量标签。
    pub metrics_labels: HashMap<String, String>,
}

/// 解析 `tikv://` 路径后的 PD 地址、GC 开关与 keyspace 名。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedPath {
    /// PD 地址列表。
    pub pd_addrs: Vec<String>,
    /// 是否禁用 GC worker。
    pub disable_gc: bool,
    /// Keyspace（多租户键空间）名称。
    pub keyspace_name: String,
}

/// 解析 `tikv://host1:port1,host2:port2?disableGC=...&keyspaceName=...`。
pub fn parse_path(path: &str) -> Result<ParsedPath, DriverError> {
    // `Url` cannot represent client-go's comma-separated host:port authority
    // (it interprets everything after the first colon as one port).  Split the
    // authority first, then use form_urlencoded only for the query component.
    // 标准 Url 无法表示逗号分隔多地址 authority，故先拆分再解析 query。
    let rest = path
        .strip_prefix("tikv://")
        .ok_or_else(|| DriverError::InvalidPath("path must start with tikv://".into()))?;
    let (authority_and_path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let authority = authority_and_path.split('/').next().unwrap_or_default();
    let pd_addrs: Vec<_> = authority
        .split(',')
        .filter(|addr| !addr.is_empty())
        .map(str::to_owned)
        .collect();
    if pd_addrs.is_empty() {
        return Err(DriverError::InvalidPath("PD address is empty".into()));
    }
    let mut disable_gc = false;
    let mut keyspace_name = String::new();
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        match key.as_ref() {
            "disableGC" => {
                disable_gc = value.parse::<bool>().map_err(|_| {
                    DriverError::InvalidPath(format!("invalid disableGC value {value}"))
                })?;
            }
            "keyspaceName" | "keyspace" => keyspace_name = value.into_owned(),
            _ => {}
        }
    }
    Ok(ParsedPath {
        pd_addrs,
        disable_gc,
        keyspace_name,
    })
}

/// Meta 服务地址信息：PD 与 etcd group。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetaServiceInfo {
    /// PD 地址。
    pub pd_addrs: Vec<String>,
    /// etcd / group 地址。
    pub group_addrs: Vec<String>,
}

/// Safe Point KV 初始化结果：meta 信息与 safe point 标识。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SafePointKvSetup {
    /// Meta 服务信息。
    pub meta_service_info: MetaServiceInfo,
    /// PD 地址副本。
    pub pd_addrs: Vec<String>,
    /// Group 地址副本。
    pub group_addrs: Vec<String>,
    /// Safe point 资源 ID。
    pub safe_point_id: String,
}

/// Go 风格别名。
pub type safePointKVSetup = SafePointKvSetup;

/// 可替换的存储后端：打开/关闭 PD、Safe Point、取时间戳与锁等待信息。
pub trait DriverBackend: Send + Sync + fmt::Debug {
    /// 打开 PD 连接，返回 cluster_id。
    fn open_pd(
        &self,
        addrs: &[String],
        keyspace: &str,
        security: &Security,
        options: &PdClientOptions,
    ) -> Result<u64, DriverError>;
    /// 创建 Safe Point KV 客户端设置。
    fn new_safe_point_kv(
        &self,
        cluster_id: u64,
        keyspace: &str,
        tls: Option<&TlsConfig>,
    ) -> Result<SafePointKvSetup, DriverError>;
    /// 关闭 PD 连接（默认空实现）。
    fn close_pd(&self, _cluster_id: u64) {}
    /// 关闭 Safe Point 资源（默认空实现）。
    fn close_safe_point(&self, _safe_point_id: &str) {}
    /// 关闭 Store（默认成功）。
    fn close_store(&self, _uuid: &str) -> Result<(), DriverError> {
        Ok(())
    }
    /// 获取当前时间戳（按事务作用域 txn_scope）。
    fn current_timestamp(&self, _txn_scope: &str) -> Result<u64, DriverError>;
    /// 收集锁等待条目（默认空）。
    fn lock_waits(&self) -> Vec<Result<Vec<WaitForEntry>, DriverError>> {
        Vec::new()
    }
}

/// client-rust 驱动的生产后端；PD/TiKV 请求复用同一个 `ClientRuntime`。
struct OfficialBackend {
    runtime: Arc<RwLock<ClientRuntime>>,
    pd_addrs: Vec<String>,
}

/// 把标准 Coprocessor 的 protobuf-neutral 锁交回同一个官方事务客户端。
struct OfficialTransactionLockResolver {
    runtime: Arc<RwLock<ClientRuntime>>,
}

impl astersql_store_copr::TransactionLockResolver for OfficialTransactionLockResolver {
    fn resolve_locks(
        &self,
        locks: &[astersql_store_copr::TransactionLock],
        caller_start_ts: u64,
    ) -> astersql_store_copr::BatchResult<()> {
        self.runtime
            .read()
            .map_err(|error| astersql_store_copr::BatchError::OtherResponse(error.to_string()))?
            .resolve_locks(locks, caller_start_ts)
            .map_err(|error| astersql_store_copr::BatchError::OtherResponse(error.to_string()))
    }
}

impl fmt::Debug for OfficialBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OfficialBackend")
            .field("pd_addrs", &self.pd_addrs)
            .finish_non_exhaustive()
    }
}

impl DriverBackend for OfficialBackend {
    fn lock_waits(&self) -> Vec<Result<Vec<WaitForEntry>, DriverError>> {
        vec![
            self.runtime
                .read()
                .map_err(|error| DriverError::Backend(error.to_string()))
                .and_then(|runtime| {
                    runtime
                        .lock_waits()
                        .map_err(|error| DriverError::Backend(error.to_string()))
                }),
        ]
    }

    fn open_pd(
        &self,
        addrs: &[String],
        _keyspace: &str,
        _security: &Security,
        _options: &PdClientOptions,
    ) -> Result<u64, DriverError> {
        Ok(stable_cluster_identity(addrs))
    }

    fn new_safe_point_kv(
        &self,
        cluster_id: u64,
        _keyspace: &str,
        _tls: Option<&TlsConfig>,
    ) -> Result<SafePointKvSetup, DriverError> {
        Ok(SafePointKvSetup {
            meta_service_info: MetaServiceInfo {
                pd_addrs: self.pd_addrs.clone(),
                group_addrs: self.pd_addrs.clone(),
            },
            pd_addrs: self.pd_addrs.clone(),
            group_addrs: self.pd_addrs.clone(),
            safe_point_id: format!("client-rust-safe-point-{cluster_id}"),
        })
    }

    fn close_store(&self, _uuid: &str) -> Result<(), DriverError> {
        self.runtime
            .write()
            .map_err(|error| DriverError::Backend(error.to_string()))?
            .close()
            .map_err(|error| DriverError::Backend(error.to_string()))
    }

    fn current_timestamp(&self, _txn_scope: &str) -> Result<u64, DriverError> {
        self.runtime
            .read()
            .map_err(|error| DriverError::Backend(error.to_string()))?
            .current_timestamp()
            .map_err(|error| DriverError::Backend(error.to_string()))
    }
}

fn stable_cluster_identity(addrs: &[String]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in addrs.join(",").bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// 内存后端：用 FNV 哈希地址列表作 cluster_id，单调递增时间戳。
#[derive(Debug, Default)]
pub struct InMemoryBackend {
    /// 下一个要分配的时间戳。
    next_timestamp: Mutex<u64>,
}

impl DriverBackend for InMemoryBackend {
    fn open_pd(
        &self,
        addrs: &[String],
        _keyspace: &str,
        _security: &Security,
        _options: &PdClientOptions,
    ) -> Result<u64, DriverError> {
        // Stable FNV-1a gives equal endpoint lists the same cache identity.
        // 稳定 FNV-1a：相同端点列表得到相同缓存身份。
        Ok(stable_cluster_identity(addrs))
    }

    fn new_safe_point_kv(
        &self,
        cluster_id: u64,
        _keyspace: &str,
        _tls: Option<&TlsConfig>,
    ) -> Result<SafePointKvSetup, DriverError> {
        let pd_addrs = vec![format!("pd-{cluster_id}")];
        let group_addrs = vec![format!("etcd-{cluster_id}")];
        Ok(SafePointKvSetup {
            meta_service_info: MetaServiceInfo {
                pd_addrs: pd_addrs.clone(),
                group_addrs: group_addrs.clone(),
            },
            pd_addrs,
            group_addrs,
            safe_point_id: format!("safe-point-{cluster_id}"),
        })
    }

    fn current_timestamp(&self, _txn_scope: &str) -> Result<u64, DriverError> {
        let mut ts = self.next_timestamp.lock().unwrap();
        *ts += 1;
        Ok(*ts)
    }
}

/// 一次性 Driver 选项闭包类型。
pub type DriverOption = Box<dyn FnOnce(&mut TiKVDriver) + Send>;
/// 与 Go `Option` 同义的别名。
pub type OptionFn = DriverOption;

/// 覆盖 Security 配置的选项。
pub fn WithSecurity(security: Security) -> DriverOption {
    Box::new(move |driver| driver.security = security)
}

/// 覆盖 TiKV 客户端配置的选项。
pub fn WithTiKVClientConfig(config: TiKvClientConfig) -> DriverOption {
    Box::new(move |driver| driver.tikv_config = config)
}

/// 覆盖本地锁存配置的选项。
pub fn WithTxnLocalLatches(config: TxnLocalLatches) -> DriverOption {
    Box::new(move |driver| driver.txn_local_latches = config)
}

/// 覆盖 PD 客户端配置的选项。
pub fn WithPDClientConfig(config: PdClientConfig) -> DriverOption {
    Box::new(move |driver| driver.pd_config = config)
}

/// TiKV Driver：持有配置与后端，负责 Open Store。
#[derive(Debug)]
pub struct TiKVDriver {
    /// PD 客户端配置。
    pub pd_config: PdClientConfig,
    /// TLS 安全配置。
    pub security: Security,
    /// TiKV 客户端配置。
    pub tikv_config: TiKvClientConfig,
    /// 本地锁存配置。
    pub txn_local_latches: TxnLocalLatches,
    /// 可替换后端实现。
    backend: Option<Arc<dyn DriverBackend>>,
}

impl Default for TiKVDriver {
    fn default() -> Self {
        let config = get_global_config();
        Self {
            pd_config: config.pd_client,
            security: config.security,
            tikv_config: config.tikv_client,
            txn_local_latches: config.txn_local_latches,
            backend: None,
        }
    }
}

impl TiKVDriver {
    /// 使用指定后端构造 Driver（其余配置取全局默认）。
    pub fn with_backend(backend: Arc<dyn DriverBackend>) -> Self {
        Self {
            backend: Some(backend),
            ..Self::default()
        }
    }

    /// 无额外选项打开 Store。
    pub fn Open(&mut self, path: &str) -> Result<TikvStore, DriverError> {
        self.OpenWithOptions(path, Vec::new())
    }

    /// 重置为全局默认后再应用选项闭包。
    pub fn setDefaultAndOptions(&mut self, options: Vec<DriverOption>) {
        let config = get_global_config();
        self.pd_config = config.pd_client;
        self.security = config.security;
        self.tikv_config = config.tikv_client;
        self.txn_local_latches = config.txn_local_latches;
        for option in options {
            option(self);
        }
    }

    /// 解析路径、打开 PD、复用或创建 Store，并初始化 Safe Point。
    pub fn OpenWithOptions(
        &mut self,
        path: &str,
        options: Vec<DriverOption>,
    ) -> Result<TikvStore, DriverError> {
        self.setDefaultAndOptions(options);
        let parsed = parse_path(path)?;
        let pd_options = self.pdClientOptions();
        let tls_config = self.security.to_tls_config()?;
        let (backend, client_runtime) = match &self.backend {
            Some(backend) => (Arc::clone(backend), None),
            None => {
                let mut config = ClientConfig::new(parsed.pd_addrs.clone())
                    .with_timeout(Duration::from_secs(self.pd_config.pd_server_timeout));
                if let Some(tls) = tls_config.clone() {
                    config = config.with_tls(tls);
                }
                if !parsed.keyspace_name.is_empty() {
                    config =
                        config.with_keyspace(KeyspaceConfig::ApiV2(parsed.keyspace_name.clone()));
                }
                let runtime = Arc::new(RwLock::new(
                    ClientRuntime::connect(config)
                        .map_err(|error| DriverError::Backend(error.to_string()))?,
                ));
                let backend: Arc<dyn DriverBackend> = Arc::new(OfficialBackend {
                    runtime: Arc::clone(&runtime),
                    pd_addrs: parsed.pd_addrs.clone(),
                });
                (backend, Some(runtime))
            }
        };
        let cluster_id = backend.open_pd(
            &parsed.pd_addrs,
            &parsed.keyspace_name,
            &self.security,
            &pd_options,
        )?;
        let uuid = format!("tikv-{cluster_id}/{}", parsed.keyspace_name);

        // 清理已失效弱引用；若同 uuid 仍存活则复用并关闭多余 PD 连接。
        let mut cache = store_cache().lock().unwrap();
        cache.retain(|_, store| store.strong_count() > 0);
        if let Some(store) = cache.get(&uuid).and_then(Weak::upgrade) {
            backend.close_pd(cluster_id);
            if client_runtime.is_some() {
                let _ = backend.close_store(&uuid);
            }
            return Ok(TikvStore { inner: store });
        }

        let safe_point =
            match backend.new_safe_point_kv(cluster_id, &parsed.keyspace_name, tls_config.as_ref())
            {
                Ok(setup) => setup,
                Err(err) => {
                    backend.close_pd(cluster_id);
                    if client_runtime.is_some() {
                        let _ = backend.close_store(&uuid);
                    }
                    return Err(err);
                }
            };

        // client-rust 0.4 未公开标准 DAG RPC；这里仅补 transport，并继续复用
        // copr::Store 内唯一的 RegionCache，不另建事务客户端或路由缓存。
        let coprocessor_store = if let Some(runtime) = client_runtime.as_ref() {
            let mut config = astersql_store_copr::NetworkConfig::new(parsed.pd_addrs.clone());
            config.timeout = Duration::from_secs(self.pd_config.pd_server_timeout);
            config.keyspace_name = parsed.keyspace_name.clone();
            config.security = tls_config
                .as_ref()
                .map(|tls| astersql_store_copr::NetworkSecurity {
                    ca_path: tls.ca_path.clone(),
                    cert_path: tls.cert_path.clone(),
                    key_path: tls.key_path.clone(),
                });
            let lock_resolver: Arc<dyn astersql_store_copr::TransactionLockResolver> =
                Arc::new(OfficialTransactionLockResolver {
                    runtime: Arc::clone(runtime),
                });
            let network = match astersql_store_copr::NetworkBackend::connect(config, lock_resolver)
            {
                Ok(network) => network,
                Err(error) => {
                    backend.close_safe_point(&safe_point.safe_point_id);
                    backend.close_pd(cluster_id);
                    let _ = backend.close_store(&uuid);
                    return Err(DriverError::Backend(error.to_string()));
                }
            };
            let network: Arc<dyn astersql_store_copr::StoreBackend> = Arc::new(network);
            match astersql_store_copr::Store::new(
                network,
                &astersql_store_copr::CoprocessorCacheConfig::default(),
                false,
                false,
            ) {
                Ok(store) => Some(Arc::new(store)),
                Err(error) => {
                    backend.close_safe_point(&safe_point.safe_point_id);
                    backend.close_pd(cluster_id);
                    let _ = backend.close_store(&uuid);
                    return Err(DriverError::Backend(error.to_string()));
                }
            }
        } else {
            None
        };

        let store = Arc::new(Mutex::new(TikvStoreInner {
            uuid: uuid.clone(),
            tls_config,
            enable_gc: !parsed.disable_gc,
            gc_worker_started: false,
            local_latches_capacity: self
                .txn_local_latches
                .enabled
                .then_some(self.txn_local_latches.capacity),
            cluster_id,
            keyspace: parsed.keyspace_name,
            closed: false,
            meta_service_info: safe_point.meta_service_info,
            safe_point_id: safe_point.safe_point_id,
            options: HashMap::new(),
            backend,
            client_runtime,
            coprocessor_store,
            metadata_snapshot: None,
        }));
        cache.insert(uuid, Arc::downgrade(&store));
        Ok(TikvStore { inner: store })
    }

    /// 由 Driver 当前配置组装 `PdClientOptions`。
    pub fn pdClientOptions(&self) -> PdClientOptions {
        PdClientOptions {
            max_receive_message_size: i32::MAX,
            keep_alive_time: Duration::from_secs(self.tikv_config.grpc_keep_alive_time),
            keep_alive_timeout: Duration::from_secs(self.tikv_config.grpc_keep_alive_timeout),
            server_timeout: Duration::from_secs(self.pd_config.pd_server_timeout),
            enable_forwarding: get_global_config().enable_forwarding,
            metrics_labels: get_const_labels(),
        }
    }
}

/// 按 uuid 缓存 Store 弱引用，避免重复打开同一集群/keyspace。
fn store_cache() -> &'static Mutex<HashMap<String, Weak<Mutex<TikvStoreInner>>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Weak<Mutex<TikvStoreInner>>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 对外可见的 TiKV Store 句柄（内部共享可变状态）。
#[derive(Clone)]
pub struct TikvStore {
    /// 共享的内部状态。
    inner: Arc<Mutex<TikvStoreInner>>,
}

/// Go 风格别名。
pub type tikvStore = TikvStore;

impl fmt::Debug for TikvStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let inner = self.inner.lock().unwrap();
        f.debug_struct("TikvStore")
            .field("uuid", &inner.uuid)
            .field("cluster_id", &inner.cluster_id)
            .field("keyspace", &inner.keyspace)
            .field("closed", &inner.closed)
            .finish()
    }
}

/// Store 内部可变状态。
struct MetadataSnapshot {
    pd: Arc<dyn astersql_metaservice::MetadataPdClient>,
    meta: Option<astersql_metaservice::DialKeyspaceMeta>,
}

struct TikvStoreInner {
    /// 缓存键 / 唯一标识。
    uuid: String,
    /// 可选 TLS 配置。
    tls_config: Option<TlsConfig>,
    /// 是否启用 GC。
    enable_gc: bool,
    /// GC worker 是否已启动。
    gc_worker_started: bool,
    /// 启用时的本地锁存容量。
    local_latches_capacity: Option<u64>,
    /// 集群 ID。
    cluster_id: u64,
    /// Keyspace 名。
    keyspace: String,
    /// 是否已关闭。
    closed: bool,
    /// Meta 服务地址。
    meta_service_info: MetaServiceInfo,
    /// Safe Point 资源 ID。
    safe_point_id: String,
    /// 类型擦除的可扩展选项表。
    options: HashMap<String, Arc<dyn Any + Send + Sync>>,
    /// 后端引用。
    backend: Arc<dyn DriverBackend>,
    /// 生产路径共享的 client-rust runtime；显式测试后端为 `None`。
    client_runtime: Option<Arc<RwLock<ClientRuntime>>>,
    /// 标准 DAG transport 与 AsterSQL 既有 RegionCache 的唯一组装实例。
    coprocessor_store: Option<Arc<astersql_store_copr::Store>>,
    metadata_snapshot: Option<Arc<MetadataSnapshot>>,
}

/// MVCC 版本号包装（通常为时间戳）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version(pub u64);

/// 轻量事务句柄，仅携带 start_ts。
#[derive(Clone, Debug)]
pub struct Transaction {
    /// 事务开始时间戳。
    pub start_ts: u64,
}

/// 指定版本的只读快照占位。
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// 快照版本。
    pub version: Version,
}

/// 锁等待图中的一条边：txn 等待 waiting_for_txn。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WaitForEntry {
    /// 等待方事务 start_ts。
    pub txn: u64,
    /// 被等待方事务 start_ts。
    pub waiting_for_txn: u64,
    pub key: Vec<u8>,
    pub key_hash: u64,
    pub resource_group_tag: Vec<u8>,
    pub wait_time: u64,
}

impl TikvStore {
    /// Whether this store owns a connected production client-rust runtime.
    pub fn has_real_client_runtime(&self) -> bool {
        self.inner.lock().unwrap().client_runtime.is_some()
    }

    pub(crate) fn client_runtime(&self) -> Option<Arc<RwLock<ClientRuntime>>> {
        self.inner.lock().unwrap().client_runtime.clone()
    }

    pub(crate) fn coprocessor_store(&self) -> Option<Arc<astersql_store_copr::Store>> {
        self.inner.lock().unwrap().coprocessor_store.clone()
    }

    #[cfg(test)]
    pub(crate) fn set_coprocessor_store_for_test(&self, store: Arc<astersql_store_copr::Store>) {
        self.inner.lock().unwrap().coprocessor_store = Some(store);
    }

    pub(crate) fn uuid(&self) -> String {
        self.inner.lock().unwrap().uuid.clone()
    }

    /// 按键取类型化选项。
    pub fn GetOption<T: Any + Send + Sync>(&self, key: &str) -> Option<Arc<T>> {
        self.inner
            .lock()
            .unwrap()
            .options
            .get(key)
            .cloned()
            .and_then(|value| value.downcast().ok())
    }

    /// 设置或清除类型化选项（`None` 表示删除）。
    pub fn SetOption<T: Any + Send + Sync>(&self, key: impl Into<String>, value: Option<T>) {
        let mut inner = self.inner.lock().unwrap();
        let key = key.into();
        match value {
            Some(value) => {
                inner.options.insert(key, Arc::new(value));
            }
            None => {
                inner.options.remove(&key);
            }
        }
    }

    /// 存储引擎名称。
    pub fn Name(&self) -> &'static str {
        "TiKV"
    }

    /// 人类可读描述。
    pub fn Describe(&self) -> &'static str {
        "TiKV is a distributed transactional key-value database"
    }

    /// 返回 etcd 地址；ldflag 打开时可改从全局 path 读取。
    pub fn EtcdAddrs(&self) -> Result<Vec<String>, DriverError> {
        if ldflag_get_etcd_addrs_from_config() == "1" {
            return Ok(get_global_config()
                .path
                .split(',')
                .map(str::to_owned)
                .collect());
        }
        Ok(self.getMetaServiceInfo()?.group_addrs)
    }

    /// 返回 PD 地址列表。
    pub fn GetPDAddrs(&self) -> Result<Vec<String>, DriverError> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .meta_service_info
            .pd_addrs
            .clone())
    }

    /// 返回 Meta 服务信息副本。
    pub fn getMetaServiceInfo(&self) -> Result<MetaServiceInfo, DriverError> {
        Ok(self.inner.lock().unwrap().meta_service_info.clone())
    }

    /// 返回 TLS 配置（若有）。
    pub fn TLSConfig(&self) -> Option<TlsConfig> {
        self.inner.lock().unwrap().tls_config.clone()
    }

    /// 在启用 GC 时标记 GC worker 已启动。
    pub fn StartGCWorker(&self) -> Result<(), DriverError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.enable_gc {
            inner.gc_worker_started = true;
        }
        Ok(())
    }

    /// 获取复用 AsterSQL RegionCache 的标准 Coprocessor 客户端。
    pub fn GetClient(&self) -> Result<astersql_store_copr::CopClient, DriverError> {
        self.coprocessor_store()
            .map(|store| store.get_client())
            .ok_or_else(|| DriverError::Backend("coprocessor transport is unavailable".into()))
    }

    /// 获取 MPP 客户端占位。
    pub fn GetMPPClient(&self) -> MppClient {
        MppClient
    }

    /// 关闭 Store：移出缓存并关闭 Safe Point / PD / Store 资源。
    pub fn Close(&self) -> Result<(), DriverError> {
        let (uuid, cluster_id, safe_point_id, backend, coprocessor_store) = {
            let mut inner = self.inner.lock().unwrap();
            if inner.closed {
                return Ok(());
            }
            inner.closed = true;
            if let Some(metadata) = inner.metadata_snapshot.take() {
                metadata.pd.close();
            }
            (
                inner.uuid.clone(),
                inner.cluster_id,
                inner.safe_point_id.clone(),
                Arc::clone(&inner.backend),
                inner.coprocessor_store.clone(),
            )
        };
        store_cache().lock().unwrap().remove(&uuid);
        let coprocessor_close = if let Some(store) = coprocessor_store {
            store.close();
            store
                .kv_store()
                .client()
                .close()
                .map_err(|error| DriverError::Backend(error.to_string()))
        } else {
            Ok(())
        };
        backend.close_safe_point(&safe_point_id);
        backend.close_pd(cluster_id);
        let store_close = backend.close_store(&uuid);
        coprocessor_close.and(store_close)
    }

    /// 获取内存管理器占位。
    pub fn GetMemCache(&self) -> MemManager {
        MemManager
    }

    /// 开启新事务：向后端申请全局 start_ts。
    pub fn Begin(&self) -> Result<Transaction, DriverError> {
        let inner = self.inner.lock().unwrap();
        Ok(Transaction {
            start_ts: inner.backend.current_timestamp("global")?,
        })
    }

    /// 使用官方 client-rust 创建悲观事务。
    pub fn BeginPessimistic(
        &self,
    ) -> Result<Box<dyn astersql_kv::Transaction>, astersql_kv::errors::SharedError> {
        crate::kv_adapter::begin_transaction(
            self,
            astersql_store_driver_txn::ClientTransactionMode::Pessimistic,
        )
    }

    /// 按版本构造快照占位。
    pub fn GetSnapshot(&self, version: Version) -> Snapshot {
        Snapshot { version }
    }

    /// 按事务作用域获取当前版本（时间戳）。
    pub fn CurrentVersion(&self, txn_scope: &str) -> Result<Version, DriverError> {
        let inner = self.inner.lock().unwrap();
        inner.backend.current_timestamp(txn_scope).map(Version)
    }

    /// 状态查询占位（尚未实现）。
    pub fn ShowStatus(&self, _key: &str) -> Result<Box<dyn Any>, DriverError> {
        Err(DriverError::NotImplemented)
    }

    /// 汇总后端返回的锁等待条目，跳过失败响应。
    pub fn GetLockWaits(&self) -> Result<Vec<WaitForEntry>, DriverError> {
        let backend = Arc::clone(&self.inner.lock().unwrap().backend);
        let mut result = Vec::new();
        // A failed or nil-equivalent store response is skipped, matching Go.
        // 失败或空响应跳过，与 Go 行为一致。
        for response in backend.lock_waits() {
            if let Ok(mut entries) = response {
                result.append(&mut entries);
            }
        }
        Ok(result)
    }

    /// 返回绑定当前 keyspace 的编解码器占位。
    pub fn GetCodec(&self) -> Codec {
        Codec {
            keyspace: self.GetKeyspace(),
        }
    }

    /// 集群 ID。
    pub fn GetClusterID(&self) -> u64 {
        self.inner.lock().unwrap().cluster_id
    }

    /// Keyspace 名。
    pub fn GetKeyspace(&self) -> String {
        self.inner.lock().unwrap().keyspace.clone()
    }

    /// Resolve and retain complete PD metadata for the store's transaction keyspace.
    fn metadata_snapshot(
        &self,
    ) -> Result<Arc<MetadataSnapshot>, astersql_metaservice::MetaServiceError> {
        use astersql_metaservice::{ConnectMetadataPD, Context, MetaServiceError, PdSecurity};
        {
            let inner = self.inner.lock().unwrap();
            if inner.closed {
                return Err(MetaServiceError::Pd("TiKV store is closed".into()));
            }
            if let Some(snapshot) = &inner.metadata_snapshot {
                return Ok(snapshot.clone());
            }
        }
        let tls = self.TLSConfig();
        let security = tls
            .map(|tls| PdSecurity {
                ca: tls.ca_path,
                cert: tls.cert_path,
                key: tls.key_path,
            })
            .unwrap_or_default();
        let pd = ConnectMetadataPD(
            &Context::default(),
            &self
                .GetPDAddrs()
                .map_err(|error| MetaServiceError::Pd(error.to_string()))?,
            &security,
        )?;
        let name = self.GetKeyspace();
        let meta = if name.is_empty() {
            None
        } else {
            match pd.load_keyspace(&Context::default(), &name) {
                Ok(Some(meta)) => Some(meta),
                Ok(None) => {
                    pd.close();
                    return Err(MetaServiceError::Pd(format!(
                        "keyspace meta not found for keyspace {name:?}"
                    )));
                }
                Err(error) => {
                    pd.close();
                    return Err(error);
                }
            }
        };
        let snapshot = Arc::new(MetadataSnapshot { pd, meta });
        let mut inner = self.inner.lock().unwrap();
        if inner.closed {
            snapshot.pd.close();
            return Err(MetaServiceError::Pd("TiKV store is closed".into()));
        }
        if let Some(existing) = &inner.metadata_snapshot {
            snapshot.pd.close();
            return Ok(existing.clone());
        }
        inner.metadata_snapshot = Some(snapshot.clone());
        Ok(snapshot)
    }

    /// Resolve the numeric PD keyspace ID used by etcd's TiDB namespace.
    pub fn etcd_namespace(&self) -> Result<String, DriverError> {
        if self.GetKeyspace().is_empty() {
            return Ok(String::new());
        }
        let metadata = self
            .metadata_snapshot()
            .map_err(|error| DriverError::Backend(error.to_string()))?;
        Ok(metadata
            .meta
            .as_ref()
            .map(|meta| format!("/keyspaces/tidb/{}", meta.id))
            .unwrap_or_default())
    }

    /// 本地锁存容量（未启用则为 `None`）。
    pub fn local_latches_capacity(&self) -> Option<u64> {
        self.inner.lock().unwrap().local_latches_capacity
    }

    /// GC worker 是否已启动。
    pub fn gc_worker_started(&self) -> bool {
        self.inner.lock().unwrap().gc_worker_started
    }

    /// Store 是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.inner.lock().unwrap().closed
    }
}

/// Keyspace 感知的编解码器占位。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Codec {
    /// 所属 keyspace。
    pub keyspace: String,
}
/// Coprocessor（协处理器）客户端占位。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoprocessorClient;
/// MPP（大规模并行处理）客户端占位。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MppClient;
/// 内存缓存管理器占位。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemManager;

/// ldflag：是否从全局配置 path 读取 etcd 地址。
fn ldflag_cell() -> &'static RwLock<String> {
    static VALUE: OnceLock<RwLock<String>> = OnceLock::new();
    VALUE.get_or_init(|| RwLock::new("0".to_owned()))
}

/// 设置 ldflag 值（测试用）。
pub fn set_ldflag_get_etcd_addrs_from_config(value: impl Into<String>) {
    *ldflag_cell().write().unwrap() = value.into();
}

/// 读取 ldflag 当前值。
pub fn ldflag_get_etcd_addrs_from_config() -> String {
    ldflag_cell().read().unwrap().clone()
}

/// 追踪信息：连接 ID 与会话别名。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TraceInfo {
    /// 连接 ID。
    pub connection_id: u64,
    /// 会话别名。
    pub session_alias: String,
}

/// 请求来源语句上下文。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceStmt {
    /// 连接 ID。
    pub connection_id: u64,
    /// 会话别名。
    pub session_alias: String,
}

/// 请求上下文，可携带 SourceStmt。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RequestContext {
    /// 可选的来源语句信息。
    pub source_stmt: Option<SourceStmt>,
}

/// 发往 TiKV 的请求封装。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    /// 请求上下文。
    pub context: RequestContext,
}

/// TiKV 响应封装。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Response {
    /// 响应载荷。
    pub payload: Vec<u8>,
}

/// 追踪上下文，可携带 TraceInfo。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TraceContext {
    /// 可选追踪信息。
    pub trace_info: Option<TraceInfo>,
}

/// TiKV 客户端发送请求的抽象接口（同步与异步）。
pub trait TikvClient: Send + Sync {
    /// 同步发送请求。
    fn send_request(
        &self,
        context: &TraceContext,
        addr: &str,
        request: &mut Request,
        timeout: Duration,
    ) -> Result<Response, DriverError>;

    /// 异步发送请求，完成后调用 callback。
    fn send_request_async(
        &self,
        context: &TraceContext,
        addr: &str,
        request: &mut Request,
        callback: Box<dyn FnOnce(Result<Response, DriverError>) + Send>,
    );
}

/// 在发送前将 TraceInfo 注入 Request.SourceStmt 的客户端包装器。
pub struct InjectTraceClient<C> {
    /// 被包装的底层客户端。
    pub client: C,
}

/// Go 风格别名。
pub type injectTraceClient<C> = InjectTraceClient<C>;

impl<C: TikvClient> InjectTraceClient<C> {
    /// 若上下文含追踪信息，则写入请求的 SourceStmt。
    fn inject(context: &TraceContext, request: &mut Request) {
        if let Some(trace) = &context.trace_info {
            let source = request
                .context
                .source_stmt
                .get_or_insert_with(Default::default);
            source.connection_id = trace.connection_id;
            source.session_alias.clone_from(&trace.session_alias);
        }
    }

    /// 注入追踪后同步发送。
    pub fn SendRequest(
        &self,
        context: &TraceContext,
        addr: &str,
        request: &mut Request,
        timeout: Duration,
    ) -> Result<Response, DriverError> {
        Self::inject(context, request);
        self.client.send_request(context, addr, request, timeout)
    }

    /// 注入追踪后异步发送。
    pub fn SendRequestAsync(
        &self,
        context: &TraceContext,
        addr: &str,
        request: &mut Request,
        callback: Box<dyn FnOnce(Result<Response, DriverError>) + Send>,
    ) {
        Self::inject(context, request);
        self.client
            .send_request_async(context, addr, request, callback);
    }
}

/// 通过 backend 创建 Safe Point KV 设置的便捷函数。
pub fn newSafePointKV(
    backend: &dyn DriverBackend,
    cluster_id: u64,
    keyspace: &str,
    tls_config: Option<&TlsConfig>,
) -> Result<SafePointKvSetup, DriverError> {
    backend.new_safe_point_kv(cluster_id, keyspace, tls_config)
}

impl astersql_metaservice::EtcdMetadataStore for TikvStore {
    fn pd_client(
        &self,
    ) -> Result<
        Arc<dyn astersql_metaservice::MetadataPdClient>,
        astersql_metaservice::MetaServiceError,
    > {
        Ok(self.metadata_snapshot()?.pd.clone())
    }
    fn keyspace_meta(
        &self,
    ) -> Result<
        Option<astersql_metaservice::DialKeyspaceMeta>,
        astersql_metaservice::MetaServiceError,
    > {
        Ok(self.metadata_snapshot()?.meta.clone())
    }
}
