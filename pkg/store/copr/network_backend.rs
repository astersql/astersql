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

//! client-rust 0.4.0 未公开的标准 TiDB DAG transport。
//!
//! 这里只补 PD Region/Store 元数据与 `tikvpb.Tikv` Coprocessor RPC。事务、TSO、
//! SafePoint 继续由 client-rust 承担；路由切分与重试继续复用本 crate 的
//! `RegionCache`/`CopClient`，本模块不维护第二份 Region 缓存。

use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_executor::block_on;
use futures_util::StreamExt;
use grpcio::{CallOption, Channel, ChannelBuilder, ChannelCredentialsBuilder, Environment};
use kvproto::{
    coprocessor as coprocessorpb, errorpb, keyspacepb, keyspacepb_grpc, kvrpcpb, metapb, pdpb,
    pdpb_grpc, tikvpb_grpc,
};
use protobuf::Message;

use crate::batch_request_sender::{
    BatchError, BatchRequest, BatchResult, CancellationToken, KeyRange, Peer, RegionMeta,
    RegionVerId, RpcContext, RpcResponse, Store as RegionStore,
};
use crate::coprocessor::{
    CopProtocolResponse, CopTask, CopWireRequest, IsolationLevel, Priority, ReplicaReadType,
    RequestType,
};
use crate::mpp::{
    MppCancelRequest, MppConnectionRequest, MppDispatchResponse, MppDispatchWireRequest, MppStream,
};
use crate::region_cache::{Buckets, KeyLocation, RegionCacheBackend};
use crate::store::{ClientEventListener, StoreBackend};

const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(60);

/// PD/TiKV gRPC 的 TLS 文件配置。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkSecurity {
    pub ca_path: String,
    pub cert_path: String,
    pub key_path: String,
}

/// 标准 DAG transport 构造参数。
#[derive(Clone, Debug)]
pub struct NetworkConfig {
    pub pd_endpoints: Vec<String>,
    pub security: Option<NetworkSecurity>,
    pub keyspace_name: String,
    pub timeout: Duration,
}

impl NetworkConfig {
    pub fn new(pd_endpoints: Vec<String>) -> Self {
        Self {
            pd_endpoints,
            security: None,
            keyspace_name: String::new(),
            timeout: DEFAULT_RPC_TIMEOUT,
        }
    }
}

/// PD Keyspace 查询错误分类，供上层保持与 client-go 相同的重试判断。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PdKeyspaceErrorKind {
    NotBootstrapped,
    KeyspaceNotExist,
    Unexpected,
}

/// PD Keyspace 查询错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PdKeyspaceError {
    pub kind: PdKeyspaceErrorKind,
    pub message: String,
}

impl PdKeyspaceError {
    fn unexpected(message: impl Into<String>) -> Self {
        Self {
            kind: PdKeyspaceErrorKind::Unexpected,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PdKeyspaceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PdKeyspaceError {}

/// 复用标准 gRPC transport 的轻量 PD Keyspace 客户端。
pub struct NetworkPdKeyspaceClient {
    _environment: Arc<Environment>,
    clients: Vec<keyspacepb_grpc::KeyspaceClient>,
    cluster_id: u64,
    caller_id: String,
    timeout: Duration,
}

impl NetworkPdKeyspaceClient {
    /// 连接 PD、探测 cluster id，并保留所有给定端点用于后续查询。
    pub fn connect(
        endpoints: &[String],
        security: Option<&NetworkSecurity>,
        timeout: Duration,
        caller_id: &str,
    ) -> Result<Self, PdKeyspaceError> {
        if endpoints.is_empty() {
            return Err(PdKeyspaceError::unexpected(
                "at least one PD endpoint is required",
            ));
        }
        let environment = Arc::new(Environment::new(2));
        let channels = Arc::new(
            ChannelFactory::new(Arc::clone(&environment), security)
                .map_err(|error| PdKeyspaceError::unexpected(error.to_string()))?,
        );
        let pd_clients = endpoints
            .iter()
            .map(|endpoint| pdpb_grpc::PdClient::new(channels.connect(endpoint)))
            .collect::<Vec<_>>();
        let mut cluster_id = None;
        let mut last_error = None;
        for client in &pd_clients {
            let mut request = pdpb::GetMembersRequest::new();
            let mut header = pd_header(0);
            header.set_caller_id(caller_id.to_owned());
            request.set_header(header);
            match client.get_members_opt(&request, CallOption::default().timeout(timeout)) {
                Ok(response) => {
                    if let Some(error) = pd_keyspace_header_error(response.get_header()) {
                        last_error = Some(error);
                        continue;
                    }
                    let id = response.get_header().get_cluster_id();
                    if id != 0 {
                        cluster_id = Some(id);
                        break;
                    }
                    last_error = Some(PdKeyspaceError::unexpected("PD returned cluster_id=0"));
                }
                Err(error) => last_error = Some(PdKeyspaceError::unexpected(error.to_string())),
            }
        }
        let cluster_id = cluster_id.ok_or_else(|| {
            last_error
                .unwrap_or_else(|| PdKeyspaceError::unexpected("PD cluster failed to respond"))
        })?;
        let clients = endpoints
            .iter()
            .map(|endpoint| keyspacepb_grpc::KeyspaceClient::new(channels.connect(endpoint)))
            .collect();
        Ok(Self {
            _environment: environment,
            clients,
            cluster_id,
            caller_id: caller_id.to_owned(),
            timeout,
        })
    }

    /// 按名称加载 Keyspace 元数据。
    pub fn load_keyspace(&self, name: &str) -> Result<u32, PdKeyspaceError> {
        let mut request = keyspacepb::LoadKeyspaceRequest::new();
        let mut header = pd_header(self.cluster_id);
        header.set_caller_id(self.caller_id.clone());
        request.set_header(header);
        request.set_name(name.to_owned());
        let mut last_error = None;
        for client in &self.clients {
            match client.load_keyspace_opt(&request, CallOption::default().timeout(self.timeout)) {
                Ok(mut response) => {
                    if let Some(error) = pd_keyspace_header_error(response.get_header()) {
                        return Err(error);
                    }
                    if response.has_keyspace() {
                        return Ok(response.take_keyspace().get_id());
                    }
                    return Err(PdKeyspaceError::unexpected(
                        "PD returned no keyspace metadata",
                    ));
                }
                Err(error) => last_error = Some(PdKeyspaceError::unexpected(error.to_string())),
            }
        }
        Err(last_error.unwrap_or_else(|| PdKeyspaceError::unexpected("PD has no endpoints")))
    }
}

fn pd_keyspace_header_error(header: &pdpb::ResponseHeader) -> Option<PdKeyspaceError> {
    if !header.has_error() {
        return None;
    }
    let error = header.get_error();
    let kind = match error.get_type() {
        pdpb::ErrorType::NotBootstrapped => PdKeyspaceErrorKind::NotBootstrapped,
        pdpb::ErrorType::EntryNotFound => PdKeyspaceErrorKind::KeyspaceNotExist,
        _ => PdKeyspaceErrorKind::Unexpected,
    };
    Some(PdKeyspaceError {
        kind,
        message: error.get_message().to_owned(),
    })
}

/// PD 元数据边界。测试可在这里稳定模拟 leader/epoch 变化。
pub trait RegionMetadataTransport: Send + Sync + 'static {
    fn batch_locate_key_ranges(
        &self,
        ranges: &[KeyRange],
        need_leader: bool,
        need_buckets: bool,
    ) -> BatchResult<Vec<KeyLocation>>;
    fn locate_key(&self, key: &[u8]) -> BatchResult<KeyLocation>;
    fn locate_end_key(&self, key: &[u8]) -> BatchResult<KeyLocation>;
    fn locate_region_by_id(&self, region_id: u64) -> BatchResult<KeyLocation>;
    fn read_replicas(&self, region_id: u64) -> BatchResult<Vec<KeyLocation>> {
        self.locate_region_by_id(region_id)
            .map(|location| vec![location])
    }

    fn invalidate_region(&self, region: RegionVerId);
    fn is_store_alive(&self, address: &str, ttl: Duration) -> bool;
}

/// 已补齐 Region context 的标准 Coprocessor 请求。
#[derive(Clone, Debug)]
pub struct StandardCoprocessorRequest {
    pub address: String,
    pub region: RegionVerId,
    pub peer: Option<Peer>,
    pub wire: CopWireRequest,
}

/// 标准 server-streaming RPC 的按包读取/关闭边界。
pub trait CoprocessorResponseStream: Send {
    fn next(&mut self) -> BatchResult<Option<CopProtocolResponse>>;
    fn close(&mut self) -> BatchResult<()>;
}

/// TiKV 标准 Coprocessor unary/stream RPC 边界。
pub trait StandardCoprocessorTransport: Send + Sync + 'static {
    fn send_unary(
        &self,
        request: &StandardCoprocessorRequest,
        timeout: Duration,
    ) -> BatchResult<CopProtocolResponse>;
    fn send_stream(
        &self,
        request: &StandardCoprocessorRequest,
        timeout: Duration,
    ) -> BatchResult<Box<dyn CoprocessorResponseStream>>;
    fn close(&self) -> BatchResult<()>;
    fn close_address(&self, address: &str) -> BatchResult<()>;
}

/// client-go `txnlock.Lock` 在标准 Coprocessor transport 上需要的完整字段。
///
/// `tikv-client` 与本 crate 使用不同 protobuf runtime，因此在这里保留一个
/// runtime-neutral 边界；生产适配器再无损转换为 client-rust 的 `ProtoLockInfo`。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TransactionLock {
    pub primary_lock: Vec<u8>,
    pub lock_version: u64,
    pub key: Vec<u8>,
    pub lock_ttl: u64,
    pub txn_size: u64,
    pub lock_type: i32,
    pub lock_for_update_ts: u64,
    pub use_async_commit: bool,
    pub min_commit_ts: u64,
    pub secondaries: Vec<Vec<u8>>,
    pub duration_to_last_update_ms: u64,
    pub is_txn_file: bool,
}

