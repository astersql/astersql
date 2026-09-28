// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0

// InfoSyncer：集群信息同步核心。
//
// 维护全局单例，协调 etcd 拓扑、PD HTTP、标签/放置/调度/TiFlash/资源组
// 等子管理器，并对外提供 ServerInfo、Prometheus 地址、规则 Bundle、
// TiFlash 进度与内部会话等查询/更新 API。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, OnceLock, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::*;

/// etcd 上各 TiDB 节点最小 startTS（事务起始时间戳）路径前缀。
pub const ServerMinStartTSPath: &str = "/tidb/server/minstartts";
/// etcd 上全部 TiDB 节点 ServerInfo 的路径前缀。
pub const ServerInformationPath: &str = "/tidb/server/info";
/// TiFlash 表同步进度在 etcd 中的路径前缀。
pub const TiFlashTableSyncProgressPath: &str = "/tiflash/table/sync";
/// Prometheus 拓扑信息在 etcd 中的键。
pub const TopologyPrometheus: &str = "/topology/prometheus";
/// TiProxy 拓扑信息在 etcd 中的前缀。
pub const TopologyTiProxy: &str = "/topology/tiproxy";
/// 拓扑条目中表示「详细 info」的键后缀。
pub const infoSuffix: &str = "/info";
/// TiCDC 拓扑信息在 etcd 中的前缀。
pub const TopologyTiCDC: &str = "/topology/ticdc";
/// Prometheus 地址本地缓存过期时间。
pub const TablePrometheusCacheExpiry: Duration = Duration::from_secs(10);
/// 请求 PD 失败后的重试间隔。
pub const RequestRetryInterval: Duration = Duration::from_millis(200);
/// 请求 PD 的最大重试次数。
pub const RequestPDMaxRetry: usize = 3;

/// etcd 键值读写抽象，供 InfoSyncer 注入真实或 mock 客户端。
pub trait EtcdClient: Send + Sync {
    /// 读取单个键；不存在时返回 None。
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;
    /// 按前缀列出键值对。
    fn get_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>>;
    /// 写入键值。
    fn put(&self, key: &str, value: Vec<u8>) -> Result<()>;
    /// 删除键。
    fn delete(&self, key: &str) -> Result<()>;
}

/// infoschema 最近变更时间戳缓存，用于计算 min startTS。
pub trait infoschemaMinTS: Send + Sync {
    /// 读取并重置近期 infoschema 时间戳；`now` 为当前时间兜底。
    fn GetAndResetRecentInfoSchemaTS(&self, now: u64) -> u64;
}

/// 信息同步器：聚合各类集群元信息管理器与本地状态。
pub struct InfoSyncer {
    /// 本节点 UUID（通常即 ddl_id）。
    pub uuid: String,
    /// 带 keyspace 前缀的 etcd 客户端。
    pub etcdCli: RwLock<Option<Arc<dyn EtcdClient>>>,
    /// 不带 keyspace 前缀的 etcd 客户端（写 minStartTS 等全局键）。
    pub unprefixedEtcdCli: Option<Arc<dyn EtcdClient>>,
    /// PD HTTP 客户端；可为空（单元测试用 mock 管理器）。
    pub pdHTTPCli: RwLock<Option<Arc<dyn PdHttpClient>>>,
    /// 本节点报告的最小 startTS。
    pub minStartTS: RwLock<u64>,
    /// 本节点 minStartTS 在 etcd 中的完整路径。
    pub minStartTSPath: String,
    /// 会话管理器占位（类型擦除）。
    pub managerMu: RwLock<Option<Arc<dyn Send + Sync>>>,
    /// Prometheus 地址及其缓存时间戳。
    prometheusAddr: RwLock<(String, Option<Instant>)>,
    /// Region 标签规则管理器。
    pub labelRuleManager: Arc<dyn LabelRuleManager>,
    /// 放置策略 Bundle 管理器。
    pub placementManager: Arc<dyn PlacementManager>,
    /// PD 调度配置管理器。
    pub scheduleManager: Arc<dyn ScheduleManager>,
    /// TiFlash 副本/放置规则管理器。
    pub tiflashReplicaManager: Arc<dyn TiFlashReplicaManager>,
    /// mock TiFlash 管理器上下文（便于测试注入）。
    mockTiFlashManager: Arc<mockTiFlashReplicaManagerCtx>,
    /// 资源组管理客户端。
    pub resourceManagerClient: Arc<dyn ResourceManagerClient>,
    /// infoschema 最小时间戳缓存（可选）。
    pub infoCache: Option<Arc<dyn infoschemaMinTS>>,
    /// TiKV Codec / Keyspace 上下文。
    pub tikvCodec: Codec,
    /// 本节点 ServerInfo 快照。
    server_info: RwLock<ServerInfo>,
    /// 内部会话 ID 集合（用于特殊会话跟踪）。
    internal_sessions: RwLock<HashSet<usize>>,
}

