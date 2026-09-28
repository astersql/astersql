// Copyright 2026 AsterSQL.
// Copyright 2019-present PingCAP, Inc.
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

// PD client used by mock unistore.
//
// This is the native Rust counterpart of `client.go`. It deliberately keeps
// the Go client's endpoint ordering, ten-attempt request retry policy and
// failed-heartbeat replay semantics while using `grpcio` and TiKV's generated
// protobuf client directly.
//
// mock unistore 使用的 PD gRPC 客户端（对应 Go `client.go`）。
//
// PD（Placement Driver）负责集群元数据、Region 路由与 TSO。本实现保留 Go
// 客户端的端点排序、最多 10 次重试，以及失败心跳重放语义，底层使用
// `grpcio` 与 TiKV 生成的 protobuf 客户端。

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use futures::{FutureExt, SinkExt, StreamExt, pin_mut, select_biased};
use grpcio::{CallOption, Channel, ChannelBuilder, Environment, WriteFlags};
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tikv_client_proto::{metapb, pdpb};
use url::{Position, Url};

/// 单次 PD RPC 超时。
pub const PD_TIMEOUT: Duration = Duration::from_secs(1);
/// 重试间隔。
pub const RETRY_INTERVAL: Duration = Duration::from_secs(1);
/// 最大重试次数（与 Go 客户端一致）。
pub const MAX_RETRY_COUNT: usize = 10;

/// 本模块统一 Result 别名。
pub type Result<T> = std::result::Result<T, PdError>;
/// Region 心跳响应回调。
type HeartbeatHandler = Arc<dyn Fn(pdpb::RegionHeartbeatResponse) + Send + Sync + 'static>;

/// PD 客户端错误：gRPC、端点非法、缺 leader/TSO、响应错误、已关闭或重试耗尽。
#[derive(Debug)]
pub enum PdError {
    Grpc(grpcio::Error),
    InvalidEndpoint(String),
    MissingLeader,
    MissingTimestamp,
    Response(String),
    Closed,
    TooManyRetries,
}

impl fmt::Display for PdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Grpc(err) => write!(f, "PD gRPC error: {err}"),
            Self::InvalidEndpoint(endpoint) => write!(f, "invalid PD endpoint: {endpoint}"),
            Self::MissingLeader => write!(f, "PD response did not contain a usable leader"),
            Self::MissingTimestamp => write!(f, "PD TSO response did not contain a timestamp"),
            Self::Response(message) => write!(f, "PD response error: {message}"),
            Self::Closed => write!(f, "PD client is closed"),
            Self::TooManyRetries => write!(f, "PD request failed too many times"),
        }
    }
}

impl std::error::Error for PdError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Grpc(err) => Some(err),
            _ => None,
        }
    }
}

impl From<grpcio::Error> for PdError {
    fn from(value: grpcio::Error) -> Self {
        Self::Grpc(value)
    }
}

/// Region information returned by PD, matching router.Region in the Go client.
/// PD 返回的 Region 信息（对齐 Go `router.Region`）：元数据、leader 与异常 peer。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Region {
    pub meta: Option<metapb::Region>,
    pub leader: Option<metapb::Peer>,
    pub down_peers: Vec<metapb::Peer>,
    pub pending_peers: Vec<metapb::Peer>,
}