impl From<&kvrpcpb::LockInfo> for TransactionLock {
    fn from(lock: &kvrpcpb::LockInfo) -> Self {
        Self {
            primary_lock: lock.get_primary_lock().to_vec(),
            lock_version: lock.get_lock_version(),
            key: lock.get_key().to_vec(),
            lock_ttl: lock.get_lock_ttl(),
            txn_size: lock.get_txn_size(),
            lock_type: lock.get_lock_type() as i32,
            lock_for_update_ts: lock.get_lock_for_update_ts(),
            use_async_commit: lock.get_use_async_commit(),
            min_commit_ts: lock.get_min_commit_ts(),
            secondaries: lock.get_secondaries().to_vec(),
            duration_to_last_update_ms: lock.get_duration_to_last_update_ms(),
            is_txn_file: lock.get_is_txn_file(),
        }
    }
}

/// Go `ResolveLocksWithOpts` 的同步 transport 边界。
pub trait TransactionLockResolver: Send + Sync + 'static {
    fn resolve_locks(&self, locks: &[TransactionLock], caller_start_ts: u64) -> BatchResult<()>;
}

/// 复用 AsterSQL `RegionCache` 与 `CopClient` 的生产网络后端。
pub struct NetworkBackend {
    metadata: Arc<dyn RegionMetadataTransport>,
    coprocessor: Arc<dyn StandardCoprocessorTransport>,
    lock_resolver: Arc<dyn TransactionLockResolver>,
    listener: Mutex<Option<Arc<dyn ClientEventListener>>>,
    closed: AtomicBool,
}

impl NetworkBackend {
    /// 建立 PD 元数据与 TiKV Coprocessor transport；不构造事务客户端。
    pub fn connect(
        config: NetworkConfig,
        lock_resolver: Arc<dyn TransactionLockResolver>,
    ) -> BatchResult<Self> {
        let environment = Arc::new(Environment::new(2));
        let channel_factory = Arc::new(ChannelFactory::new(
            Arc::clone(&environment),
            config.security.as_ref(),
        )?);
        let metadata = Arc::new(GrpcRegionMetadataTransport::connect(
            &config.pd_endpoints,
            Arc::clone(&channel_factory),
            config.timeout,
            &config.keyspace_name,
        )?);
        let coprocessor = Arc::new(GrpcStandardCoprocessorTransport::new(
            environment,
            channel_factory,
            Arc::clone(&metadata.codec),
        ));
        Ok(Self::from_transports(metadata, coprocessor, lock_resolver))
    }

    /// 由可注入 RPC 边界构造，供单元测试固定 Region 与流行为。
    pub fn from_transports(
        metadata: Arc<dyn RegionMetadataTransport>,
        coprocessor: Arc<dyn StandardCoprocessorTransport>,
        lock_resolver: Arc<dyn TransactionLockResolver>,
    ) -> Self {
        Self {
            metadata,
            coprocessor,
            lock_resolver,
            listener: Mutex::new(None),
            closed: AtomicBool::new(false),
        }
    }

    fn located_request(
        &self,
        task: &CopTask,
        wire: &CopWireRequest,
    ) -> BatchResult<StandardCoprocessorRequest> {
        if self.closed.load(Ordering::Acquire) {
            return Err(BatchError::Closed);
        }
        let location = self.metadata.locate_region_by_id(task.region.id)?;
        if location.region != task.region {
            self.metadata.invalidate_region(task.region);
        }
        let address = location
            .store
            .as_ref()
            .map(|store| store.address.clone())
            .filter(|address| !address.is_empty())
            .ok_or_else(|| BatchError::MissingRegion(task.region))?;
        Ok(StandardCoprocessorRequest {
            address,
            region: location.region,
            peer: location.peer,
            wire: wire.clone(),
        })
    }

    fn timeout_for(task: &CopTask) -> Duration {
        if task.client_read_timeout.is_zero() {
            DEFAULT_RPC_TIMEOUT
        } else {
            task.client_read_timeout
        }
    }

    /// 打开 TiKV 标准 server-streaming Coprocessor RPC。
    pub fn send_coprocessor_stream(
        &self,
        task: &CopTask,
        wire: &CopWireRequest,
    ) -> BatchResult<Box<dyn CoprocessorResponseStream>> {
        let request = self.located_request(task, wire)?;
        self.coprocessor
            .send_stream(&request, Self::timeout_for(task))
    }
}

impl RegionCacheBackend for NetworkBackend {
    fn batch_locate_key_ranges(
        &self,
        ranges: &[KeyRange],
        need_leader: bool,
        need_buckets: bool,
    ) -> BatchResult<Vec<KeyLocation>> {
        self.metadata
            .batch_locate_key_ranges(ranges, need_leader, need_buckets)
    }

    fn locate_key(&self, key: &[u8]) -> BatchResult<KeyLocation> {
        self.metadata.locate_key(key)
    }

    fn locate_end_key(&self, key: &[u8]) -> BatchResult<KeyLocation> {
        self.metadata.locate_end_key(key)
    }

    fn locate_region_from_pd(&self, region_id: u64) -> BatchResult<KeyLocation> {
        self.metadata.locate_region_by_id(region_id)
    }