/// 进程级全局 InfoSyncer 槽位。
static globalInfoSyncer: OnceLock<RwLock<Option<Arc<InfoSyncer>>>> = OnceLock::new();
/// 列存进度采集超时（测试可改）。
static columnarCollectTimeout: OnceLock<RwLock<Duration>> = OnceLock::new();
/// 列存进度采集器注入点（测试可替换）。
static columnarProgressCollector: OnceLock<RwLock<Option<Arc<dyn ColumnarProgressCollector>>>> =
    OnceLock::new();

/// 列存（columnar）同步进度采集器，可在超时后取消。
pub trait ColumnarProgressCollector: Send + Sync {
    /// 采集指定表在各 store 上的就绪进度；`cancelled` 为协作取消标志。
    fn collect(
        &self,
        cancelled: Arc<AtomicBool>,
        table_id: i64,
        stores: HashMap<i64, StoreInfo>,
    ) -> Result<f64>;
}

/// 测试用：临时替换列存进度采集器，返回恢复闭包。
pub fn SetColumnarProgressCollectorForTest(
    collector: Option<Arc<dyn ColumnarProgressCollector>>,
) -> impl FnOnce() {
    let slot = columnarProgressCollector.get_or_init(|| RwLock::new(None));
    let original = std::mem::replace(&mut *slot.write().unwrap(), collector);
    move || *slot.write().unwrap() = original
}

/// 测试用：临时设置列存采集超时，返回恢复闭包。
pub fn SetColumnarCollectTimeoutForTest(timeout: Duration) -> impl FnOnce() {
    let slot = columnarCollectTimeout.get_or_init(|| RwLock::new(Duration::from_secs(10)));
    let original = std::mem::replace(&mut *slot.write().unwrap(), timeout);
    move || *slot.write().unwrap() = original
}
/// 获取或初始化全局 InfoSyncer 槽位。
fn global_slot() -> &'static RwLock<Option<Arc<InfoSyncer>>> {
    globalInfoSyncer.get_or_init(|| RwLock::new(None))
}

/// 返回已初始化的全局 InfoSyncer；未初始化则报错。
pub fn getGlobalInfoSyncer() -> Result<Arc<InfoSyncer>> {
    global_slot()
        .read()
        .unwrap()
        .clone()
        .ok_or(Error::NotInitialized)
}
/// 设置（覆盖）全局 InfoSyncer。
pub fn setGlobalInfoSyncer(syncer: Arc<InfoSyncer>) {
    *global_slot().write().unwrap() = Some(syncer);
}

/// 测试用：临时替换 PD HTTP 客户端，返回恢复闭包。
pub fn SetPDHttpCliForTest(client: Arc<dyn PdHttpClient>) -> Result<impl FnOnce()> {
    let syncer = getGlobalInfoSyncer()?;
    let original = syncer.pdHTTPCli.write().unwrap().replace(client);
    Ok(move || *syncer.pdHTTPCli.write().unwrap() = original)
}

/// 构造并注册全局 InfoSyncer。
///
/// 无 PD 客户端时各子管理器回退到内存 mock；初始化后写入全局槽位，
/// 并登记到 MockGlobalServerInfoManager。
pub fn GlobalInfoSyncerInit(
    uuid: String,
    serverIDGetter: Arc<dyn Fn() -> u64 + Send + Sync>,
    etcdCli: Option<Arc<dyn EtcdClient>>,
    unprefixedEtcdCli: Option<Arc<dyn EtcdClient>>,
    pdHTTPCli: Option<Arc<dyn PdHttpClient>>,
    codec: Codec,
    _skipRegisterToDashboard: bool,
    infoCache: Option<Arc<dyn infoschemaMinTS>>,
) -> Result<Arc<InfoSyncer>> {
    // 有 PD 则走真实 HTTP 管理器，否则使用内存 mock。
    let label_manager: Arc<dyn LabelRuleManager> = match pdHTTPCli.clone() {
        Some(client) => Arc::new(PDLabelManager { pdHTTPCli: client }),
        None => Arc::new(mockLabelManager::default()),
    };
    let placement_manager: Arc<dyn PlacementManager> = match pdHTTPCli.clone() {
        Some(client) => Arc::new(PDPlacementManager { pdHTTPCli: client }),
        None => Arc::new(mockPlacementManager::default()),
    };
    let schedule_manager: Arc<dyn ScheduleManager> = match pdHTTPCli.clone() {
        Some(client) => Arc::new(PDScheduleManager { Client: client }),
        None => Arc::new(mockScheduleManager::default()),
    };
    let mock_tiflash_manager = Arc::new(mockTiFlashReplicaManagerCtx::default());
    let tiflash_manager: Arc<dyn TiFlashReplicaManager> = mock_tiflash_manager.clone();
    let resource_manager: Arc<dyn ResourceManagerClient> = Arc::from(NewMockResourceManagerClient(
        codec.keyspace_id.unwrap_or_default(),
    ));
    let server_info = ServerInfo {
        ID: uuid.clone(),
        JSONServerID: serverIDGetter(),
        Labels: astersql_config::get_global_config().labels.clone(),
        ..Default::default()
    };
    let syncer = Arc::new(InfoSyncer {
        uuid: uuid.clone(),
        etcdCli: RwLock::new(etcdCli),
        unprefixedEtcdCli,
        pdHTTPCli: RwLock::new(pdHTTPCli),
        minStartTS: RwLock::new(0),
        minStartTSPath: format!("{ServerMinStartTSPath}/{uuid}"),
        managerMu: RwLock::new(None),
        prometheusAddr: RwLock::new((String::new(), None)),
        labelRuleManager: label_manager,
        placementManager: placement_manager,
        scheduleManager: schedule_manager,
        tiflashReplicaManager: tiflash_manager,
        mockTiFlashManager: mock_tiflash_manager,
        resourceManagerClient: resource_manager,
        infoCache,
        tikvCodec: codec,
        server_info: RwLock::new(server_info),
        internal_sessions: RwLock::new(HashSet::new()),
    });
    syncer.init()?;
    setGlobalInfoSyncer(syncer.clone());
    MockGlobalServerInfoManagerEntry().Add(uuid, serverIDGetter);
    Ok(syncer)
}

