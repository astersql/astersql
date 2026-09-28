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

// ServerInfo Syncer：将本节点信息同步到 etcd，并维护拓扑与最小 TS 上报。
//
// 对齐 Go `infosync` 中与 server info / topology 相关的逻辑：创建 etcd session、
// 写入 `/tidb/server/info`、清理同 IP:Port 的陈旧登记与 DDL owner 键、
// 刷新 `/topology/tidb` 的 info/ttl，以及周期上报 min start TS。

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::info::*;

#[derive(Clone)]
/// 简化版取消/超时上下文，对齐 Go `context.Context` 的 Done 语义。
pub struct Context {
    cancelled: Arc<AtomicBool>,
    deadline: Option<Instant>,
}

impl Context {
    /// 无取消、无截止时间的后台上下文。
    pub fn Background() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline: None,
        }
    }
    /// 派生带超时的上下文（共享取消标志）。
    pub fn WithTimeout(&self, timeout: Duration) -> Self {
        Self {
            cancelled: self.cancelled.clone(),
            deadline: Some(Instant::now() + timeout),
        }
    }
    /// 标记已取消。
    pub fn Cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
    /// 是否已取消或超过截止时间。
    pub fn Done(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
            || self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Syncer / etcd 操作错误。
pub struct SyncError(pub String);
impl fmt::Display for SyncError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for SyncError {}
impl From<ServerInfoError> for SyncError {
    fn from(error: ServerInfoError) -> Self {
        Self(error.to_string())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// etcd 键值项（可选 lease）。
pub struct KeyValue {
    /// 键。
    pub key: String,
    /// 值字节。
    pub value: Vec<u8>,
    /// 关联租约 ID（若有）。
    pub lease: Option<i64>,
}

/// Syncer 依赖的 etcd 客户端抽象（Get/Put/Delete）。
pub trait EtcdClient: Send + Sync {
    /// 按精确键或前缀读取。
    fn Get(&self, context: &Context, key: &str, prefix: bool) -> Result<Vec<KeyValue>, SyncError>;
    /// 写入键值，可选绑定 lease。
    fn Put(
        &self,
        context: &Context,
        key: &str,
        value: Vec<u8>,
        lease: Option<i64>,
    ) -> Result<(), SyncError>;
    /// 删除单个键。
    fn Delete(&self, context: &Context, key: &str) -> Result<(), SyncError>;
    /// 删除前缀下全部键。
    fn DeletePrefix(&self, context: &Context, prefix: &str) -> Result<(), SyncError>;
}

#[derive(Default)]
/// 内存版 etcd，供单测注入瞬时 Get 失败。
pub struct MemoryEtcdClient {
    values: Mutex<BTreeMap<String, KeyValue>>,
    transient_get_failures: AtomicUsize,
}

impl MemoryEtcdClient {
    /// 让随后若干次 Get 返回瞬时失败。
    pub fn FailNextGets(&self, count: usize) {
        self.transient_get_failures.store(count, Ordering::SeqCst);
    }
    /// 导出当前全部键值快照。
    pub fn Snapshot(&self) -> BTreeMap<String, KeyValue> {
        self.values.lock().expect("etcd lock poisoned").clone()
    }
}

impl EtcdClient for MemoryEtcdClient {
    fn Get(&self, context: &Context, key: &str, prefix: bool) -> Result<Vec<KeyValue>, SyncError> {
        if context.Done() {
            return Err(SyncError("context cancelled".into()));
        }
        // 消耗一次瞬时失败计数。
        if self
            .transient_get_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                count.checked_sub(1)
            })
            .is_ok()
        {
            return Err(SyncError("transient etcd get failure".into()));
        }
        let values = self.values.lock().expect("etcd lock poisoned");
        Ok(if prefix {
            values
                .range(key.to_owned()..)
                .take_while(|(candidate, _)| candidate.starts_with(key))
                .map(|(_, value)| value.clone())
                .collect()
        } else {
            values.get(key).cloned().into_iter().collect()
        })
    }
    fn Put(
        &self,
        context: &Context,
        key: &str,
        value: Vec<u8>,
        lease: Option<i64>,
    ) -> Result<(), SyncError> {
        if context.Done() {
            return Err(SyncError("context cancelled".into()));
        }
        self.values.lock().expect("etcd lock poisoned").insert(
            key.into(),
            KeyValue {
                key: key.into(),
                value,
                lease,
            },
        );
        Ok(())
    }
    fn Delete(&self, context: &Context, key: &str) -> Result<(), SyncError> {
        if context.Done() {
            return Err(SyncError("context cancelled".into()));
        }
        self.values.lock().expect("etcd lock poisoned").remove(key);
        Ok(())
    }
    fn DeletePrefix(&self, context: &Context, prefix: &str) -> Result<(), SyncError> {
        if context.Done() {
            return Err(SyncError("context cancelled".into()));
        }
        self.values
            .lock()
            .expect("etcd lock poisoned")
            .retain(|key, _| !key.starts_with(prefix));
        Ok(())
    }
}

static NEXT_LEASE_ID: AtomicI64 = AtomicI64::new(1);

#[derive(Clone)]
/// 简化的 etcd session：持有 lease_id 与 Done 标志。
pub struct Session {
    lease_id: i64,
    ttl: i32,
    done: Arc<AtomicBool>,
}

impl Session {
    /// 分配新的 lease_id 并记录 TTL。
    pub fn New(ttl: i32) -> Self {
        Self {
            lease_id: NEXT_LEASE_ID.fetch_add(1, Ordering::SeqCst),
            ttl,
            done: Arc::new(AtomicBool::new(false)),
        }
    }
    /// 返回 lease ID。
    pub fn Lease(&self) -> i64 {
        self.lease_id
    }
    /// 返回会话 TTL。
    pub fn TTL(&self) -> i32 {
        self.ttl
    }
    /// 会话是否已关闭。
    pub fn Done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }
    /// 标记会话结束。
    pub fn Close(&self) {
        self.done.store(true, Ordering::SeqCst);
    }
}

