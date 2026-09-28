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

// Mock Region 管理与 Mock PD / GC 状态机。
//
// Region 是键空间分片；本模块维护 Region 元数据、Store/Peer、分裂与扫描，
// 并提供 MockPd 门面与 GcStatesManager（txn/gc safe point 与 GC Barrier）。

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Store 标签键值对（用于调度/亲和）。
pub struct StoreLabel {
    pub key: String,
    pub value: String,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// TiKV Store 元数据：标识、地址与标签。
pub struct Store {
    pub id: u64,
    pub address: String,
    pub labels: Vec<StoreLabel>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Region 副本（Peer）：归属某个 Store。
pub struct Peer {
    pub id: u64,
    pub store_id: u64,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Region 纪元：conf_ver（成员变更）与 version（分裂/合并）。
pub struct RegionEpoch {
    pub conf_ver: u64,
    pub version: u64,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Region 内桶边界键列表（热点调度用）。
pub struct Buckets {
    pub keys: Vec<Vec<u8>>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Region 元数据：键范围 `[start_key, end_key)`、纪元与副本集。
pub struct Region {
    pub id: u64,
    pub start_key: Vec<u8>,
    pub end_key: Vec<u8>,
    pub epoch: RegionEpoch,
    pub peers: Vec<Peer>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Region 运行时上下文：元数据、Leader、桶与宕机 Peer。
pub struct RegionCtx {
    pub meta: Region,
    pub leader: Option<Peer>,
    pub buckets: Option<Buckets>,
    pub down_peers: Vec<Peer>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Region/集群操作错误（未引导、纪元不匹配、分裂键非法等）。
pub enum RegionError {
    NotBootstrapped,
    AlreadyBootstrapped,
    StoreNotMatch,
    RegionNotFound(u64),
    EpochNotMatch(Region),
    SplitKeyOutOfRange,
    InvalidSplitKeys,
}
impl fmt::Display for RegionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for RegionError {}

#[derive(Default)]
/// 内存中的集群状态：Region 索引、Store 表与 MPP 任务。
struct RegionState {
    regions: HashMap<u64, RegionCtx>,
    by_start: BTreeMap<Vec<u8>, u64>,
    stores: HashMap<u64, Store>,
    primary_store: Option<u64>,
    mpp_tasks: HashMap<u64, HashMap<i64, String>>,
}

/// 模拟 PD 侧的 Region 管理器：分配 ID、引导、分裂与 Store 维护。
pub struct MockRegionManager {
    state: RwLock<RegionState>,
    id: AtomicU64,
    cluster_id: u64,
    region_size: i64,
    closed: AtomicBool,
}

impl MockRegionManager {
    /// 创建管理器；`region_size` 供上层估算分裂阈值。
    pub fn new(cluster_id: u64, region_size: i64) -> Self {
        Self {
            state: RwLock::new(RegionState::default()),
            id: AtomicU64::new(0),
            cluster_id,
            region_size,
            closed: AtomicBool::new(false),
        }
    }
    /// 标记管理器已关闭。
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }
    /// 分配下一个全局递增 ID。
    pub fn alloc_id(&self) -> u64 {
        self.id.fetch_add(1, Ordering::AcqRel) + 1
    }
    /// 批量分配连续 ID。
    pub fn alloc_ids(&self, count: usize) -> Vec<u64> {
        let max = self.id.fetch_add(count as u64, Ordering::AcqRel) + count as u64;
        (max - count as u64 + 1..=max).collect()
    }
    /// 返回集群 ID。
    pub fn cluster_id(&self) -> u64 {
        self.cluster_id
    }
    /// 返回配置的 Region 目标大小。
    pub fn region_size(&self) -> i64 {
        self.region_size
    }

