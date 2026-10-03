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

// 进程内 mock PD 客户端（对应 Go unistore/pd.go）。
//
// PD（Placement Driver）负责集群元数据、Region 路由与 TSO。本模块用
// `MockPd` + 内存状态模拟全局配置、外部时间戳与 Keyspace，供嵌入式
// unistore 不依赖真实 PD 集群。

use crate::tikv::mock_region::MockPd;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, RwLock};
use std::thread;

/// Keyspace ID 合法上限（与 PD 协议一致）。
pub const MAX_KEYSPACE_ID: u32 = 0x00ff_ffff;
/// Matches `constants.NullKeyspaceID` from PD client.
/// 空 Keyspace ID，对应 PD 客户端常量 `NullKeyspaceID`。
pub const NULL_KEYSPACE_ID: u32 = u32::MAX;
/// TSO 中逻辑部分占用的位数（物理时间戳左移量）。
const LOGICAL_BITS: u32 = 18;

/// 全局配置变更事件类型（Watch 推送用）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EventType {
    #[default]
    None,
    Put,
}

/// 一条全局配置项及其事件类型。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GlobalConfigItem {
    pub name: String,
    pub value: String,
    pub event_type: EventType,
}

/// Keyspace 生命周期状态：启用 / 禁用 / 归档。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum KeyspaceState {
    #[default]
    Enabled,
    Disabled,
    Archived,
}

/// Keyspace 元数据：ID、名称与状态。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KeyspaceMeta {
    pub id: u32,
    pub name: String,
    pub state: KeyspaceState,
}

/// mock PD 操作失败时的错误消息包装。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PdError(pub String);

impl std::fmt::Display for PdError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for PdError {}
/// 本模块统一 Result 别名。
pub type Result<T> = std::result::Result<T, PdError>;

/// Member listing returned by the mock PD, in injected endpoint order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PdMember {
    pub member_id: u64,
    pub client_urls: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PdMembersResponse {
    pub members: Vec<PdMember>,
    pub leader: Option<PdMember>,
}

fn normalize_mock_pd_addrs(addresses: Vec<String>) -> Vec<String> {
    addresses
        .into_iter()
        .filter_map(|address| {
            astersql_util::service_url::NormalizeServiceURL(&address, "http").ok()
        })
        .collect()
}

/// 进程内 PD 门面：转发 TSO、管理全局配置与 Keyspace。
pub struct PdClient {
    pd: Arc<MockPd>,
    keyspaces: MockKeyspaceManager,
    global_config: RwLock<HashMap<String, String>>,
    external_timestamp: AtomicU64,
    addresses: Vec<String>,
    current_keyspace_id: u32,
}

impl PdClient {
    /// 构造 mock PdClient；校验并装载初始 Keyspace 列表。
    pub fn new(
        pd: Arc<MockPd>,
        addresses: Vec<String>,
        current_keyspace_id: u32,
        keyspaces: Vec<KeyspaceMeta>,
    ) -> Result<Self> {
        Ok(Self {
            pd,
            keyspaces: MockKeyspaceManager::new(keyspaces)?,
            global_config: RwLock::new(HashMap::new()),
            external_timestamp: AtomicU64::new(0),
            addresses: normalize_mock_pd_addrs(addresses),
            current_keyspace_id,
        })
    }

    pub fn get_all_members(&self) -> PdMembersResponse {
        let members: Vec<_> = self
            .addresses
            .iter()
            .enumerate()
            .map(|(index, address)| PdMember {
                member_id: index as u64 + 1,
                client_urls: vec![address.clone()],
            })
            .collect();
        let leader = members.first().cloned();
        PdMembersResponse { members, leader }
    }

    /// 按名称列表加载全局配置；返回项列表与 revision（mock 固定为 0）。
    pub fn load_global_config(
        &self,
        names: &[String],
        _path: &str,
    ) -> (Vec<GlobalConfigItem>, i64) {
        let config = self
            .global_config
            .read()
            .expect("global-config lock poisoned");
        let items = names
            .iter()
            .map(|name| {
                let full_name = format!("/global/config/{name}");
                match config.get(&full_name) {
                    Some(value) => GlobalConfigItem {
                        name: full_name,
                        value: value.clone(),
                        event_type: EventType::Put,
                    },
                    None => GlobalConfigItem {
                        name: full_name,
                        ..GlobalConfigItem::default()
                    },
                }
            })
            .collect();
        (items, 0)
    }