/// 存储占位 trait（min TS 上报回调参数）。
pub trait Storage: Send + Sync {}
impl<T: Send + Sync> Storage for T {}

/// 向集群上报本节点观察到的最小事务开始时间戳（min start TS）。
pub trait MinStartTSReporter: Send + Sync {
    /// 执行一次 min start TS 上报。
    fn ReportMinStartTS(&self, store: &dyn Storage, session: &Session);
}

#[derive(Default)]
/// 空实现的 min start TS 上报器。
pub struct NoopMinStartTSReporter;
impl MinStartTSReporter for NoopMinStartTSReporter {
    fn ReportMinStartTS(&self, _store: &dyn Storage, _session: &Session) {}
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 构造 ServerInfo 所用的全局服务器配置快照。
pub struct ServerConfig {
    /// 对外宣告地址。
    pub AdvertiseAddress: String,
    /// SQL 端口。
    pub Port: u32,
    /// status 端口。
    pub StatusPort: u32,
    /// 租约描述。
    pub Lease: String,
    /// 所属 keyspace。
    pub Keyspace: String,
    /// 初始标签。
    pub Labels: HashMap<String, String>,
    /// Git 哈希。
    pub GitHash: String,
    /// 测试开关：注入固定 StartTimestamp 与标签。
    pub MockServerInfo: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            AdvertiseAddress: "127.0.0.1".into(),
            Port: 4000,
            StatusPort: 10080,
            Lease: "45s".into(),
            Keyspace: String::new(),
            Labels: HashMap::new(),
            GitHash: "None".into(),
            MockServerInfo: false,
        }
    }
}

static GLOBAL_CONFIG: OnceLock<RwLock<ServerConfig>> = OnceLock::new();
/// 设置进程级全局 ServerConfig。
pub fn SetGlobalServerConfig(config: ServerConfig) {
    *GLOBAL_CONFIG
        .get_or_init(|| RwLock::new(ServerConfig::default()))
        .write()
        .expect("config lock poisoned") = config;
}
/// 读取进程级全局 ServerConfig 副本。
pub fn GetGlobalServerConfig() -> ServerConfig {
    GLOBAL_CONFIG
        .get_or_init(|| RwLock::new(ServerConfig::default()))
        .read()
        .expect("config lock poisoned")
        .clone()
}