impl InfoSyncer {
    /// 初始化钩子（当前无额外逻辑，占位对齐 Go）。
    pub fn init(&self) -> Result<()> {
        Ok(())
    }
    /// 注入会话管理器。
    pub fn SetSessionManager(&self, manager: Arc<dyn Send + Sync>) {
        *self.managerMu.write().unwrap() = Some(manager);
    }
    /// 取出当前会话管理器（若有）。
    pub fn GetSessionManager(&self) -> Option<Arc<dyn Send + Sync>> {
        self.managerMu.read().unwrap().clone()
    }
    /// 初始化标签规则管理器（当前为空实现占位）。
    pub fn initLabelRuleManager(&self) {}
    /// 初始化放置管理器（当前为空实现占位）。
    pub fn initPlacementManager(&self) {}
    /// 初始化资源管理客户端（当前为空实现占位）。
    pub fn initResourceManagerClient(&self) {}
    /// 初始化 TiFlash 副本管理器（当前为空实现占位）。
    pub fn initTiFlashReplicaManager(&self) {}
    /// 初始化调度管理器（当前为空实现占位）。
    pub fn initScheduleManager(&self) {}
    /// 读取本节点当前最小 startTS。
    pub fn GetMinStartTS(&self) -> u64 {
        *self.minStartTS.read().unwrap()
    }
    /// 获取用于写 minStartTS 的 etcd 客户端（无前缀）。
    pub fn getEtcdClientForMinStartTS(&self) -> Option<Arc<dyn EtcdClient>> {
        self.unprefixedEtcdCli.clone()
    }
    /// 将当前 minStartTS 持久化到 etcd。
    pub fn storeMinStartTS(&self) -> Result<()> {
        let Some(client) = self.getEtcdClientForMinStartTS() else {
            return Ok(());
        };
        client.put(
            &self.minStartTSPath,
            self.GetMinStartTS().to_string().into_bytes(),
        )
    }
    /// 从 etcd 删除本节点的 minStartTS 键。
    pub fn RemoveMinStartTS(&self) -> Result<()> {
        match self.getEtcdClientForMinStartTS() {
            Some(client) => client.delete(&self.minStartTSPath),
            None => Ok(()),
        }
    }
    /// 根据 store 与 infoschema 时间戳更新并上报 minStartTS。
    pub fn ReportMinStartTS(&self, store_min_ts: u64, now: u64) -> Result<()> {
        let schema_ts = self
            .infoCache
            .as_ref()
            .map(|cache| cache.GetAndResetRecentInfoSchemaTS(now))
            .unwrap_or(now);
        // 取 store 与 schema 时间戳较小者，保证安全的 GC 下界。
        *self.minStartTS.write().unwrap() = store_min_ts.min(schema_ts);
        self.storeMinStartTS()
    }
    /// 克隆本节点 ServerInfo。
    pub fn ServerInfoSyncer(&self) -> ServerInfo {
        self.server_info.read().unwrap().clone()
    }
    /// 获取 Prometheus 地址（带本地 TTL 缓存）。
    pub fn getPrometheusAddr(&self) -> Result<String> {
        let cached = self.prometheusAddr.read().unwrap();
        if cached
            .1
            .is_some_and(|modified| modified.elapsed() < TablePrometheusCacheExpiry)
        {
            return Ok(cached.0.clone());
        }
        drop(cached);
        let client = self.etcdCli.read().unwrap().clone();
        let address = client
            .as_ref()
            .and_then(|client| self.getPrometheusAddrFromEtcd(client).ok())
            .flatten()
            .ok_or(Error::PrometheusAddressNotSet)?;
        *self.prometheusAddr.write().unwrap() = (address.clone(), Some(Instant::now()));
        Ok(address)
    }
    /// 从 etcd 拓扑键解析 Prometheus `ip:port`。
    fn getPrometheusAddrFromEtcd(&self, client: &Arc<dyn EtcdClient>) -> Result<Option<String>> {
        let Some(value) = client.get(TopologyPrometheus)? else {
            return Ok(None);
        };
        #[derive(Deserialize)]
        struct Prometheus {
            ip: String,
            port: u16,
        }
        let p: Prometheus = serde_json::from_slice(&value)?;
        Ok(Some(format!("http://{}:{}", p.ip, p.port)))
    }
    /// 从 etcd 读取全部 TiProxy 拓扑 info。
    pub fn getTiProxyServerInfo(&self) -> Result<HashMap<String, TiProxyServerInfo>> {
        let client = self.etcdCli.read().unwrap().clone();
        let Some(client) = client else {
            return Ok(HashMap::new());
        };
        let mut result = HashMap::new();
        for (key, value) in client.get_prefix(TopologyTiProxy)? {
            if !key.ends_with(infoSuffix) {
                continue;
            }
            let Some(address) = key
                .strip_prefix(TopologyTiProxy)
                .and_then(|key| key.strip_prefix('/'))
                .and_then(|key| key.strip_suffix(infoSuffix))
            else {
                continue;
            };
            result.insert(address.to_owned(), serde_json::from_slice(&value)?);
        }
        Ok(result)
    }
    /// 从 etcd 读取全部 TiCDC 拓扑 info。
    pub fn getTiCDCServerInfo(&self) -> Result<Vec<TiCDCInfo>> {
        let client = self.etcdCli.read().unwrap().clone();
        let Some(client) = client else {
            return Ok(Vec::new());
        };
        let mut result = Vec::new();
        for (key, value) in client.get_prefix(TopologyTiCDC)? {
            let key_parts: Vec<_> = key.split('/').collect();
            if key_parts.len() < 3 {
                continue;
            }
            let mut info: TiCDCInfo = serde_json::from_slice(&value)?;
            info.Version = info
                .Version
                .strip_prefix('v')
                .unwrap_or(&info.Version)
                .to_owned();
            info.ClusterID = key_parts[1].to_owned();
            result.push(info);
        }
        Ok(result)
    }
}