/// PD 客户端能力抽象：集群 ID、Bootstrap、Store/Region 查询、心跳、TSO 等。
pub trait Client: Send + Sync {
    fn get_cluster_id(&self) -> u64;
    fn alloc_id(&self) -> Result<u64>;
    /// 引导集群：注册首个 Store 与 Region。
    fn bootstrap(
        &self,
        store: metapb::Store,
        region: metapb::Region,
    ) -> Result<pdpb::BootstrapResponse>;
    fn is_bootstrapped(&self) -> Result<bool>;
    fn put_store(&self, store: metapb::Store) -> Result<()>;
    fn get_store(&self, store_id: u64) -> Result<Option<metapb::Store>>;
    fn get_region(&self, key: Vec<u8>) -> Result<Region>;
    fn get_region_by_id(&self, region_id: u64) -> Result<Region>;
    fn report_region(&self, request: pdpb::RegionHeartbeatRequest) -> Result<()>;
    fn ask_split(&self, region: metapb::Region) -> Result<pdpb::AskSplitResponse>;
    /// 请求批量分裂所需的 ID。
    fn ask_batch_split(
        &self,
        region: metapb::Region,
        count: i32,
    ) -> Result<pdpb::AskBatchSplitResponse>;
    fn report_batch_split(&self, regions: Vec<metapb::Region>) -> Result<()>;
    fn get_gc_safe_point(&self) -> Result<u64>;
    fn store_heartbeat(&self, stats: pdpb::StoreStats) -> Result<()>;
    fn get_ts(&self) -> Result<(i64, i64)>;
    fn set_region_heartbeat_response_handler(&self, handler: Option<HeartbeatHandler>);
    fn close(&self);
}

/// 发送失败时暂存的 Region 心跳请求，供流重建后重放。
#[derive(Default)]
pub struct PendingHeartbeat {
    request: Option<pdpb::RegionHeartbeatRequest>,
}

impl PendingHeartbeat {
    /// 将失败请求放回 pending，等待下次发送。
    pub fn restore(&mut self, request: pdpb::RegionHeartbeatRequest) {
        self.request = Some(request);
    }

    /// 取出并清空 pending 请求。
    pub fn take(&mut self) -> Option<pdpb::RegionHeartbeatRequest> {
        self.request.take()
    }

    /// 优先返回 pending，否则阻塞从队列取下一条心跳。
    pub fn next(
        &mut self,
        queue: &Receiver<pdpb::RegionHeartbeatRequest>,
    ) -> Result<pdpb::RegionHeartbeatRequest> {
        if let Some(request) = self.take() {
            return Ok(request);
        }
        queue.recv().map_err(|_| PdError::Closed)
    }
}

/// 已建立的 gRPC Channel 缓存与当前 leader 地址。
struct ConnectionState {
    clients: HashMap<String, Channel>,
    leader: String,
}

/// PdClient 共享内部状态：URL 列表、连接、心跳通道与后台线程。
struct Inner {
    urls: RwLock<Vec<String>>,
    cluster_id: AtomicU64,
    tag: String,
    environment: Arc<Environment>,
    connections: RwLock<ConnectionState>,
    check_leader_tx: Sender<()>,
    check_leader_rx: Receiver<()>,
    region_tx: Sender<pdpb::RegionHeartbeatRequest>,
    region_rx: Receiver<pdpb::RegionHeartbeatRequest>,
    pending: Mutex<PendingHeartbeat>,
    heartbeat_handler: RwLock<HeartbeatHandler>,
    stopped: AtomicBool,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

/// 可克隆的 PD 客户端句柄（内部经 Arc 共享）。
#[derive(Clone)]
pub struct PdClient {
    inner: Arc<Inner>,
}

/// 规范化 PD 地址：无 scheme 时补上 `http://`。
pub fn normalize_pd_urls<I>(addresses: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    addresses
        .into_iter()
        .map(|address| {
            if address.contains("://") {
                address
            } else {
                format!("http://{address}")
            }
        })
        .collect()
}

/// 按 Go 客户端顺序排列成员 URL：非 leader 在前，leader 的 URL 置后。
pub fn ordered_member_urls(members: &[pdpb::Member], leader: &pdpb::Member) -> Vec<String> {
    let mut urls = Vec::with_capacity(members.len());
    for member in members {
        if member.member_id != leader.member_id {
            urls.extend(member.client_urls.iter().cloned());
        }
    }
    urls.extend(leader.client_urls.iter().cloned());
    urls
}

/// 按固定次数与间隔重试操作；全部失败返回 `TooManyRetries`。
pub fn retry_with_policy<T, F, N>(
    attempts: usize,
    interval: Duration,
    mut operation: F,
    mut notify_failure: N,
) -> Result<T>
where
    F: FnMut() -> Result<T>,
    N: FnMut(&PdError),
{
    for _ in 0..attempts {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) => {
                notify_failure(&error);
                thread::sleep(interval);
            }
        }
    }
    Err(PdError::TooManyRetries)
}