/// 将本节点 ServerInfo / 拓扑同步到 etcd，并驱动上报循环。
pub struct Syncer {
    /// etcd 客户端；None 时多数写路径直接成功返回。
    pub etcdCli: Option<Arc<dyn EtcdClient>>,
    /// min start TS 上报器。
    pub reporter: Arc<dyn MinStartTSReporter>,
    /// 本地 ServerInfo 缓存。
    pub info: Arc<RwLock<ServerInfo>>,
    /// 本节点在 etcd 上的 ServerInfo 完整路径。
    pub serverInfoPath: String,
    /// ServerInfo 写入所用 session。
    pub session: Option<Session>,
    /// 拓扑 ttl 写入所用 session。
    pub topologySession: Option<Session>,
}

/// 由节点 ID 拼出 ServerInfo etcd 键。
pub fn serverInfoKeyPath(id: &str) -> String {
    format!("{ServerInformationPath}/{id}")
}

/// 创建本 keyspace 的 Syncer（AssumedKeyspace 为空）。
pub fn NewSyncer(
    uuid: String,
    server_id_getter: Arc<dyn Fn() -> u64 + Send + Sync>,
    etcd_client: Option<Arc<dyn EtcdClient>>,
    reporter: Arc<dyn MinStartTSReporter>,
) -> Box<Syncer> {
    newSyncer(uuid, server_id_getter, etcd_client, reporter, String::new())
}

/// 创建跨 keyspace（cross-KS）Syncer，写入假定目标 keyspace。
pub fn NewCrossKSSyncer(
    uuid: String,
    server_id_getter: Arc<dyn Fn() -> u64 + Send + Sync>,
    etcd_client: Option<Arc<dyn EtcdClient>>,
    reporter: Arc<dyn MinStartTSReporter>,
    target_keyspace: String,
) -> Box<Syncer> {
    newSyncer(
        uuid,
        server_id_getter,
        etcd_client,
        reporter,
        target_keyspace,
    )
}

/// 内部构造：填充路径、reporter 与初始 ServerInfo。
fn newSyncer(
    uuid: String,
    server_id_getter: Arc<dyn Fn() -> u64 + Send + Sync>,
    etcd_client: Option<Arc<dyn EtcdClient>>,
    reporter: Arc<dyn MinStartTSReporter>,
    assumed_keyspace: String,
) -> Box<Syncer> {
    Box::new(Syncer {
        serverInfoPath: serverInfoKeyPath(&uuid),
        etcdCli: etcd_client,
        reporter,
        info: Arc::new(RwLock::new(*getServerInfo(
            uuid,
            server_id_getter,
            assumed_keyspace,
        ))),
        session: None,
        topologySession: None,
    })
}

impl Syncer {
    /// 清理陈旧登记后新建 session，并把本节点 ServerInfo 写入 etcd。
    pub fn NewSessionAndStoreServerInfo(&mut self, context: Context) -> Result<(), SyncError> {
        if self.etcdCli.is_none() {
            return Ok(());
        }
        // 先清同地址陈旧节点，再建立 session 并落盘。
        self.cleanupStaleServerAndOwnerInfo(context.clone());
        self.session = Some(Session::New(45));
        self.StoreServerInfo(context)
    }

    /// 将本地 ServerInfo Marshal 后 Put 到 etcd（绑定 session lease）。
    pub fn StoreServerInfo(&self, context: Context) -> Result<(), SyncError> {
        let Some(client) = &self.etcdCli else {
            return Ok(());
        };
        let lease = self
            .session
            .as_ref()
            .ok_or_else(|| SyncError("server info session is not initialized".into()))?
            .Lease();
        let data = self
            .info
            .write()
            .expect("server info lock poisoned")
            .Marshal()?;
        client.Put(&context, &self.serverInfoPath, data, Some(lease))
    }

    /// 返回本地缓存的 ServerInfo 副本。
    pub fn GetLocalServerInfo(&self) -> ServerInfo {
        self.info.read().expect("server info lock poisoned").clone()
    }

