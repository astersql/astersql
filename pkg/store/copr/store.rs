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

// Coprocessor / MPP 侧的 KV Store 组装层。
//
// 将可注入的 `StoreBackend` 与 `RegionCache` 组合为 `Store`：
// 提供 TikvClient、CopClient、MppClient，并管理 coprocessor 缓存与事件监听。
// EndpointType 描述 TiKV / TiFlash / TiFlash Compute / TiDB 等端点角色。

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::batch_request_sender::{
    BatchRequest, BatchResult, CancellationToken, RpcClient, RpcResponse, Store as RegionStore,
};
use crate::coprocessor::{
    BatchedCopTask, CopBackend, CopClient, CopProtocolResponse, CopRequest, CopTask,
    LocatedKeyRanges, ReplicaReadType, StoreType,
};
use crate::coprocessor_cache::{CoprocessorCache, CoprocessorCacheConfig};
use crate::mpp::{
    MppCancelRequest, MppClient, MppConnectionRequest, MppDispatchResponse, MppDispatchWireRequest,
    MppStream, MppTransport,
};
use crate::mpp_probe::MppAliveClient;
use crate::region_cache::{RegionCache, RegionCacheBackend};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 存储或计算端点类型。
pub enum EndpointType {
    #[default]
    TiKv,
    TiFlash,
    TiFlashCompute,
    TiDb,
}

/// 客户端事件监听器（连接/错误等回调）。
pub trait ClientEventListener: Send + Sync + 'static {
    /// 处理命名事件。
    fn on_event(&self, event: &str);
}

/// 具体集成提供传输与可见性操作；Region 相关能力可单独注入以便测试。
/// Concrete integrations provide transport and visibility operations while the
/// region-specific surface remains separately injectable and testable.
pub trait StoreBackend: Send + Sync + 'static {
    /// 返回 Region 缓存后端。
    fn region_backend(&self) -> Arc<dyn RegionCacheBackend>;
    /// 关闭底层客户端。
    fn close_client(&self) -> BatchResult<()>;
    /// 关闭到指定地址的连接。
    fn close_address(&self, address: &str) -> BatchResult<()>;
    /// 发送 batch 请求。
    fn send_request(
        &self,
        address: &str,
        request: &BatchRequest,
        timeout: Duration,
        cancellation: &CancellationToken,
    ) -> BatchResult<RpcResponse>;
    /// 设置事件监听器。
    fn set_event_listener(&self, listener: Option<Arc<dyn ClientEventListener>>);
    /// 发送 Coprocessor 请求。
    fn send_coprocessor(
        &self,
        task: &CopTask,
        request: &crate::coprocessor::CopWireRequest,
    ) -> BatchResult<CopProtocolResponse>;
    /// 打开标准 TiKV Coprocessor server-streaming RPC。
    fn send_coprocessor_stream(
        &self,
        _task: &CopTask,
        _request: &crate::coprocessor::CopWireRequest,
    ) -> BatchResult<Box<dyn crate::network_backend::CoprocessorResponseStream>> {
        Err(crate::batch_request_sender::BatchError::OtherResponse(
            "standard coprocessor stream transport is unavailable".to_owned(),
        ))
    }
    /// 解析事务锁（两阶段提交中的锁冲突处理）。
    fn resolve_lock(&self, lock: &[u8], start_ts: u64) -> BatchResult<()>;
    /// 检查快照可见性。
    fn check_visibility(&self, start_ts: u64) -> BatchResult<()>;
    /// 分发 MPP 任务。
    fn dispatch_mpp(
        &self,
        address: &str,
        request: &MppDispatchWireRequest,
        timeout: Duration,
    ) -> BatchResult<MppDispatchResponse>;
    /// 取消 MPP 任务。
    fn cancel_mpp(
        &self,
        address: &str,
        request: &MppCancelRequest,
        timeout: Duration,
    ) -> BatchResult<()>;
    /// 建立 MPP 连接。
    fn establish_mpp(
        &self,
        address: &str,
        request: &MppConnectionRequest,
        timeout: Duration,
    ) -> BatchResult<MppStream>;
    /// 使计算 store 缓存失效。
    fn invalidate_compute_stores(&self);
}

#[derive(Clone)]
/// 面向 TiKV 的 RPC 客户端封装，委托 StoreBackend。
pub struct TikvClient {
    /// 底层存储后端。
    backend: Arc<dyn StoreBackend>,
}