    /// 引导集群：注册 Store 与首个 Region（已引导则幂等成功）。
    pub fn bootstrap(&self, stores: Vec<Store>, mut region: Region) -> Result<(), RegionError> {
        let mut state = self.state.write().expect("region manager lock poisoned");
        // 已引导则直接返回，避免重复初始化。
        if !state.regions.is_empty() {
            return Ok(());
        }
        let first = stores.first().ok_or(RegionError::NotBootstrapped)?.id;
        for store in stores {
            self.id.fetch_max(store.id, Ordering::AcqRel);
            state.mpp_tasks.entry(store.id).or_default();
            state.stores.insert(store.id, store);
        }
        // 引导时将纪元置为 1，并更新 ID 生成器水位。
        region.epoch = RegionEpoch {
            conf_ver: 1,
            version: 1,
        };
        self.id.fetch_max(region.id, Ordering::AcqRel);
        for peer in &region.peers {
            self.id
                .fetch_max(peer.id.max(peer.store_id), Ordering::AcqRel);
        }
        let context = RegionCtx {
            leader: region.peers.first().cloned(),
            meta: region.clone(),
            buckets: None,
            down_peers: Vec::new(),
        };
        state.primary_store = Some(first);
        state.by_start.insert(region.start_key.clone(), region.id);
        state.regions.insert(region.id, context);
        Ok(())
    }

    /// 是否已完成引导（存在至少一个 Region）。
    pub fn is_bootstrapped(&self) -> bool {
        !self
            .state
            .read()
            .expect("region manager lock poisoned")
            .regions
            .is_empty()
    }
    /// 按地址查找 Store ID。
    pub fn get_store_id_by_addr(&self, address: &str) -> Result<u64, RegionError> {
        self.state
            .read()
            .expect("region manager lock poisoned")
            .stores
            .values()
            .find(|store| store.address == address)
            .map(|store| store.id)
            .ok_or(RegionError::StoreNotMatch)
    }
    /// 按 Store ID 查找地址。
    pub fn get_store_addr_by_id(&self, store_id: u64) -> Result<String, RegionError> {
        self.state
            .read()
            .expect("region manager lock poisoned")
            .stores
            .get(&store_id)
            .map(|store| store.address.clone())
            .ok_or(RegionError::StoreNotMatch)
    }
    /// 按 Region ID 取元数据。
    pub fn get_region(&self, id: u64) -> Option<Region> {
        self.state
            .read()
            .expect("region manager lock poisoned")
            .regions
            .get(&id)
            .map(|region| region.meta.clone())
    }

    /// 按用户键定位所属 Region（`by_start` 有序索引 + 范围包含判断）。
    pub fn get_region_by_key(&self, key: &[u8]) -> Option<RegionCtx> {
        let state = self.state.read().expect("region manager lock poisoned");
        // 取 start_key ≤ key 的最大 Region；恰好落在 end_key 时，
        // B-tree 查找会继续取下一个 start_key。
        let candidate = state
            .by_start
            .range(..=key.to_vec())
            .next_back()
            .and_then(|(_, id)| state.regions.get(id))
            .filter(|region| contains(&region.meta, key));
        if candidate.is_some() {
            return candidate.cloned();
        }
        state.by_start.range(key.to_vec()..).find_map(|(_, id)| {
            state
                .regions
                .get(id)
                .filter(|region| contains(&region.meta, key))
                .cloned()
        })
    }
    /// 按 end_key 语义查找“前一个”Region（prev region）。
    pub fn get_region_by_end_key(&self, key: &[u8]) -> Option<RegionCtx> {
        let state = self.state.read().expect("region manager lock poisoned");
        state
            .by_start
            .range(..key.to_vec())
            .next_back()
            .and_then(|(_, id)| {
                state
                    .regions
                    .get(id)
                    .filter(|region| {
                        region.meta.start_key.as_slice() < key
                            && (region.meta.end_key.is_empty()
                                || key <= region.meta.end_key.as_slice())
                    })
                    .cloned()
            })
    }

    /// 校验请求上下文：Store 存在、Region 存在且纪元不过期。
    pub fn validate_context(
        &self,
        region_id: u64,
        store_id: Option<u64>,
        epoch: Option<&RegionEpoch>,
    ) -> Result<RegionCtx, RegionError> {
        let state = self.state.read().expect("region manager lock poisoned");
        if let Some(store_id) = store_id
            && !state.stores.contains_key(&store_id)
        {
            return Err(RegionError::StoreNotMatch);
        }
        let region = state
            .regions
            .get(&region_id)
            .cloned()
            .ok_or(RegionError::RegionNotFound(region_id))?;
        // 客户端纪元落后则返回 EpochNotMatch 与最新 Region。
        if let Some(epoch) = epoch
            && (epoch.conf_ver < region.meta.epoch.conf_ver
                || epoch.version < region.meta.epoch.version)
        {
            return Err(RegionError::EpochNotMatch(region.meta.clone()));
        }
        Ok(region)
    }