/// 检查 PD 响应头中的错误字段。
pub fn check_response_header(header: &pdpb::ResponseHeader) -> Result<()> {
    match header.error.as_ref() {
        Some(error) => {
            let error_type = format!("{:?}", error.get_type());
            let mut go_error_type = String::with_capacity(error_type.len());
            for (index, character) in error_type.chars().enumerate() {
                if index > 0 && character.is_ascii_uppercase() {
                    go_error_type.push('_');
                }
                go_error_type.push(character.to_ascii_uppercase());
            }
            let diagnostic = if error.message.is_empty() {
                format!("type:{go_error_type}")
            } else {
                format!("type:{go_error_type} message:{:?}", error.message)
            };
            Err(PdError::Response(diagnostic))
        }
        None => Ok(()),
    }
}

/// 可选响应头：缺省视为成功。
fn check_optional_header(header: Option<&pdpb::ResponseHeader>) -> Result<()> {
    match header {
        Some(header) => check_response_header(header),
        None => Ok(()),
    }
}

/// 将 GetRegion 响应映射为本地 `Region`。
fn response_region(response: pdpb::GetRegionResponse) -> Region {
    Region {
        meta: response.region.into_option(),
        leader: response.leader.into_option(),
        down_peers: response
            .down_peers
            .into_iter()
            .filter_map(|stats| stats.peer.into_option())
            .collect(),
        pending_peers: response.pending_peers.into_vec(),
    }
}

impl PdClient {
    /// 创建客户端：规范化地址、探测 leader、写入 cluster_id 并启动后台循环。
    pub fn new(pd_addresses: Vec<String>, tag: impl Into<String>) -> Result<Self> {
        let urls = normalize_pd_urls(pd_addresses);
        let (check_leader_tx, check_leader_rx) = crossbeam_channel::bounded(1);
        let (region_tx, region_rx) = crossbeam_channel::bounded(64);
        let inner = Arc::new(Inner {
            urls: RwLock::new(urls),
            cluster_id: AtomicU64::new(0),
            tag: tag.into(),
            environment: Arc::new(Environment::new(2)),
            connections: RwLock::new(ConnectionState {
                clients: HashMap::new(),
                leader: String::new(),
            }),
            check_leader_tx,
            check_leader_rx,
            region_tx,
            region_rx,
            pending: Mutex::new(PendingHeartbeat::default()),
            heartbeat_handler: RwLock::new(Arc::new(|_| {})),
            stopped: AtomicBool::new(false),
            workers: Mutex::new(Vec::new()),
        });
        let client = Self { inner };

        // 启动前最多重试 MAX_RETRY_COUNT 次以获取成员与 leader。
        let mut last_error = None;
        let mut members = None;
        for _ in 0..MAX_RETRY_COUNT {
            match client.update_leader() {
                Ok(response) => {
                    members = Some(response);
                    break;
                }
                Err(error) => {
                    last_error = Some(error);
                    thread::sleep(RETRY_INTERVAL);
                }
            }
        }
        let members = match members {
            Some(response) => response,
            None => return Err(last_error.unwrap_or(PdError::MissingLeader)),
        };
        client.inner.cluster_id.store(
            members
                .header
                .as_ref()
                .map_or(0, |header| header.cluster_id),
            Ordering::Release,
        );
        client.start_workers();
        Ok(client)
    }

    /// 启动 leader 检查与 Region 心跳流两个后台线程。
    fn start_workers(&self) {
        let leader_client = self.clone();
        let heartbeat_client = self.clone();
        let handles = vec![
            thread::spawn(move || leader_client.check_leader_loop()),
            thread::spawn(move || heartbeat_client.heartbeat_stream_loop()),
        ];
        self.inner
            .workers
            .lock()
            .expect("workers lock poisoned")
            .extend(handles);
    }