impl TikvClient {
    /// 用给定后端构造客户端。
    pub fn new(backend: Arc<dyn StoreBackend>) -> Self {
        Self { backend }
    }

    /// 关闭客户端连接资源。
    pub fn close(&self) -> BatchResult<()> {
        self.backend.close_client()
    }

    /// 关闭到指定地址的连接。
    pub fn close_address(&self, address: &str) -> BatchResult<()> {
        self.backend.close_address(address)
    }

    /// 打开标准 TiKV Coprocessor server stream。
    pub fn send_coprocessor_stream(
        &self,
        task: &CopTask,
        request: &crate::coprocessor::CopWireRequest,
    ) -> BatchResult<Box<dyn crate::network_backend::CoprocessorResponseStream>> {
        self.backend.send_coprocessor_stream(task, request)
    }

    /// 在独立线程中异步发送请求并回调结果。
    pub fn send_request_async<F>(
        &self,
        address: String,
        request: BatchRequest,
        timeout: Duration,
        callback: F,
    ) -> JoinHandle<()>
    where
        F: FnOnce(BatchResult<RpcResponse>) + Send + 'static,
    {
        let client = self.clone();
        thread::spawn(move || {
            callback(client.send_request(
                &address,
                &request,
                timeout,
                &CancellationToken::default(),
            ));
        })
    }

    /// 设置客户端事件监听器。
    pub fn set_event_listener(&self, listener: Option<Arc<dyn ClientEventListener>>) {
        self.backend.set_event_listener(listener);
    }
}

impl RpcClient for TikvClient {
    fn send_request(
        &self,
        address: &str,
        request: &BatchRequest,
        timeout: Duration,
        cancellation: &CancellationToken,
    ) -> BatchResult<RpcResponse> {
        self.backend
            .send_request(address, request, timeout, cancellation)
    }
}

impl MppAliveClient for TikvClient {
    /// 探活指定地址。
    fn is_alive(&self, address: &str, timeout: Duration) -> bool {
        self.backend
            .region_backend()
            .is_store_alive(address, timeout)
    }
}

/// 将 StoreBackend + RegionCache 适配为 CopBackend。
struct StoreCopBackend {
    /// 存储后端。
    backend: Arc<dyn StoreBackend>,
    /// Region 定位与拆分缓存。
    region_cache: Arc<RegionCache>,
}

impl CopBackend for StoreCopBackend {
    /// 按 Region 切分 Cop 任务 ranges。
    fn split_key_ranges(
        &self,
        ranges: &crate::batch_request_sender::KeyRanges,
        skip_buckets: bool,
    ) -> BatchResult<Vec<LocatedKeyRanges>> {
        // skip_buckets 时仅按 Region location 拆分，否则再按 bucket 细分。
        let locations = if skip_buckets {
            self.region_cache.split_key_ranges_by_locations(
                ranges.clone(),
                crate::region_cache::UNSPECIFIED_LIMIT,
                false,
                false,
            )?
        } else {
            self.region_cache
                .split_key_ranges_by_buckets(ranges.clone())?
        };
        Ok(locations
            .into_iter()
            .map(|location| {
                let bucket_version = location.location.bucket_version();
                let store_address = location
                    .location
                    .store
                    .as_ref()
                    .map(|store| store.address.clone())
                    .unwrap_or_default();
                let store_id = location
                    .location
                    .store
                    .as_ref()
                    .map(|store| store.id)
                    .unwrap_or_default();
                LocatedKeyRanges {
                    region: location.location.region,
                    ranges: location.ranges,
                    location_start: location.location.start_key,
                    location_end: location.location.end_key,
                    bucket_version,
                    store_address,
                    store_id,
                    peer: location.location.peer,
                }
            })
            .collect())
    }

    /// 构建 batch Cop 任务。
    fn build_batch_task(
        &self,
        task: &CopTask,
        replica_read: ReplicaReadType,
    ) -> BatchResult<Option<BatchedCopTask>> {
        self.region_cache
            .build_batch_task(&CopRequest::default(), task, replica_read)
    }

    /// TiDB server 地址。
    fn tidb_server_addresses(&self) -> BatchResult<Vec<(u64, String)>> {
        self.backend.region_backend().tidb_server_addresses()
    }