    /// 将配置项写入内存表，键前缀为 `/global/config/`。
    pub fn store_global_config(&self, _path: &str, items: &[GlobalConfigItem]) {
        let mut config = self
            .global_config
            .write()
            .expect("global-config lock poisoned");
        for item in items {
            config.insert(format!("/global/config/{}", item.name), item.value.clone());
        }
    }

    /// 启动后台线程，循环推送当前配置快照（最多约 10 轮），模拟 Watch。
    pub fn watch_global_config(
        &self,
        _path: &str,
        _revision: i64,
    ) -> Receiver<Vec<GlobalConfigItem>> {
        let config = self
            .global_config
            .read()
            .expect("global-config lock poisoned")
            .clone();
        let (sender, receiver) = mpsc::sync_channel(16);
        thread::spawn(move || {
            for _ in 0..10 {
                for (name, value) in &config {
                    if sender
                        .send(vec![GlobalConfigItem {
                            name: name.clone(),
                            value: value.clone(),
                            event_type: EventType::None,
                        }])
                        .is_err()
                    {
                        return;
                    }
                }
            }
        });
        receiver
    }

    /// 从底层 MockPd 获取 TSO（物理、逻辑时间戳）。
    pub fn get_ts(&self) -> (i64, i64) {
        self.pd.get_ts()
    }

    /// 本地 DC TSO；mock 中等同于 `get_ts`。
    pub fn get_local_ts(&self, _dc_location: &str) -> (i64, i64) {
        self.get_ts()
    }

    /// 返回仅可 Wait 一次的异步 TSO Future。
    pub fn get_ts_async(self: &Arc<Self>) -> MockTsFuture {
        MockTsFuture {
            client: Arc::clone(self),
            used: AtomicBool::new(false),
        }
    }

    /// 基于配置的 PD 地址构造服务发现视图。
    pub fn service_discovery(&self) -> MockPdServiceDiscovery {
        MockPdServiceDiscovery::new(self.addresses.clone())
    }

    /// mock 场景下的 leader URL 固定为 `"mockpd"`。
    pub fn leader_url(&self) -> &'static str {
        "mockpd"
    }

    /// 设置外部时间戳：不得大于当前全局 TSO，也不得回退。
    pub fn set_external_timestamp(&self, new_timestamp: u64) -> Result<()> {
        let (physical, logical) = self.get_ts();
        let current_tso = ((physical as u64) << LOGICAL_BITS) + logical as u64;
        if new_timestamp > current_tso {
            return Err(PdError(
                "external timestamp is greater than global tso".into(),
            ));
        }
        // CAS 循环：并发下拒绝减小，相同值视为成功。
        loop {
            let current = self.external_timestamp.load(Ordering::Acquire);
            if current > new_timestamp {
                return Err(PdError("cannot decrease the external timestamp".into()));
            }
            if current == new_timestamp {
                return Ok(());
            }
            if self
                .external_timestamp
                .compare_exchange(current, new_timestamp, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Ok(());
            }
        }
    }

    /// 读取当前外部时间戳。
    pub fn external_timestamp(&self) -> u64 {
        self.external_timestamp.load(Ordering::Acquire)
    }

    /// 当前会话绑定的 Keyspace ID。
    pub fn current_keyspace_id(&self) -> u32 {
        self.current_keyspace_id
    }

    /// 按名称加载 Keyspace 元数据。
    pub fn load_keyspace(&self, name: &str) -> Result<KeyspaceMeta> {
        self.keyspaces.load(name)
    }

    /// 按 ID 加载 Keyspace 元数据。
    pub fn load_keyspace_by_id(&self, id: u32) -> Result<KeyspaceMeta> {
        self.keyspaces.load_by_id(id)
    }

    /// 从 `start_id` 起分页列出 Keyspace；`limit==0` 表示不限制。
    pub fn all_keyspaces(&self, start_id: u32, limit: u32) -> Vec<KeyspaceMeta> {
        self.keyspaces.all(start_id, limit)
    }

    /// 更新指定 Keyspace 的状态并返回最新元数据。
    pub fn update_keyspace_state(&self, id: u32, state: KeyspaceState) -> Result<KeyspaceMeta> {
        self.keyspaces.update_state(id, state)
    }

    /// Go `pd.Client.Close` is a no-op for the in-process mock.
    /// 关闭客户端；进程内 mock 为空操作。
    pub fn close(&self) {}
}