    fn invalidate_region(&self, region: RegionVerId) {
        self.metadata.invalidate_region(region);
    }

    fn update_buckets(&self, region: RegionVerId, _old_version: u64, _new_version: u64) {
        self.metadata.invalidate_region(region);
    }

    fn on_send_fail_tiflash(
        &self,
        _store: &RegionStore,
        region: RegionVerId,
        _meta: &RegionMeta,
        _schedule_reload: bool,
        _error: &BatchError,
    ) {
        self.metadata.invalidate_region(region);
    }

    fn tikv_rpc_context(
        &self,
        region: RegionVerId,
        _replica_read: ReplicaReadType,
    ) -> BatchResult<Option<RpcContext>> {
        self.metadata
            .locate_region_by_id(region.id)
            .map(|location| Some(location_to_rpc_context(location)))
    }

    fn tiflash_rpc_context(
        &self,
        _region: RegionVerId,
        _is_mpp: bool,
    ) -> BatchResult<Option<RpcContext>> {
        Ok(None)
    }

    fn all_valid_tiflash_store_ids(
        &self,
        _region: RegionVerId,
        _primary_store_id: u64,
    ) -> Vec<u64> {
        Vec::new()
    }

    fn all_tiflash_stores(&self) -> Vec<RegionStore> {
        Vec::new()
    }

    fn compute_stores(&self) -> BatchResult<Vec<RegionStore>> {
        Ok(Vec::new())
    }

    fn fetch_topology(&self) -> BatchResult<Vec<String>> {
        Ok(Vec::new())
    }

    fn is_store_alive(&self, address: &str, ttl: Duration) -> bool {
        self.metadata.is_store_alive(address, ttl)
    }

    fn tidb_server_addresses(&self) -> BatchResult<Vec<(u64, String)>> {
        Ok(Vec::new())
    }
}

impl StoreBackend for NetworkBackend {
    fn region_backend(&self) -> Arc<dyn RegionCacheBackend> {
        Arc::new(NetworkRegionBackend {
            metadata: Arc::clone(&self.metadata),
        })
    }

    fn close_client(&self) -> BatchResult<()> {
        if !self.closed.swap(true, Ordering::AcqRel) {
            self.coprocessor.close()?;
        }
        Ok(())
    }

    fn close_address(&self, address: &str) -> BatchResult<()> {
        self.coprocessor.close_address(address)
    }

    fn send_request(
        &self,
        _address: &str,
        _request: &BatchRequest,
        _timeout: Duration,
        _cancellation: &CancellationToken,
    ) -> BatchResult<RpcResponse> {
        Err(BatchError::OtherResponse(
            "standard DAG backend does not provide batch-coprocessor transport".to_owned(),
        ))
    }

    fn set_event_listener(&self, listener: Option<Arc<dyn ClientEventListener>>) {
        *self
            .listener
            .lock()
            .expect("network listener lock poisoned") = listener;
    }

    fn send_coprocessor(
        &self,
        task: &CopTask,
        wire: &CopWireRequest,
    ) -> BatchResult<CopProtocolResponse> {
        let request = self.located_request(task, wire)?;
        let timed_out = |error: &BatchError| matches!(error, BatchError::Transport(message) if message.to_ascii_lowercase().replace('_', "").contains("deadlineexceeded"));
        let send = |request: &StandardCoprocessorRequest, timeout| {
            let started = std::time::Instant::now();
            let result = self.coprocessor.send_unary(request, timeout);
            if let Some(stats) = &task.read_stats {
                stats.record_attempt(
                    "cop",
                    request.peer.as_ref().map_or(0, |peer| peer.store_id),
                    started.elapsed(),
                    result.as_ref().err().is_some_and(&timed_out),
                );
            }
            result
        };
        let response = if task.client_read_timeout.is_zero() {
            send(&request, DEFAULT_RPC_TIMEOUT)
        } else {
            let mut response = None;
            for location in self.metadata.read_replicas(task.region.id)? {
                let mut attempt = request.clone();
                attempt.address = location
                    .store
                    .as_ref()
                    .map(|store| store.address.clone())
                    .ok_or(BatchError::MissingRegion(task.region))?;
                attempt.peer = location.peer;
                if attempt.peer != request.peer {
                    attempt.wire.replica_read = ReplicaReadType::Follower;
                }
                let result = send(&attempt, task.client_read_timeout);
                if result.as_ref().err().is_some_and(&timed_out) {
                    continue;
                }
                response = Some(result);
                break;
            }
            response.unwrap_or_else(|| send(&request, DEFAULT_RPC_TIMEOUT))
        };
        match &response {
            Ok(response) if response.region_error.is_some() => {
                self.metadata.invalidate_region(task.region);
            }
            Err(_) => {
                self.metadata.invalidate_region(task.region);
            }
            _ => {}
        }
        response
    }

    fn send_coprocessor_stream(
        &self,
        task: &CopTask,
        wire: &CopWireRequest,
    ) -> BatchResult<Box<dyn CoprocessorResponseStream>> {
        NetworkBackend::send_coprocessor_stream(self, task, wire)
    }

    fn resolve_lock(&self, lock: &[u8], start_ts: u64) -> BatchResult<()> {
        let lock: kvrpcpb::LockInfo = protobuf::parse_from_bytes(lock)
            .map_err(|error| BatchError::OtherResponse(format!("decode TiKV lock: {error}")))?;
        // 对齐 Go handleLockErr：SharedLock wrapper 本身不描述事务，必须逐个处理
        // shared_lock_infos；普通锁则处理 wrapper 自身。
        let locks = if lock.get_shared_lock_infos().is_empty() {
            vec![TransactionLock::from(&lock)]
        } else {
            lock.get_shared_lock_infos()
                .iter()
                .map(TransactionLock::from)
                .collect()
        };
        self.lock_resolver.resolve_locks(&locks, start_ts)
    }

    fn check_visibility(&self, _start_ts: u64) -> BatchResult<()> {
        Ok(())
    }

    fn dispatch_mpp(
        &self,
        _address: &str,
        _request: &MppDispatchWireRequest,
        _timeout: Duration,
    ) -> BatchResult<MppDispatchResponse> {
        Err(BatchError::OtherResponse(
            "standard DAG backend does not provide MPP".to_owned(),
        ))
    }

    fn cancel_mpp(
        &self,
        _address: &str,
        _request: &MppCancelRequest,
        _timeout: Duration,
    ) -> BatchResult<()> {
        Err(BatchError::OtherResponse(
            "standard DAG backend does not provide MPP".to_owned(),
        ))
    }

    fn establish_mpp(
        &self,
        _address: &str,
        _request: &MppConnectionRequest,
        _timeout: Duration,
    ) -> BatchResult<MppStream> {
        Err(BatchError::OtherResponse(
            "standard DAG backend does not provide MPP".to_owned(),
        ))
    }

    fn invalidate_compute_stores(&self) {}
}

struct NetworkRegionBackend {
    metadata: Arc<dyn RegionMetadataTransport>,
}

impl RegionCacheBackend for NetworkRegionBackend {
    fn batch_locate_key_ranges(
        &self,
        ranges: &[KeyRange],
        need_leader: bool,
        need_buckets: bool,
    ) -> BatchResult<Vec<KeyLocation>> {
        self.metadata
            .batch_locate_key_ranges(ranges, need_leader, need_buckets)
    }

    fn locate_key(&self, key: &[u8]) -> BatchResult<KeyLocation> {
        self.metadata.locate_key(key)
    }

    fn locate_end_key(&self, key: &[u8]) -> BatchResult<KeyLocation> {
        self.metadata.locate_end_key(key)
    }