/// 获取当前注入的 MockTiFlash（若有）。
pub fn GetMockTiFlash() -> Result<Option<Arc<MockTiFlash>>> {
    Ok(getGlobalInfoSyncer()?.mockTiFlashManager.GetMockTiFlash())
}
/// 注入 MockTiFlash 实例。
pub fn SetMockTiFlash(tiflash: Arc<MockTiFlash>) -> Result<()> {
    let syncer = getGlobalInfoSyncer()?;
    syncer.mockTiFlashManager.SetMockTiFlash(tiflash);
    Ok(())
}
/// 获取本节点 ServerInfo。
pub fn GetServerInfo() -> Result<ServerInfo> {
    Ok(getGlobalInfoSyncer()?.ServerInfoSyncer())
}
/// 按 ID 从 mock 全局表查找 ServerInfo。
pub fn GetServerInfoByID(id: &str) -> Result<ServerInfo> {
    MockGlobalServerInfoManagerEntry()
        .GetAllServerInfo()
        .remove(id)
        .ok_or_else(|| Error::External(format!("server {id} not found")))
}
/// 通过 PD 更新 Keyspace（键空间）配置。
pub fn SetKeyspaceConfig(keyspaceName: &str, config: UpdateKeyspaceConfigParams) -> Result<()> {
    getGlobalInfoSyncer()?
        .pdHTTPCli
        .read()
        .unwrap()
        .as_ref()
        .ok_or(Error::PdHttpClientMissing)?
        .update_keyspace_config(keyspaceName, &config)
}
/// 获取全部 ServerInfo：有 etcd 时读取集群注册表，否则返回本节点信息。
pub fn GetAllServerInfo() -> Result<HashMap<String, ServerInfo>> {
    let syncer = getGlobalInfoSyncer()?;
    let client = syncer.etcdCli.read().unwrap().clone();
    let Some(client) = client else {
        let info = syncer.server_info.read().unwrap().clone();
        return Ok(HashMap::from([(info.ID.clone(), info)]));
    };

    let mut server_infos = HashMap::new();
    for (_, value) in client.get_prefix(ServerInformationPath)? {
        let info: ServerInfo = serde_json::from_slice(&value)?;
        server_infos.insert(info.ID.clone(), info);
    }
    Ok(server_infos)
}
/// 更新本节点 labels。
pub fn UpdateServerLabel(labels: HashMap<String, String>) -> Result<()> {
    getGlobalInfoSyncer()?.server_info.write().unwrap().Labels = labels;
    Ok(())
}
/// 从缓存删除指定表的 TiFlash 同步进度。
pub fn DeleteTiFlashTableSyncProgress(tableInfo: &model::TableInfo) -> Result<()> {
    let syncer = getGlobalInfoSyncer()?;
    if let Some(partition) = tableInfo.GetPartitionInfo() {
        for definition in &partition.Definitions {
            syncer
                .tiflashReplicaManager
                .DeleteTiFlashProgressFromCache(definition.ID);
        }
    } else {
        syncer
            .tiflashReplicaManager
            .DeleteTiFlashProgressFromCache(tableInfo.ID);
    }
    Ok(())
}

