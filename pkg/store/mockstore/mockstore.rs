// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// mockstore 核心工厂：创建测试用的模拟 KV 存储（MockStorage）。
//
// 提供 MockTiKV / 嵌入式 UniStore 两种后端，以及 keyspace（键空间）、
// 集群 bootstrap、客户端劫持（hijack）等测试辅助选项。Keyspace 用于
// 多租户数据隔离；Region 是 TiKV 中按 key 范围划分的分片单元。

use crate::embedded_unistore::cluster::Cluster;
use crate::embedded_unistore::pd::{KeyspaceMeta, KeyspaceState, MAX_KEYSPACE_ID, PdClient};
use crate::embedded_unistore::rpc::RPCClient;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

/// 表示未指定 keyspace 的哨兵 ID（u32::MAX）。
pub const NULL_KEYSPACE_ID: u32 = u32::MAX;
/// 系统 keyspace 名称，对应 NextGen 场景下的 SYSTEM 键空间。
pub const SYSTEM_KEYSPACE_NAME: &str = "SYSTEM";
/// keyspace 配置中 GC 管理类型的键名。
pub const KEYSPACE_GC_MANAGEMENT_TYPE: &str = "gc_management_type";
/// GC 管理类型取值：按 keyspace 级别做垃圾回收。
pub const KEYSPACE_GC_KEYSPACE_LEVEL: &str = "keyspace_level";
/// UniStore bootstrap 镜像目录路径。
pub const IMAGE_FILE_PATH: &str = "/tmp/tidb-unistore-bootstraped-image/";

/// 模拟存储后端类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreType {
    /// 以 MockTiKV 身份使用嵌入式协议服务。
    MockTiKv,
    /// 进程内嵌入式 UniStore。
    EmbedUnistore,
}

/// 默认后端：嵌入式 UniStore。
pub const DEFAULT_STORE_TYPE: StoreType = StoreType::EmbedUnistore;

/// 测试用 keyspace 元数据（含配置项）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MockKeyspaceMeta {
    /// keyspace 数字 ID。
    pub id: u32,
    /// keyspace 名称。
    pub name: String,
    /// keyspace 生命周期状态。
    pub state: KeyspaceState,
    /// 附加配置（如 GC 管理类型）。
    pub config: HashMap<String, String>,
}

impl MockKeyspaceMeta {
    /// 转换为嵌入式 UniStore 使用的 KeyspaceMeta。
    pub fn to_embedded(&self) -> KeyspaceMeta {
        KeyspaceMeta {
            id: self.id,
            name: self.name.clone(),
            state: self.state,
        }
    }
}

/// mockstore 操作错误，包装可读错误信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoreError(pub String);
impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for StoreError {}
/// mockstore 统一 Result 别名。
pub type Result<T> = std::result::Result<T, StoreError>;

/// 模拟 KV 存储门面：持有 RPC/PD 客户端、集群与当前 keyspace。
pub struct MockStorage {
    /// 实际后端类型（MockTiKV 或 EmbedUnistore）。
    pub backend: StoreType,
    /// 向 store 发送 KV/Coprocessor RPC 的客户端。
    pub client: Arc<RPCClient>,
    /// Placement Driver（PD）客户端，负责 Region 路由与时间戳分配。
    pub pd_client: Arc<PdClient>,
    /// 进程内模拟集群拓扑。
    pub cluster: Arc<Cluster>,
    /// 当前会话绑定的 keyspace；None 表示未指定。
    pub current_keyspace: Option<MockKeyspaceMeta>,
    /// 事务本地闩（latch）容量；>0 表示启用本地冲突检测。
    pub txn_local_latches: usize,
    /// 是否已注入 DDL checker。
    pub ddl_checked: bool,
}

impl MockStorage {
    /// 关闭底层 RPC 客户端并释放连接。
    pub fn close(&self) -> Result<()> {
        self.client
            .close()
            .map_err(|error| StoreError(error.to_string()))
    }