    fn locate_region_from_pd(&self, region_id: u64) -> BatchResult<KeyLocation> {
        self.metadata.locate_region_by_id(region_id)
    }

    fn invalidate_region(&self, region: RegionVerId) {
        self.metadata.invalidate_region(region);
    }

    fn update_buckets(&self, region: RegionVerId, _old_version: u64, _new_version: u64) {
        self.metadata.invalidate_region(region);
    }

    fn on_send_fail_tiflash(
        &self,
        _store: &RegionStore,
        region: RegionVerId,
        _meta: &RegionMeta,
        _schedule_reload: bool,
        _error: &BatchError,
    ) {
        self.metadata.invalidate_region(region);
    }

    fn tikv_rpc_context(
        &self,
        region: RegionVerId,
        _replica_read: ReplicaReadType,
    ) -> BatchResult<Option<RpcContext>> {
        self.metadata
            .locate_region_by_id(region.id)
            .map(|location| Some(location_to_rpc_context(location)))
    }

    fn tiflash_rpc_context(
        &self,
        _region: RegionVerId,
        _is_mpp: bool,
    ) -> BatchResult<Option<RpcContext>> {
        Ok(None)
    }

    fn all_valid_tiflash_store_ids(
        &self,
        _region: RegionVerId,
        _primary_store_id: u64,
    ) -> Vec<u64> {
        Vec::new()
    }

    fn all_tiflash_stores(&self) -> Vec<RegionStore> {
        Vec::new()
    }

    fn compute_stores(&self) -> BatchResult<Vec<RegionStore>> {
        Ok(Vec::new())
    }

    fn fetch_topology(&self) -> BatchResult<Vec<String>> {
        Ok(Vec::new())
    }

    fn is_store_alive(&self, address: &str, ttl: Duration) -> bool {
        self.metadata.is_store_alive(address, ttl)
    }

    fn tidb_server_addresses(&self) -> BatchResult<Vec<(u64, String)>> {
        Ok(Vec::new())
    }
}

fn location_to_rpc_context(location: KeyLocation) -> RpcContext {
    RpcContext {
        region: location.region,
        address: location
            .store
            .as_ref()
            .map(|store| store.address.clone())
            .unwrap_or_default(),
        meta: Some(RegionMeta {
            id: location.region.id,
            peers: location.peer.iter().map(|peer| peer.id).collect(),
        }),
        store: location.store,
        peer: location.peer,
    }
}

struct ChannelFactory {
    environment: Arc<Environment>,
    tls: Option<TlsMaterial>,
}

struct TlsMaterial {
    root: Vec<u8>,
    cert: Vec<u8>,
    key: Vec<u8>,
}

impl ChannelFactory {
    fn new(environment: Arc<Environment>, security: Option<&NetworkSecurity>) -> BatchResult<Self> {
        let tls = security
            .map(|security| {
                let root = fs::read(&security.ca_path).map_err(transport_error)?;
                let cert = fs::read(&security.cert_path).map_err(transport_error)?;
                let key = fs::read(&security.key_path).map_err(transport_error)?;
                Ok(TlsMaterial { root, cert, key })
            })
            .transpose()?;
        Ok(Self { environment, tls })
    }

    fn connect(&self, endpoint: &str) -> Channel {
        let endpoint = normalize_endpoint(endpoint);
        let builder = ChannelBuilder::new(Arc::clone(&self.environment));
        match &self.tls {
            Some(tls) => builder
                .set_credentials(
                    ChannelCredentialsBuilder::new()
                        .root_cert(tls.root.clone())
                        .cert(tls.cert.clone(), tls.key.clone())
                        .build(),
                )
                .connect(&endpoint),
            None => builder.connect(&endpoint),
        }
    }
}

fn normalize_endpoint(endpoint: &str) -> String {
    endpoint
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .to_owned()
}

fn transport_error(error: impl ToString) -> BatchError {
    BatchError::Transport(error.to_string())
}

fn pd_header(cluster_id: u64) -> pdpb::RequestHeader {
    let mut header = pdpb::RequestHeader::new();
    header.set_cluster_id(cluster_id);
    header.set_caller_id("astersql-standard-dag".to_owned());
    header
}

fn check_pd_header(header: &pdpb::ResponseHeader) -> BatchResult<()> {
    if header.has_error() {
        return Err(BatchError::Transport(
            header.get_error().get_message().to_owned(),
        ));
    }
    Ok(())
}

const MEM_GROUP_SIZE: usize = 8;
const MEM_MARKER: u8 = 0xff;
const TXN_MODE_PREFIX: u8 = b'x';

/// client-go Txn codec 的最小 transport 侧镜像：Region key 使用 memcomparable
/// 编码，API V2 额外添加 keyspace 前缀；它只转换键，不保存任何 Region 状态。
#[derive(Debug)]
pub struct KeyCodec {
    keyspace_name: String,
    keyspace_id: Option<u32>,
    prefix: Vec<u8>,
    end_key: Vec<u8>,
}

impl KeyCodec {
    pub fn v1() -> Self {
        Self {
            keyspace_name: String::new(),
            keyspace_id: None,
            prefix: Vec::new(),
            end_key: Vec::new(),
        }
    }

    pub fn v2(keyspace_name: String, keyspace_id: u32) -> BatchResult<Self> {
        if keyspace_id > 0x00ff_ffff {
            return Err(BatchError::Transport(format!(
                "keyspace id {keyspace_id} does not fit in 24 bits"
            )));
        }
        let id = keyspace_id.to_be_bytes();
        let prefix = vec![TXN_MODE_PREFIX, id[1], id[2], id[3]];
        let mut end_key = (u32::from_be_bytes(prefix.clone().try_into().unwrap()) + 1)
            .to_be_bytes()
            .to_vec();
        if keyspace_id == 0x00ff_ffff {
            end_key = vec![TXN_MODE_PREFIX + 1, 0, 0, 0];
        }
        Ok(Self {
            keyspace_name,
            keyspace_id: Some(keyspace_id),
            prefix,
            end_key,
        })
    }

    pub fn encode_key(&self, key: &[u8]) -> Vec<u8> {
        if self.keyspace_id.is_none() {
            return key.to_vec();
        }
        let mut encoded = Vec::with_capacity(self.prefix.len() + key.len());
        encoded.extend_from_slice(&self.prefix);
        encoded.extend_from_slice(key);
        encoded
    }

    pub(crate) fn encode_end_region_key(&self, key: &[u8]) -> Vec<u8> {
        if !key.is_empty() {
            return mem_encode(&self.encode_key(key));
        }
        match self.keyspace_id {
            None => Vec::new(),
            Some(_) => mem_encode(&self.end_key),
        }
    }

    pub(crate) fn encode_range(&self, start: &[u8], end: &[u8]) -> (Vec<u8>, Vec<u8>) {
        if self.keyspace_id.is_none() {
            return (start.to_vec(), end.to_vec());
        }
        (
            self.encode_key(start),
            if end.is_empty() {
                self.end_key.clone()
            } else {
                self.encode_key(end)
            },
        )
    }

    pub(crate) fn decode_range(&self, start: &[u8], end: &[u8]) -> BatchResult<(Vec<u8>, Vec<u8>)> {
        if self.keyspace_id.is_none() {
            return Ok((start.to_vec(), end.to_vec()));
        }
        if start >= self.end_key.as_slice() || (!end.is_empty() && end <= self.prefix.as_slice()) {
            return Err(BatchError::Transport(
                "PD returned a Region outside the requested keyspace".to_owned(),
            ));
        }
        let start = start
            .strip_prefix(self.prefix.as_slice())
            .unwrap_or_default()
            .to_vec();
        let end = end
            .strip_prefix(self.prefix.as_slice())
            .unwrap_or_default()
            .to_vec();
        Ok((start, end))
    }