/// 带熔断的 TiFlash/列存进度查询：采集超时则视为进度 1.0 并标记触发熔断。
pub fn MustGetTiFlashProgressWithCircuitBreaker(
    tableID: i64,
    replicaCount: u64,
    tiFlashStores: &HashMap<i64, StoreInfo>,
    tikvStores: &HashMap<i64, StoreInfo>,
) -> Result<(f64, bool)> {
    if let Some(collector) = columnarProgressCollector
        .get_or_init(|| RwLock::new(None))
        .read()
        .unwrap()
        .clone()
    {
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancellation = cancelled.clone();
        let stores = tikvStores.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        // 后台线程执行采集，主线程限时等待以实现熔断。
        thread::spawn(move || {
            let _ = sender.send(collector.collect(cancellation, tableID, stores));
        });
        let timeout = *columnarCollectTimeout
            .get_or_init(|| RwLock::new(Duration::from_secs(10)))
            .read()
            .unwrap();
        match receiver.recv_timeout(timeout) {
            Ok(result) => return Ok((result?, false)),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                cancelled.store(true, Ordering::Release);
                return Ok((1.0, true));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(Error::External(
                    "columnar progress collector stopped".into(),
                ));
            }
        }
    }
    Ok((
        MustGetTiFlashProgress(tableID, replicaCount, tiFlashStores, tikvStores)?,
        false,
    ))
}
/// 计算 TiFlash 进度；若无 TiFlash 进度则回落到列存进度。
pub fn MustGetTiFlashProgress(
    tableID: i64,
    replicaCount: u64,
    tiFlashStores: &HashMap<i64, StoreInfo>,
    tikvStores: &HashMap<i64, StoreInfo>,
) -> Result<f64> {
    let syncer = getGlobalInfoSyncer()?;
    if let Some(progress) = syncer
        .tiflashReplicaManager
        .GetTiFlashProgressFromCache(tableID)
    {
        return Ok(progress);
    }

    let mut tiflash_progress = 1.0;
    let mut columnar_progress = 1.0;
    if !tiFlashStores.is_empty() {
        tiflash_progress = syncer
            .tiflashReplicaManager
            .CalculateTiFlashProgress(tableID, replicaCount, tiFlashStores)?
            .0;
    }
    if !tikvStores.is_empty() {
        columnar_progress = CalculateColumnarProgress(tableID, tikvStores)?;
    }
    let progress = tiflash_progress.min(columnar_progress);
    syncer
        .tiflashReplicaManager
        .UpdateTiFlashProgressCache(tableID, progress);
    Ok(progress)
}
/// 将 PD HTTP 状态码映射为 Result；404/412 与 Go 一样视为兼容性成功。
pub fn pdResponseHandler(status: u16, body: &[u8]) -> Result<()> {
    match status {
        200 | 404 | 412 => Ok(()),
        _ => Err(Error::DomainService(
            String::from_utf8_lossy(body).into_owned(),
        )),
    }
}

/// 获取全部放置规则 Bundle。
pub fn GetAllRuleBundles() -> Result<Vec<placement::Bundle>> {
    getGlobalInfoSyncer()?.placementManager.GetAllRuleBundles()
}
/// 按名称获取放置规则 Bundle。
pub fn GetRuleBundle(name: &str) -> Result<placement::Bundle> {
    getGlobalInfoSyncer()?.placementManager.GetRuleBundle(name)
}
/// 批量写入放置规则 Bundle。
pub fn PutRuleBundles(bundles: &[placement::Bundle]) -> Result<()> {
    getGlobalInfoSyncer()?
        .placementManager
        .PutRuleBundles(bundles)
}
/// 带重试地写入 Bundle；`DomainService` 错误立即失败不重试。
pub fn PutRuleBundlesWithRetry(
    bundles: &[placement::Bundle],
    maxRetry: usize,
    interval: Duration,
) -> Result<()> {
    let mut last = None;
    for attempt in 0..=maxRetry {
        match PutRuleBundles(bundles) {
            Ok(()) => return Ok(()),
            // 领域服务错误视为不可重试。
            Err(error @ Error::DomainService(_)) => return Err(error),
            Err(error) => last = Some(error),
        }
        if attempt < maxRetry {
            thread::sleep(interval);
        }
    }
    Err(last.unwrap_or_else(|| Error::External("put rule bundles failed".into())))
}
/// 使用默认重试次数与间隔写入 Bundle。
pub fn PutRuleBundlesWithDefaultRetry(bundles: &[placement::Bundle]) -> Result<()> {
    PutRuleBundlesWithRetry(bundles, RequestPDMaxRetry, RequestRetryInterval)
}