    /// Matches Go `tikv.KVStore.IsLatchEnabled` for the mock store facade.
    /// 判断事务本地闩是否启用（容量大于 0）。
    pub fn is_latch_enabled(&self) -> bool {
        self.txn_local_latches > 0
    }
}

/// 包装 RPC 客户端的劫持回调（测试注入故障/拦截）。
pub type ClientHijacker = Arc<dyn Fn(Arc<RPCClient>) -> Arc<RPCClient> + Send + Sync>;
/// 包装 PD 客户端的劫持回调。
pub type PdClientHijacker = Arc<dyn Fn(Arc<PdClient>) -> Arc<PdClient> + Send + Sync>;
/// 集群创建后的检查/改写回调（如添加 TiFlash peer）。
pub type ClusterInspector = Arc<dyn Fn(&Arc<Cluster>) + Send + Sync>;

/// 创建 MockStorage 时的可配置选项集合。
pub struct MockOptions {
    /// 集群就绪后的检查器。
    pub cluster_inspector: ClusterInspector,
    /// 可选的 RPC 客户端劫持器。
    pub client_hijacker: Option<ClientHijacker>,
    /// 可选的 PD 客户端劫持器。
    pub pd_client_hijacker: Option<PdClientHijacker>,
    /// 存储数据路径；空字符串表示内存/临时路径。
    pub path: String,
    /// 事务本地闩容量。
    pub txn_local_latches: usize,
    /// 后端类型。
    pub store_type: StoreType,
    /// 是否启用 DDL checker 劫持。
    pub ddl_checker_hijack: bool,
    /// 透传给 TiKV 风格后端的额外选项。
    pub tikv_options: Vec<String>,
    /// PD 地址列表。
    pub pd_addresses: Vec<String>,
    /// 是否显式指定了 keyspace 相关选项。
    pub keyspace_specified: bool,
    /// 当前 keyspace ID；为 NULL_KEYSPACE_ID 表示未绑定。
    pub current_keyspace_id: u32,
    /// 集群中注册的全部 keyspace 元数据。
    pub cluster_keyspaces: Vec<MockKeyspaceMeta>,
}

impl Default for MockOptions {
    fn default() -> Self {
        Self {
            // Canonical Rust server::new_mock already creates one store and
            // one region, equivalent to Go's default bootstrap inspector.
            // 默认检查器为空操作：嵌入式 server 已创建单 store / 单 Region。
            cluster_inspector: Arc::new(|_| {}),
            client_hijacker: None,
            pd_client_hijacker: None,
            path: String::new(),
            txn_local_latches: 0,
            store_type: DEFAULT_STORE_TYPE,
            ddl_checker_hijack: false,
            tikv_options: Vec::new(),
            pd_addresses: Vec::new(),
            keyspace_specified: false,
            current_keyspace_id: NULL_KEYSPACE_ID,
            cluster_keyspaces: Vec::new(),
        }
    }
}

impl MockOptions {
    /// 按 current_keyspace_id 在 cluster_keyspaces 中查找元数据。
    pub fn current_keyspace_meta(&self) -> Option<MockKeyspaceMeta> {
        if self.current_keyspace_id == NULL_KEYSPACE_ID {
            return None;
        }
        self.cluster_keyspaces
            .iter()
            .find(|meta| meta.id == self.current_keyspace_id)
            .cloned()
            .or_else(|| panic!("currentKeyspaceID and clusterKeyspaces mismatches"))
    }
}

/// 修改 MockOptions 的函数式选项类型（对应 Go 的 Option 模式）。
pub type MockTiKVStoreOption = Box<dyn Fn(&mut MockOptions) + Send + Sync>;

/// 将多个选项合并为一个。
pub fn WithMultipleOptions(options: Vec<MockTiKVStoreOption>) -> MockTiKVStoreOption {
    Box::new(move |arguments| {
        for option in &options {
            option(arguments);
        }
    })
}

/// 设置 PD 地址列表。
pub fn WithPDAddr(addresses: Vec<String>) -> MockTiKVStoreOption {
    Box::new(move |options| options.pd_addresses.clone_from(&addresses))
}

