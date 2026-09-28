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

// Region 元数据、Latch 与独立 RegionManager：管理键空间分片与并发闩锁。
//
// Region 是 TiKV 的键空间分片单位；Epoch（conf_ver/version）在分裂/成员变更时递增。
// Latch 按 key 指纹串行化写请求，避免同一键上的事务互相交错。

use crate::mock_region::{
    Peer, Region, RegionCtx as RegionMetadata, RegionEpoch, RegionError, Store,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};

/// 内部键前缀（0xff），与用户键空间隔离。
pub const INTERNAL_KEY_PREFIX: &[u8] = &[0xff];
/// Region 元数据内部键前缀。
pub const INTERNAL_REGION_META_PREFIX: &[u8] = b"\xffregion";
/// Store 元数据内部键。
pub const INTERNAL_STORE_META_KEY: &[u8] = b"\xffstore";
/// GC safe point 内部键。
pub const INTERNAL_SAFE_POINT_KEY: &[u8] = b"\xffsafepoint";
/// 构造指定 Region ID 的内部元数据键。
pub fn internal_region_meta_key(region_id: u64) -> Vec<u8> {
    let mut key = INTERNAL_REGION_META_PREFIX.to_vec();
    key.extend_from_slice(region_id.to_string().as_bytes());
    key
}

/// Latch 等待者：通过条件变量等待前持有者释放。
struct LatchWaiter {
    released: Mutex<bool>,
    ready: Condvar,
}
/// 分槽 Latch 表：按哈希高 8 位选槽，槽内按完整哈希排队。
pub struct Latches {
    slots: Vec<Mutex<HashMap<u64, Arc<LatchWaiter>>>>,
}
impl Default for Latches {
    fn default() -> Self {
        Self {
            slots: (0..256).map(|_| Mutex::new(HashMap::new())).collect(),
        }
    }
}
impl Latches {
    /// 按序获取一组哈希对应的 Latch，返回等待次数。
    pub fn acquire(&self, hashes: &[u64]) -> usize {
        let ticket = Arc::new(LatchWaiter {
            released: Mutex::new(false),
            ready: Condvar::new(),
        });
        let mut waits = 0;
        for hash in hashes {
            // 若槽内已有同哈希等待者则阻塞，直至其释放。
            loop {
                let slot = &self.slots[(hash >> 56) as usize];
                let previous = {
                    let mut values = slot.lock().expect("latch slot poisoned");
                    if let Some(waiter) = values.get(hash) {
                        Some(waiter.clone())
                    } else {
                        values.insert(*hash, ticket.clone());
                        None
                    }
                };
                let Some(previous) = previous else {
                    break;
                };
                let mut released = previous.released.lock().expect("latch waiter poisoned");
                while !*released {
                    released = previous
                        .ready
                        .wait(released)
                        .expect("latch waiter poisoned");
                }
                waits += 1;
            }
        }
        waits
    }
    /// 释放一组 Latch 并唤醒等待者。
    pub fn release(&self, hashes: &[u64]) {
        let mut ticket = None;
        for hash in hashes {
            if let Some(waiter) = self.slots[(hash >> 56) as usize]
                .lock()
                .expect("latch slot poisoned")
                .remove(hash)
            {
                ticket.get_or_insert(waiter);
            }
        }
        if let Some(ticket) = ticket {
            *ticket.released.lock().expect("latch waiter poisoned") = true;
            ticket.ready.notify_all();
        }
    }
}