/// 按名称查询资源组。
pub fn GetResourceGroup(name: &str) -> Result<ResourceGroup> {
    getGlobalInfoSyncer()?
        .resourceManagerClient
        .get_resource_group(name)
}
/// 列出全部资源组。
pub fn ListResourceGroups() -> Result<Vec<ResourceGroup>> {
    Ok(getGlobalInfoSyncer()?
        .resourceManagerClient
        .list_resource_groups())
}
/// 新增资源组。
pub fn AddResourceGroup(group: ResourceGroup) -> Result<()> {
    getGlobalInfoSyncer()?
        .resourceManagerClient
        .add_resource_group(group)
        .map(drop)
}
/// 修改资源组。
pub fn ModifyResourceGroup(group: ResourceGroup) -> Result<()> {
    getGlobalInfoSyncer()?
        .resourceManagerClient
        .modify_resource_group(group)
        .map(drop)
}
/// 删除资源组。
pub fn DeleteResourceGroup(name: &str) -> Result<()> {
    getGlobalInfoSyncer()?
        .resourceManagerClient
        .delete_resource_group(name)
        .map(drop)
}

/// 获取 Prometheus 地址（委托全局 InfoSyncer）。
pub fn GetPrometheusAddr() -> Result<String> {
    getGlobalInfoSyncer()?.getPrometheusAddr()
}
/// 写入单条 Region 标签规则。
pub fn PutLabelRule(rule: Option<&label::Rule>) -> Result<()> {
    if rule.is_none() {
        return Ok(());
    }
    getGlobalInfoSyncer()?.labelRuleManager.PutLabelRule(rule)
}
/// 批量 patch 标签规则。
pub fn UpdateLabelRules(patch: Option<&LabelRulePatch>) -> Result<()> {
    if patch.is_none_or(|patch| patch.DeleteRules.is_empty() && patch.SetRules.is_empty()) {
        return Ok(());
    }
    getGlobalInfoSyncer()?
        .labelRuleManager
        .UpdateLabelRules(patch)
}
/// 获取全部标签规则（按本节点 Codec 过滤）。
pub fn GetAllLabelRules() -> Result<Vec<label::Rule>> {
    let is = getGlobalInfoSyncer()?;
    is.labelRuleManager.GetAllLabelRules(is.tikvCodec)
}
/// 按 ID 列表获取标签规则。
pub fn GetLabelRules(ruleIDs: &[String]) -> Result<HashMap<String, label::Rule>> {
    if ruleIDs.is_empty() {
        return Ok(HashMap::new());
    }
    getGlobalInfoSyncer()?
        .labelRuleManager
        .GetLabelRules(ruleIDs)
}
/// 触发 TiFlash 同步指定表 schema。
pub fn SyncTiFlashTableSchema(tableID: i64) -> Result<()> {
    let syncer = getGlobalInfoSyncer()?;
    let stores = syncer.tiflashReplicaManager.GetStoresStat()?.Stores;
    let (tiflash_stores, _) = partitionTiFlashProgressStores(stores);
    let tiflash_stores: Vec<_> = tiflash_stores.into_values().collect();
    syncer
        .tiflashReplicaManager
        .SyncTiFlashTableSchema(tableID, &tiflash_stores)
}
/// 计算 TiFlash 副本同步进度。
pub fn CalculateTiFlashProgress(
    tableID: i64,
    replicaCount: u64,
    tiFlashStores: &HashMap<i64, StoreInfo>,
) -> Result<(f64, f64)> {
    getGlobalInfoSyncer()?
        .tiflashReplicaManager
        .CalculateTiFlashProgress(tableID, replicaCount, tiFlashStores)
}
/// 带上下文的 TiFlash 进度计算（当前直接委托）。
pub fn calculateTiFlashProgressWithCtx(
    tableID: i64,
    replicaCount: u64,
    stores: &HashMap<i64, StoreInfo>,
) -> Result<(f64, f64)> {
    CalculateTiFlashProgress(tableID, replicaCount, stores)
}
/// 计算列存同步进度。
pub fn CalculateColumnarProgress(
    tableID: i64,
    tikvStores: &HashMap<i64, StoreInfo>,
) -> Result<f64> {
    calculateColumnarProgressWithCtx(tableID, tikvStores)
}
/// 按 store label `table-<id>-ready=true` 比例估算列存进度。
pub fn calculateColumnarProgressWithCtx(
    tableID: i64,
    stores: &HashMap<i64, StoreInfo>,
) -> Result<f64> {
    if stores.is_empty() {
        return Ok(0.0);
    }
    let ready = stores
        .values()
        .filter(|store| {
            store
                .Store
                .Labels
                .get(&format!("table-{tableID}-ready"))
                .is_some_and(|v| v == "true")
        })
        .count();
    Ok(ready as f64 / stores.len() as f64)
}
/// 更新 TiFlash 进度缓存。
pub fn UpdateTiFlashProgressCache(tableID: i64, progress: f64) -> Result<()> {
    getGlobalInfoSyncer()?
        .tiflashReplicaManager
        .UpdateTiFlashProgressCache(tableID, progress);
    Ok(())
}
/// 从缓存读取 TiFlash 进度。
pub fn GetTiFlashProgressFromCache(tableID: i64) -> Result<Option<f64>> {
    Ok(getGlobalInfoSyncer()?
        .tiflashReplicaManager
        .GetTiFlashProgressFromCache(tableID))
}
/// 清空 TiFlash 进度缓存。
pub fn CleanTiFlashProgressCache() -> Result<()> {
    getGlobalInfoSyncer()?
        .tiflashReplicaManager
        .CleanTiFlashProgressCache();
    Ok(())
}
/// 按索引就绪 label 比例估算列存索引进度。
pub fn CalculateColumnarIndexProgress(
    tableID: i64,
    indexID: i64,
    stores: &HashMap<i64, StoreInfo>,
) -> Result<f64> {
    if stores.is_empty() {
        return Ok(0.0);
    }
    let key = format!("table-{tableID}-index-{indexID}-ready");
    Ok(stores
        .values()
        .filter(|s| s.Store.Labels.get(&key).is_some_and(|v| v == "true"))
        .count() as f64
        / stores.len() as f64)
}
/// 设置 TiFlash 规则组配置。
pub fn SetTiFlashGroupConfig() -> Result<()> {
    getGlobalInfoSyncer()?
        .tiflashReplicaManager
        .SetTiFlashGroupConfig()
}
/// 写入单条 TiFlash 放置规则。
pub fn SetTiFlashPlacementRule(rule: &TiFlashRule) -> Result<()> {
    getGlobalInfoSyncer()?
        .tiflashReplicaManager
        .SetPlacementRule(rule)
}
/// 按物理表 ID 批量删除 TiFlash 放置规则。
pub fn DeleteTiFlashPlacementRules(physicalTableIDs: &[i64]) -> Result<()> {
    let is = getGlobalInfoSyncer()?;
    let rules: Vec<_> = physicalTableIDs
        .iter()
        .map(|id| MakeNewRule(*id, 0, Vec::new()))
        .collect();
    is.tiflashReplicaManager.SetPlacementRuleBatch(&rules)
}
/// 获取指定规则组下的 TiFlash 规则列表。
pub fn GetTiFlashGroupRules(group: &str) -> Result<Vec<TiFlashRule>> {
    getGlobalInfoSyncer()?
        .tiflashReplicaManager
        .GetGroupRules(group)
}
/// 从 PD 查询表对应的 Region 数量。
pub fn GetTiFlashRegionCountFromPD(tableID: i64) -> Result<usize> {
    getGlobalInfoSyncer()?
        .tiflashReplicaManager
        .GetRegionCountFromPD(tableID)
}
/// 获取表的 TiFlash 放置规则。
pub fn GetPlacementRule(tableID: i64) -> Result<TiFlashRule> {
    getGlobalInfoSyncer()?
        .tiflashReplicaManager
        .GetPlacementRule(tableID)
}
/// 获取 TiFlash 相关 store 统计。
pub fn GetTiFlashStoresStat() -> Result<StoresInfo> {
    getGlobalInfoSyncer()?.tiflashReplicaManager.GetStoresStat()
}
/// 将 store 列表拆分为 TiFlash 与 TiKV 两组。
pub fn GetTiFlashProgressStores() -> Result<(HashMap<i64, StoreInfo>, HashMap<i64, StoreInfo>)> {
    let stores = GetTiFlashStoresStat()?.Stores;
    Ok(partitionTiFlashProgressStores(stores))
}