/// 设置透传给 TiKV 后端的字符串选项。
pub fn WithTiKVOptions(values: Vec<String>) -> MockTiKVStoreOption {
    Box::new(move |options| options.tikv_options.clone_from(&values))
}

/// 安装 RPC 客户端劫持器。
pub fn WithClientHijacker(hijacker: ClientHijacker) -> MockTiKVStoreOption {
    Box::new(move |options| options.client_hijacker = Some(Arc::clone(&hijacker)))
}

/// 安装 PD 客户端劫持器。
pub fn WithPDClientHijacker(hijacker: PdClientHijacker) -> MockTiKVStoreOption {
    Box::new(move |options| options.pd_client_hijacker = Some(Arc::clone(&hijacker)))
}

/// 安装集群检查器。
pub fn WithClusterInspector(inspector: ClusterInspector) -> MockTiKVStoreOption {
    Box::new(move |options| options.cluster_inspector = Arc::clone(&inspector))
}

/// 指定存储后端类型。
pub fn WithStoreType(store_type: StoreType) -> MockTiKVStoreOption {
    Box::new(move |options| options.store_type = store_type)
}

/// 指定存储数据路径。
pub fn WithPath(path: impl Into<String>) -> MockTiKVStoreOption {
    let path = path.into();
    Box::new(move |options| options.path.clone_from(&path))
}

/// 设置事务本地闩容量。
pub fn WithTxnLocalLatches(capacity: usize) -> MockTiKVStoreOption {
    Box::new(move |options| options.txn_local_latches = capacity)
}

/// 启用 DDL checker 劫持（需事先注册 injector）。
pub fn WithDDLChecker() -> MockTiKVStoreOption {
    Box::new(|options| options.ddl_checker_hijack = true)
}

/// 在默认 Region 上追加若干 TiFlash peer，并强制使用 EmbedUnistore。
/// TiFlash 是列存分析引擎，通过 store label `engine=tiflash` 标识。
pub fn WithMockTiFlash(nodes: usize) -> MockTiKVStoreOption {
    WithMultipleOptions(vec![
        WithClusterInspector(Arc::new(move |cluster| {
            let manager = cluster.region_manager();
            // 取已 bootstrap 的首个 Region，向其添加 TiFlash 副本。
            let region_id = manager
                .scan_regions(&[], &[], 1)
                .first()
                .expect("mock cluster has no bootstrapped region")
                .meta
                .id;
            for index in 0..nodes {
                let store_id = manager.alloc_id();
                let peer_id = manager.alloc_id();
                manager.add_store(
                    store_id,
                    format!("tiflash{index}"),
                    vec![crate::embedded_unistore::tikv::mock_region::StoreLabel {
                        key: "engine".into(),
                        value: "tiflash".into(),
                    }],
                );
                manager
                    .add_peer(region_id, store_id, peer_id)
                    .expect("failed to add TiFlash peer");
            }
        })),
        WithStoreType(StoreType::EmbedUnistore),
    ])
}

/// 若未配置 GC 管理类型，则默认写入 keyspace 级别 GC。
pub fn enable_keyspace_level_gc_if_not_set(meta: &mut MockKeyspaceMeta) {
    meta.config
        .entry(KEYSPACE_GC_MANAGEMENT_TYPE.into())
        .or_insert_with(|| KEYSPACE_GC_KEYSPACE_LEVEL.into());
}

/// 设置当前唯一 keyspace；传入 None 表示清除绑定。
pub fn WithCurrentKeyspaceMeta(meta: Option<MockKeyspaceMeta>) -> MockTiKVStoreOption {
    Box::new(move |options| {
        options.keyspace_specified = true;
        if let Some(mut meta) = meta.clone() {
            enable_keyspace_level_gc_if_not_set(&mut meta);
            options.current_keyspace_id = meta.id;
            options.cluster_keyspaces = vec![meta];
        } else {
            options.current_keyspace_id = NULL_KEYSPACE_ID;
            options.cluster_keyspaces.clear();
        }
    })
}