    /// 在 `key` 处分裂 Region：左半保留原 ID，右半使用新 ID 与新 Peer。
    pub fn split(
        &self,
        region_id: u64,
        new_region_id: u64,
        key: Vec<u8>,
        peer_ids: &[u64],
    ) -> Result<Region, RegionError> {
        let mut state = self.state.write().expect("region manager lock poisoned");
        let old = state
            .regions
            .get(&region_id)
            .cloned()
            .ok_or(RegionError::RegionNotFound(region_id))?;
        // 分裂键必须落在原 Region 开区间内。
        if key <= old.meta.start_key || (!old.meta.end_key.is_empty() && key >= old.meta.end_key) {
            return Err(RegionError::SplitKeyOutOfRange);
        }
        if peer_ids.len() != old.meta.peers.len() {
            return Err(RegionError::InvalidSplitKeys);
        }
        // 左 Region 截断 end_key 并提升 version；右 Region 新建。
        let mut left = old.clone();
        left.meta.end_key = key.clone();
        left.meta.epoch.version += 1;
        let right_peers = old
            .meta
            .peers
            .iter()
            .zip(peer_ids)
            .map(|(peer, id)| Peer {
                id: *id,
                store_id: peer.store_id,
            })
            .collect::<Vec<_>>();
        let right = Region {
            id: new_region_id,
            start_key: key.clone(),
            end_key: old.meta.end_key,
            epoch: RegionEpoch {
                conf_ver: 1,
                version: 1,
            },
            peers: right_peers.clone(),
        };
        state.regions.insert(region_id, left);
        state.regions.insert(
            new_region_id,
            RegionCtx {
                meta: right.clone(),
                leader: right_peers.first().cloned(),
                buckets: None,
                down_peers: Vec::new(),
            },
        );
        state.by_start.insert(key, new_region_id);
        self.id.fetch_max(new_region_id, Ordering::AcqRel);
        Ok(right)
    }

    /// 按多把键依次分裂：排序去重后对每把键执行单点分裂。
    pub fn split_keys(&self, mut keys: Vec<Vec<u8>>) -> Result<Vec<Region>, RegionError> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        // 排序去重，保证从左到右稳定分裂。
        keys.sort();
        keys.dedup();
        let mut result = Vec::new();
        for key in keys {
            let current = self
                .get_region_by_key(&key)
                .ok_or(RegionError::InvalidSplitKeys)?;
            if key == current.meta.start_key {
                continue;
            }
            let new_region_id = self.alloc_id();
            let peer_ids = current
                .meta
                .peers
                .iter()
                .map(|_| self.alloc_id())
                .collect::<Vec<_>>();
            result.push(self.split(current.meta.id, new_region_id, key, &peer_ids)?);
        }
        Ok(result)
    }

    /// 在候选键中均匀选取分裂点，将区间大致均分为 `count` 段。
    pub fn calculate_split_keys(
        &self,
        keys: &[Vec<u8>],
        start: &[u8],
        end: &[u8],
        count: usize,
    ) -> Vec<Vec<u8>> {
        if count <= 1 {
            return Vec::new();
        }
        let candidates = keys
            .iter()
            .filter(|key| key.as_slice() >= start && (end.is_empty() || key.as_slice() < end))
            .collect::<Vec<_>>();
        let quotient = candidates.len() / count;
        let mut remainder = candidates.len() % count;
        let mut offset = 0;
        let mut split_keys = Vec::with_capacity(count.saturating_sub(1));
        while offset < candidates.len() {
            let mut region_entry_count = quotient;
            if remainder > 0 {
                remainder -= 1;
                region_entry_count += 1;
            }
            offset += region_entry_count;
            if offset < candidates.len() {
                split_keys.push(candidates[offset].clone());
            }
        }
        split_keys
    }