    /// 发送 Cop 请求。
    fn send(
        &self,
        task: &CopTask,
        request: &crate::coprocessor::CopWireRequest,
    ) -> BatchResult<CopProtocolResponse> {
        self.backend.send_coprocessor(task, request)
    }

    /// 失效 Region。
    fn invalidate_region(&self, region: crate::batch_request_sender::RegionVerId) {
        self.region_cache.invalidate_region(region);
    }

    /// 更新 buckets。
    fn update_buckets(
        &self,
        region: crate::batch_request_sender::RegionVerId,
        old_version: u64,
        new_version: u64,
    ) {
        self.region_cache
            .update_buckets(region, old_version, new_version);
    }

    fn resolve_lock(&self, lock: &[u8], start_ts: u64) -> BatchResult<()> {
        self.backend.resolve_lock(lock, start_ts)
    }

    /// 检查可见性。
    fn check_visibility(&self, start_ts: u64) -> BatchResult<()> {
        self.backend.check_visibility(start_ts)
    }
}

/// 将 StoreBackend 适配为 MppTransport。
struct StoreMppTransport {
    /// 存储后端。
    backend: Arc<dyn StoreBackend>,
}

impl MppTransport for StoreMppTransport {
    /// 分发 MPP。
    fn dispatch(
        &self,
        address: &str,
        request: &MppDispatchWireRequest,
        timeout: Duration,
    ) -> BatchResult<MppDispatchResponse> {
        self.backend.dispatch_mpp(address, request, timeout)
    }

    /// 取消 MPP。
    fn cancel(
        &self,
        address: &str,
        request: &MppCancelRequest,
        timeout: Duration,
    ) -> BatchResult<()> {
        self.backend.cancel_mpp(address, request, timeout)
    }

    /// 建连。
    fn establish(
        &self,
        address: &str,
        request: &MppConnectionRequest,
        timeout: Duration,
    ) -> BatchResult<MppStream> {
        self.backend.establish_mpp(address, request, timeout)
    }

    fn check_visibility(&self, start_ts: u64) -> BatchResult<()> {
        self.backend.check_visibility(start_ts)
    }

    /// 全部 store。
    fn all_stores(&self) -> BatchResult<Vec<RegionStore>> {
        Ok(self.backend.region_backend().all_tiflash_stores())
    }

    fn invalidate_region(&self, region: crate::batch_request_sender::RegionVerId) {
        self.backend.region_backend().invalidate_region(region);
    }

    /// 失效计算 store。
    fn invalidate_compute_stores(&self) {
        self.backend.invalidate_compute_stores();
    }
}

/// 持有后端与 RegionCache 的底层 KV store 句柄。
pub struct KvStore {
    /// 存储后端。
    backend: Arc<dyn StoreBackend>,
    /// Region 缓存。
    region_cache: Arc<RegionCache>,
}

impl KvStore {
    /// 返回共享 RegionCache。
    pub fn region_cache(&self) -> Arc<RegionCache> {
        Arc::clone(&self.region_cache)
    }

    /// 检查快照 start_ts 的可见性。
    pub fn check_visibility(&self, start_ts: u64) -> BatchResult<()> {
        self.backend.check_visibility(start_ts)
    }

    /// 构造绑定本后端的 TikvClient。
    pub fn client(&self) -> TikvClient {
        TikvClient::new(Arc::clone(&self.backend))
    }
}

/// 对外 Store：组装 coprocessor 缓存、副本读种子与 MPP/Cop 客户端工厂。
pub struct Store {
    /// 底层 KV store。
    kv_store: Arc<KvStore>,
    /// 可选的 coprocessor 结果缓存。
    coprocessor_cache: Mutex<Option<Arc<CoprocessorCache>>>,
    /// 副本读负载均衡用的递增种子。
    replica_read_seed: AtomicU32,
    /// 本地可用并行度，传给 CopClient。
    cpu_count: usize,
    /// 是否存算分离 TiFlash。
    disaggregated_tiflash: bool,
    /// 是否启用自动扩缩容。
    use_auto_scaler: bool,
    /// Store 是否已关闭。
    closed: AtomicBool,
    /// 已注册的事件监听器。
    clients: Mutex<Vec<Arc<dyn ClientEventListener>>>,
}