/// 注册多个 keyspace 并指定当前使用的 ID。
pub fn WithKeyspacesAndCurrentKeyspaceID(
    keyspaces: Vec<MockKeyspaceMeta>,
    current_keyspace_id: u32,
) -> MockTiKVStoreOption {
    Box::new(move |options| {
        options.keyspace_specified = true;
        options.cluster_keyspaces.clone_from(&keyspaces);
        options.current_keyspace_id = current_keyspace_id;
        for meta in &mut options.cluster_keyspaces {
            enable_keyspace_level_gc_if_not_set(meta);
        }
    })
}

/// NextGen 模式开关：开启后未显式指定 keyspace 时自动绑定 SYSTEM。
static NEXT_GEN: AtomicBool = AtomicBool::new(false);
/// 设置全局 NextGen 标志。
pub fn set_next_gen(enabled: bool) {
    NEXT_GEN.store(enabled, Ordering::Release);
}

/// DDL checker 注入器：包装 MockStorage 以注入 DDL 校验逻辑。
type DdlInjector = Arc<dyn Fn(MockStorage) -> MockStorage + Send + Sync>;
static DDL_INJECTOR: OnceLock<RwLock<Option<DdlInjector>>> = OnceLock::new();

/// 注册或清除全局 DDL checker 注入器。
pub fn set_ddl_checker_injector(injector: Option<DdlInjector>) {
    *DDL_INJECTOR
        .get_or_init(|| RwLock::new(None))
        .write()
        .expect("DDL injector lock poisoned") = injector;
}

/// 按选项列表创建 MockStorage（对应 Go `NewMockStore`）。
pub fn NewMockStore(options: Vec<MockTiKVStoreOption>) -> Result<MockStorage> {
    let mut config = MockOptions::default();
    for option in options {
        option(&mut config);
    }
    // NextGen 且未指定 keyspace 时，自动绑定 SYSTEM keyspace。
    if NEXT_GEN.load(Ordering::Acquire) && !config.keyspace_specified {
        WithCurrentKeyspaceMeta(Some(MockKeyspaceMeta {
            id: MAX_KEYSPACE_ID - 1,
            name: SYSTEM_KEYSPACE_NAME.into(),
            ..MockKeyspaceMeta::default()
        }))(&mut config);
    }
    let mut store = match config.store_type {
        StoreType::MockTiKv => crate::tikv::new_mock_tikv_store(&config)?,
        StoreType::EmbedUnistore => crate::unistore::new_unistore(&config)?,
    };
    // 按需通过全局 injector 包装存储以启用 DDL 检查。
    if config.ddl_checker_hijack {
        if let Some(injector) = DDL_INJECTOR
            .get_or_init(|| RwLock::new(None))
            .read()
            .expect("DDL injector lock poisoned")
            .as_ref()
        {
            store = injector(store);
        } else {
            return Err(StoreError("DDL checker injector is not installed".into()));
        }
    }
    Ok(store)
}

/// 通过 `mocktikv://` URI 打开 MockTiKV 存储的驱动。
pub struct MockTiKVDriver;
impl MockTiKVDriver {
    /// 解析 URI 并创建 MockTiKV 后端存储；全局配置启用 latch 时一并生效。
    pub fn open(&self, uri: &str) -> Result<MockStorage> {
        let (scheme, path) = parse_uri(uri)?;
        if !scheme.eq_ignore_ascii_case("mocktikv") {
            return Err(StoreError(format!(
                "Uri scheme expected(mocktikv) but found ({scheme})"
            )));
        }
        let mut opts: Vec<MockTiKVStoreOption> =
            vec![WithPath(path), WithStoreType(StoreType::MockTiKv)];
        let latches = astersql_config::get_global_config()
            .txn_local_latches
            .clone();
        if latches.enabled {
            opts.push(WithTxnLocalLatches(latches.capacity as usize));
        }
        NewMockStore(opts)
    }
}