/// Go 风格构造函数：创建 `Arc<PdClient>`，Keyspace 非法时 panic。
pub fn newPDClient(
    pd: Arc<MockPd>,
    addresses: Vec<String>,
    current_keyspace_id: u32,
    keyspaces: Vec<KeyspaceMeta>,
) -> Arc<PdClient> {
    Arc::new(
        PdClient::new(pd, addresses, current_keyspace_id, keyspaces)
            .expect("failed to create mock keyspace manager"),
    )
}

/// 一次性异步 TSO Future：`wait` 只能成功调用一次。
pub struct MockTsFuture {
    client: Arc<PdClient>,
    used: AtomicBool,
}

impl MockTsFuture {
    /// 等待并返回 TSO；第二次调用返回错误。
    pub fn wait(&self) -> Result<(i64, i64)> {
        if self.used.swap(true, Ordering::AcqRel) {
            return Err(PdError("cannot wait tso twice".into()));
        }
        Ok(self.client.get_ts())
    }
}

/// 单个 mock PD 服务端点客户端（地址规范化后始终可用）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MockPdServiceClient {
    address: String,
}

impl MockPdServiceClient {
    /// 若地址无 scheme 则补上 `http://`。
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
        }
    }

    /// 规范化后的服务地址。
    pub fn address(&self) -> &str {
        &self.address
    }
    /// mock 始终可用。
    pub fn available(&self) -> bool {
        true
    }
    /// mock 无需重试。
    pub fn need_retry(&self) -> bool {
        false
    }
    /// mock 视为已连上 leader。
    pub fn connected_to_leader(&self) -> bool {
        true
    }
}

/// 基于地址列表过滤合法 URL 并构造客户端集合。
#[derive(Clone, Debug, Default)]
pub struct MockPdServiceDiscovery {
    addresses: Vec<String>,
    clients: Vec<MockPdServiceClient>,
}

impl MockPdServiceDiscovery {
    /// 过滤非法地址后构建发现视图。
    pub fn new(addresses: Vec<String>) -> Self {
        let addresses = normalize_mock_pd_addrs(addresses);
        let clients = addresses
            .iter()
            .cloned()
            .map(MockPdServiceClient::new)
            .collect();
        Self { addresses, clients }
    }

    /// 返回过滤后的服务 URL 列表。
    pub fn service_urls(&self) -> Vec<String> {
        self.addresses.clone()
    }
    /// 返回第一个可用客户端（若有）。
    pub fn service_client(&self) -> Option<MockPdServiceClient> {
        self.clients.first().cloned()
    }
    /// Go mock discovery has no cached gRPC connection to remove.
    pub fn remove_client_conn(&self, _address: &str) {}

    /// 返回全部客户端副本。
    pub fn all_service_clients(&self) -> Vec<MockPdServiceClient> {
        self.clients.clone()
    }
}

/// Go 风格构造函数：`NewMockPDServiceDiscovery`。
pub fn NewMockPDServiceDiscovery(addresses: Vec<String>) -> MockPdServiceDiscovery {
    MockPdServiceDiscovery::new(addresses)
}

/// Mirrors govalidator.IsURL as used by Go NewMockPDServiceDiscovery: accept
/// `http(s)://...` and bare `host:port`, reject opaque strings without a port.
/// 判断地址是否为合法 URL（对齐 Go govalidator.IsURL 子集）。
pub fn is_url(address: &str) -> bool {
    valid_url(address)
}

/// 接受 `http(s)://...` 或无 scheme 的 `host:port`。
fn valid_url(address: &str) -> bool {
    astersql_util::service_url::NormalizeServiceURL(address, "http").is_ok()
}