    /// 按节点 ID 查询 ServerInfo；本机 ID 或无 etcd 时直接返回本地。
    pub fn GetServerInfoByID(&self, context: Context, id: &str) -> Result<ServerInfo, SyncError> {
        let local = self.GetLocalServerInfo();
        let Some(client) = &self.etcdCli else {
            return Ok(local);
        };
        if id == local.StaticInfo.ID {
            return Ok(local);
        }
        getInfo(
            context,
            client.as_ref(),
            &serverInfoKeyPath(id),
            KeyOpDefaultRetryCnt,
            KeyOpDefaultTimeout,
            false,
        )?
        .remove(id)
        .ok_or_else(|| {
            SyncError(format!(
                "[info-syncer] get {} failed",
                serverInfoKeyPath(id)
            ))
        })
    }

    /// 合并更新 Labels；有变更时写回 etcd 并刷新本地 DynamicInfo。
    pub fn UpdateServerLabel(
        &self,
        context: Context,
        labels: HashMap<String, String>,
    ) -> Result<(), SyncError> {
        let Some(client) = &self.etcdCli else {
            return Ok(());
        };
        // 仅当标签实际变化时才写 etcd。
        let mut dynamic = self.cloneDynamicServerInfo();
        let mut changed = false;
        for (key, value) in labels {
            if dynamic.Labels.get(&key) != Some(&value) {
                dynamic.Labels.insert(key, value);
                changed = true;
            }
        }
        if !changed {
            return Ok(());
        }
        let mut info = self.GetLocalServerInfo();
        info.DynamicInfo = (*dynamic).clone();
        let data = info.Marshal()?;
        let lease = self
            .session
            .as_ref()
            .ok_or_else(|| SyncError("server info session is not initialized".into()))?
            .Lease();
        client.Put(&context, &self.serverInfoPath, data, Some(lease))?;
        self.setDynamicServerInfo(dynamic);
        Ok(())
    }

    /// 克隆当前动态信息。
    pub fn cloneDynamicServerInfo(&self) -> Box<DynamicInfo> {
        self.info
            .read()
            .expect("server info lock poisoned")
            .DynamicInfo
            .Clone()
    }

    /// 覆盖本地动态信息。
    pub fn setDynamicServerInfo(&self, dynamic: Box<DynamicInfo>) {
        self.info
            .write()
            .expect("server info lock poisoned")
            .DynamicInfo = *dynamic;
    }

    /// 拉取全部节点 ServerInfo；无 etcd 时仅返回根据本地重建的一项。
    pub fn GetAllServerInfo(
        &self,
        context: Context,
    ) -> Result<HashMap<String, ServerInfo>, SyncError> {
        let Some(client) = &self.etcdCli else {
            let local = self.GetLocalServerInfo();
            return Ok(HashMap::from([(
                local.StaticInfo.ID.clone(),
                *getServerInfo(
                    local.StaticInfo.ID.clone(),
                    local
                        .StaticInfo
                        .ServerIDGetter
                        .clone()
                        .unwrap_or_else(|| Arc::new(|| 0)),
                    String::new(),
                ),
            )]));
        };
        getInfo(
            context,
            client.as_ref(),
            ServerInformationPath,
            KeyOpDefaultRetryCnt,
            KeyOpDefaultTimeout,
            true,
        )
    }

    /// ServerInfo session 是否已结束（需重启同步）。
    pub fn Done(&self) -> bool {
        self.etcdCli.is_some() && self.session.as_ref().is_some_and(Session::Done)
    }
    /// 重新建立 session 并存储 ServerInfo。
    pub fn Restart(&mut self, context: Context) -> Result<(), SyncError> {
        self.NewSessionAndStoreServerInfo(context)
    }

    /// 删除与本机同 IP:Port 但不同 ID 的陈旧 ServerInfo 及对应 DDL owner 键。
    pub fn cleanupStaleServerAndOwnerInfo(&self, context: Context) {
        let Some(client) = &self.etcdCli else {
            return;
        };
        let Ok(all_info) = getInfo(
            context.clone(),
            client.as_ref(),
            ServerInformationPath,
            KeyOpDefaultRetryCnt,
            KeyOpDefaultTimeout,
            true,
        ) else {
            return;
        };
        // 只清理同地址、不同 uuid 的陈旧登记。
        let local = self.GetLocalServerInfo();
        for (id, info) in all_info {
            if id == local.StaticInfo.ID
                || info.StaticInfo.IP != local.StaticInfo.IP
                || info.StaticInfo.Port != local.StaticInfo.Port
            {
                continue;
            }
            if let Ok(owner_keys) = client.Get(&context, "/tidb/ddl/fg/owner/", true) {
                for owner in owner_keys {
                    let owner_id = owner
                        .value
                        .split(|byte| *byte == b'_')
                        .next()
                        .unwrap_or_default();
                    if owner_id == id.as_bytes() {
                        let _ = client.Delete(&context, &owner.key);
                        break;
                    }
                }
            }
            let _ = client.Delete(&context, &serverInfoKeyPath(&id));
        }
    }

