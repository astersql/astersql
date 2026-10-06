// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Rust counterpart of `helper.go`.
//
// The Go package sits between TiDB and the TiKV/PD clients.  Rust does not have
// those Go interfaces, so this module exposes object-safe transport traits and
// keeps the production algorithms independent from a concrete client.  A real
// client or a deterministic test double can implement the same traits.
// Store Helper：TiDB 与 TiKV/PD 之间的诊断与管理辅助层。
//
// 提供 Region（键空间分片）查询、MVCC（多版本并发控制）检视、热点统计、
// 表/索引与 Region 的映射，以及 TiFlash / Columnar 状态采集等能力。
// 通过 object-safe trait 抽象客户端，便于生产实现与测试替身共享同一算法。

use std::any::Any;
use std::collections::HashMap;
use std::io::BufRead;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

/// 原始键字节。
pub type Key = Vec<u8>;
/// Keyspace（多租户键空间）标识。
pub type KeyspaceID = u32;

#[derive(Clone, Debug, Default)]
/// 开启事务时的选项占位（对应 Go TxnOption）。
pub struct TxnOption;
#[derive(Clone, Debug, Default)]
/// 事务句柄占位。
pub struct Transaction;
#[derive(Clone, Debug, Default)]
/// 只读快照占位。
pub struct Snapshot;
#[derive(Clone, Debug, Default)]
/// KV 客户端占位。
pub struct KvClient;
#[derive(Clone, Debug, Default)]
/// MPP（大规模并行处理）客户端占位。
pub struct MppClient;
#[derive(Clone, Debug, Default)]
/// 内存/缓存管理器占位。
pub struct MemManager;
#[derive(Clone, Debug, Default)]
/// 安全点相关 KV 访问占位。
pub struct SafePointKv;
#[derive(Clone, Debug, Default)]
/// TiKV RPC 客户端占位。
pub struct TikvClient;
#[derive(Clone, Debug, Default)]
/// 锁等待条目占位。
pub struct WaitForEntry;

/// A small context value retaining the Go entry points' cancellation and
/// deadline contract for blocking HTTP requests.
#[derive(Clone, Debug)]
/// 请求上下文：保留取消与截止时间语义，供阻塞 HTTP 调用使用。
pub struct RequestContext {
    cancelled: Arc<AtomicBool>,
    deadline: Option<Instant>,
}

impl Default for RequestContext {
    fn default() -> Self {
        Self::background()
    }
}

impl RequestContext {
    /// 无取消、无截止时间的后台上下文。
    pub fn background() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline: None,
        }
    }

    /// 带超时截止时间的上下文。
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline: Some(Instant::now() + timeout),
        }
    }

    /// 标记请求已取消。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// 检查取消与截止时间，超时或取消则返回错误。
    fn check(&self) -> Result<()> {
        if self.cancelled.load(Ordering::Acquire) {
            bail!("context canceled");
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            bail!("context deadline exceeded");
        }
        Ok(())
    }

    /// 计算剩余超时，供 HTTP 客户端使用。
    fn request_timeout(&self) -> Duration {
        self.deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_secs(30))
            .max(Duration::from_millis(1))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 键编码 API 版本（v1 无 keyspace 前缀，v2 带 keyspace）。
enum ApiVersion {
    V1,
    V2(u32),
}

/// TiKV region-key codec. API V1 memcomparable-encodes raw keys; API V2 first
/// adds the `x + uint24(keyspace)` prefix and then uses the same encoding.
#[derive(Clone, Debug, Eq, PartialEq)]
/// 键编解码器，按 API 版本处理 keyspace 前缀。
pub struct Codec(ApiVersion);

impl Default for Codec {
    fn default() -> Self {
        Self::v1()
    }
}

impl Codec {
    /// 构造 v1 编解码器。
    pub fn v1() -> Self {
        Self(ApiVersion::V1)
    }

    /// 构造绑定指定 keyspace 的 v2 编解码器。
    pub fn v2(keyspace_id: u32) -> Result<Self> {
        if keyspace_id > 0x00ff_ffff {
            bail!("keyspace ID must fit uint24");
        }
        Ok(Self(ApiVersion::V2(keyspace_id)))
    }

    /// 按版本编码用户键。
    pub fn EncodeKey(&self, key: &[u8]) -> Vec<u8> {
        match self.0 {
            ApiVersion::V1 => key.to_vec(),
            ApiVersion::V2(id) => {
                let mut encoded = vec![
                    b'x',
                    ((id >> 16) & 0xff) as u8,
                    ((id >> 8) & 0xff) as u8,
                    (id & 0xff) as u8,
                ];
                encoded.extend_from_slice(key);
                encoded
            }
        }
    }