/// 单个 Region 的运行时上下文：元数据、键范围、近似大小与 Latch。
pub struct RegionContext {
    meta: RwLock<Region>,
    raw_start: Vec<u8>,
    raw_end: Vec<u8>,
    approximate_size: AtomicI64,
    diff: AtomicI64,
    latches: Arc<Latches>,
}
impl RegionContext {
    /// 用元数据与共享 Latch 构造上下文；空 end_key 映射到内部前缀上界。
    pub fn new(meta: Region, latches: Arc<Latches>) -> Self {
        let raw_start = meta.start_key.clone();
        // 空 end_key 表示最大右界，映射到内部键前缀以便比较。
        let raw_end = if meta.end_key.is_empty() {
            INTERNAL_KEY_PREFIX.to_vec()
        } else {
            meta.end_key.clone()
        };
        Self {
            meta: RwLock::new(meta),
            raw_start,
            raw_end,
            approximate_size: AtomicI64::new(0),
            diff: AtomicI64::new(0),
            latches,
        }
    }
    /// 克隆当前 Region 元数据。
    pub fn meta(&self) -> Region {
        self.meta.read().expect("region meta poisoned").clone()
    }
    /// Region 原始起始键。
    pub fn raw_start(&self) -> &[u8] {
        &self.raw_start
    }
    /// Region 原始结束键（空上界已展开为内部前缀）。
    pub fn raw_end(&self) -> &[u8] {
        &self.raw_end
    }
    /// 相对近似大小的增量。
    pub fn diff(&self) -> i64 {
        self.diff.load(Ordering::Acquire)
    }
    /// 累加大小增量。
    pub fn add_diff(&self, value: i64) {
        self.diff.fetch_add(value, Ordering::AcqRel);
    }
    /// 当前近似数据大小。
    pub fn approximate_size(&self) -> i64 {
        self.approximate_size.load(Ordering::Acquire)
    }
    /// 设置近似数据大小。
    pub fn set_approximate_size(&self, size: i64) {
        self.approximate_size.store(size, Ordering::Release);
    }
    /// 在本 Region 上获取 Latch。
    pub fn acquire_latches(&self, hashes: &[u64]) -> usize {
        self.latches.acquire(hashes)
    }
    /// 释放本 Region 上的 Latch。
    pub fn release_latches(&self, hashes: &[u64]) {
        self.latches.release(hashes);
    }
    /// 添加 Peer 并递增 conf_ver（配置变更代数）。
    pub fn add_peer(&self, peer_id: u64, store_id: u64) {
        let mut meta = self.meta.write().expect("region meta poisoned");
        meta.peers.push(Peer {
            id: peer_id,
            store_id,
        });
        meta.epoch.conf_ver += 1;
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 创建 RegionManager 时的地址与目标 Region 大小选项。
pub struct RegionOptions {
    pub store_address: String,
    pub pd_address: String,
    pub region_size: i64,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// RPC 请求携带的 Region 定位信息（ID、Store、Epoch）。
pub struct RequestContext {
    pub region_id: u64,
    pub store_id: Option<u64>,
    pub epoch: Option<RegionEpoch>,
}

/// Region 管理接口：解析请求上下文、Store 映射与分裂。
pub trait RegionManager: Send + Sync {
    /// 校验 Store/Epoch 后返回对应 Region；失败返回 RegionError。
    fn get_region_from_context(
        &self,
        context: &RequestContext,
    ) -> Result<Arc<RegionContext>, RegionError>;
    fn get_store_info(&self, context: &RequestContext) -> Result<(String, u64), RegionError>;
    fn get_store_id_by_address(&self, address: &str) -> Result<u64, RegionError>;
    fn get_store_address_by_id(&self, store_id: u64) -> Result<String, RegionError>;
    fn split_region(&self, region_id: u64, keys: Vec<Vec<u8>>) -> Result<Vec<Region>, RegionError>;
    fn close(&self);
}

/// 单机 mock 用的 RegionManager：内存维护 Region 表与本地 Store。
pub struct StandAloneRegionManager {
    regions: RwLock<HashMap<u64, Arc<RegionContext>>>,
    store: RwLock<Store>,
    latches: Arc<Latches>,
    next_id: AtomicU64,
    closed: AtomicBool,
    region_size: i64,
}
impl StandAloneRegionManager {
    /// 以根 Region 与本地 Store 初始化管理器。
    pub fn new(store: Store, root: Region, options: RegionOptions) -> Self {
        let latches = Arc::new(Latches::default());
        let root_id = root.id;
        let max_id = root
            .peers
            .iter()
            .fold(root_id.max(store.id), |value, peer| value.max(peer.id));
        Self {
            regions: RwLock::new(HashMap::from([(
                root_id,
                Arc::new(RegionContext::new(root, latches.clone())),
            )])),
            store: RwLock::new(store),
            latches,
            next_id: AtomicU64::new(max_id),
            closed: AtomicBool::new(false),
            region_size: options.region_size,
        }
    }
    /// 触发分裂检查的目标 Region 大小。
    pub fn region_size(&self) -> i64 {
        self.region_size
    }
    /// 分配一批单调递增的 ID（Peer/Region）。
    pub fn alloc_ids(&self, count: usize) -> Vec<u64> {
        let end = self.next_id.fetch_add(count as u64, Ordering::AcqRel) + count as u64;
        (end - count as u64 + 1..=end).collect()
    }
    /// 按键查找所属 Region（半开区间 `[start, end)`）。
    pub fn region_for_key(&self, key: &[u8]) -> Option<Arc<RegionContext>> {
        self.regions
            .read()
            .expect("regions poisoned")
            .values()
            .find(|region| {
                let meta = region.meta();
                meta.start_key.as_slice() <= key
                    && (meta.end_key.is_empty() || key < meta.end_key.as_slice())
            })
            .cloned()
    }
    /// 根据采样键大小决定是否分裂；超过阈值时在中点附近切分。
    pub fn split_check_region(
        &self,
        region_id: u64,
        sampled_keys: &[(Vec<u8>, i64)],
    ) -> Result<Option<Vec<Region>>, RegionError> {
        let region = self
            .regions
            .read()
            .expect("regions poisoned")
            .get(&region_id)
            .cloned()
            .ok_or(RegionError::RegionNotFound(region_id))?;
        let total = sampled_keys.iter().map(|(_, size)| *size).sum::<i64>();
        if total < self.region_size {
            region.set_approximate_size(total);
            return Ok(None);
        }
        let mut accumulated = 0;
        let split = sampled_keys
            .iter()
            .find(|(_, size)| {
                accumulated += *size;
                accumulated >= total / 2
            })
            .map(|(key, _)| key.clone())
            .ok_or(RegionError::InvalidSplitKeys)?;
        self.split_region(region_id, vec![split]).map(Some)
    }
}
impl RegionManager for StandAloneRegionManager {
    /// 校验 Store/Epoch 后返回对应 Region；失败返回 RegionError。
    fn get_region_from_context(
        &self,
        context: &RequestContext,
    ) -> Result<Arc<RegionContext>, RegionError> {
        let store = self.store.read().expect("store meta poisoned");
        if context.store_id.is_some_and(|id| id != store.id) {
            return Err(RegionError::StoreNotMatch);
        }
        drop(store);
        let region = self
            .regions
            .read()
            .expect("regions poisoned")
            .get(&context.region_id)
            .cloned()
            .ok_or(RegionError::RegionNotFound(context.region_id))?;
        // Epoch 不一致说明客户端与本地 Region 视图不同，需刷新路由。
        if context.epoch.as_ref().is_some_and(|epoch| {
            let current = region.meta().epoch;
            epoch.conf_ver != current.conf_ver || epoch.version != current.version
        }) {
            return Err(RegionError::EpochNotMatch(region.meta()));
        }
        Ok(region)
    }
    /// 返回本地 Store 地址与 ID（校验请求 Store 匹配）。
    fn get_store_info(&self, context: &RequestContext) -> Result<(String, u64), RegionError> {
        let store = self.store.read().expect("store meta poisoned");
        if context.store_id.is_some_and(|id| id != store.id) {
            Err(RegionError::StoreNotMatch)
        } else {
            Ok((store.address.clone(), store.id))
        }
    }
    /// 按地址解析 Store ID。
    fn get_store_id_by_address(&self, address: &str) -> Result<u64, RegionError> {
        let store = self.store.read().expect("store meta poisoned");
        (store.address == address)
            .then_some(store.id)
            .ok_or(RegionError::StoreNotMatch)
    }
    /// 按 Store ID 解析地址。
    fn get_store_address_by_id(&self, store_id: u64) -> Result<String, RegionError> {
        let store = self.store.read().expect("store meta poisoned");
        (store.id == store_id)
            .then(|| store.address.clone())
            .ok_or(RegionError::StoreNotMatch)
    }
    /// 按切分键生成新 Region 列表并替换内存表。
    fn split_region(
        &self,
        region_id: u64,
        mut keys: Vec<Vec<u8>>,
    ) -> Result<Vec<Region>, RegionError> {
        keys.sort();
        keys.dedup();
        let old = self
            .regions
            .read()
            .expect("regions poisoned")
            .get(&region_id)
            .map(|region| region.meta())
            .ok_or(RegionError::RegionNotFound(region_id))?;
        // 仅保留落在原 Region 半开区间内的切分键。
        keys.retain(|key| *key > old.start_key && (old.end_key.is_empty() || *key < old.end_key));
        if keys.is_empty() {
            return Err(RegionError::InvalidSplitKeys);
        }
        // 用 start + 切分键 + end 拼出各子 Region 边界。
        let mut boundaries = vec![old.start_key.clone()];
        boundaries.extend(keys);
        boundaries.push(old.end_key.clone());
        let mut created = Vec::new();
        for index in 0..boundaries.len() - 1 {
            let is_rightmost = index == boundaries.len() - 2;
            let id = if is_rightmost {
                old.id
            } else {
                self.alloc_ids(1)[0]
            };
            created.push(Region {
                id,
                start_key: boundaries[index].clone(),
                end_key: boundaries[index + 1].clone(),
                epoch: if is_rightmost {
                    RegionEpoch {
                        conf_ver: old.epoch.conf_ver,
                        version: old.epoch.version + 1,
                    }
                } else {
                    RegionEpoch {
                        conf_ver: 1,
                        version: 1,
                    }
                },
                peers: old.peers.clone(),
            });
        }
        let mut regions = self.regions.write().expect("regions poisoned");
        regions.remove(&region_id);
        for region in &created {
            regions.insert(
                region.id,
                Arc::new(RegionContext::new(region.clone(), self.latches.clone())),
            );
        }
        Ok(created)
    }
    /// 标记管理器已关闭。
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }
}

/// 将 RegionContext 转为路由用元数据（默认首个 Peer 为 Leader）。
pub fn router_region(context: &RegionContext) -> RegionMetadata {
    let meta = context.meta();
    RegionMetadata {
        leader: meta.peers.first().cloned(),
        meta,
        buckets: None,
        down_peers: Vec::new(),
    }
}