    /// 扫描与 `[start, end)` 相交的 Region，按 start_key 排序并可截断。
    pub fn scan_regions(&self, start: &[u8], end: &[u8], limit: usize) -> Vec<RegionCtx> {
        let state = self.state.read().expect("region manager lock poisoned");
        let mut regions = state
            .regions
            .values()
            .filter(|region| {
                (region.meta.end_key.is_empty() || region.meta.end_key.as_slice() > start)
                    && (end.is_empty() || region.meta.start_key.as_slice() < end)
            })
            .cloned()
            .collect::<Vec<_>>();
        regions.sort_by(|left, right| left.meta.start_key.cmp(&right.meta.start_key));
        if limit > 0 {
            regions.truncate(limit);
        }
        regions
    }

    /// 注册或覆盖一个 Store，并初始化其 MPP 任务表。
    pub fn add_store(&self, store_id: u64, address: String, labels: Vec<StoreLabel>) {
        let mut state = self.state.write().expect("region manager lock poisoned");
        state.stores.insert(
            store_id,
            Store {
                id: store_id,
                address,
                labels,
            },
        );
        state.mpp_tasks.entry(store_id).or_default();
        self.id.fetch_max(store_id, Ordering::AcqRel);
    }
    /// 移除 Store 及其 MPP 任务。
    pub fn remove_store(&self, store_id: u64) {
        let mut state = self.state.write().expect("region manager lock poisoned");
        state.stores.remove(&store_id);
        state.mpp_tasks.remove(&store_id);
    }
    /// 返回当前全部 Store 快照。
    pub fn all_stores(&self) -> Vec<Store> {
        self.state
            .read()
            .expect("region manager lock poisoned")
            .stores
            .values()
            .cloned()
            .collect()
    }
    /// 向 Region 追加 Peer，并递增 conf_ver。
    pub fn add_peer(&self, region_id: u64, store_id: u64, peer_id: u64) -> Result<(), RegionError> {
        let mut state = self.state.write().expect("region manager lock poisoned");
        let region = state
            .regions
            .get_mut(&region_id)
            .ok_or(RegionError::RegionNotFound(region_id))?;
        region.meta.peers.push(Peer {
            id: peer_id,
            store_id,
        });
        region.meta.epoch.conf_ver += 1;
        Ok(())
    }
    /// 在指定 Store 上注册 MPP（大规模并行处理）任务元数据。
    pub fn register_mpp_task(
        &self,
        store_id: u64,
        task_id: i64,
        value: String,
    ) -> Result<(), RegionError> {
        self.state
            .write()
            .expect("region manager lock poisoned")
            .mpp_tasks
            .get_mut(&store_id)
            .ok_or(RegionError::StoreNotMatch)?
            .insert(task_id, value);
        Ok(())
    }
}

/// 判断 `key` 是否落在 Region 的半开区间内；空 end_key 表示正无穷。
fn contains(region: &Region, key: &[u8]) -> bool {
    region.start_key.as_slice() <= key
        && (region.end_key.is_empty() || key < region.end_key.as_slice())
}

/// Mock PD：封装 Region 管理、GC 状态与外部时间戳。
pub struct MockPd {
    manager: Arc<MockRegionManager>,
    gc: GcStatesManager,
    external_timestamp: AtomicU64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 设置 external timestamp 时与 Go MockPD 一致的校验错误。
pub enum ExternalTimestampError {
    GreaterThanGlobalTso,
    Decreasing,
}

impl fmt::Display for ExternalTimestampError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GreaterThanGlobalTso => {
                f.write_str("external timestamp is greater than global tso")
            }
            Self::Decreasing => f.write_str("cannot decrease the external timestamp"),
        }
    }
}

impl std::error::Error for ExternalTimestampError {}

impl MockPd {
    /// 用共享 Region 管理器构造 Mock PD。
    pub fn new(manager: Arc<MockRegionManager>) -> Self {
        Self {
            manager,
            gc: GcStatesManager::default(),
            external_timestamp: AtomicU64::new(0),
        }
    }
    /// 转发集群 ID 查询。
    pub fn cluster_id(&self) -> u64 {
        self.manager.cluster_id()
    }
    /// 转发全局 ID 分配。
    pub fn alloc_id(&self) -> u64 {
        self.manager.alloc_id()
    }
    /// 按键查询 Region 上下文。
    pub fn get_region(&self, key: &[u8]) -> Option<RegionCtx> {
        self.manager.get_region_by_key(key)
    }
    /// 按 ID 取 Region，再通过 start_key 取完整上下文。
    pub fn get_region_by_id(&self, id: u64) -> Option<RegionCtx> {
        let region = self.manager.get_region(id)?;
        self.manager.get_region_by_key(&region.start_key)
    }
    /// 查询键之前的 Region。
    pub fn get_prev_region(&self, key: &[u8]) -> Option<RegionCtx> {
        self.manager.get_region_by_end_key(key)
    }
    /// 转发 Region 范围扫描。
    pub fn scan_regions(&self, start: &[u8], end: &[u8], limit: usize) -> Vec<RegionCtx> {
        self.manager.scan_regions(start, end, limit)
    }
    /// 转发 Store 列表查询。
    pub fn all_stores(&self) -> Vec<Store> {
        self.manager.all_stores()
    }
    /// 访问 GC 状态管理器。
    pub fn gc_states(&self) -> &GcStatesManager {
        &self.gc
    }
    /// 读取外部时间戳（external timestamp）。
    pub fn get_external_timestamp(&self) -> u64 {
        self.external_timestamp.load(Ordering::Acquire)
    }
    /// 设置外部时间戳。
    pub fn set_external_timestamp(&self, timestamp: u64) -> Result<(), ExternalTimestampError> {
        let (physical, logical) = get_ts();
        let current_tso = ((physical as u64) << 18) | logical as u64;
        if timestamp > current_tso {
            return Err(ExternalTimestampError::GreaterThanGlobalTso);
        }
        loop {
            let current = self.external_timestamp.load(Ordering::Acquire);
            if current > timestamp {
                return Err(ExternalTimestampError::Decreasing);
            }
            if self
                .external_timestamp
                .compare_exchange(current, timestamp, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Ok(());
            }
        }
    }
    /// 分配逻辑时间戳（物理毫秒 + 逻辑计数）。
    pub fn get_ts(&self) -> (i64, i64) {
        get_ts()
    }
}

/// 全局单调时间戳：同一毫秒内递增逻辑部分，避免碰撞。
pub fn get_ts() -> (i64, i64) {
    static LAST: OnceLock<Mutex<(i64, i64)>> = OnceLock::new();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let mut last = LAST
        .get_or_init(|| Mutex::new((0, 0)))
        .lock()
        .expect("timestamp lock poisoned");
    // 物理时钟未前进（同毫秒或回拨）时递增逻辑部分，否则重置。
    if last.0 >= now {
        last.1 += 1;
    } else {
        *last = (now, 0);
    }
    *last
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// GC Barrier：以 barrier_ts 阻止 txn safe point 越过该水位。
pub struct GcBarrier {
    pub id: String,
    pub barrier_ts: u64,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 某 keyspace 的 GC 视图：safe point 与可选 barrier 列表。
pub struct GcState {
    pub keyspace_id: u32,
    pub txn_safe_point: u64,
    pub gc_safe_point: u64,
    pub barriers: Vec<GcBarrier>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 推进 safe point 的结果：旧值、目标、实际新值与阻塞描述。
pub struct AdvanceResult {
    pub old: u64,
    pub target: u64,
    pub new: u64,
    pub blocker: Option<String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
/// GC 状态机错误：参数非法、回退、barrier 过低或 GC 超前事务。
pub enum GcError {
    InvalidArguments,
    BarrierBehindTxnSafePoint { barrier: u64, txn_safe_point: u64 },
    DecreasingSafePoint { current: u64, target: u64 },
    GcAheadOfTxn { gc: u64, txn: u64 },
}
impl fmt::Display for GcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for GcError {}

#[derive(Default)]
/// 单 keyspace 内部 GC 状态。
struct InternalGcState {
    txn_safe_point: u64,
    gc_safe_point: u64,
    barriers: HashMap<String, u64>,
}
#[derive(Default)]
/// 按 keyspace 管理 txn/gc safe point 与 GC Barrier。
pub struct GcStatesManager {
    states: Mutex<HashMap<u32, InternalGcState>>,
}
impl GcStatesManager {
    /// 设置 barrier；不得低于当前 txn safe point。
    pub fn set_barrier(&self, keyspace: u32, id: String, ts: u64) -> Result<GcBarrier, GcError> {
        if id.is_empty() || ts == 0 {
            return Err(GcError::InvalidArguments);
        }
        let mut states = self.states.lock().expect("GC states lock poisoned");
        let state = states.entry(keyspace).or_default();
        if ts < state.txn_safe_point {
            return Err(GcError::BarrierBehindTxnSafePoint {
                barrier: ts,
                txn_safe_point: state.txn_safe_point,
            });
        }
        state.barriers.insert(id.clone(), ts);
        Ok(GcBarrier { id, barrier_ts: ts })
    }
    /// 删除 barrier 并返回被删项。
    pub fn delete_barrier(&self, keyspace: u32, id: &str) -> Option<GcBarrier> {
        self.states
            .lock()
            .expect("GC states lock poisoned")
            .entry(keyspace)
            .or_default()
            .barriers
            .remove(id)
            .map(|barrier_ts| GcBarrier {
                id: id.into(),
                barrier_ts,
            })
    }
    /// 推进 txn safe point；若存在更低 barrier 则停在 barrier 处。
    pub fn advance_txn_safe_point(
        &self,
        keyspace: u32,
        target: u64,
    ) -> Result<AdvanceResult, GcError> {
        let mut states = self.states.lock().expect("GC states lock poisoned");
        let state = states.entry(keyspace).or_default();
        let old = state.txn_safe_point;
        if target < old {
            return Err(GcError::DecreasingSafePoint {
                current: old,
                target,
            });
        }
        // 取最小 barrier；若小于 target 则成为阻塞点。
        let blocker = state
            .barriers
            .iter()
            .min_by_key(|(_, ts)| **ts)
            .filter(|(_, ts)| **ts < target)
            .map(|(id, ts)| (id.clone(), *ts));
        let mut new = blocker.as_ref().map_or(target, |(_, ts)| *ts);
        if new < old {
            new = old;
        }
        state.txn_safe_point = new;
        Ok(AdvanceResult {
            old,
            target,
            new,
            blocker: blocker.map(|(id, ts)| format!("GCBarrier {id} at {ts}")),
        })
    }
    /// 推进 gc safe point；不可回退，也不可超过 txn safe point。
    pub fn advance_gc_safe_point(
        &self,
        keyspace: u32,
        target: u64,
    ) -> Result<AdvanceResult, GcError> {
        let mut states = self.states.lock().expect("GC states lock poisoned");
        let state = states.entry(keyspace).or_default();
        let old = state.gc_safe_point;
        if target < old {
            return Err(GcError::DecreasingSafePoint {
                current: old,
                target,
            });
        }
        if target > state.txn_safe_point {
            return Err(GcError::GcAheadOfTxn {
                gc: target,
                txn: state.txn_safe_point,
            });
        }
        state.gc_safe_point = target;
        Ok(AdvanceResult {
            old,
            target,
            new: target,
            blocker: None,
        })
    }
    /// 读取 GC 状态；`include_barriers` 控制是否带上 barrier 列表。
    pub fn state(&self, keyspace: u32, include_barriers: bool) -> GcState {
        let mut states = self.states.lock().expect("GC states lock poisoned");
        let state = states.entry(keyspace).or_default();
        GcState {
            keyspace_id: keyspace,
            txn_safe_point: state.txn_safe_point,
            gc_safe_point: state.gc_safe_point,
            barriers: if include_barriers {
                state
                    .barriers
                    .iter()
                    .map(|(id, ts)| GcBarrier {
                        id: id.clone(),
                        barrier_ts: *ts,
                    })
                    .collect()
            } else {
                Vec::new()
            },
        }
    }
}