    fn decode_boundary(&self, key: &[u8]) -> BatchResult<Vec<u8>> {
        if self.keyspace_id.is_none() {
            return Ok(key.to_vec());
        }
        if key == self.end_key || key <= self.prefix.as_slice() {
            return Ok(Vec::new());
        }
        key.strip_prefix(self.prefix.as_slice())
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                BatchError::Transport("PD bucket key is outside the requested keyspace".to_owned())
            })
    }

    fn decode_transaction_key(&self, key: &[u8]) -> BatchResult<Vec<u8>> {
        if self.keyspace_id.is_none() {
            return Ok(key.to_vec());
        }
        key.strip_prefix(self.prefix.as_slice())
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                BatchError::Transport("TiKV lock key is outside the requested keyspace".to_owned())
            })
    }

    /// client-rust 的锁解析入口接收逻辑 key，并在发请求时自行编码 keyspace。
    /// 直连 Coprocessor 返回的是物理 key，因此这里与其 `TruncateKeyspace`
    /// 一样递归剥离 key、primary 与 async-commit secondaries。
    pub(crate) fn decode_lock_info(&self, lock: &mut kvrpcpb::LockInfo) -> BatchResult<()> {
        if !lock.get_shared_lock_infos().is_empty() {
            for shared in lock.mut_shared_lock_infos().iter_mut() {
                self.decode_lock_info(shared)?;
            }
            return Ok(());
        }
        lock.set_key(self.decode_transaction_key(lock.get_key())?);
        lock.set_primary_lock(self.decode_transaction_key(lock.get_primary_lock())?);
        let secondaries = lock
            .get_secondaries()
            .iter()
            .map(|secondary| self.decode_transaction_key(secondary))
            .collect::<BatchResult<Vec<_>>>()?;
        lock.set_secondaries(secondaries.into());
        Ok(())
    }

    pub(crate) fn encode_region_range(&self, start: &[u8], end: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let (start, end) = self.encode_range(start, end);
        (mem_encode(&start), mem_encode(&end))
    }

    pub(crate) fn decode_region_range(
        &self,
        encoded_start: &[u8],
        encoded_end: &[u8],
    ) -> BatchResult<(Vec<u8>, Vec<u8>)> {
        let start = if encoded_start.is_empty() {
            Vec::new()
        } else {
            mem_decode(encoded_start)?
        };
        let end = if encoded_end.is_empty() {
            Vec::new()
        } else {
            mem_decode(encoded_end)?
        };
        self.decode_range(&start, &end)
    }
}

fn mem_encode(key: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity((key.len() / MEM_GROUP_SIZE + 1) * 9);
    for offset in (0..=key.len()).step_by(MEM_GROUP_SIZE) {
        let remaining = key.len() - offset;
        let group_length = remaining.min(MEM_GROUP_SIZE);
        encoded.extend_from_slice(&key[offset..offset + group_length]);
        let padding = MEM_GROUP_SIZE - group_length;
        encoded.resize(encoded.len() + padding, 0);
        encoded.push(MEM_MARKER - padding as u8);
        if padding != 0 {
            break;
        }
    }
    encoded
}

fn mem_decode(encoded: &[u8]) -> BatchResult<Vec<u8>> {
    let mut decoded = Vec::with_capacity(encoded.len());
    let mut remaining = encoded;
    loop {
        if remaining.len() < MEM_GROUP_SIZE + 1 {
            return Err(BatchError::Transport(
                "invalid memcomparable Region key: incomplete group".to_owned(),
            ));
        }
        let group = &remaining[..MEM_GROUP_SIZE];
        let padding = MEM_MARKER.saturating_sub(remaining[MEM_GROUP_SIZE]) as usize;
        if padding > MEM_GROUP_SIZE
            || group[MEM_GROUP_SIZE - padding..]
                .iter()
                .any(|byte| *byte != 0)
        {
            return Err(BatchError::Transport(
                "invalid memcomparable Region key padding".to_owned(),
            ));
        }
        decoded.extend_from_slice(&group[..MEM_GROUP_SIZE - padding]);
        remaining = &remaining[MEM_GROUP_SIZE + 1..];
        if padding != 0 {
            if !remaining.is_empty() {
                return Err(BatchError::Transport(
                    "invalid memcomparable Region key trailing bytes".to_owned(),
                ));
            }
            return Ok(decoded);
        }
    }
}

struct GrpcRegionMetadataTransport {
    clients: Vec<pdpb_grpc::PdClient>,
    cluster_id: u64,
    timeout: Duration,
    codec: Arc<KeyCodec>,
}

impl GrpcRegionMetadataTransport {
    fn connect(
        endpoints: &[String],
        channels: Arc<ChannelFactory>,
        timeout: Duration,
        keyspace_name: &str,
    ) -> BatchResult<Self> {
        if endpoints.is_empty() {
            return Err(BatchError::Transport(
                "at least one PD endpoint is required".to_owned(),
            ));
        }
        let clients = endpoints
            .iter()
            .map(|endpoint| pdpb_grpc::PdClient::new(channels.connect(endpoint)))
            .collect::<Vec<_>>();
        let mut last_error = None;
        for client in &clients {
            let mut request = pdpb::GetMembersRequest::new();
            request.set_header(pd_header(0));
            match client.get_members_opt(&request, CallOption::default().timeout(timeout)) {
                Ok(response) => {
                    check_pd_header(response.get_header())?;
                    let cluster_id = response.get_header().get_cluster_id();
                    if cluster_id != 0 {
                        let codec = if keyspace_name.is_empty() {
                            Arc::new(KeyCodec::v1())
                        } else {
                            let keyspace_clients = endpoints
                                .iter()
                                .map(|endpoint| {
                                    keyspacepb_grpc::KeyspaceClient::new(channels.connect(endpoint))
                                })
                                .collect::<Vec<_>>();
                            let mut request = keyspacepb::LoadKeyspaceRequest::new();
                            request.set_header(pd_header(cluster_id));
                            request.set_name(keyspace_name.to_owned());
                            let mut keyspace_id = None;
                            let mut keyspace_error = None;
                            for keyspace_client in keyspace_clients {
                                match keyspace_client.load_keyspace_opt(
                                    &request,
                                    CallOption::default().timeout(timeout),
                                ) {
                                    Ok(mut response) => {
                                        check_pd_header(response.get_header())?;
                                        if response.has_keyspace() {
                                            keyspace_id = Some(response.take_keyspace().get_id());
                                            break;
                                        }
                                        keyspace_error =
                                            Some("PD returned no keyspace metadata".to_owned());
                                    }
                                    Err(error) => keyspace_error = Some(error.to_string()),
                                }
                            }
                            Arc::new(KeyCodec::v2(
                                keyspace_name.to_owned(),
                                keyspace_id.ok_or_else(|| {
                                    BatchError::Transport(keyspace_error.unwrap_or_else(|| {
                                        format!("keyspace {keyspace_name} was not found")
                                    }))
                                })?,
                            )?)
                        };
                        return Ok(Self {
                            clients,
                            cluster_id,
                            timeout,
                            codec,
                        });
                    }
                    last_error = Some("PD returned cluster_id=0".to_owned());
                }
                Err(error) => last_error = Some(error.to_string()),
            }
        }
        Err(BatchError::Transport(last_error.unwrap_or_else(|| {
            "PD cluster failed to respond".to_owned()
        })))
    }