/// 按 Go `engine.IsTiFlashHTTPResp` / `IsTiFlashWriteHTTPResp` 语义拆分 store。
/// NextGen compute 节点不存 Region，必须从进度计算的两组中都排除。
pub(crate) fn partitionTiFlashProgressStores(
    stores: Vec<StoreInfo>,
) -> (HashMap<i64, StoreInfo>, HashMap<i64, StoreInfo>) {
    let mut tiflash = HashMap::new();
    let mut tikv = HashMap::new();
    for store in stores {
        match store.Store.Labels.get("engine").map(String::as_str) {
            Some("tiflash") => {
                tiflash.insert(store.Store.ID, store);
            }
            Some("tiflash_compute") => {}
            _ => {
                tikv.insert(store.Store.ID, store);
            }
        }
    }
    (tiflash, tikv)
}
/// 关闭 TiFlash 管理器资源。
pub fn CloseTiFlashManager() -> Result<()> {
    getGlobalInfoSyncer()?.tiflashReplicaManager.Close();
    Ok(())
}
/// 为整表配置 TiFlash PD 放置规则。
pub fn ConfigureTiFlashPDForTable(id: i64, count: u64, locationLabels: &[String]) -> Result<()> {
    SetTiFlashPlacementRule(&MakeNewRule(id, count, locationLabels.to_vec()))
}
/// 为分区批量配置 TiFlash 放置规则，可选加速调度，并删除表级规则。
pub fn ConfigureTiFlashPDForPartitions(
    accel: bool,
    definitions: &[model::PartitionDefinition],
    count: u64,
    locationLabels: &[String],
    tableID: i64,
) -> Result<()> {
    let ids: Vec<_> = definitions.iter().map(|definition| definition.ID).collect();
    let rules: Vec<_> = definitions
        .iter()
        .map(|definition| MakeNewRule(definition.ID, count, locationLabels.to_vec()))
        .collect();
    let is = getGlobalInfoSyncer()?;
    is.tiflashReplicaManager.SetPlacementRuleBatch(&rules)?;
    if accel {
        is.tiflashReplicaManager.PostAccelerateScheduleBatch(&ids)?;
    }
    let _ = tableID;
    Ok(())
}