/// 内存 Keyspace 管理器：按 ID 排序存储，并维护名称索引。
pub struct MockKeyspaceManager {
    keyspaces: RwLock<BTreeMap<u32, KeyspaceMeta>>,
    names: RwLock<HashMap<String, u32>>,
}

impl MockKeyspaceManager {
    /// 装载初始列表；ID/名称重复或 ID 超限则报错。
    pub fn new(keyspaces: Vec<KeyspaceMeta>) -> Result<Self> {
        let mut by_id = BTreeMap::new();
        let mut names = HashMap::new();
        for keyspace in keyspaces {
            if keyspace.id > MAX_KEYSPACE_ID {
                return Err(PdError(format!("invalid keyspace ID: {}", keyspace.id)));
            }
            if by_id.contains_key(&keyspace.id) {
                return Err(PdError(format!("keyspace ID {} duplicated", keyspace.id)));
            }
            if names.insert(keyspace.name.clone(), keyspace.id).is_some() {
                return Err(PdError(format!(
                    "keyspace name {} duplicated",
                    keyspace.name
                )));
            }
            by_id.insert(keyspace.id, keyspace);
        }
        Ok(Self {
            keyspaces: RwLock::new(by_id),
            names: RwLock::new(names),
        })
    }

    /// 按名称查找；不存在时返回 `ENTRY_NOT_FOUND`。
    pub fn load(&self, name: &str) -> Result<KeyspaceMeta> {
        let id = self
            .names
            .read()
            .expect("keyspace-name lock poisoned")
            .get(name)
            .copied()
            .ok_or_else(|| PdError("ENTRY_NOT_FOUND".into()))?;
        self.keyspaces
            .read()
            .expect("keyspace lock poisoned")
            .get(&id)
            .cloned()
            .ok_or_else(|| PdError("keyspace list and name map mismatch".into()))
    }

    /// ID lookup uses the same ordered metadata store as name lookup and listing.
    pub fn load_by_id(&self, id: u32) -> Result<KeyspaceMeta> {
        self.keyspaces
            .read()
            .expect("keyspace lock poisoned")
            .get(&id)
            .cloned()
            .ok_or_else(|| PdError("ENTRY_NOT_FOUND".into()))
    }

    /// 从 `start_id` 起按 ID 升序取至多 `limit` 条。
    pub fn all(&self, start_id: u32, limit: u32) -> Vec<KeyspaceMeta> {
        let keyspaces = self.keyspaces.read().expect("keyspace lock poisoned");
        let take = if limit == 0 {
            usize::MAX
        } else {
            limit as usize
        };
        keyspaces
            .range(start_id..)
            .take(take)
            .map(|(_, keyspace)| keyspace.clone())
            .collect()
    }

    /// 更新状态；ID 不存在时返回 `ENTRY_NOT_FOUND`。
    pub fn update_state(&self, id: u32, state: KeyspaceState) -> Result<KeyspaceMeta> {
        let mut keyspaces = self.keyspaces.write().expect("keyspace lock poisoned");
        let keyspace = keyspaces
            .get_mut(&id)
            .ok_or_else(|| PdError("ENTRY_NOT_FOUND".into()))?;
        keyspace.state = state;
        Ok(keyspace.clone())
    }

    /// Sorted keyspace metas, matching Go `mockKeyspaceManager.keyspaces`.
    /// 按 ID 排序返回全部 Keyspace 元数据副本。
    pub fn keyspaces(&self) -> Vec<KeyspaceMeta> {
        self.keyspaces
            .read()
            .expect("keyspace lock poisoned")
            .values()
            .cloned()
            .collect()
    }

    /// Name → id map, matching Go `mockKeyspaceManager.keyspaceNamesMap`.
    /// 返回名称到 ID 的映射副本。
    pub fn keyspace_names_map(&self) -> HashMap<String, u32> {
        self.names
            .read()
            .expect("keyspace-name lock poisoned")
            .clone()
    }
}

/// Go-compatible constructor name used by pd_test.
/// Go 兼容构造函数名，供 pd_test 调用。
#[allow(non_snake_case)]
pub fn newMockKeyspaceManager(keyspaces: Vec<KeyspaceMeta>) -> Result<MockKeyspaceManager> {
    MockKeyspaceManager::new(keyspaces)
}