    fn get_store(&self, store_id: u64) -> BatchResult<RegionStore> {
        let mut request = pdpb::GetStoreRequest::new();
        request.set_header(pd_header(self.cluster_id));
        request.set_store_id(store_id);
        let mut last_error = None;
        for client in &self.clients {
            match client.get_store_opt(&request, CallOption::default().timeout(self.timeout)) {
                Ok(mut response) => {
                    check_pd_header(response.get_header())?;
                    if !response.has_store() {
                        return Err(BatchError::Transport(format!(
                            "PD returned no store for id {store_id}"
                        )));
                    }
                    return Ok(pb_store(response.take_store()));
                }
                Err(error) => last_error = Some(error.to_string()),
            }
        }
        Err(BatchError::Transport(last_error.unwrap_or_else(|| {
            format!("PD get_store({store_id}) failed")
        })))
    }

    fn location_from_response(
        &self,
        mut response: pdpb::GetRegionResponse,
    ) -> BatchResult<KeyLocation> {
        check_pd_header(response.get_header())?;
        if !response.has_region() {
            return Err(BatchError::Transport(
                "PD returned an empty Region".to_owned(),
            ));
        }
        let region = response.take_region();
        let leader = response.take_leader();
        let buckets = if response.has_buckets() {
            let buckets = response.take_buckets();
            Some(Buckets {
                version: buckets.get_version(),
                keys: buckets.get_keys().to_vec(),
            })
        } else {
            None
        };
        self.location(region, leader, buckets)
    }

    fn location(
        &self,
        region: metapb::Region,
        leader: metapb::Peer,
        mut buckets: Option<Buckets>,
    ) -> BatchResult<KeyLocation> {
        let epoch = region.get_region_epoch();
        let region_id = region.get_id();
        let peer = pb_peer(&leader);
        if peer.id == 0 || peer.store_id == 0 {
            return Err(BatchError::MissingRegion(RegionVerId::new(
                region_id,
                epoch.get_conf_ver(),
                epoch.get_version(),
            )));
        }
        let store = self.get_store(peer.store_id)?;
        let (start_key, end_key) = self
            .codec
            .decode_region_range(region.get_start_key(), region.get_end_key())?;
        if let Some(buckets) = &mut buckets {
            buckets.keys = buckets
                .keys
                .iter()
                .map(|key| {
                    if key.is_empty() {
                        Ok(Vec::new())
                    } else {
                        let decoded = mem_decode(key)?;
                        self.codec.decode_boundary(&decoded)
                    }
                })
                .collect::<BatchResult<Vec<_>>>()?;
        }
        Ok(KeyLocation {
            region: RegionVerId::new(region_id, epoch.get_conf_ver(), epoch.get_version()),
            start_key,
            end_key,
            buckets,
            store: Some(store),
            peer: Some(peer),
        })
    }
}

impl RegionMetadataTransport for GrpcRegionMetadataTransport {
    fn batch_locate_key_ranges(
        &self,
        ranges: &[KeyRange],
        _need_leader: bool,
        _need_buckets: bool,
    ) -> BatchResult<Vec<KeyLocation>> {
        if ranges.is_empty() {
            return Ok(Vec::new());
        }
        let start_key = ranges
            .iter()
            .map(|range| range.start.as_slice())
            .min()
            .unwrap_or_default()
            .to_vec();
        let end_key = ranges
            .iter()
            .map(|range| range.end.as_slice())
            .max()
            .unwrap_or_default()
            .to_vec();
        let (start_key, end_key) = self.codec.encode_region_range(&start_key, &end_key);
        let mut request = pdpb::ScanRegionsRequest::new();
        request.set_header(pd_header(self.cluster_id));
        request.set_start_key(start_key);
        request.set_end_key(end_key);
        request.set_limit(10_240);
        let mut last_error = None;
        for client in &self.clients {
            match client.scan_regions_opt(&request, CallOption::default().timeout(self.timeout)) {
                Ok(mut response) => {
                    check_pd_header(response.get_header())?;
                    let mut locations = Vec::new();
                    if !response.get_regions().is_empty() {
                        for mut item in response.take_regions().into_iter() {
                            if !item.has_region() {
                                continue;
                            }
                            let buckets = if item.has_buckets() {
                                let buckets = item.take_buckets();
                                Some(Buckets {
                                    version: buckets.get_version(),
                                    keys: buckets.get_keys().to_vec(),
                                })
                            } else {
                                None
                            };
                            locations.push(self.location(
                                item.take_region(),
                                item.take_leader(),
                                buckets,
                            )?);
                        }
                    } else {
                        let leaders = response.take_leaders().into_vec();
                        for (region, leader) in response
                            .take_region_metas()
                            .into_vec()
                            .into_iter()
                            .zip(leaders)
                        {
                            locations.push(self.location(region, leader, None)?);
                        }
                    }
                    return Ok(locations);
                }
                Err(error) => last_error = Some(error.to_string()),
            }
        }
        Err(BatchError::Transport(
            last_error.unwrap_or_else(|| "PD scan_regions failed".to_owned()),
        ))
    }

    fn locate_key(&self, key: &[u8]) -> BatchResult<KeyLocation> {
        let mut request = pdpb::GetRegionRequest::new();
        request.set_header(pd_header(self.cluster_id));
        request.set_region_key(mem_encode(&self.codec.encode_key(key)));
        request.set_need_buckets(true);
        let mut last_error = None;
        for client in &self.clients {
            match client.get_region_opt(&request, CallOption::default().timeout(self.timeout)) {
                Ok(response) => return self.location_from_response(response),
                Err(error) => last_error = Some(error.to_string()),
            }
        }
        Err(BatchError::Transport(
            last_error.unwrap_or_else(|| "PD get_region failed".to_owned()),
        ))
    }

    fn locate_end_key(&self, key: &[u8]) -> BatchResult<KeyLocation> {
        let mut request = pdpb::GetRegionRequest::new();
        request.set_header(pd_header(self.cluster_id));
        request.set_region_key(self.codec.encode_end_region_key(key));
        request.set_need_buckets(true);
        let mut last_error = None;
        for client in &self.clients {
            match client.get_prev_region_opt(&request, CallOption::default().timeout(self.timeout))
            {
                Ok(response) => return self.location_from_response(response),
                Err(error) => last_error = Some(error.to_string()),
            }
        }
        Err(BatchError::Transport(
            last_error.unwrap_or_else(|| "PD get_prev_region failed".to_owned()),
        ))
    }

    fn read_replicas(&self, region_id: u64) -> BatchResult<Vec<KeyLocation>> {
        let mut request = pdpb::GetRegionByIdRequest::new();
        request.set_header(pd_header(self.cluster_id));
        request.set_region_id(region_id);
        let mut last_error = None;
        for client in &self.clients {
            match client.get_region_by_id_opt(&request, CallOption::default().timeout(self.timeout))
            {
                Ok(response) => {
                    check_pd_header(response.get_header())?;
                    let region = response.get_region();
                    let leader_id = response.get_leader().get_id();
                    let mut peers = region.get_peers().to_vec();
                    peers.sort_by_key(|peer| peer.get_id() != leader_id);
                    return peers
                        .into_iter()
                        .map(|peer| self.location(region.clone(), peer, None))
                        .collect();
                }
                Err(error) => last_error = Some(error.to_string()),
            }
        }
        Err(BatchError::Transport(
            last_error.unwrap_or_else(|| "PD get Region replicas failed".into()),
        ))
    }