/// 登记内部会话；返回是否为新插入。
pub fn StoreInternalSession(se: usize) -> Result<bool> {
    let is = getGlobalInfoSyncer()?;
    Ok(is.internal_sessions.write().unwrap().insert(se))
}
/// 删除内部会话登记。
pub fn DeleteInternalSession(se: usize) -> Result<()> {
    getGlobalInfoSyncer()?
        .internal_sessions
        .write()
        .unwrap()
        .remove(&se);
    Ok(())
}
/// 查询是否包含指定内部会话。
pub fn ContainsInternalSession(se: usize) -> Result<bool> {
    Ok(getGlobalInfoSyncer()?
        .internal_sessions
        .read()
        .unwrap()
        .contains(&se))
}
/// 替换 etcd 客户端；仅供测试在初始化完成后调用。
pub fn SetEtcdClient(etcdCli: Option<Arc<dyn EtcdClient>>) -> Result<()> {
    *getGlobalInfoSyncer()?.etcdCli.write().unwrap() = etcdCli;
    Ok(())
}
/// 获取当前 etcd 客户端。
pub fn GetEtcdClient() -> Result<Option<Arc<dyn EtcdClient>>> {
    Ok(getGlobalInfoSyncer()?.etcdCli.read().unwrap().clone())
}
/// 读取 PD 调度配置。
pub fn GetPDScheduleConfig() -> Result<HashMap<String, ConfigValue>> {
    getGlobalInfoSyncer()?.scheduleManager.GetScheduleConfig()
}
/// 写入 PD 调度配置。
pub fn SetPDScheduleConfig(config: &HashMap<String, ConfigValue>) -> Result<()> {
    getGlobalInfoSyncer()?
        .scheduleManager
        .SetScheduleConfig(config)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
/// TiProxy 节点拓扑信息。
pub struct TiProxyServerInfo {
    /// 版本号。
    #[serde(rename = "version")]
    pub Version: String,
    /// Git commit hash。
    #[serde(rename = "git_hash")]
    pub GitHash: String,
    /// IP 地址。
    #[serde(rename = "ip")]
    pub IP: String,
    /// 服务端口。
    #[serde(rename = "port")]
    pub Port: String,
    /// 状态端口。
    #[serde(rename = "status_port")]
    pub StatusPort: String,
    /// 启动时间戳。
    #[serde(rename = "start_timestamp")]
    pub StartTimestamp: i64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
/// TiCDC 节点拓扑信息。
pub struct TiCDCInfo {
    /// 节点 ID。
    #[serde(rename = "id")]
    pub ID: String,
    /// 访问地址。
    #[serde(rename = "address")]
    pub Address: String,
    /// 版本号。
    #[serde(rename = "version")]
    pub Version: String,
    /// Git commit hash。
    #[serde(rename = "git-hash")]
    pub GitHash: String,
    /// 部署路径。
    #[serde(rename = "deploy-path")]
    pub DeployPath: String,
    /// 启动时间戳。
    #[serde(rename = "start-timestamp")]
    pub StartTimestamp: i64,
    /// TiCDC 集群 ID（由 etcd 键路径补齐）。
    #[serde(default, rename = "cluster-id")]
    pub ClusterID: String,
}
/// 获取全部 TiProxy 拓扑信息。
pub fn GetTiProxyServerInfo() -> Result<HashMap<String, TiProxyServerInfo>> {
    getGlobalInfoSyncer()?.getTiProxyServerInfo()
}
/// 获取全部 TiCDC 拓扑信息。
pub fn GetTiCDCServerInfo() -> Result<Vec<TiCDCInfo>> {
    getGlobalInfoSyncer()?.getTiCDCServerInfo()
}