    /// 从 etcd 删除本节点 ServerInfo 键。
    pub fn RemoveServerInfo(&self) {
        if let Some(client) = &self.etcdCli {
            let _ = client.Delete(&Context::Background(), &self.serverInfoPath);
        }
    }

    /// 后台循环：session 失效则重启，并按间隔上报 min start TS。
    pub fn ServerInfoSyncLoop(&mut self, store: &dyn Storage, exit: mpsc::Receiver<()>) {
        let mut next_report = Instant::now() + minTSReportInterval;
        loop {
            let now = Instant::now();
            let until_report = next_report.saturating_duration_since(now);
            match exit.recv_timeout(until_report.min(Duration::from_millis(100))) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                // 超时唤醒：检查 session，到期则上报 min start TS。
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if self.Done() {
                        let _ = self.Restart(Context::Background());
                    }
                    if Instant::now() >= next_report {
                        if let Some(session) = &self.session {
                            self.reporter.ReportMinStartTS(store, session);
                        }
                        next_report = Instant::now() + minTSReportInterval;
                    }
                }
            }
        }
    }

    /// 新建拓扑 session 并写入拓扑 info + ttl。
    pub fn NewTopologySessionAndStoreServerInfo(
        &mut self,
        context: Context,
    ) -> Result<(), SyncError> {
        if self.etcdCli.is_none() {
            return Ok(());
        }
        self.topologySession = Some(Session::New(TopologySessionTTL));
        self.StoreTopologyInfo(context)
    }

    /// 写入 `/topology/tidb/<addr>/info`（无 lease），并刷新 aliveness ttl。
    pub fn StoreTopologyInfo(&self, context: Context) -> Result<(), SyncError> {
        let Some(client) = &self.etcdCli else {
            return Ok(());
        };
        let info = self.GetLocalServerInfo();
        let address = join_host_port(&info.StaticInfo.IP, info.StaticInfo.Port);
        client.Put(
            &context,
            &format!("{TopologyInformationPath}/{address}/info"),
            info.ToTopologyInfo().Marshal(),
            None,
        )?;
        self.updateTopologyAliveness(context)
    }

    /// 用当前纳秒时间戳刷新 `/topology/tidb/<addr>/ttl`（绑定拓扑 session lease）。
    pub fn updateTopologyAliveness(&self, context: Context) -> Result<(), SyncError> {
        let Some(client) = &self.etcdCli else {
            return Ok(());
        };
        let info = self.GetLocalServerInfo();
        let lease = self
            .topologySession
            .as_ref()
            .ok_or_else(|| SyncError("topology session is not initialized".into()))?
            .Lease();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_string()
            .into_bytes();
        client.Put(
            &context,
            &format!(
                "{TopologyInformationPath}/{}/ttl",
                join_host_port(&info.StaticInfo.IP, info.StaticInfo.Port)
            ),
            now,
            Some(lease),
        )
    }

    /// 前缀扫描拓扑路径，解析所有以 `/info` 结尾的条目。
    pub fn GetAllTiDBTopology(&self, context: Context) -> Result<Vec<TopologyInfo>, SyncError> {
        let Some(client) = &self.etcdCli else {
            return Ok(Vec::new());
        };
        client
            .Get(&context, TopologyInformationPath, true)?
            .into_iter()
            .filter(|key_value| key_value.key.ends_with("/info"))
            .map(|key_value| TopologyInfo::Unmarshal(&key_value.value).map_err(Into::into))
            .collect()
    }

    /// 删除本节点拓扑前缀下全部键。
    pub fn RemoveTopologyInfo(&self) {
        if let Some(client) = &self.etcdCli {
            let info = self.GetLocalServerInfo();
            let _ = client.DeletePrefix(
                &Context::Background(),
                &format!(
                    "{TopologyInformationPath}/{}",
                    join_host_port(&info.StaticInfo.IP, info.StaticInfo.Port)
                ),
            );
        }
    }
    /// 拓扑 session 是否已结束。
    pub fn TopologyDone(&self) -> bool {
        self.etcdCli.is_some() && self.topologySession.as_ref().is_some_and(Session::Done)
    }
    /// 重新建立拓扑 session 并存储拓扑信息。
    pub fn RestartTopology(&mut self, context: Context) -> Result<(), SyncError> {
        self.NewTopologySessionAndStoreServerInfo(context)
    }

    /// 后台循环：拓扑 session 失效则重启，否则按间隔刷新拓扑。
    pub fn TopologySyncLoop(&mut self, exit: mpsc::Receiver<()>) {
        let mut next_refresh = Instant::now() + TopologyTimeToRefresh;
        loop {
            let now = Instant::now();
            let until_refresh = next_refresh.saturating_duration_since(now);
            match exit.recv_timeout(until_refresh.min(Duration::from_millis(100))) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if self.TopologyDone() {
                        let _ = self.RestartTopology(Context::Background());
                        next_refresh = Instant::now() + TopologyTimeToRefresh;
                    } else if Instant::now() >= next_refresh {
                        let _ = self.StoreTopologyInfo(Context::Background());
                        next_refresh = Instant::now() + TopologyTimeToRefresh;
                    }
                }
            }
        }
    }
}