    fn locate_region_by_id(&self, region_id: u64) -> BatchResult<KeyLocation> {
        let mut request = pdpb::GetRegionByIdRequest::new();
        request.set_header(pd_header(self.cluster_id));
        request.set_region_id(region_id);
        request.set_need_buckets(true);
        let mut last_error = None;
        for client in &self.clients {
            match client.get_region_by_id_opt(&request, CallOption::default().timeout(self.timeout))
            {
                Ok(response) => return self.location_from_response(response),
                Err(error) => last_error = Some(error.to_string()),
            }
        }
        Err(BatchError::Transport(last_error.unwrap_or_else(|| {
            format!("PD get_region_by_id({region_id}) failed")
        })))
    }

    fn invalidate_region(&self, _region: RegionVerId) {
        // 本 transport 不持有 Region 数据；下一次定位天然重新查询 PD。
    }

    fn is_store_alive(&self, address: &str, _ttl: Duration) -> bool {
        !address.is_empty()
    }
}

fn pb_peer(peer: &metapb::Peer) -> Peer {
    Peer {
        id: peer.get_id(),
        store_id: peer.get_store_id(),
    }
}

fn pb_store(store: metapb::Store) -> RegionStore {
    RegionStore {
        id: store.get_id(),
        address: store.get_address().to_owned(),
        labels: store
            .get_labels()
            .iter()
            .map(|label| (label.get_key().to_owned(), label.get_value().to_owned()))
            .collect(),
    }
}

struct GrpcStandardCoprocessorTransport {
    _environment: Arc<Environment>,
    channels: Arc<ChannelFactory>,
    clients: Mutex<HashMap<String, tikvpb_grpc::TikvClient>>,
    codec: Arc<KeyCodec>,
    closed: AtomicBool,
}

impl GrpcStandardCoprocessorTransport {
    fn new(
        environment: Arc<Environment>,
        channels: Arc<ChannelFactory>,
        codec: Arc<KeyCodec>,
    ) -> Self {
        Self {
            _environment: environment,
            channels,
            clients: Mutex::new(HashMap::new()),
            codec,
            closed: AtomicBool::new(false),
        }
    }

    fn client(&self, address: &str) -> BatchResult<tikvpb_grpc::TikvClient> {
        if self.closed.load(Ordering::Acquire) {
            return Err(BatchError::Closed);
        }
        let mut clients = self.clients.lock().expect("TiKV client cache poisoned");
        Ok(clients
            .entry(address.to_owned())
            .or_insert_with(|| tikvpb_grpc::TikvClient::new(self.channels.connect(address)))
            .clone())
    }

    fn protobuf_request(
        &self,
        request: &StandardCoprocessorRequest,
    ) -> BatchResult<coprocessorpb::Request> {
        let mut context = kvrpcpb::Context::new();
        context.set_region_id(request.region.id);
        let mut epoch = metapb::RegionEpoch::new();
        epoch.set_conf_ver(request.region.conf_ver);
        epoch.set_version(request.region.version);
        context.set_region_epoch(epoch);
        if let Some(peer) = &request.peer {
            let mut protobuf_peer = metapb::Peer::new();
            protobuf_peer.set_id(peer.id);
            protobuf_peer.set_store_id(peer.store_id);
            context.set_peer(protobuf_peer);
        }
        context.set_priority(match request.wire.priority {
            Priority::Low => kvrpcpb::CommandPri::Low,
            Priority::Normal => kvrpcpb::CommandPri::Normal,
            Priority::High => kvrpcpb::CommandPri::High,
        });
        context.set_isolation_level(match request.wire.isolation_level {
            IsolationLevel::ReadCommitted => kvrpcpb::IsolationLevel::Rc,
            IsolationLevel::SnapshotIsolation => kvrpcpb::IsolationLevel::Si,
            IsolationLevel::ReadCommittedCheckTs => kvrpcpb::IsolationLevel::RcCheckTs,
        });
        context.set_not_fill_cache(request.wire.not_fill_cache);
        context.set_record_time_stat(true);
        context.set_record_scan_stat(true);
        context.set_replica_read(request.wire.replica_read != ReplicaReadType::Leader);
        context.set_is_retry_request(request.wire.retry_request);
        context.set_stale_read(request.wire.is_staleness);
        context.set_max_execution_duration_ms(request.wire.max_execution_duration_ms);
        context.set_busy_threshold_ms(
            request
                .wire
                .busy_threshold
                .as_millis()
                .min(u128::from(u32::MAX)) as u32,
        );
        context.set_buckets_version(request.wire.bucket_version);
        if !request.wire.read_type.is_empty() {
            context.set_request_source(request.wire.read_type.clone());
        }
        let mut resource = kvrpcpb::ResourceControlContext::new();
        resource.set_resource_group_name(request.wire.resource_group_name.clone());
        context.set_resource_control_context(resource);
        if let Some(keyspace_id) = self.codec.keyspace_id {
            context.set_api_version(kvrpcpb::ApiVersion::V2);
            context.set_keyspace_name(self.codec.keyspace_name.clone());
            context.set_keyspace_id(keyspace_id);
        }

        let mut protobuf_request = coprocessorpb::Request::new();
        protobuf_request.set_context(context);
        protobuf_request.set_tp(match request.wire.request_type {
            RequestType::Dag => 103,
            RequestType::Analyze => 104,
            RequestType::Checksum => 105,
        });
        protobuf_request.set_start_ts(request.wire.start_ts);
        protobuf_request.set_data(request.wire.data.clone());
        protobuf_request.set_schema_ver(request.wire.schema_version);
        protobuf_request.set_paging_size(request.wire.paging_size);
        protobuf_request.set_max_keys_read(request.wire.maximum_keys_read);
        protobuf_request.set_paging_size_bytes(request.wire.paging_size_bytes);
        protobuf_request.set_connection_id(request.wire.connection_id);
        protobuf_request.set_connection_alias(request.wire.connection_alias.clone());
        protobuf_request.set_ranges(protobuf::RepeatedField::from_vec(
            request
                .wire
                .ranges
                .iter()
                .map(|range| pb_key_range(range, &self.codec))
                .collect(),
        ));
        protobuf_request.set_tasks(protobuf::RepeatedField::from_vec(
            request
                .wire
                .tasks
                .iter()
                .map(|task| {
                    let mut protobuf_task = coprocessorpb::StoreBatchTask::new();
                    protobuf_task.set_region_id(task.region.id);
                    let mut epoch = metapb::RegionEpoch::new();
                    epoch.set_conf_ver(task.region.conf_ver);
                    epoch.set_version(task.region.version);
                    protobuf_task.set_region_epoch(epoch);
                    if let Some(peer) = &task.peer {
                        let mut protobuf_peer = metapb::Peer::new();
                        protobuf_peer.set_id(peer.id);
                        protobuf_peer.set_store_id(peer.store_id);
                        protobuf_task.set_peer(protobuf_peer);
                    }
                    protobuf_task.set_ranges(protobuf::RepeatedField::from_vec(
                        task.ranges
                            .iter()
                            .map(|range| pb_key_range(range, &self.codec))
                            .collect(),
                    ));
                    protobuf_task.set_task_id(task.task_id);
                    protobuf_task.set_buckets_version(task.bucket_version);
                    protobuf_task
                })
                .collect(),
        ));
        Ok(protobuf_request)
    }
}