    /// 编码 Region 查询用的起止键。
    pub fn EncodeRegionRange(&self, start: &[u8], end: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let start = self.EncodeKey(start);
        let end = self.EncodeKey(end);
        (
            tablecodec::codec::EncodeBytes(Vec::new(), &start),
            tablecodec::codec::EncodeBytes(Vec::new(), &end),
        )
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
/// Region 版本标识（Region ID + 版本号）。
pub struct RegionVerID {
    pub ID: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 键所在 Region 的定位信息（含边界与版本）。
pub struct KeyLocation {
    pub Region: RegionVerID,
    pub StartKey: Vec<u8>,
    pub EndKey: Vec<u8>,
}

impl KeyLocation {
    /// 判断键是否落在该 Region 的半开区间内。
    pub fn Contains(&self, key: &[u8]) -> bool {
        self.StartKey.as_slice() <= key && (self.EndKey.is_empty() || key < self.EndKey.as_slice())
    }
}

/// Region 缓存：按键或 ID 定位 Region。
pub trait RegionCache: Send + Sync {
    /// 按键定位所属 Region。
    fn LocateKey(&self, backoffer: &mut Backoffer, key: &[u8]) -> Result<KeyLocation>;
    /// 按 Region ID 定位。
    fn LocateRegionByID(&self, backoffer: &mut Backoffer, id: u64) -> Result<KeyLocation>;
}

#[derive(Default)]
/// 空 Region 缓存占位实现。
struct EmptyRegionCache;

impl RegionCache for EmptyRegionCache {
    /// 空实现：无法定位键。
    fn LocateKey(&self, _: &mut Backoffer, _: &[u8]) -> Result<KeyLocation> {
        bail!("region cache unavailable")
    }

    /// 空实现：无法按 ID 定位。
    fn LocateRegionByID(&self, _: &mut Backoffer, _: u64) -> Result<KeyLocation> {
        bail!("region cache unavailable")
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// MVCC 锁信息。
pub struct LockInfo {
    pub Primary: Vec<u8>,
    pub StartTS: u64,
    pub TTL: u64,
    pub TxnSize: u64,
    pub LockType: i32,
    pub UseAsyncCommit: bool,
    pub ForUpdateTS: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 某键的 MVCC 多版本信息。
pub struct MvccInfo {
    pub Lock: Option<LockInfo>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 按编码键查询 MVCC 的响应。
pub struct MvccGetByKeyResponse {
    pub Info: Option<MvccInfo>,
    pub RegionError: Option<String>,
    pub Error: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 按事务 start_ts 查询 MVCC 的响应。
pub struct MvccGetByStartTsResponse {
    pub Info: Option<MvccInfo>,
    pub RegionError: Option<String>,
    pub Error: String,
    pub Key: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 发往 TiKV 的 KV 请求变体。
pub enum KvRequest {
    MvccGetByKey { key: Vec<u8> },
    MvccGetByStartTs { start_ts: u64, low_priority: bool },
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// TiKV KV 响应变体。
pub enum KvResponse {
    MvccGetByKey(MvccGetByKeyResponse),
    MvccGetByStartTs(MvccGetByStartTsResponse),
}

/// 时间戳 Oracle（全局授时）抽象。
pub trait Oracle: Send + Sync {
    /// 获取低精度时间戳。
    fn GetLowResolutionTimestamp(&self) -> Result<u64>;
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 事务锁描述。
pub struct Lock {
    pub Key: Vec<u8>,
    pub Primary: Vec<u8>,
    pub TxnID: u64,
    pub TTL: u64,
    pub TxnSize: u64,
    pub LockType: i32,
    pub UseAsyncCommit: bool,
    pub LockForUpdateTS: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 解析锁选项。
pub struct ResolveLocksOptions {
    pub CallerStartTS: u64,
    pub Locks: Vec<Lock>,
    pub Lite: bool,
    pub ForRead: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 解析锁结果。
pub struct ResolveLockResult {
    pub TTL: u64,
}

/// 锁解析器：清理或推进未决事务锁。
pub trait LockResolver: Send + Sync {
    /// 按选项批量解析锁。
    fn ResolveLocksWithOpts(
        &self,
        backoffer: &mut Backoffer,
        options: ResolveLocksOptions,
    ) -> Result<ResolveLockResult>;
}

#[derive(Debug)]
/// 退避重试器：失败后按策略睡眠再试。
pub struct Backoffer {
    max_sleep_ms: u64,
    total_sleep_ms: u64,
}

impl Backoffer {
    /// 构造最大累计睡眠上限的 Backoffer。
    pub fn new(max_sleep_ms: u64) -> Self {
        Self {
            max_sleep_ms,
            total_sleep_ms: 0,
        }
    }

    /// 执行一次退避睡眠；超过上限则返回错误。
    pub fn Backoff(&mut self, sleep_ms: u64, cause: impl std::fmt::Display) -> Result<()> {
        let next = self.total_sleep_ms.saturating_add(sleep_ms);
        if next > self.max_sleep_ms {
            bail!("backoff exhausted after {}ms: {cause}", self.total_sleep_ms);
        }
        self.total_sleep_ms = next;
        if sleep_ms > 0 {
            std::thread::sleep(Duration::from_millis(sleep_ms.min(10)));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
/// 单个 Region 的元信息（ID、起止键等）。
pub struct RegionInfo {
    #[serde(rename = "id")]
    pub ID: i64,
    #[serde(rename = "start_key")]
    pub StartKey: String,
    #[serde(rename = "end_key")]
    pub EndKey: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
/// 一组 Region 信息的集合。
pub struct RegionsInfo {
    #[serde(rename = "count")]
    pub Count: i64,
    #[serde(rename = "regions")]
    pub Regions: Vec<RegionInfo>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
/// 单个 Peer 的热点统计。
pub struct HotPeerStat {
    #[serde(rename = "region_id")]
    pub RegionID: u64,
    #[serde(rename = "byte_rate")]
    pub ByteRate: f64,
    #[serde(rename = "hot_degree")]
    pub HotDegree: i32,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
/// 多个 Peer 热点统计汇总。
pub struct HotPeersStat {
    #[serde(rename = "statistics")]
    pub Stats: Vec<HotPeerStat>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
/// 按 Store 分组的热点 Peer 信息。
pub struct StoreHotPeersInfos {
    #[serde(rename = "as_leader")]
    pub AsLeader: HashMap<u64, HotPeersStat>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
/// Region 级统计（读写流量等）。
pub struct RegionStats {
    #[serde(rename = "count")]
    pub Count: i64,
}

/// PD（Placement Driver）HTTP/客户端抽象：Region、热点查询等。
pub trait PdClient: Send + Sync {
    /// 附带调用方 ID 的客户端视图。
    fn WithCallerID(&self, caller_id: &str) -> Arc<dyn PdClient>;
    /// 按键范围查询 Region 列表。
    fn GetRegionsByKeyRange(
        &self,
        ctx: &RequestContext,
        start_key: &[u8],
        end_key: &[u8],
        limit: i32,
    ) -> Result<RegionsInfo>;
    /// 获取读热点 Region。
    fn GetHotReadRegions(&self, ctx: &RequestContext) -> Result<StoreHotPeersInfos>;
    /// 获取写热点 Region。
    fn GetHotWriteRegions(&self, ctx: &RequestContext) -> Result<StoreHotPeersInfos>;
    /// 按键范围获取 Region 状态统计。
    fn GetRegionStatusByKeyRange(
        &self,
        ctx: &RequestContext,
        start_key: &[u8],
        end_key: &[u8],
        only_count: bool,
    ) -> Result<RegionStats>;
}

/// 默认 trait 方法：标记尚未实现。
fn unsupported<T>(method: &str) -> Result<T> {
    bail!("{method} is not implemented by this storage")
}

/// Storage retains the Go interface surface. Methods unused by this helper have
/// defaults so codec-only and HTTP-only stores remain easy to construct.
/// 存储引擎抽象：事务、快照、Region 缓存、PD 客户端等。
pub trait Storage: Send + Sync {
    /// 开启事务。
    fn Begin(&self, _: Vec<TxnOption>) -> Result<Transaction> {
        unsupported("Begin")
    }
    /// 获取指定版本快照。
    fn GetSnapshot(&self, _: u64) -> Result<Snapshot> {
        unsupported("GetSnapshot")
    }
    /// 获取 KV 客户端。
    fn GetClient(&self) -> Result<KvClient> {
        unsupported("GetClient")
    }
    /// 获取 MPP 客户端。
    fn GetMPPClient(&self) -> Result<MppClient> {
        unsupported("GetMPPClient")
    }
    /// 关闭存储。
    fn Close(&self) -> Result<()> {
        Ok(())
    }
    /// 存储实例 UUID。
    fn UUID(&self) -> String {
        String::new()
    }
    /// 当前版本号。
    fn CurrentVersion(&self, _: &str) -> Result<u64> {
        unsupported("CurrentVersion")
    }
    /// 当前时间戳。
    fn CurrentTimestamp(&self, _: &str) -> Result<u64> {
        unsupported("CurrentTimestamp")
    }
    /// 获取 Oracle。
    fn GetOracle(&self) -> Result<Arc<dyn Oracle>> {
        unsupported("GetOracle")
    }
    /// 是否支持 delete-range。
    fn SupportDeleteRange(&self) -> bool {
        false
    }
    /// 存储名称。
    fn Name(&self) -> String {
        String::new()
    }
    /// 存储描述。
    fn Describe(&self) -> String {
        String::new()
    }
    /// 展示状态。
    fn ShowStatus(&self, _: &RequestContext, _: &str) -> Result<Box<dyn Any + Send + Sync>> {
        unsupported("ShowStatus")
    }
    /// 获取内存缓存管理器。
    fn GetMemCache(&self) -> Result<MemManager> {
        unsupported("GetMemCache")
    }
    /// 获取 Region 缓存。
    fn GetRegionCache(&self) -> Arc<dyn RegionCache> {
        Arc::new(EmptyRegionCache)
    }
    /// 发送 KV 请求。
    fn SendReq(
        &self,
        _: &mut Backoffer,
        _: KvRequest,
        _: RegionVerID,
        _: Duration,
    ) -> Result<KvResponse> {
        unsupported("SendReq")
    }
    /// 获取锁解析器。
    fn GetLockResolver(&self) -> Result<Arc<dyn LockResolver>> {
        unsupported("GetLockResolver")
    }
    /// 获取安全点 KV。
    fn GetSafePointKV(&self) -> Result<SafePointKv> {
        unsupported("GetSafePointKV")
    }
    fn UpdateTxnSafePointCache(&self, _: u64, _: Instant) {}
    fn SetOracle(&self, _: Arc<dyn Oracle>) {}
    fn SetTiKVClient(&self, _: TikvClient) {}
    /// 获取 TiKV 客户端。
    fn GetTiKVClient(&self) -> Result<TikvClient> {
        unsupported("GetTiKVClient")
    }
    /// 是否已关闭。
    fn Closed(&self) -> bool {
        false
    }
    /// 最小安全时间戳。
    fn GetMinSafeTS(&self, _: &str) -> u64 {
        0
    }
    /// 获取锁等待信息。
    fn GetLockWaits(&self) -> Result<Vec<WaitForEntry>> {
        unsupported("GetLockWaits")
    }
    /// 获取键编解码器。
    fn GetCodec(&self) -> Codec {
        Codec::v1()
    }
    /// 获取 PD HTTP 客户端。
    fn GetPDHTTPClient(&self) -> Option<Arc<dyn PdClient>> {
        None
    }
    fn GetOption(&self, _: &dyn Any) -> Option<Box<dyn Any + Send + Sync>> {
        None
    }
    fn SetOption(&self, _: Box<dyn Any + Send + Sync>, _: Box<dyn Any + Send + Sync>) {}
    /// 集群 ID。
    fn GetClusterID(&self) -> u64 {
        0
    }
    /// 当前 Keyspace 名。
    fn GetKeyspace(&self) -> String {
        String::new()
    }
    /// PD 地址列表。
    fn GetPDAddrs(&self) -> Result<Vec<String>> {
        unsupported("GetPDAddrs")
    }
}

/// Store Helper 主体，封装对 Storage/PD 的诊断查询。
pub struct Helper {
    pub Store: Option<Arc<dyn Storage>>,
    pub RegionCache: Option<Arc<dyn RegionCache>>,
    pdHTTPCli: Option<Arc<dyn PdClient>>,
}

impl Default for Helper {
    fn default() -> Self {
        Self {
            Store: None,
            RegionCache: None,
            pdHTTPCli: None,
        }
    }
}

/// 由 Storage 构造 Helper。
pub fn NewHelper(store: Arc<dyn Storage>) -> Helper {
    let region_cache = store.GetRegionCache();
    Helper {
        Store: Some(store),
        RegionCache: Some(region_cache),
        pdHTTPCli: None,
    }
}

impl Helper {
    /// 惰性获取 PD HTTP 客户端。
    pub fn TryGetPDHTTPClient(&mut self) -> Result<Arc<dyn PdClient>> {
        if let Some(client) = &self.pdHTTPCli {
            return Ok(client.clone());
        }
        let client = self
            .Store
            .as_ref()
            .and_then(|store| store.GetPDHTTPClient())
            .ok_or_else(|| anyhow!("pd http client unavailable"))?;
        let client = client.WithCallerID("tidb-store-helper");
        self.pdHTTPCli = Some(client.clone());
        Ok(client)
    }

    /// 拉取全量或上下文相关的 Region 列表。
    pub fn GetRegions(&mut self, ctx: &RequestContext) -> Result<RegionsInfo> {
        let client = self.TryGetPDHTTPClient()?;
        let store = self
            .Store
            .as_ref()
            .ok_or_else(|| anyhow!("storage unavailable"))?;
        // 空起止键表示全键空间；按 Codec（含 keyspace）编码后查询。
        let (start, end) = store.GetCodec().EncodeRegionRange(&[], &[]);
        client.GetRegionsByKeyRange(ctx, &start, &end, -1)
    }
}

/// MVCC 查询的最大退避超时（毫秒）。
pub const MaxBackoffTimeoutForMvccGet: u64 = 5_000;

impl Helper {
    /// 按编码键与时间戳查询 MVCC；遇锁则解析后重试。
    pub fn GetMvccByEncodedKeyWithTS(
        &self,
        encoded_key: Key,
        start_ts: u64,
    ) -> Result<MvccGetByKeyResponse> {
        let store = self
            .Store
            .as_ref()
            .ok_or_else(|| anyhow!("storage unavailable"))?;
        let cache = self
            .RegionCache
            .as_ref()
            .ok_or_else(|| anyhow!("region cache unavailable"))?;
        // 定位 Region 并发送 MVCC 查询；遇 Region 错误或锁则退避重试。
        let mut backoffer = Backoffer::new(MaxBackoffTimeoutForMvccGet);
        loop {
            let location = cache.LocateKey(&mut backoffer, &encoded_key)?;
            let response = store.SendReq(
                &mut backoffer,
                KvRequest::MvccGetByKey {
                    key: encoded_key.clone(),
                },
                location.Region,
                Duration::from_secs(60),
            )?;
            let KvResponse::MvccGetByKey(response) = response else {
                bail!("unexpected response for MVCC get by key");
            };
            // Region epoch 过期等错误：退避后重新定位。
            if let Some(region_error) = &response.RegionError {
                backoffer.Backoff(1, region_error)?;
                continue;
            }
            if !response.Error.is_empty() {
                bail!(response.Error.clone());
            }
            let info = response
                .Info
                .as_ref()
                .ok_or_else(|| anyhow!("Invalid mvcc response result, the info field is nil"))?;
            // 快照时间戳晚于锁：尝试 ResolveLocks 后再查。
            if start_ts > 0
                && let Some(lock_info) = &info.Lock
            {
                let latest_ts = store.GetOracle()?.GetLowResolutionTimestamp()?;
                if start_ts > latest_ts {
                    bail!(
                        "Snapshot ts={} is larger than latest allocated ts={}, lock could not be resolved",
                        start_ts,
                        latest_ts
                    );
                }
                let lock = Lock {
                    Key: encoded_key.clone(),
                    Primary: lock_info.Primary.clone(),
                    TxnID: lock_info.StartTS,
                    TTL: lock_info.TTL,
                    TxnSize: lock_info.TxnSize,
                    LockType: lock_info.LockType,
                    UseAsyncCommit: lock_info.UseAsyncCommit,
                    LockForUpdateTS: lock_info.ForUpdateTS,
                };
                let result = store.GetLockResolver()?.ResolveLocksWithOpts(
                    &mut backoffer,
                    ResolveLocksOptions {
                        CallerStartTS: start_ts,
                        Locks: vec![lock.clone()],
                        Lite: true,
                        ForRead: false,
                    },
                )?;
                if result.TTL > 0 {
                    backoffer.Backoff(result.TTL, format!("resolve lock fails lock: {lock:?}"))?;
                }
                continue;
            }
            return Ok(response);
        }
    }

    /// 按编码键查询 MVCC（使用低精度时间戳）。
    pub fn GetMvccByEncodedKey(&self, encoded_key: Key) -> Result<MvccGetByKeyResponse> {
        self.GetMvccByEncodedKeyWithTS(encoded_key, 0)
    }

    /// 按事务 start_ts 反查 MVCC 信息。
    pub fn GetMvccByStartTs(
        &self,
        start_ts: u64,
        mut start_key: Key,
        end_key: Key,
    ) -> Result<Option<MvccKV>> {
        let store = self
            .Store
            .as_ref()
            .ok_or_else(|| anyhow!("storage unavailable"))?;
        let cache = self
            .RegionCache
            .as_ref()
            .ok_or_else(|| anyhow!("region cache unavailable"))?;
        let mut backoffer = Backoffer::new(5_000);
        loop {
            let location = cache.LocateKey(&mut backoffer, &start_key)?;
            let response = store.SendReq(
                &mut backoffer,
                KvRequest::MvccGetByStartTs {
                    start_ts,
                    low_priority: true,
                },
                location.Region,
                Duration::from_secs(60 * 60),
            )?;
            let KvResponse::MvccGetByStartTs(data) = response else {
                bail!("unexpected response for MVCC get by start timestamp");
            };
            if let Some(region_error) = &data.RegionError {
                backoffer.Backoff(1, region_error)?;
                continue;
            }
            if !data.Error.is_empty() {
                bail!(data.Error);
            }
            if !data.Key.is_empty() {
                return Ok(Some(MvccKV {
                    Key: bytesKeyToHex(&data.Key),
                    RegionID: location.Region.ID,
                    Value: MvccGetByKeyResponse {
                        Info: data.Info,
                        RegionError: data.RegionError,
                        Error: data.Error,
                    },
                }));
            }
            if (!end_key.is_empty() && location.Contains(&end_key)) || location.EndKey.is_empty() {
                return Ok(None);
            }
            start_key = location.EndKey;
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
/// MVCC 键值展示结构。
pub struct MvccKV {
    pub Key: String,
    pub RegionID: u64,
    #[serde(skip)]
    pub Value: MvccGetByKeyResponse,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
/// Region 流量/负载指标。
pub struct RegionMetric {
    pub FlowBytes: u64,
    pub MaxHotDegree: i32,
    pub Count: i32,
}

/// 热点类型：读。
pub const HotRead: &str = "read";
/// 热点类型：写。
pub const HotWrite: &str = "write";

impl Helper {
    /// 从 PD 拉取指定类型（读/写）的热点 Region。
    pub fn FetchHotRegion(
        &mut self,
        ctx: &RequestContext,
        read_or_write: &str,
    ) -> Result<HashMap<u64, RegionMetric>> {
        let client = self.TryGetPDHTTPClient()?;
        let response = match read_or_write {
            HotRead => client.GetHotReadRegions(ctx)?,
            HotWrite => client.GetHotWriteRegions(ctx)?,
            value => bail!("unknown hot-region kind {value:?}"),
        };
        let capacity = response
            .AsLeader
            .values()
            .map(|entry| entry.Stats.len())
            .sum();
        let mut metrics = HashMap::with_capacity(capacity);
        for peer in response.AsLeader.values().flat_map(|entry| &entry.Stats) {
            metrics.insert(
                peer.RegionID,
                RegionMetric {
                    FlowBytes: peer.ByteRate as u64,
                    MaxHotDegree: peer.HotDegree,
                    Count: 0,
                },
            );
        }
        Ok(metrics)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 大小写不敏感字符串（CiStr），用于库表名比较。
pub struct CIStr {
    pub O: String,
    pub L: String,
}

impl CIStr {
    /// 由原始字符串构造 CIStr。
    pub fn new(value: &str) -> Self {
        Self {
            O: value.to_owned(),
            L: value.to_lowercase(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 索引元信息。
pub struct IndexMeta {
    pub ID: i64,
    pub Name: CIStr,
    pub Global: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 分区定义。
pub struct PartitionDefinition {
    pub ID: i64,
    pub Name: CIStr,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 表分区信息。
pub struct PartitionInfo {
    pub Definitions: Vec<PartitionDefinition>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 表元信息（含索引、分区）。
pub struct TableMeta {
    pub ID: i64,
    pub Name: CIStr,
    pub Indices: Vec<Arc<IndexMeta>>,
    pub Partition: Option<PartitionInfo>,
    pub IsCommonHandle: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 数据库及其表集合。
pub struct DatabaseInfo {
    pub Name: CIStr,
    pub Tables: Vec<Arc<TableMeta>>,
}

/// Schema 提供者：枚举库表元数据。
pub trait SchemaAndTable: Send + Sync {
    /// 返回全部数据库。
    fn AllSchemas(&self) -> Vec<Arc<DatabaseInfo>>;
    /// 返回指定库下的表元信息。
    fn SchemaTableInfos(&self, database_name: &CIStr) -> Result<Vec<Arc<TableMeta>>>;
}

/// Schema 过滤器回调类型。
pub type SchemaFilter = dyn Fn(Vec<Arc<DatabaseInfo>>) -> Vec<Arc<DatabaseInfo>> + Send + Sync;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 表或索引在键空间中的定位描述。
pub struct TblIndex {
    pub DbName: String,
    pub TableName: String,
    pub TableID: i64,
    pub IndexName: String,
    pub IndexID: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
/// Region 边界对应的表/索引帧（frame）项。
pub struct FrameItem {
    pub DBName: String,
    pub TableName: String,
    pub TableID: i64,
    pub IsRecord: bool,
    pub RecordID: i64,
    pub IndexName: String,
    pub IndexID: i64,
    pub IndexValues: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Region 起止边界对应的帧范围。
pub struct RegionFrameRange {
    pub First: FrameItem,
    pub Last: FrameItem,
    pub region: KeyLocation,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
/// 热点 Region 关联的表/索引信息。
pub struct HotTableIndex {
    pub RegionID: u64,
    pub RegionMetric: RegionMetric,
    pub DbName: String,
    pub TableName: String,
    pub TableID: i64,
    pub IndexName: String,
    pub IndexID: i64,
}

impl Helper {
    /// 采集热点信息并关联到表/索引。
    pub fn ScrapeHotInfo(
        &mut self,
        ctx: &RequestContext,
        read_or_write: &str,
        info_schema: &dyn SchemaAndTable,
        filter: Option<&SchemaFilter>,
    ) -> Result<Vec<HotTableIndex>> {
        let metrics = self.FetchHotRegion(ctx, read_or_write)?;
        self.FetchRegionTableIndex(metrics, info_schema, filter)
    }

    /// 为热点 Region 解析所属表/索引。
    pub fn FetchRegionTableIndex(
        &self,
        metrics: HashMap<u64, RegionMetric>,
        info_schema: &dyn SchemaAndTable,
        _filter: Option<&SchemaFilter>,
    ) -> Result<Vec<HotTableIndex>> {
        let cache = self
            .RegionCache
            .as_ref()
            .ok_or_else(|| anyhow!("region cache unavailable"))?;
        let mut hot_tables = Vec::with_capacity(metrics.len());
        for (region_id, region_metric) in metrics {
            let mut item = HotTableIndex {
                RegionID: region_id,
                RegionMetric: region_metric,
                ..Default::default()
            };
            let mut backoffer = Backoffer::new(500);
            let Ok(location) = cache.LocateRegionByID(&mut backoffer, region_id) else {
                continue;
            };
            let mut range = NewRegionFrameRange(location)?;
            if let Some(frame) = self.FindTableIndexOfRegion(info_schema, &mut range) {
                item.DbName = frame.DBName;
                item.TableName = frame.TableName;
                item.TableID = frame.TableID;
                item.IndexName = frame.IndexName;
                item.IndexID = frame.IndexID;
            }
            hot_tables.push(item);
        }
        Ok(hot_tables)
    }

    /// 在 schema 中查找覆盖该 Region 的表/索引。
    pub fn FindTableIndexOfRegion(
        &self,
        info_schema: &dyn SchemaAndTable,
        hot_range: &mut RegionFrameRange,
    ) -> Option<FrameItem> {
        for database in info_schema.AllSchemas() {
            let Ok(tables) = info_schema.SchemaTableInfos(&database.Name) else {
                continue;
            };
            for table in tables {
                if let Some(frame) = findRangeInTable(hot_range, &database, &table) {
                    return Some(frame);
                }
            }
        }
        None
    }
}

/// 在逻辑表（含分区）中查找与 Region 相交的范围。
pub fn findRangeInTable(
    hot_range: &mut RegionFrameRange,
    database: &DatabaseInfo,
    table: &TableMeta,
) -> Option<FrameItem> {
    // 分区表：逐物理分区匹配键范围。
    if let Some(partition) = &table.Partition {
        for definition in &partition.Definitions {
            let partition_name = format!("{}({})", table.Name.O, definition.Name.O);
            if let Some(frame) = findRangeInPhysicalTable(
                hot_range,
                definition.ID,
                &database.Name.O,
                &partition_name,
                &table.Indices,
                table.IsCommonHandle,
            ) {
                return Some(frame);
            }
        }
        None
    } else {
        findRangeInPhysicalTable(
            hot_range,
            table.ID,
            &database.Name.O,
            &table.Name.O,
            &table.Indices,
            table.IsCommonHandle,
        )
    }
}

/// 在物理表键空间中查找与 Region 相交的记录/索引范围。
pub fn findRangeInPhysicalTable(
    hot_range: &mut RegionFrameRange,
    physical_id: i64,
    database_name: &str,
    table_name: &str,
    indices: &[Arc<IndexMeta>],
    is_common_handle: bool,
) -> Option<FrameItem> {
    if let Some(frame) =
        hot_range.GetRecordFrame(physical_id, database_name, table_name, is_common_handle)
    {
        return Some(frame);
    }
    for index in indices {
        if let Some(frame) = hot_range.GetIndexFrame(
            physical_id,
            index.ID,
            database_name,
            table_name,
            &index.Name.O,
        ) {
            return Some(frame);
        }
    }
    None
}

/// 由 Region 定位信息构造帧范围。
pub fn NewRegionFrameRange(region: KeyLocation) -> Result<RegionFrameRange> {
    let first = if region.StartKey.is_empty() {
        FrameItem {
            TableID: i64::MIN,
            IndexID: i64::MIN,
            IsRecord: false,
            ..Default::default()
        }
    } else {
        NewFrameItemFromRegionKey(region.StartKey.clone())?
    };
    let last = if region.EndKey.is_empty() {
        FrameItem {
            TableID: i64::MAX,
            IndexID: i64::MAX,
            IsRecord: true,
            ..Default::default()
        }
    } else {
        NewFrameItemFromRegionKey(region.EndKey.clone())?
    };
    Ok(RegionFrameRange {
        First: first,
        Last: last,
        region,
    })
}

/// 解码 Region 边界键为 FrameItem（表/索引 ID 等）。
pub fn NewFrameItemFromRegionKey(key: Vec<u8>) -> Result<FrameItem> {
    let table_key = tablecodec::kv::Key(key.clone());
    match tablecodec::DecodeKeyHead(table_key.clone()) {
        Ok((table_id, index_id, is_record)) => {
            let mut frame = FrameItem {
                TableID: table_id,
                IndexID: index_id,
                IsRecord: is_record,
                ..Default::default()
            };
            if is_record {
                if let Ok((_, handle)) = tablecodec::DecodeRecordKey(table_key) {
                    if handle.IsInt() {
                        frame.RecordID = handle.IntValue();
                    } else {
                        frame.IndexName = "PRIMARY".to_owned();
                        frame.IndexValues = handle
                            .Data()
                            .map_err(|error| anyhow!(error.to_string()))?
                            .into_iter()
                            .map(|datum| {
                                datum.ToString().map_err(|error| anyhow!(error.to_string()))
                            })
                            .collect::<Result<Vec<_>>>()?;
                    }
                }
            } else if let Ok((_, _, values)) = tablecodec::DecodeIndexKey(table_key) {
                frame.IndexValues = values;
            }
            // Go intentionally ignores record/index payload decode errors once
            // the table/index/record head has been decoded.
            Ok(frame)
        }
        Err(head_error) => {
            if key.starts_with(tablecodec::TablePrefix()) {
                if key.len() == tablecodec::TableSplitKeyLen {
                    return Ok(FrameItem {
                        TableID: tablecodec::DecodeTableID(tablecodec::kv::Key(key)),
                        ..Default::default()
                    });
                }
                return Err(anyhow!(head_error.to_string()));
            }
            if key.as_slice() < tablecodec::TablePrefix() {
                Ok(FrameItem {
                    TableID: i64::MIN,
                    IndexID: i64::MIN,
                    IsRecord: false,
                    ..Default::default()
                })
            } else {
                // This preserves helper.go, including its duplicate TableID
                // assignment (IndexID therefore remains zero).
                Ok(FrameItem {
                    TableID: i64::MAX,
                    IsRecord: true,
                    ..Default::default()
                })
            }
        }
    }
}

impl RegionFrameRange {
    /// 获取记录（行数据）键空间上的帧项。
    pub fn GetRecordFrame(
        &mut self,
        table_id: i64,
        database_name: &str,
        table_name: &str,
        is_common_handle: bool,
    ) -> Option<FrameItem> {
        let mut frame = if table_id == self.First.TableID && self.First.IsRecord {
            self.First.DBName = database_name.to_owned();
            self.First.TableName = table_name.to_owned();
            Some(self.First.clone())
        } else if table_id == self.Last.TableID && self.Last.IsRecord {
            self.Last.DBName = database_name.to_owned();
            self.Last.TableName = table_name.to_owned();
            Some(self.Last.clone())
        } else if table_id >= self.First.TableID && table_id < self.Last.TableID {
            Some(FrameItem {
                DBName: database_name.to_owned(),
                TableName: table_name.to_owned(),
                TableID: table_id,
                IsRecord: true,
                ..Default::default()
            })
        } else {
            None
        };
        if is_common_handle && let Some(frame) = &mut frame {
            frame.IndexName = "PRIMARY".to_owned();
            if table_id == self.First.TableID && self.First.IsRecord {
                self.First.IndexName = frame.IndexName.clone();
            } else if table_id == self.Last.TableID && self.Last.IsRecord {
                self.Last.IndexName = frame.IndexName.clone();
            }
        }
        frame
    }

    /// 获取索引键空间上的帧项。
    pub fn GetIndexFrame(
        &mut self,
        table_id: i64,
        index_id: i64,
        database_name: &str,
        table_name: &str,
        index_name: &str,
    ) -> Option<FrameItem> {
        if table_id == self.First.TableID && !self.First.IsRecord && index_id == self.First.IndexID
        {
            self.First.DBName = database_name.to_owned();
            self.First.TableName = table_name.to_owned();
            self.First.IndexName = index_name.to_owned();
            return Some(self.First.clone());
        }
        if table_id == self.Last.TableID && index_id == self.Last.IndexID {
            self.Last.DBName = database_name.to_owned();
            self.Last.TableName = table_name.to_owned();
            self.Last.IndexName = index_name.to_owned();
            return Some(self.Last.clone());
        }
        let greater_than_first = table_id > self.First.TableID
            || (table_id == self.First.TableID
                && !self.First.IsRecord
                && index_id > self.First.IndexID);
        let less_than_last = table_id < self.Last.TableID
            || (table_id == self.Last.TableID
                && (self.Last.IsRecord || index_id < self.Last.IndexID));
        (greater_than_first && less_than_last).then(|| FrameItem {
            DBName: database_name.to_owned(),
            TableName: table_name.to_owned(),
            TableID: table_id,
            IsRecord: false,
            IndexName: index_name.to_owned(),
            IndexID: index_id,
            ..Default::default()
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 简化表信息（用于区间交集计算）。
pub struct TableInfo {
    pub DB: Arc<DatabaseInfo>,
    pub Table: Arc<TableMeta>,
    pub IsPartition: bool,
    pub Partition: Option<PartitionDefinition>,
    pub IsIndex: bool,
    pub Index: Option<Arc<IndexMeta>>,
}

/// 可提供起止键的区间抽象。
pub trait WithKeyRange {
    /// 区间起始键（十六进制或编码串）。
    fn GetStartKey(&self) -> &str;
    /// 区间结束键。
    fn GetEndKey(&self) -> &str;
}

/// 判断两个键区间是否相交。
pub fn isIntersecting<X: WithKeyRange, Y: WithKeyRange>(left: &X, right: &Y) -> bool {
    isIntersectingKeyRange(left, right.GetStartKey(), right.GetEndKey())
}

/// 判断对象区间是否与给定起止键相交。
pub fn isIntersectingKeyRange<X: WithKeyRange>(value: &X, start_key: &str, end_key: &str) -> bool {
    !isBeforeKeyRange(value, start_key, end_key) && !isBehindKeyRange(value, start_key, end_key)
}

/// 判断 left 是否完全在 right 之后。
pub fn isBehind<X: WithKeyRange, Y: WithKeyRange>(left: &X, right: &Y) -> bool {
    isBehindKeyRange(left, right.GetStartKey(), right.GetEndKey())
}

/// 判断对象是否完全在给定 start 之前。
pub fn isBeforeKeyRange<X: WithKeyRange>(value: &X, start_key: &str, _: &str) -> bool {
    !value.GetEndKey().is_empty() && value.GetEndKey() <= start_key
}

/// 判断对象是否完全在给定 end 之后。
pub fn isBehindKeyRange<X: WithKeyRange>(value: &X, _: &str, end_key: &str) -> bool {
    !end_key.is_empty() && value.GetStartKey() >= end_key
}

impl WithKeyRange for RegionInfo {
    fn GetStartKey(&self) -> &str {
        &self.StartKey
    }
    fn GetEndKey(&self) -> &str {
        &self.EndKey
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 带编码起止键的表/索引信息。
pub struct TableInfoWithKeyRange {
    pub TableInfo: TableInfo,
    pub StartKey: String,
    pub EndKey: String,
}

impl WithKeyRange for TableInfoWithKeyRange {
    fn GetStartKey(&self) -> &str {
        &self.StartKey
    }
    fn GetEndKey(&self) -> &str {
        &self.EndKey
    }
}

/// 为表构造带键范围的描述（按 Codec 版本编码）。
pub fn NewTableWithKeyRange(
    database: Arc<DatabaseInfo>,
    table: Arc<TableMeta>,
    codec: Codec,
) -> TableInfoWithKeyRange {
    newTableInfoWithKeyRange(database, table, None, None, codec)
}

/// 为索引构造带键范围的描述。
pub fn NewIndexWithKeyRange(
    database: Arc<DatabaseInfo>,
    table: Arc<TableMeta>,
    index: Arc<IndexMeta>,
    codec: Codec,
) -> TableInfoWithKeyRange {
    newTableInfoWithKeyRange(database, table, None, Some(index), codec)
}

/// 内部：组合表信息与起止键。
pub fn newTableInfoWithKeyRange(
    database: Arc<DatabaseInfo>,
    table: Arc<TableMeta>,
    partition: Option<PartitionDefinition>,
    index: Option<Arc<IndexMeta>>,
    codec: Codec,
) -> TableInfoWithKeyRange {
    let physical_id = partition
        .as_ref()
        .map_or(table.ID, |partition| partition.ID);
    let (start, end) = if let Some(index) = &index {
        tablecodec::GetTableIndexKeyRange(physical_id, index.ID)
    } else {
        tablecodec::GetTableHandleKeyRange(physical_id)
    };
    let (start, end) = codec.EncodeRegionRange(&start, &end);
    TableInfoWithKeyRange {
        TableInfo: TableInfo {
            DB: database,
            Table: table,
            IsPartition: partition.is_some(),
            Partition: partition,
            IsIndex: index.is_some(),
            Index: index,
        },
        StartKey: bytesKeyToHex(&start),
        EndKey: bytesKeyToHex(&end),
    }
}

impl Helper {
    /// 过滤内存库（INFORMATION_SCHEMA 等）以外的用户库。
    pub fn FilterMemDBs(&self, schemas: Vec<Arc<DatabaseInfo>>) -> Vec<Arc<DatabaseInfo>> {
        schemas
            .into_iter()
            .filter(|database| !metadef::IsMemDB(&database.Name.L))
            .collect()
    }

    /// 收集与键范围相交的表/索引信息。
    pub fn GetTablesInfoWithKeyRange(
        &self,
        info_schema: &dyn SchemaAndTable,
        filter: Option<&SchemaFilter>,
    ) -> Vec<TableInfoWithKeyRange> {
        let mut databases = info_schema.AllSchemas();
        if let Some(filter) = filter {
            databases = filter(databases);
        }
        let codec = self
            .Store
            .as_ref()
            .map_or_else(Codec::v1, |store| store.GetCodec());
        let mut tables = Vec::new();
        for database in databases {
            let Ok(table_infos) = info_schema.SchemaTableInfos(&database.Name) else {
                continue;
            };
            for table in table_infos {
                if let Some(partition) = &table.Partition {
                    for definition in &partition.Definitions {
                        tables.push(newTableInfoWithKeyRange(
                            database.clone(),
                            table.clone(),
                            Some(definition.clone()),
                            None,
                            codec.clone(),
                        ));
                    }
                } else {
                    tables.push(newTableInfoWithKeyRange(
                        database.clone(),
                        table.clone(),
                        None,
                        None,
                        codec.clone(),
                    ));
                }
                for index in &table.Indices {
                    if table.Partition.is_none() || index.Global {
                        tables.push(newTableInfoWithKeyRange(
                            database.clone(),
                            table.clone(),
                            None,
                            Some(index.clone()),
                            codec.clone(),
                        ));
                    } else if let Some(partition) = &table.Partition {
                        for definition in &partition.Definitions {
                            tables.push(newTableInfoWithKeyRange(
                                database.clone(),
                                table.clone(),
                                Some(definition.clone()),
                                Some(index.clone()),
                                codec.clone(),
                            ));
                        }
                    }
                }
            }
        }
        tables.sort_by(|left, right| left.StartKey.cmp(&right.StartKey));
        tables
    }

    /// 查询各 Region 覆盖的表信息。
    pub fn GetRegionsTableInfo(
        &self,
        regions: RegionsInfo,
        info_schema: &dyn SchemaAndTable,
        filter: Option<&SchemaFilter>,
    ) -> HashMap<i64, Vec<TableInfo>> {
        let tables = self.GetTablesInfoWithKeyRange(info_schema, filter);
        ParseRegionsTableInfos(regions.Regions, tables)
    }

    /// Helper 方法：解析 Region 与表的映射。
    pub fn ParseRegionsTableInfos(
        &self,
        regions: Vec<RegionInfo>,
        tables: Vec<TableInfoWithKeyRange>,
    ) -> HashMap<i64, Vec<TableInfo>> {
        ParseRegionsTableInfos(regions, tables)
    }
}

/// 将 Region 列表与表键范围做交集，生成 Region→表 映射。
pub fn ParseRegionsTableInfos(
    mut regions: Vec<RegionInfo>,
    tables: Vec<TableInfoWithKeyRange>,
) -> HashMap<i64, Vec<TableInfo>> {
    let mut result = HashMap::with_capacity(regions.len());
    if tables.is_empty() || regions.is_empty() {
        return result;
    }
    regions.sort_by(|left, right| left.StartKey.cmp(&right.StartKey));
    let mut table_index = 0;
    // 对每个 Region 求与表/索引键范围的半开区间交集。
    for region in regions {
        result.insert(region.ID, Vec::new());
        while isBehind(&region, &tables[table_index]) {
            table_index += 1;
            if table_index >= tables.len() {
                return result;
            }
        }
        let mut index = table_index;
        while index < tables.len() && isIntersecting(&region, &tables[index]) {
            result
                .get_mut(&region.ID)
                .expect("region inserted")
                .push(tables[index].TableInfo.clone());
            index += 1;
        }
    }
    result
}

/// 键字节转十六进制字符串。
pub fn bytesKeyToHex(key: &[u8]) -> String {
    hex::encode_upper(key)
}

impl Helper {
    /// 获取 PD 地址列表。
    pub fn GetPDAddr(&self) -> Result<Vec<String>> {
        let store = self
            .Store
            .as_ref()
            .ok_or_else(|| anyhow!("not implemented"))?;
        let addresses = store.GetPDAddrs().context("get PD addresses")?;
        if addresses.is_empty() {
            bail!("pd unavailable");
        }
        Ok(addresses)
    }

    /// 按键范围向 PD 查询 Region 统计。
    pub fn GetPDRegionStats(
        &mut self,
        ctx: &RequestContext,
        table_id: i64,
        no_index_stats: bool,
    ) -> Result<RegionStats> {
        let client = self.TryGetPDHTTPClient()?;
        let store = self
            .Store
            .as_ref()
            .ok_or_else(|| anyhow!("storage unavailable"))?;
        let start = if no_index_stats {
            tablecodec::GenTableRecordPrefix(table_id).0
        } else {
            tablecodec::EncodeTablePrefix(table_id).0
        };
        let end = prefix_next(&start);
        let (start, end) = store.GetCodec().EncodeRegionRange(&start, &end);
        client.GetRegionStatusByKeyRange(ctx, &start, &end, false)
    }
}

/// 计算字典序上的下一个前缀（用于半开区间上界）。
fn prefix_next(key: &[u8]) -> Vec<u8> {
    let mut next = key.to_vec();
    for index in (0..next.len()).rev() {
        next[index] = next[index].wrapping_add(1);
        if next[index] != 0 {
            return next;
        }
    }
    next.clear();
    next.extend_from_slice(key);
    next.push(0);
    next
}

/// 从 TiFlash 相关 end key 解析表 ID。
pub fn GetTiFlashTableIDFromEndKey(end_key: &str) -> i64 {
    let encoded = hex::decode(end_key).unwrap_or_default();
    let decoded = tablecodec::codec::DecodeBytes(&encoded, None)
        .map(|(_, decoded)| decoded)
        .unwrap_or_default();
    tablecodec::DecodeTableID(tablecodec::kv::Key(decoded)) - 1
}

/// 解析 TiFlash 状态响应文本，累加各 Region 副本数。
pub fn ComputeTiFlashStatus<R: BufRead>(
    reader: &mut R,
    region_replica: &mut HashMap<i64, i32>,
) -> Result<()> {
    let mut count_line = String::new();
    if reader.read_line(&mut count_line)? == 0 {
        bail!("unexpected EOF while reading TiFlash region count");
    }
    let claimed_count: i64 = count_line.trim_matches(['\r', '\n', '\t']).parse()?;
    let mut regions_line = String::new();
    if reader.read_line(&mut regions_line)? == 0 {
        bail!("unexpected EOF while reading TiFlash regions");
    }
    let mut actual_count = 0_i64;
    for region in regions_line.trim_matches(['\r', '\n', '\t']).split(' ') {
        if region.is_empty() {
            continue;
        }
        actual_count += 1;
        let region: i64 = region.parse()?;
        *region_replica.entry(region).or_insert(0) += 1;
    }
    // helper.go only logs a claimed/actual mismatch; it is deliberately not an error.
    let _count_mismatch = claimed_count != actual_count;
    Ok(())
}

/// 内部 HTTP 使用的 URL scheme。
fn internal_http_schema() -> String {
    std::env::var("TIDB_INTERNAL_HTTP_SCHEMA").unwrap_or_else(|_| "http".to_owned())
}

/// 拼接状态查询 URL。
fn status_url(address: &str, path: &str) -> String {
    if address.starts_with("http://") || address.starts_with("https://") {
        format!("{}{}", address.trim_end_matches('/'), path)
    } else {
        format!(
            "{}://{}{}",
            internal_http_schema(),
            address.trim_end_matches('/'),
            path
        )
    }
}

/// 按上下文超时构造阻塞 HTTP 客户端。
fn http_client(ctx: &RequestContext) -> Result<reqwest::blocking::Client> {
    ctx.check()?;
    reqwest::blocking::Client::builder()
        .timeout(ctx.request_timeout())
        .build()
        .context("build internal HTTP client")
}

/// 带上下文地采集 TiFlash Region 副本状态。
pub fn CollectTiFlashStatusWithCtx(
    ctx: &RequestContext,
    status_address: &str,
    keyspace_id: KeyspaceID,
    table_id: i64,
    region_replica: &mut HashMap<i64, i32>,
) -> Result<()> {
    let url = status_url(
        status_address,
        &format!("/tiflash/sync-status/keyspace/{keyspace_id}/table/{table_id}"),
    );
    let response = http_client(ctx)?
        .get(url)
        .send()
        .context("query TiFlash sync status")?;
    ctx.check()?;
    let mut reader = std::io::BufReader::new(response);
    ComputeTiFlashStatus(&mut reader, region_replica)
}

/// 采集 TiFlash 状态（使用默认上下文）。
pub fn CollectTiFlashStatus(
    status_address: &str,
    keyspace_id: KeyspaceID,
    table_id: i64,
    region_replica: &mut HashMap<i64, i32>,
) -> Result<()> {
    CollectTiFlashStatusWithCtx(
        &RequestContext::background(),
        status_address,
        keyspace_id,
        table_id,
        region_replica,
    )
}

/// 将表 schema 同步到 TiFlash。
pub fn SyncTableSchemaToTiFlash(
    status_address: &str,
    keyspace_id: KeyspaceID,
    table_id: i64,
) -> Result<()> {
    let ctx = RequestContext::background();
    let url = status_url(
        status_address,
        &format!("/tiflash/sync-schema/keyspace/{keyspace_id}/table/{table_id}"),
    );
    http_client(&ctx)?
        .get(url)
        .send()
        .context("sync TiFlash table schema")?;
    Ok(())
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
/// Columnar 引擎状态响应。
pub struct ColumnarStatusResp {
    #[serde(rename = "ready")]
    pub Ready: u64,
    #[serde(rename = "vector-index-ready")]
    pub VectorIndexReady: u64,
    #[serde(default, rename = "fts-index-ready")]
    pub FtsIndexReady: u64,
    #[serde(rename = "total")]
    pub Total: u64,
    #[serde(default, skip)]
    pub HasFtsIndexReady: bool,
}

impl<'de> Deserialize<'de> for ColumnarStatusResp {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Payload {
            #[serde(rename = "ready")]
            ready: u64,
            #[serde(rename = "vector-index-ready")]
            vector_index_ready: u64,
            #[serde(default, rename = "fts-index-ready")]
            fts_index_ready: Option<u64>,
            #[serde(rename = "total")]
            total: u64,
        }

        let payload = Payload::deserialize(deserializer)?;
        Ok(Self {
            Ready: payload.ready,
            VectorIndexReady: payload.vector_index_ready,
            FtsIndexReady: payload.fts_index_ready.unwrap_or_default(),
            Total: payload.total,
            HasFtsIndexReady: payload.fts_index_ready.is_some(),
        })
    }
}

/// 带上下文采集 Columnar 状态。
pub fn CollectColumnarStatusWithCtx(
    ctx: &RequestContext,
    status_address: &str,
    keyspace_id: KeyspaceID,
    table_id: i64,
    index_id: Option<i64>,
) -> Result<ColumnarStatusResp> {
    let mut path =
        format!("/kvengine/columnar_status?keyspace_id={keyspace_id}&table_id={table_id}");
    if let Some(index_id) = index_id {
        path.push_str(&format!("&index_id={index_id}"));
    }
    let response = http_client(ctx)?
        .get(status_url(status_address, &path))
        .send()?;
    ctx.check()?;
    let status = response.status();
    if status != reqwest::StatusCode::OK {
        let body = response.text().unwrap_or_default();
        bail!(
            "TiKV columnar status API returned status {}: {}",
            status.as_u16(),
            body
        );
    }
    response
        .json()
        .context("decode TiKV columnar status response")
}

/// 采集 Columnar 状态（默认上下文）。
pub fn CollectColumnarStatus(
    status_address: &str,
    keyspace_id: KeyspaceID,
    table_id: i64,
    index_id: Option<i64>,
) -> Result<ColumnarStatusResp> {
    CollectColumnarStatusWithCtx(
        &RequestContext::background(),
        status_address,
        keyspace_id,
        table_id,
        index_id,
    )
}