impl Store {
    /// 创建 Store 并初始化 RegionCache 与可选 coprocessor 缓存。
    pub fn new(
        backend: Arc<dyn StoreBackend>,
        cache_config: &CoprocessorCacheConfig,
        disaggregated_tiflash: bool,
        use_auto_scaler: bool,
    ) -> BatchResult<Self> {
        let region_cache = Arc::new(RegionCache::new(backend.region_backend()));
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        Ok(Self {
            kv_store: Arc::new(KvStore {
                backend,
                region_cache,
            }),
            coprocessor_cache: Mutex::new(CoprocessorCache::new(cache_config)?.map(Arc::new)),
            replica_read_seed: AtomicU32::new(seed),
            cpu_count: thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1),
            disaggregated_tiflash,
            use_auto_scaler,
            closed: AtomicBool::new(false),
            clients: Mutex::new(Vec::new()),
        })
    }

    /// 关闭 Store 拥有的资源（不清底层 KvStore，对齐 Go）。
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.clients
            .lock()
            .expect("store listener lock poisoned")
            .clear();
        self.coprocessor_cache
            .lock()
            .expect("coprocessor cache owner lock poisoned")
            .take();
        // 底层 KvStore 故意不在此关闭，与 Go 行为一致。
        // The wrapped KV store is intentionally not closed here, matching Go;
        // Store only owns coprocessor-side resources.
    }

    /// 是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// 取下一个副本读种子（先加后返回风格）。
    pub fn next_replica_read_seed(&self) -> u32 {
        self.replica_read_seed.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// 构造绑定本 Store 后端的 CopClient。
    pub fn get_client(&self) -> CopClient {
        let backend: Arc<dyn CopBackend> = Arc::new(StoreCopBackend {
            backend: Arc::clone(&self.kv_store.backend),
            region_cache: self.kv_store.region_cache(),
        });
        let _seed = self.next_replica_read_seed();
        let cache = self
            .coprocessor_cache
            .lock()
            .expect("coprocessor cache owner lock poisoned")
            .clone();
        CopClient::new(backend, cache, self.cpu_count)
    }

    /// 构造绑定本 Store 后端的 MppClient。
    pub fn get_mpp_client(&self) -> MppClient {
        let source: Arc<dyn crate::batch_coprocessor::BatchTaskSource> =
            self.kv_store.region_cache();
        let transport: Arc<dyn MppTransport> = Arc::new(StoreMppTransport {
            backend: Arc::clone(&self.kv_store.backend),
        });
        MppClient::new(
            source,
            transport,
            self.disaggregated_tiflash,
            self.use_auto_scaler,
        )
    }

    /// 返回底层 KvStore。
    pub fn kv_store(&self) -> Arc<KvStore> {
        Arc::clone(&self.kv_store)
    }

    /// 注册事件监听器并同步到底层后端。
    pub fn add_event_listener(&self, listener: Arc<dyn ClientEventListener>) {
        self.clients
            .lock()
            .expect("store listener lock poisoned")
            .push(Arc::clone(&listener));
        self.kv_store.backend.set_event_listener(Some(listener));
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        self.close();
    }
}

/// 由 StoreType 与是否存算分离映射为 EndpointType。
pub fn endpoint_type(store_type: StoreType, disaggregated_tiflash: bool) -> EndpointType {
    match store_type {
        StoreType::TiKv => EndpointType::TiKv,
        StoreType::TiFlash if disaggregated_tiflash => EndpointType::TiFlashCompute,
        StoreType::TiFlash => EndpointType::TiFlash,
        StoreType::TiDb => EndpointType::TiDb,
    }
}

#[allow(non_camel_case_types)]
/// Go 风格别名。
pub type kvStore = KvStore;
#[allow(non_camel_case_types)]
/// Go 风格别名。
pub type tikvClient = TikvClient;

#[allow(non_snake_case)]
/// Go 风格构造：返回堆上 Store。
pub fn NewStore(
    backend: Arc<dyn StoreBackend>,
    cache_config: &CoprocessorCacheConfig,
    disaggregated_tiflash: bool,
    use_auto_scaler: bool,
) -> BatchResult<Box<Store>> {
    Store::new(
        backend,
        cache_config,
        disaggregated_tiflash,
        use_auto_scaler,
    )
    .map(Box::new)
}