    /// 周期或被唤醒时刷新 leader 信息。
    fn check_leader_loop(&self) {
        while !self.is_stopped() {
            match self
                .inner
                .check_leader_rx
                .recv_timeout(Duration::from_secs(60))
            {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {
                    if !self.is_stopped() {
                        let _ = self.update_leader();
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    /// 维护与 leader 的双向 Region 心跳流；失败后调度刷新并退避重试。
    fn heartbeat_stream_loop(&self) {
        while !self.is_stopped() {
            let stream = self
                .leader_client()
                .and_then(|client| client.region_heartbeat().map_err(PdError::Grpc));
            match stream {
                Ok((sender, receiver)) => {
                    if let Err(error) =
                        futures::executor::block_on(self.run_heartbeat_stream(sender, receiver))
                    {
                        if !self.is_stopped() {
                            eprintln!("PD heartbeat stream failed for {}: {error}", self.inner.tag);
                        }
                    }
                }
                Err(error) => {
                    if !self.is_stopped() {
                        eprintln!(
                            "failed to create PD heartbeat stream for {}: {error}",
                            self.inner.tag
                        );
                    }
                }
            }
            if !self.is_stopped() {
                self.schedule_update_leader();
                self.sleep_until_retry();
            }
        }
    }

    /// 在双向流上发送 pending/队列中的心跳，并分发响应给 handler。
    async fn run_heartbeat_stream(
        &self,
        mut sender: grpcio::ClientDuplexSender<pdpb::RegionHeartbeatRequest>,
        mut receiver: grpcio::ClientDuplexReceiver<pdpb::RegionHeartbeatResponse>,
    ) -> Result<()> {
        loop {
            if self.is_stopped() {
                sender.cancel();
                receiver.cancel();
                return Ok(());
            }

            let request = self
                .inner
                .pending
                .lock()
                .expect("pending lock poisoned")
                .take()
                .or_else(|| self.inner.region_rx.try_recv().ok());
            if let Some(mut request) = request {
                request.header = Some(self.request_header()).into();
                // 发送失败则 restore，供流重建后重放。
                if let Err(error) = sender.send((request.clone(), WriteFlags::default())).await {
                    self.inner
                        .pending
                        .lock()
                        .expect("pending lock poisoned")
                        .restore(request);
                    return Err(PdError::Grpc(error));
                }
            }

            let response = receiver.next().fuse();
            let tick = futures_timer::Delay::new(Duration::from_millis(20)).fuse();
            pin_mut!(response, tick);
            select_biased! {
                response = response => match response {
                    Some(Ok(response)) => {
                        let handler = self.inner.heartbeat_handler.read().expect("handler lock poisoned").clone();
                        handler(response);
                    }
                    Some(Err(error)) => return Err(PdError::Grpc(error)),
                    None => return Err(PdError::Closed),
                },
                _ = tick => {},
            }
        }
    }

    /// 约 1 秒的可中断退避（100×10ms），关闭时提前退出。
    fn sleep_until_retry(&self) {
        for _ in 0..100 {
            if self.is_stopped() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// 客户端是否已关闭。
    fn is_stopped(&self) -> bool {
        self.inner.stopped.load(Ordering::Acquire)
    }

    /// 非阻塞通知 leader 检查循环执行刷新。
    fn schedule_update_leader(&self) {
        let _ = self.inner.check_leader_tx.try_send(());
    }

    /// 遍历已知端点 GetMembers，切换到可用 leader 并重排 URL。
    fn update_leader(&self) -> Result<pdpb::GetMembersResponse> {
        let urls = self.inner.urls.read().expect("urls lock poisoned").clone();
        for endpoint in urls {
            if self.is_stopped() {
                return Err(PdError::Closed);
            }
            let response = match self.get_members(&endpoint) {
                Ok(response) => response,
                Err(_) => continue,
            };
            let leader = match response.leader.as_ref() {
                Some(leader) if !leader.client_urls.is_empty() => leader,
                _ => continue,
            };
            *self.inner.urls.write().expect("urls lock poisoned") =
                ordered_member_urls(&response.members, leader);
            self.switch_leader(&leader.client_urls)?;
            return Ok(response);
        }
        Err(PdError::MissingLeader)
    }

    /// 向指定端点发起 GetMembers RPC。
    fn get_members(&self, endpoint: &str) -> Result<pdpb::GetMembersResponse> {
        let channel = self.get_or_create_channel(endpoint)?;
        let client = pdpb::PdClient::new(channel);
        client
            .get_members_opt(
                &pdpb::GetMembersRequest::default(),
                CallOption::default().timeout(PD_TIMEOUT),
            )
            .map_err(PdError::Grpc)
    }

    /// 切换当前 leader 连接；地址未变则直接返回。
    fn switch_leader(&self, addresses: &[String]) -> Result<()> {
        let new_leader = addresses.first().ok_or(PdError::MissingLeader)?.clone();
        if self
            .inner
            .connections
            .read()
            .expect("connections lock poisoned")
            .leader
            == new_leader
        {
            return Ok(());
        }
        self.get_or_create_channel(&new_leader)?;
        self.inner
            .connections
            .write()
            .expect("connections lock poisoned")
            .leader = new_leader;
        Ok(())
    }

    /// 按端点复用或新建 gRPC Channel。
    fn get_or_create_channel(&self, endpoint: &str) -> Result<Channel> {
        if let Some(channel) = self
            .inner
            .connections
            .read()
            .expect("connections lock poisoned")
            .clients
            .get(endpoint)
        {
            return Ok(channel.clone());
        }
        // 解析 URL，仅用 host:port 作为 ChannelBuilder 连接目标。
        let parsed =
            Url::parse(endpoint).map_err(|_| PdError::InvalidEndpoint(endpoint.to_owned()))?;
        if parsed.host_str().is_none() {
            return Err(PdError::InvalidEndpoint(endpoint.to_owned()));
        }
        let target = parsed[Position::BeforeHost..Position::AfterPort].to_owned();
        if target.is_empty() {
            return Err(PdError::InvalidEndpoint(endpoint.to_owned()));
        }
        let channel = ChannelBuilder::new(Arc::clone(&self.inner.environment)).connect(&target);
        let mut state = self
            .inner
            .connections
            .write()
            .expect("connections lock poisoned");
        Ok(state
            .clients
            .entry(endpoint.to_owned())
            .or_insert_with(|| channel.clone())
            .clone())
    }

    /// 获取指向当前 leader 的 protobuf PD 客户端。
    fn leader_client(&self) -> Result<pdpb::PdClient> {
        let state = self
            .inner
            .connections
            .read()
            .expect("connections lock poisoned");
        let channel = state
            .clients
            .get(&state.leader)
            .cloned()
            .ok_or(PdError::MissingLeader)?;
        Ok(pdpb::PdClient::new(channel))
    }

    /// 对 leader 发起带超时与重试的一元 RPC；失败时调度刷新 leader。
    fn do_request<T, F>(&self, mut operation: F) -> Result<T>
    where
        F: FnMut(&pdpb::PdClient, CallOption) -> grpcio::Result<T>,
    {
        retry_with_policy(
            MAX_RETRY_COUNT,
            RETRY_INTERVAL,
            || {
                if self.is_stopped() {
                    return Err(PdError::Closed);
                }
                let client = self.leader_client()?;
                operation(&client, CallOption::default().timeout(PD_TIMEOUT)).map_err(PdError::Grpc)
            },
            |_| self.schedule_update_leader(),
        )
    }

    /// 构造带当前 cluster_id 的请求头。
    fn request_header(&self) -> pdpb::RequestHeader {
        pdpb::RequestHeader {
            cluster_id: self.get_cluster_id(),
            ..Default::default()
        }
    }

    /// 获取全部非 Tombstone Store。
    pub fn get_all_stores(&self) -> Result<Vec<metapb::Store>> {
        let request = pdpb::GetAllStoresRequest {
            header: Some(self.request_header()).into(),
            exclude_tombstone_stores: true,
            ..Default::default()
        };
        let response =
            self.do_request(|client, option| client.get_all_stores_opt(&request, option))?;
        check_optional_header(response.header.as_ref())?;
        Ok(response.stores.into_vec())
    }

    /// 获取集群配置元数据。
    pub fn get_cluster_config(&self) -> Result<Option<metapb::Cluster>> {
        let request = pdpb::GetClusterConfigRequest {
            header: Some(self.request_header()).into(),
            ..Default::default()
        };
        let response =
            self.do_request(|client, option| client.get_cluster_config_opt(&request, option))?;
        check_optional_header(response.header.as_ref())?;
        Ok(response.cluster.into_option())
    }
}

impl Client for PdClient {
    /// 返回缓存的集群 ID。
    fn get_cluster_id(&self) -> u64 {
        self.inner.cluster_id.load(Ordering::Acquire)
    }

    /// 向 PD 分配全局唯一 ID。
    fn alloc_id(&self) -> Result<u64> {
        let request = pdpb::AllocIdRequest {
            header: Some(self.request_header()).into(),
            ..Default::default()
        };
        Ok(self
            .do_request(|client, option| client.alloc_id_opt(&request, option))?
            .id)
    }

    fn bootstrap(
        &self,
        store: metapb::Store,
        region: metapb::Region,
    ) -> Result<pdpb::BootstrapResponse> {
        let request = pdpb::BootstrapRequest {
            header: Some(self.request_header()).into(),
            store: Some(store).into(),
            region: Some(region).into(),
            ..Default::default()
        };
        self.do_request(|client, option| client.bootstrap_opt(&request, option))
    }

    /// 查询集群是否已 Bootstrap。
    fn is_bootstrapped(&self) -> Result<bool> {
        let request = pdpb::IsBootstrappedRequest {
            header: Some(self.request_header()).into(),
            ..Default::default()
        };
        let response =
            self.do_request(|client, option| client.is_bootstrapped_opt(&request, option))?;
        check_optional_header(response.header.as_ref())?;
        Ok(response.bootstrapped)
    }

    /// 注册或更新 Store 元数据。
    fn put_store(&self, store: metapb::Store) -> Result<()> {
        let request = pdpb::PutStoreRequest {
            header: Some(self.request_header()).into(),
            store: Some(store).into(),
            ..Default::default()
        };
        let response = self.do_request(|client, option| client.put_store_opt(&request, option))?;
        check_optional_header(response.header.as_ref())
    }

    /// 按 ID 查询 Store。
    fn get_store(&self, store_id: u64) -> Result<Option<metapb::Store>> {
        let request = pdpb::GetStoreRequest {
            header: Some(self.request_header()).into(),
            store_id,
            ..Default::default()
        };
        let response = self.do_request(|client, option| client.get_store_opt(&request, option))?;
        check_optional_header(response.header.as_ref())?;
        Ok(response.store.into_option())
    }

    /// 按 key 定位所属 Region（含 leader 与异常 peer）。
    fn get_region(&self, key: Vec<u8>) -> Result<Region> {
        let request = pdpb::GetRegionRequest {
            header: Some(self.request_header()).into(),
            region_key: key,
            ..Default::default()
        };
        let response = self.do_request(|client, option| client.get_region_opt(&request, option))?;
        check_optional_header(response.header.as_ref())?;
        Ok(response_region(response))
    }

    /// 按 Region ID 查询。
    fn get_region_by_id(&self, region_id: u64) -> Result<Region> {
        let request = pdpb::GetRegionByIdRequest {
            header: Some(self.request_header()).into(),
            region_id,
            ..Default::default()
        };
        let response =
            self.do_request(|client, option| client.get_region_by_id_opt(&request, option))?;
        check_optional_header(response.header.as_ref())?;
        Ok(response_region(response))
    }

    /// 将 Region 心跳请求投入发送队列。
    fn report_region(&self, request: pdpb::RegionHeartbeatRequest) -> Result<()> {
        self.inner
            .region_tx
            .send(request)
            .map_err(|_| PdError::Closed)
    }

    /// 请求 PD 为 Region 分配分裂 ID。
    fn ask_split(&self, region: metapb::Region) -> Result<pdpb::AskSplitResponse> {
        let request = pdpb::AskSplitRequest {
            header: Some(self.request_header()).into(),
            region: Some(region).into(),
            ..Default::default()
        };
        let response = self.do_request(|client, option| client.ask_split_opt(&request, option))?;
        check_optional_header(response.header.as_ref())?;
        Ok(response)
    }

    fn ask_batch_split(
        &self,
        region: metapb::Region,
        count: i32,
    ) -> Result<pdpb::AskBatchSplitResponse> {
        let request = pdpb::AskBatchSplitRequest {
            header: Some(self.request_header()).into(),
            region: Some(region).into(),
            split_count: count as u32,
            ..Default::default()
        };
        let response =
            self.do_request(|client, option| client.ask_batch_split_opt(&request, option))?;
        check_optional_header(response.header.as_ref())?;
        Ok(response)
    }

    /// 向 PD 汇报批量分裂结果。
    fn report_batch_split(&self, regions: Vec<metapb::Region>) -> Result<()> {
        let request = pdpb::ReportBatchSplitRequest {
            header: Some(self.request_header()).into(),
            regions: regions.into(),
            ..Default::default()
        };
        let response =
            self.do_request(|client, option| client.report_batch_split_opt(&request, option))?;
        check_optional_header(response.header.as_ref())
    }

    /// 获取 GC safe point（可安全回收的最大时间戳）。
    fn get_gc_safe_point(&self) -> Result<u64> {
        let request = pdpb::GetGcSafePointRequest {
            header: Some(self.request_header()).into(),
            ..Default::default()
        };
        let response =
            self.do_request(|client, option| client.get_gc_safe_point_opt(&request, option))?;
        check_optional_header(response.header.as_ref())?;
        Ok(response.safe_point)
    }

    /// 上报 Store 心跳统计。
    fn store_heartbeat(&self, stats: pdpb::StoreStats) -> Result<()> {
        let request = pdpb::StoreHeartbeatRequest {
            header: Some(self.request_header()).into(),
            stats: Some(stats).into(),
            ..Default::default()
        };
        let response =
            self.do_request(|client, option| client.store_heartbeat_opt(&request, option))?;
        check_optional_header(response.header.as_ref())
    }

    /// 通过 TSO 双向流申请一个时间戳，返回 (physical, logical)。
    fn get_ts(&self) -> Result<(i64, i64)> {
        let client = self.clone();
        let response = self.do_request(|rpc, option| {
            let (mut sender, mut receiver) = rpc.tso_opt(option)?;
            let request = pdpb::TsoRequest {
                header: Some(client.request_header()).into(),
                count: 1,
                ..Default::default()
            };
            futures::executor::block_on(async move {
                sender.send((request, WriteFlags::default())).await?;
                sender.close().await?;
                receiver
                    .next()
                    .await
                    .ok_or_else(|| grpcio::Error::RemoteStopped)??
                    .pipe(Ok)
            })
        })?;
        check_optional_header(response.header.as_ref())?;
        let timestamp = response
            .timestamp
            .into_option()
            .ok_or(PdError::MissingTimestamp)?;
        Ok((timestamp.physical, timestamp.logical))
    }

    /// 设置 Region 心跳响应回调；`None` 时恢复为空实现。
    fn set_region_heartbeat_response_handler(&self, handler: Option<HeartbeatHandler>) {
        *self
            .inner
            .heartbeat_handler
            .write()
            .expect("handler lock poisoned") = handler.unwrap_or_else(|| Arc::new(|_| {}));
    }

    /// 关闭客户端：停止后台线程并清空连接缓存。
    fn close(&self) {
        if self.inner.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        let _ = self.inner.check_leader_tx.try_send(());
        let handles =
            std::mem::take(&mut *self.inner.workers.lock().expect("workers lock poisoned"));
        for handle in handles {
            let _ = handle.join();
        }
        self.inner
            .connections
            .write()
            .expect("connections lock poisoned")
            .clients
            .clear();
    }
}

/// Compatibility constructor for legacy Go callers.
/// Go 兼容构造函数名。
#[allow(non_snake_case)]
pub fn NewClient(pd_addresses: Vec<String>, tag: String) -> Result<PdClient> {
    PdClient::new(pd_addresses, tag)
}

/// 小工具：把值管道传给闭包（便于 async 块内链式处理）。
trait Pipe: Sized {
    fn pipe<T>(self, function: impl FnOnce(Self) -> T) -> T {
        function(self)
    }
}

impl<T> Pipe for T {}