impl StandardCoprocessorTransport for GrpcStandardCoprocessorTransport {
    fn send_unary(
        &self,
        request: &StandardCoprocessorRequest,
        timeout: Duration,
    ) -> BatchResult<CopProtocolResponse> {
        let protobuf_request = self.protobuf_request(request)?;
        let delay = fail::eval("tikvclient/mockBatchClientSendDelay", |value| value)
            .flatten()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_default();
        let client = self.client(&request.address)?;
        let response = if delay == 0 {
            client
                .coprocessor_opt(&protobuf_request, CallOption::default().timeout(timeout))
                .map_err(transport_error)?
        } else {
            static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> =
                std::sync::OnceLock::new();
            let runtime = RUNTIME.get_or_init(|| {
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .enable_time()
                    .build()
                    .unwrap()
            });
            runtime.block_on(async {
                tokio::time::timeout(timeout, async {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    client
                        .coprocessor_async_opt(
                            &protobuf_request,
                            CallOption::default().timeout(timeout),
                        )
                        .map_err(transport_error)?
                        .await
                        .map_err(transport_error)
                })
                .await
                .map_err(|_| {
                    BatchError::Transport("DEADLINE_EXCEEDED while queued for Coprocessor".into())
                })?
            })?
        };
        pb_response(response, &self.codec)
    }

    fn send_stream(
        &self,
        request: &StandardCoprocessorRequest,
        timeout: Duration,
    ) -> BatchResult<Box<dyn CoprocessorResponseStream>> {
        let protobuf_request = self.protobuf_request(request)?;
        let receiver = self
            .client(&request.address)?
            .coprocessor_stream_opt(&protobuf_request, CallOption::default().timeout(timeout))
            .map_err(transport_error)?;
        Ok(Box::new(GrpcCoprocessorResponseStream {
            receiver,
            codec: Arc::clone(&self.codec),
            closed: false,
        }))
    }

    fn close(&self) -> BatchResult<()> {
        self.closed.store(true, Ordering::Release);
        self.clients
            .lock()
            .expect("TiKV client cache poisoned")
            .clear();
        Ok(())
    }

    fn close_address(&self, address: &str) -> BatchResult<()> {
        self.clients
            .lock()
            .expect("TiKV client cache poisoned")
            .remove(address);
        Ok(())
    }
}

struct GrpcCoprocessorResponseStream {
    receiver: grpcio::ClientSStreamReceiver<coprocessorpb::Response>,
    codec: Arc<KeyCodec>,
    closed: bool,
}

impl CoprocessorResponseStream for GrpcCoprocessorResponseStream {
    fn next(&mut self) -> BatchResult<Option<CopProtocolResponse>> {
        if self.closed {
            return Ok(None);
        }
        match block_on(self.receiver.next())
            .transpose()
            .map_err(transport_error)?
        {
            Some(response) => pb_response(response, &self.codec).map(Some),
            None => Ok(None),
        }
    }

    fn close(&mut self) -> BatchResult<()> {
        if !self.closed {
            self.receiver.cancel();
            self.closed = true;
        }
        Ok(())
    }
}

impl Drop for GrpcCoprocessorResponseStream {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn pb_key_range(range: &KeyRange, codec: &KeyCodec) -> coprocessorpb::KeyRange {
    let (start, end) = codec.encode_range(&range.start, &range.end);
    let mut protobuf_range = coprocessorpb::KeyRange::new();
    protobuf_range.set_start(start);
    protobuf_range.set_end(end);
    protobuf_range
}

fn key_range(range: &coprocessorpb::KeyRange, codec: &KeyCodec) -> BatchResult<KeyRange> {
    let (start, end) = codec.decode_range(range.get_start(), range.get_end())?;
    Ok(KeyRange { start, end })
}

fn lock_bytes(mut lock: kvrpcpb::LockInfo, codec: &KeyCodec) -> BatchResult<Vec<u8>> {
    codec.decode_lock_info(&mut lock)?;
    lock.write_to_bytes()
        .map_err(|error| BatchError::OtherResponse(format!("encode TiKV lock: {error}")))
}

fn error_text(error: &errorpb::Error) -> String {
    format!("{error:?}")
}

fn response_processed_keys_v2(details: &kvrpcpb::ExecDetailsV2) -> u64 {
    if details.has_scan_detail_v2() {
        details.get_scan_detail_v2().get_processed_versions()
    } else {
        0
    }
}

fn response_read_bytes_v2(details: &kvrpcpb::ExecDetailsV2) -> u64 {
    if details.has_scan_detail_v2() {
        details.get_scan_detail_v2().get_processed_versions_size()
    } else {
        0
    }
}

fn response_kv_cpu_ms_v2(details: &kvrpcpb::ExecDetailsV2) -> f64 {
    if details.has_time_detail_v2() {
        details.get_time_detail_v2().get_process_wall_time_ns() as f64 / 1_000_000.0
    } else if details.has_time_detail() {
        details.get_time_detail().get_process_wall_time_ms() as f64
    } else {
        0.0
    }
}

fn response_processed_keys(response: &coprocessorpb::Response) -> u64 {
    if response.has_exec_details_v2() {
        return response_processed_keys_v2(response.get_exec_details_v2());
    }
    if response.has_exec_details() && response.get_exec_details().has_scan_detail() {
        return response
            .get_exec_details()
            .get_scan_detail()
            .get_write()
            .get_processed()
            .max(0) as u64;
    }
    0
}

fn pb_response(
    mut response: coprocessorpb::Response,
    codec: &KeyCodec,
) -> BatchResult<CopProtocolResponse> {
    let batch_responses = response
        .take_batch_responses()
        .into_iter()
        .map(|mut batch| {
            let task_id = batch.get_task_id();
            let locked = if batch.has_locked() {
                Some(lock_bytes(batch.take_locked(), codec)?)
            } else {
                None
            };
            let protocol = CopProtocolResponse {
                data: batch.take_data(),
                region_error: batch
                    .has_region_error()
                    .then(|| error_text(batch.get_region_error())),
                locked,
                other_error: batch.take_other_error(),
                scanned_keys: batch
                    .has_exec_details_v2()
                    .then(|| response_processed_keys_v2(batch.get_exec_details_v2()))
                    .unwrap_or_default(),
                read_bytes: batch
                    .has_exec_details_v2()
                    .then(|| response_read_bytes_v2(batch.get_exec_details_v2()))
                    .unwrap_or_default(),
                kv_cpu_ms: batch
                    .has_exec_details_v2()
                    .then(|| response_kv_cpu_ms_v2(batch.get_exec_details_v2()))
                    .unwrap_or_default(),
                ..CopProtocolResponse::default()
            };
            Ok((task_id, protocol))
        })
        .collect::<BatchResult<_>>()?;
    let range = if response.has_range() {
        Some(key_range(response.get_range(), codec)?)
    } else {
        None
    };
    let locked = if response.has_locked() {
        Some(lock_bytes(response.take_locked(), codec)?)
    } else {
        None
    };
    let scanned_keys = response_processed_keys(&response);
    let read_bytes = response
        .has_exec_details_v2()
        .then(|| response_read_bytes_v2(response.get_exec_details_v2()))
        .unwrap_or_default();
    let kv_cpu_ms = response
        .has_exec_details_v2()
        .then(|| response_kv_cpu_ms_v2(response.get_exec_details_v2()))
        .unwrap_or_default();
    Ok(CopProtocolResponse {
        data: response.take_data(),
        region_error: response
            .has_region_error()
            .then(|| error_text(response.get_region_error())),
        locked,
        other_error: response.take_other_error(),
        range,
        latest_bucket_version: response.get_latest_buckets_version(),
        cache_last_version: response.get_cache_last_version(),
        can_be_cached: response.get_can_be_cached(),
        scanned_keys,
        read_bytes,
        kv_cpu_ms,
        batch_responses,
        ..CopProtocolResponse::default()
    })
}

#[cfg(test)]
#[path = "network_backend_runaway_test.rs"]
mod network_backend_runaway_test;