/// 带重试的 etcd Get，并将返回值反序列化为 `ID -> ServerInfo` 映射。
pub fn getInfo(
    context: Context,
    client: &dyn EtcdClient,
    key: &str,
    retry_count: i32,
    timeout: Duration,
    prefix: bool,
) -> Result<HashMap<String, ServerInfo>, SyncError> {
    // 有限次重试；每次 Get 带独立超时上下文。
    let mut last_error = SyncError("etcd get failed without an attempt".into());
    for attempt in 0..retry_count.max(0) {
        if context.Done() {
            return Err(SyncError("context cancelled".into()));
        }
        match client.Get(&context.WithTimeout(timeout), key, prefix) {
            Ok(response) => {
                let mut all_info = HashMap::new();
                for key_value in response {
                    let mut info = ServerInfo::default();
                    info.Unmarshal(&key_value.value)?;
                    all_info.insert(info.StaticInfo.ID.clone(), info);
                }
                return Ok(all_info);
            }
            Err(error) => {
                last_error = error;
                if attempt + 1 < retry_count {
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    }
    Err(last_error)
}

/// 根据全局配置与 uuid 构造初始 ServerInfo（MockServerInfo 时注入固定值）。
pub fn getServerInfo(
    id: String,
    server_id_getter: Arc<dyn Fn() -> u64 + Send + Sync>,
    assumed_keyspace: String,
) -> Box<ServerInfo> {
    let config = GetGlobalServerConfig();
    let mut info = ServerInfo {
        StaticInfo: StaticInfo {
            VersionInfo: VersionInfo {
                Version: ServerVersion.into(),
                GitHash: config.GitHash,
            },
            ID: id,
            IP: config.AdvertiseAddress,
            Port: config.Port,
            StatusPort: config.StatusPort,
            Lease: config.Lease,
            StartTimestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            Keyspace: config.Keyspace,
            AssumedKeyspace: assumed_keyspace,
            ServerIDGetter: Some(server_id_getter),
            JSONServerID: 0,
        },
        DynamicInfo: DynamicInfo {
            Labels: config.Labels,
        },
    };
    // 测试模式：固定时间戳与标签，便于断言。
    if config.MockServerInfo {
        info.StaticInfo.StartTimestamp = 1_282_967_700;
        info.DynamicInfo.Labels = HashMap::from([("foo".into(), "bar".into())]);
    }
    Box::new(info)
}

/// 拼接 host:port；IPv6 裸地址自动加方括号。
fn join_host_port(host: &str, port: u32) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}