/// 通过 `unistore://` URI 打开嵌入式 UniStore 的驱动。
pub struct EmbedUnistoreDriver;
impl EmbedUnistoreDriver {
    /// 解析 URI 并创建 EmbedUnistore 后端存储。
    pub fn open(&self, uri: &str) -> Result<MockStorage> {
        let (scheme, path) = parse_uri(uri)?;
        if !scheme.eq_ignore_ascii_case("unistore") {
            return Err(StoreError(format!(
                "Uri scheme expected(unistore) but found ({scheme})"
            )));
        }
        let mut opts: Vec<MockTiKVStoreOption> =
            vec![WithPath(path), WithStoreType(StoreType::EmbedUnistore)];
        let latches = astersql_config::get_global_config()
            .txn_local_latches
            .clone();
        if latches.enabled {
            opts.push(WithTxnLocalLatches(latches.capacity as usize));
        }
        NewMockStore(opts)
    }
}

/// 将 `scheme://path` 拆成 scheme 与路径；允许省略 `//`。
fn parse_uri(uri: &str) -> Result<(&str, String)> {
    let (scheme, remainder) = uri
        .split_once(':')
        .ok_or_else(|| StoreError(format!("invalid storage URI: {uri}")))?;
    let path = remainder.strip_prefix("//").unwrap_or(remainder).to_owned();
    Ok((scheme, path))
}

/// 检查 bootstrap 镜像目录及其 kv 子目录是否可读。
pub fn ImageAvailable() -> bool {
    Path::new(IMAGE_FILE_PATH).read_dir().is_ok()
        && Path::new(IMAGE_FILE_PATH).join("kv").read_dir().is_ok()
}

/// 从已 bootstrap 集群取出单 store / peer / region ID 三元组。
pub fn BootstrapWithSingleStore(cluster: &Cluster) -> Result<(u64, u64, u64)> {
    let manager = cluster.region_manager();
    let region = manager
        .scan_regions(&[], &[], 1)
        .into_iter()
        .next()
        .ok_or_else(|| StoreError("cluster is not bootstrapped".into()))?;
    let peer = region
        .leader
        .or_else(|| region.meta.peers.first().cloned())
        .ok_or_else(|| StoreError("region has no peer".into()))?;
    Ok((peer.store_id, peer.id, region.meta.id))
}

/// 在单 Region 上扩展到多个 store/peer，返回 stores、peers、region_id、首 peer。
pub fn BootstrapWithMultiStores(
    cluster: &Cluster,
    count: usize,
) -> Result<(Vec<u64>, Vec<u64>, u64, u64)> {
    let (first_store, first_peer, region_id) = BootstrapWithSingleStore(cluster)?;
    let manager = cluster.region_manager();
    let mut stores = vec![first_store];
    let mut peers = vec![first_peer];
    // 从第二个 store 起分配新 ID 并加入同一 Region。
    for index in 1..count {
        let store = manager.alloc_id();
        let peer = manager.alloc_id();
        manager.add_store(store, format!("store{store}"), Vec::new());
        manager
            .add_peer(region_id, store, peer)
            .map_err(|error| StoreError(error.to_string()))?;
        stores.push(store);
        peers.push(peer);
        let _ = index;
    }
    Ok((stores, peers.clone(), region_id, peers[0]))
}

/// 按 split_keys 依次分裂 Region，返回 store_id、region_ids、peer_ids。
pub fn BootstrapWithMultiRegions(
    cluster: &Cluster,
    split_keys: &[Vec<u8>],
) -> Result<(u64, Vec<u64>, Vec<u64>)> {
    let (store_id, peer_id, region_id) = BootstrapWithSingleStore(cluster)?;
    let manager = cluster.region_manager();
    let mut region_ids = vec![region_id];
    region_ids.extend(manager.alloc_ids(split_keys.len()));
    let mut peer_ids = vec![peer_id];
    peer_ids.extend(manager.alloc_ids(split_keys.len()));
    // Go 先批量分配 Region/Peer ID，再用当前位置的 peer 拆分当前位置的 Region。
    for (index, key) in split_keys.iter().enumerate() {
        cluster
            .split(
                region_ids[index],
                region_ids[index + 1],
                key,
                &[peer_ids[index]],
                peer_ids[index],
            )
            .map_err(|error| StoreError(error.to_string()))?;
    }
    Ok((store_id, region_ids, peer_ids))
}
