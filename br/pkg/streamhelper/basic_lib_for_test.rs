// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

//! Go-equivalent fixtures from `basic_lib_for_test.go`.
//! In-crate fake cluster (no utiltest dep / no kvproto/grpcio).
//! 为 streamhelper 单测提供内存假集群与 TestEnv：
//! Store/Region 拓扑、检查点推进、GC 安全点、日志备份客户端与锁解决钩子。
//! 与 Go fakecluster/testEnv 语义对齐；flush 在 slim 端口为空操作边界。
//! SharedFakeCluster 提供可读实时 flush TS 的 LogBackupClient。
//! TestEnv 实现完整 Env，供 CheckpointAdvancer 单测注入失败与任务事件。

// 假集群避免依赖真实 PD/TiKV，保证单测可确定性复现。
// Region 切分后 Leader 轮转，模拟多 Store 负载分布。
// advance_checkpoints 使用递增 delta，便于观察最小检查点变化。
// locks_below 仅用于测试 resolve 路径，生产代码无此字段。
// sim_enabled 预留给更重的仿真；slim 端口多数用例可忽略。
// ClearCache 记录 Store ID，便于断言订阅缓存失效。
// LiveClusterLogClient 在 scatter 后容忍跨 Store 查询。
// TestEnv.checkpoint 与 Region flush TS 分离，importantTick 写入前者。
// 任务通道模拟异步 EventAdd/Del/Pause/Resume。
// max_ts 非 0 时强制 ResolveLocks 的 maxVersion 匹配。
// one_store_failure 用 CAS 保证只失败一次。
// many_regions 生成零填充键，保持字典序切分稳定。
// install_subscribe_support 用于订阅拓扑相关用例。
// PD 断连模拟为单次失败，避免测试卡死。
// Begin 推送本地 task 快照，对齐 AdvancerExt.Begin。
// SharedFakeCluster 的 StreamMeta 为空实现，检查点由 TestEnv 持有。
// 服务安全点与 cluster current_ts 独立维护。
// GetLastFlushTSOfRegion 只返回请求中出现的 Region。
// 默认 whole 任务名为测试约定，与 advancer_test 一致。
// ranges 为空时 put_task 回退到默认全范围。
// fail_get_client 优先于 shared 委托，便于注入。
// block_gc_attempted 供断言 BlockGC 是否被调用。
// scan_locked_err 直接短路，模拟 ScanLock locked 串。
// 钩子 resolve_locks 可记录调用次数与版本序列。
// FakeCluster.GetLogBackupClient 故意失败以强制用 Shared。
// UnblockGC 仅置删除标志，不重置 service_gc 值。
// RegionScan 在有序 Region 列表上可提前结束扫描。
// Stores 列表用于订阅拓扑 UpdateStoreTopology。
// flush 空操作避免引入异步订阅时序耦合。
// 与 Go basic_lib_for_test.go 符号命名保持对照。
// ClusterInner.on_get_client 可在运行中热替换。
// service_gc_set 用于断言 BlockGCUntil 成功路径。
// next_id 分配保证 Region/Store ID 不冲突。
// supports_sub 影响订阅拓扑填充结果。
// Env trait 由 TestEnv 的多 impl 组合满足。
// SharedFakeCluster.new 内部调用 FakeCluster::new_basic。
// 切分时继承旧 Region 的 checkpoint 与 locks_below。
// remove_store 后若无幸存 Store 则直接返回。
// advance_checkpoint_by 使用 ts_add_duration 保持 TSO 布局。
// TestEnv.Upload 在回退时返回 rolling back 错误。
// 假集群避免依赖真实 PD/TiKV，保证单测可确定性复现。
// Region 切分后 Leader 轮转，模拟多 Store 负载分布。
// advance_checkpoints 使用递增 delta，便于观察最小检查点变化。
// locks_below 仅用于测试 resolve 路径，生产代码无此字段。
// sim_enabled 预留给更重的仿真；slim 端口多数用例可忽略。
// ClearCache 记录 Store ID，便于断言订阅缓存失效。
// LiveClusterLogClient 在 scatter 后容忍跨 Store 查询。
// TestEnv.checkpoint 与 Region flush TS 分离，importantTick 写入前者。
// 任务通道模拟异步 EventAdd/Del/Pause/Resume。
// max_ts 非 0 时强制 ResolveLocks 的 maxVersion 匹配。
// one_store_failure 用 CAS 保证只失败一次。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use astersql_br_pkg_streamhelper_config::{Config, DefaultCommandConfig};
use astersql_br_pkg_streamhelper_spans::{Overlaps, Span};

use crate::advancer_cliext::{EventType, TaskEvent};
use crate::advancer_env::{LogBackupFlushIntervalGetter, RegionLockResolver, StreamMeta};
use crate::regioniter::{RegionWithLeader, Store, TiKVClusterMeta};
use crate::stubs::{
    GetLastFlushTSOfRegionRequest, GetLastFlushTSOfRegionResponse, KeyRange, LogBackupClient,
    LogBackupService, Peer, Region, RegionCheckpoint, RegionEpoch, RegionIdentity,
    StreamBackupTaskInfo,
};

/// TSO 物理时间左移位数，与 oracle.GetPhysical 一致。
const PHYSICAL_SHIFT_BITS: u64 = 18;

/// 由物理毫秒与逻辑部分合成 TSO。
fn compose_ts(physical_ms: u64, logical: u64) -> u64 {
    (physical_ms << PHYSICAL_SHIFT_BITS) + logical
}

/// 提取 TSO 的物理毫秒部分。
fn extract_physical(ts: u64) -> u64 {
    ts >> PHYSICAL_SHIFT_BITS
}

/// 在 TSO 上增加物理时长，逻辑位清零。
fn ts_add_duration(ts: u64, d: Duration) -> u64 {
    compose_ts(extract_physical(ts) + d.as_millis() as u64, 0)
}

#[derive(Clone, Debug)]
/// 假 Region：键范围、Leader、epoch、检查点与可选锁上界。
pub struct FakeRegion {
    /// Region/Store ID。
    pub id: u64,
    /// 范围起点（含）。
    pub start: Vec<u8>,
    /// 范围终点（不含）；空表示正无穷。
    pub end: Vec<u8>,
    /// Leader 所在 Store ID。
    pub leader: u64,
    /// Region epoch 版本。
    pub epoch: u64,
    /// 最近 flush 检查点 TSO。
    pub checkpoint: u64,
    /// 最近完成 flush 的 Region epoch；0 表示尚未 flush。
    pub flushed_epoch: u64,
    /// 若存在，表示该 Region 含低于该版本的锁。
    pub locks_below: Option<u64>,
}

#[derive(Clone, Debug)]
/// 假 Store：启动时间与 flush 订阅能力。
pub struct FakeStore {
    /// Store ID。
    pub id: u64,
    /// 启动时间戳（测试用）。
    pub boot_at: u64,
    /// 是否支持 flush 订阅推送。
    pub supports_sub: bool,
}

/// 集群内部可变状态，由 FakeCluster 的 Mutex 保护。
struct ClusterInner {
    stores: HashMap<u64, FakeStore>,
    regions: Vec<FakeRegion>,
    next_id: u64,
    current_ts: u64,
    max_ts: u64,
    service_gc: u64,
    service_gc_set: u64,
    service_gc_deleted: u64,
    on_get_client: Option<Arc<dyn Fn(u64) -> Result<(), String> + Send + Sync>>,
    on_clear_cache: Option<Arc<dyn Fn(u64) -> Result<(), String> + Send + Sync>>,
    cleared_cache: Vec<u64>,
    flush_subscribers: Vec<(u64, std::sync::mpsc::Sender<Vec<crate::stubs::FlushEvent>>)>,
    sim_enabled: bool,
}

/// Go `fakecluster.Cluster` 的内存替身，供 streamhelper 单测使用。
pub struct FakeCluster {
    inner: Mutex<ClusterInner>,
}

impl FakeCluster {
    pub fn new_basic(n: usize, sim_enabled: bool) -> Arc<Self> {
        let mut stores = HashMap::new();
        let mut next_id = 1u64;
        for _ in 0..n {
            let id = next_id;
            next_id += 1;
            stores.insert(
                id,
                FakeStore {
                    id,
                    boot_at: 0,
                    supports_sub: false,
                },
            );
        }
        let leader = stores.keys().copied().next().unwrap_or(1);
        let regions = vec![FakeRegion {
            id: next_id,
            start: Vec::new(),
            end: Vec::new(),
            leader,
            epoch: 0,
            checkpoint: 0,
            flushed_epoch: 0,
            locks_below: None,
        }];
        next_id += 1;
        Arc::new(Self {
            inner: Mutex::new(ClusterInner {
                stores,
                regions,
                next_id,
                current_ts: 0,
                max_ts: 0,
                service_gc: 0,
                service_gc_set: 0,
                service_gc_deleted: 0,
                on_get_client: None,
                on_clear_cache: None,
                cleared_cache: Vec::new(),
                flush_subscribers: Vec::new(),
                sim_enabled,
            }),
        })
    }

    pub fn split_and_scatter(&self, keys: &[&str]) {
        let mut g = self.inner.lock().unwrap();
        for key in keys {
            let k = key.as_bytes().to_vec();
            let idx = g
                .regions
                .iter()
                .position(|r| {
                    k.as_slice() >= r.start.as_slice()
                        && (r.end.is_empty() || k.as_slice() < r.end.as_slice())
                })
                .expect("inconsistent key space");
            let old = g.regions[idx].clone();
            let new_id = g.next_id;
            g.next_id += 1;
            g.regions[idx].end = k.clone();
            g.regions[idx].epoch += 1;
            g.regions[idx].flushed_epoch = 0;
            g.regions.insert(
                idx + 1,
                FakeRegion {
                    id: new_id,
                    start: k,
                    end: old.end,
                    leader: old.leader,
                    epoch: old.epoch + 1,
                    checkpoint: old.checkpoint,
                    flushed_epoch: 0,
                    locks_below: old.locks_below,
                },
            );
        }
        let store_ids: Vec<u64> = g.stores.keys().copied().collect();
        if store_ids.is_empty() {
            return;
        }
        for (i, r) in g.regions.iter_mut().enumerate() {
            r.leader = store_ids[i % store_ids.len()];
        }
    }

    pub fn remove_store(&self, id: u64) {
        let mut g = self.inner.lock().unwrap();
        g.stores.remove(&id);
        let survivors: Vec<u64> = g.stores.keys().copied().collect();
        if survivors.is_empty() {
            return;
        }
        for r in &mut g.regions {
            if r.leader == id {
                r.leader = survivors[0];
            }
        }
    }

    pub fn advance_checkpoints(&self) -> u64 {
        let mut g = self.inner.lock().unwrap();
        let mut min_cp = u64::MAX;
        for (i, r) in g.regions.iter_mut().enumerate() {
            let delta = ((i as u64) % 256) + 1;
            r.checkpoint = r.checkpoint.saturating_add(delta);
            r.flushed_epoch = 0;
            min_cp = min_cp.min(r.checkpoint);
        }
        if min_cp == u64::MAX { 0 } else { min_cp }
    }

    pub fn advance_checkpoint_by(&self, duration: Duration) -> u64 {
        let mut g = self.inner.lock().unwrap();
        let mut min_cp = u64::MAX;
        for r in &mut g.regions {
            r.checkpoint = ts_add_duration(r.checkpoint, duration);
            r.flushed_epoch = 0;
            min_cp = min_cp.min(r.checkpoint);
        }
        if min_cp == u64::MAX { 0 } else { min_cp }
    }

    pub fn advance_cluster_time_by(&self, duration: Duration) -> u64 {
        let mut g = self.inner.lock().unwrap();
        g.current_ts = ts_add_duration(g.current_ts, duration);
        g.current_ts
    }

    pub fn flush_all(&self) {
        let mut g = self.inner.lock().unwrap();
        for region in &mut g.regions {
            region.flushed_epoch = region.epoch;
        }
        Self::publish_flush_events(&mut g);
    }

    pub fn flush_all_except(&self, keys: &[&str]) {
        let mut g = self.inner.lock().unwrap();
        for region in &mut g.regions {
            let contains_excluded_key = keys.iter().any(|key| {
                let key = key.as_bytes();
                key >= region.start.as_slice()
                    && (region.end.is_empty() || key < region.end.as_slice())
            });
            if !contains_excluded_key {
                region.flushed_epoch = region.epoch;
            }
        }
        Self::publish_flush_events(&mut g);
    }

    fn publish_flush_events(g: &mut ClusterInner) {
        let regions = g.regions.clone();
        g.flush_subscribers.retain(|(store_id, tx)| {
            let supported = g
                .stores
                .get(store_id)
                .is_some_and(|store| store.supports_sub);
            if !supported {
                return tx.send(Vec::new()).is_ok();
            }
            let events = regions
                .iter()
                .filter(|region| region.leader == *store_id && region.flushed_epoch == region.epoch)
                .map(|region| crate::stubs::FlushEvent {
                    StartKey: region.start.clone(),
                    EndKey: region.end.clone(),
                    Checkpoint: region.checkpoint,
                })
                .collect();
            tx.send(events).is_ok()
        });
    }

    pub fn store_list(&self) -> Vec<FakeStore> {
        let g = self.inner.lock().unwrap();
        let mut v: Vec<_> = g.stores.values().cloned().collect();
        v.sort_by_key(|s| s.id);
        v
    }

    pub fn region_list(&self) -> Vec<FakeRegion> {
        let g = self.inner.lock().unwrap();
        let mut v = g.regions.clone();
        v.sort_by(|a, b| a.start.cmp(&b.start));
        v
    }

    pub fn set_support_flush_sub_all(&self, support: bool) {
        let mut g = self.inner.lock().unwrap();
        for s in g.stores.values_mut() {
            s.supports_sub = support;
        }
    }

    pub fn set_support_flush_sub_n(&self, n: usize) {
        let mut g = self.inner.lock().unwrap();
        let mut ids: Vec<u64> = g.stores.keys().copied().collect();
        ids.sort();
        for (i, id) in ids.into_iter().enumerate() {
            if let Some(s) = g.stores.get_mut(&id) {
                s.supports_sub = i < n;
            }
        }
    }

    pub fn set_on_get_client(
        &self,
        hook: Option<Arc<dyn Fn(u64) -> Result<(), String> + Send + Sync>>,
    ) {
        self.inner.lock().unwrap().on_get_client = hook;
    }

    pub fn set_on_clear_cache(
        &self,
        hook: Option<Arc<dyn Fn(u64) -> Result<(), String> + Send + Sync>>,
    ) {
        self.inner.lock().unwrap().on_clear_cache = hook;
    }

    pub fn take_cleared_cache(&self) -> Vec<u64> {
        std::mem::take(&mut self.inner.lock().unwrap().cleared_cache)
    }

    pub fn set_region_locks_below(&self, region_id: u64, max_txn: Option<u64>) {
        let mut g = self.inner.lock().unwrap();
        if let Some(r) = g.regions.iter_mut().find(|r| r.id == region_id) {
            r.locks_below = max_txn;
        }
    }

    pub fn set_max_ts(&self, ts: u64) {
        self.inner.lock().unwrap().max_ts = ts;
    }

    pub fn max_ts(&self) -> u64 {
        self.inner.lock().unwrap().max_ts
    }

    pub fn service_gc_safe_point(&self) -> u64 {
        self.inner.lock().unwrap().service_gc
    }

    pub fn service_gc_set(&self) -> u64 {
        self.inner.lock().unwrap().service_gc_set
    }

    fn to_region_with_leader(r: &FakeRegion) -> RegionWithLeader {
        RegionWithLeader {
            Region: Region {
                Id: r.id,
                StartKey: r.start.clone(),
                EndKey: r.end.clone(),
                RegionEpoch: RegionEpoch {
                    Version: r.epoch,
                    ConfVer: 0,
                },
            },
            Leader: Peer {
                Id: r.id,
                StoreId: r.leader,
            },
        }
    }
}

/// FakeCluster 的 PD/TiKV 元数据实现：扫描、Store 列表与 GC 阻塞。
impl TiKVClusterMeta for FakeCluster {
    /// 返回与键范围重叠的 Region，受 limit 限制。
    fn RegionScan(
        &self,
        key: &[u8],
        endKey: &[u8],
        limit: i32,
    ) -> Result<Vec<RegionWithLeader>, String> {
        let regions = self.region_list();
        let mut result = Vec::new();
        for r in regions {
            let a = Span {
                StartKey: key.to_vec(),
                EndKey: endKey.to_vec(),
            };
            let b = Span {
                StartKey: r.start.clone(),
                EndKey: r.end.clone(),
            };
            if Overlaps(&a, &b) && (result.len() as i32) < limit {
                result.push(Self::to_region_with_leader(&r));
            } else if r.start.as_slice() > key {
                break;
            }
        }
        Ok(result)
    }

    /// 列出全部 Store。
    fn Stores(&self) -> Result<Vec<Store>, String> {
        Ok(self
            .store_list()
            .into_iter()
            .map(|s| Store {
                ID: s.id,
                BootAt: s.boot_at,
            })
            .collect())
    }

    /// 提升服务安全点；目标低于当前值则报错。
    fn BlockGCUntil(&self, at: u64) -> Result<u64, String> {
        let mut g = self.inner.lock().unwrap();
        if g.service_gc > at {
            return Err(format!(
                "minimal safe point {} is greater than the target {at}",
                g.service_gc
            ));
        }
        g.service_gc = at;
        g.service_gc_set = 1;
        Ok(at)
    }

    /// 标记解除 GC 阻塞（测试计数用）。
    fn UnblockGC(&self) -> Result<(), String> {
        let mut g = self.inner.lock().unwrap();
        g.service_gc_deleted = 1;
        Ok(())
    }

    /// 返回集群当前 TSO。
    fn FetchCurrentTS(&self) -> Result<u64, String> {
        Ok(self.inner.lock().unwrap().current_ts)
    }
}

/// 基础集群不提供 live client；测试应使用 SharedFakeCluster。
impl LogBackupService for FakeCluster {
    /// 获取 Store 的日志备份客户端。
    fn GetLogBackupClient(&self, storeID: u64) -> Result<Arc<dyn LogBackupClient>, String> {
        if let Some(hook) = self.inner.lock().unwrap().on_get_client.clone() {
            hook(storeID)?;
        }
        if !self.inner.lock().unwrap().stores.contains_key(&storeID) {
            return Err(format!("the store {storeID} doesn't exist"));
        }
        // live 检查点客户端由 SharedFakeCluster 提供。
        Err(format!(
            "use SharedFakeCluster for live log backup client (store {storeID})"
        ))
    }

    /// 清理 Store 客户端缓存。
    fn ClearCache(&self, storeID: u64) -> Result<(), String> {
        let hook = {
            let mut g = self.inner.lock().unwrap();
            g.cleared_cache.push(storeID);
            g.on_clear_cache.clone()
        };
        if let Some(hook) = hook {
            hook(storeID)?;
        }
        Ok(())
    }
}

/// 共享集群包装，使 LogBackupClient 能读取实时 Region 检查点。
pub struct SharedFakeCluster {
    /// 底层假集群（可变拓扑）。
    pub cluster: Arc<FakeCluster>,
}

impl SharedFakeCluster {
    /// 构造共享假集群。
    pub fn new(n: usize, sim_enabled: bool) -> Arc<Self> {
        Arc::new(Self {
            cluster: FakeCluster::new_basic(n, sim_enabled),
        })
    }
}

/// 元数据调用全部委托给内部 FakeCluster。
impl TiKVClusterMeta for SharedFakeCluster {
    /// 返回与键范围重叠的 Region，受 limit 限制。
    fn RegionScan(
        &self,
        key: &[u8],
        endKey: &[u8],
        limit: i32,
    ) -> Result<Vec<RegionWithLeader>, String> {
        self.cluster.RegionScan(key, endKey, limit)
    }
    /// 列出全部 Store。
    fn Stores(&self) -> Result<Vec<Store>, String> {
        self.cluster.Stores()
    }
    /// 提升服务安全点；目标低于当前值则报错。
    fn BlockGCUntil(&self, at: u64) -> Result<u64, String> {
        self.cluster.BlockGCUntil(at)
    }
    /// 标记解除 GC 阻塞（测试计数用）。
    fn UnblockGC(&self) -> Result<(), String> {
        self.cluster.UnblockGC()
    }
    /// 返回集群当前 TSO。
    fn FetchCurrentTS(&self) -> Result<u64, String> {
        self.cluster.FetchCurrentTS()
    }
}

/// 返回绑定 Store 的 LiveClusterLogClient。
impl LogBackupService for SharedFakeCluster {
    /// 获取 Store 的日志备份客户端。
    fn GetLogBackupClient(&self, storeID: u64) -> Result<Arc<dyn LogBackupClient>, String> {
        if let Some(hook) = self.cluster.inner.lock().unwrap().on_get_client.clone() {
            hook(storeID)?;
        }
        let exists = self
            .cluster
            .inner
            .lock()
            .unwrap()
            .stores
            .contains_key(&storeID);
        if !exists {
            return Err(format!("the store {storeID} doesn't exist"));
        }
        Ok(Arc::new(LiveClusterLogClient {
            cluster: self.cluster.clone(),
            store_id: storeID,
        }))
    }
    /// 清理 Store 客户端缓存。
    fn ClearCache(&self, storeID: u64) -> Result<(), String> {
        self.cluster.ClearCache(storeID)
    }
}

/// 面向单个 Store 的实时 flush TS 客户端。
struct LiveClusterLogClient {
    cluster: Arc<FakeCluster>,
    store_id: u64,
}

/// 按请求 Region 列表填充 Checkpoints；必要时回退全表查找。
impl LogBackupClient for LiveClusterLogClient {
    /// 按请求填充各 Region 的最近 flush TS。
    fn GetLastFlushTSOfRegion(
        &self,
        req: &GetLastFlushTSOfRegionRequest,
    ) -> Result<GetLastFlushTSOfRegionResponse, String> {
        let sim_enabled = self.cluster.inner.lock().unwrap().sim_enabled;
        let regions = self.cluster.region_list();
        let mut out = Vec::new();
        for id in req.GetRegions() {
            let Some(region) = regions.iter().find(|region| region.id == id.Id) else {
                out.push(RegionCheckpoint {
                    Region: id.clone(),
                    Checkpoint: 0,
                    Err: Some(crate::stubs::RegionError {
                        NotLeader: true,
                        ..Default::default()
                    }),
                });
                continue;
            };
            let err = if region.leader != self.store_id {
                Some(crate::stubs::RegionError {
                    NotLeader: true,
                    ..Default::default()
                })
            } else if region.epoch != id.EpochVersion
                || (sim_enabled && region.flushed_epoch != id.EpochVersion)
            {
                Some(crate::stubs::RegionError {
                    EpochNotMatch: true,
                    ..Default::default()
                })
            } else {
                None
            };
            out.push(RegionCheckpoint {
                Region: RegionIdentity {
                    Id: region.id,
                    EpochVersion: region.epoch,
                },
                Checkpoint: if err.is_none() { region.checkpoint } else { 0 },
                Err: err,
            });
        }
        Ok(GetLastFlushTSOfRegionResponse { Checkpoints: out })
    }

    fn SubscribeFlushEvents(
        &self,
    ) -> Result<std::sync::mpsc::Receiver<Vec<crate::stubs::FlushEvent>>, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut inner = self.cluster.inner.lock().unwrap();
        let store = inner
            .stores
            .get(&self.store_id)
            .ok_or_else(|| format!("the store {} doesn't exist", self.store_id))?;
        if !store.supports_sub {
            return Err("Unimplemented: flush subscription".into());
        }
        inner.flush_subscribers.push((self.store_id, tx));
        Ok(rx)
    }
}

/// Go `testEnv`：带全局检查点与锁注入的完整 Env 模拟。
pub struct TestEnv {
    pub cluster: Arc<FakeCluster>,
    /// 共享包装，提供 LogBackupClient。
    pub shared: Arc<SharedFakeCluster>,
    /// Env 侧持久化的 V3 全局检查点。
    checkpoint: Mutex<u64>,
    /// 模拟 PD 瞬时断开，下一次 Get 失败后自动恢复。
    pd_disconnected: AtomicBool,
    /// 当前任务范围，可供测试改写。
    pub ranges: Mutex<Vec<KeyRange>>,
    /// 可选任务事件通道，模拟 StartTaskListener。
    task_ch: Mutex<Option<std::sync::mpsc::Sender<TaskEvent>>>,
    /// Begin 时推送的当前任务快照。
    pub task: Mutex<TaskEvent>,
    /// 可选 resolve-lock 钩子；优先于默认清锁逻辑。
    pub resolve_locks:
        Mutex<Option<Arc<dyn Fn(u64, &[u8], &[u8]) -> Result<(), String> + Send + Sync>>>,
    /// 若设置，ResolveLocksForRange 直接返回该错误串。
    pub scan_locked_err: Mutex<Option<String>>,
    /// 可选 flush 间隔钩子；缺省读 Command 默认配置。
    pub get_log_backup_flush_interval:
        Mutex<Option<Arc<dyn Fn() -> Result<Duration, String> + Send + Sync>>>,
    /// 是否已调用过 BlockGCUntil。
    pub block_gc_attempted: AtomicBool,
    /// 为 true 时 GetLogBackupClient 注入失败。
    pub fail_get_client: AtomicBool,
}

/// 便捷构造 SharedFakeCluster。
pub fn create_fake_cluster(n: usize, sim_enabled: bool) -> Arc<SharedFakeCluster> {
    SharedFakeCluster::new(n, sim_enabled)
}

/// 由共享集群构造默认 whole 任务的 TestEnv。
pub fn new_test_env(c: &Arc<SharedFakeCluster>) -> Arc<TestEnv> {
    let ranges = vec![KeyRange::default()];
    let task = TaskEvent {
        Type: EventType::EventAdd,
        Name: "whole".into(),
        Info: Some(StreamBackupTaskInfo {
            Name: "whole".into(),
            StartTs: 0,
            ..Default::default()
        }),
        Ranges: ranges.clone(),
        Err: None,
    };
    Arc::new(TestEnv {
        cluster: c.cluster.clone(),
        shared: c.clone(),
        checkpoint: Mutex::new(0),
        pd_disconnected: AtomicBool::new(false),
        ranges: Mutex::new(ranges),
        task_ch: Mutex::new(None),
        task: Mutex::new(task),
        resolve_locks: Mutex::new(None),
        scan_locked_err: Mutex::new(None),
        get_log_backup_flush_interval: Mutex::new(None),
        block_gc_attempted: AtomicBool::new(false),
        fail_get_client: AtomicBool::new(false),
    })
}

/// TestEnv 辅助方法：检查点、任务事件与 PD 断连模拟。
impl TestEnv {
    pub fn get_checkpoint(&self) -> u64 {
        *self.checkpoint.lock().unwrap()
    }

    pub fn mock_pd_connection_error(&self) {
        self.pd_disconnected.store(true, Ordering::SeqCst);
    }

    fn connect_pd(&self) -> bool {
        if !self.pd_disconnected.load(Ordering::SeqCst) {
            return true;
        }
        self.pd_disconnected.store(false, Ordering::SeqCst);
        false
    }

    pub fn advance_checkpoint_by(&self, duration: Duration) {
        let mut cp = self.checkpoint.lock().unwrap();
        *cp = ts_add_duration(*cp, duration);
    }

    pub fn unregister_task(&self) {
        if let Some(ch) = self.task_ch.lock().unwrap().as_ref() {
            let _ = ch.send(TaskEvent {
                Type: EventType::EventDel,
                Name: "whole".into(),
                Info: None,
                Ranges: Vec::new(),
                Err: None,
            });
        }
    }

    pub fn put_task(&self) {
        let rngs = self.ranges.lock().unwrap().clone();
        let rngs = if rngs.is_empty() {
            vec![KeyRange::default()]
        } else {
            rngs
        };
        let ev = TaskEvent {
            Type: EventType::EventAdd,
            Name: "whole".into(),
            Info: Some(StreamBackupTaskInfo {
                Name: "whole".into(),
                StartTs: 0,
                ..Default::default()
            }),
            Ranges: rngs,
            Err: None,
        };
        *self.task.lock().unwrap() = ev.clone();
        if let Some(ch) = self.task_ch.lock().unwrap().as_ref() {
            let _ = ch.send(ev);
        }
    }

    pub fn resume_task(&self) {
        if let Some(ch) = self.task_ch.lock().unwrap().as_ref() {
            let _ = ch.send(TaskEvent {
                Type: EventType::EventResume,
                Name: "whole".into(),
                Info: None,
                Ranges: Vec::new(),
                Err: None,
            });
        }
    }

    pub fn bind_task_channel(&self, tx: std::sync::mpsc::Sender<TaskEvent>) {
        *self.task_ch.lock().unwrap() = Some(tx);
    }
}

/// TestEnv 元数据委托 shared，并记录 BlockGC 尝试。
impl TiKVClusterMeta for TestEnv {
    /// 返回与键范围重叠的 Region，受 limit 限制。
    fn RegionScan(
        &self,
        key: &[u8],
        endKey: &[u8],
        limit: i32,
    ) -> Result<Vec<RegionWithLeader>, String> {
        self.shared.RegionScan(key, endKey, limit)
    }
    /// 列出全部 Store。
    fn Stores(&self) -> Result<Vec<Store>, String> {
        self.shared.Stores()
    }
    /// 提升服务安全点；目标低于当前值则报错。
    fn BlockGCUntil(&self, at: u64) -> Result<u64, String> {
        self.block_gc_attempted.store(true, Ordering::SeqCst);
        self.shared.BlockGCUntil(at)
    }
    /// 标记解除 GC 阻塞（测试计数用）。
    fn UnblockGC(&self) -> Result<(), String> {
        self.shared.UnblockGC()
    }
    /// 返回集群当前 TSO。
    fn FetchCurrentTS(&self) -> Result<u64, String> {
        self.shared.FetchCurrentTS()
    }
}

/// 可注入 GetClient 失败，否则委托 shared。
impl LogBackupService for TestEnv {
    /// 获取 Store 的日志备份客户端。
    fn GetLogBackupClient(&self, storeID: u64) -> Result<Arc<dyn LogBackupClient>, String> {
        // 注入失败优先于委托 shared。
        if self.fail_get_client.load(Ordering::SeqCst) {
            return Err("injected get client failure".into());
        }
        self.shared.GetLogBackupClient(storeID)
    }
    /// 清理 Store 客户端缓存。
    fn ClearCache(&self, storeID: u64) -> Result<(), String> {
        self.shared.ClearCache(storeID)
    }
}

/// 内存版 StreamMeta：全局检查点单调上传，支持暂停事件。
impl StreamMeta for TestEnv {
    /// 将当前任务推入事件列表。
    fn Begin(&self, ch: &mut Vec<TaskEvent>) -> Result<(), String> {
        let task = self.task.lock().unwrap().clone();
        ch.push(task);
        Ok(())
    }
    /// 单调上传任务全局检查点。
    fn UploadV3GlobalCheckpointForTask(
        &self,
        _taskName: &str,
        checkpoint: u64,
    ) -> Result<(), String> {
        let mut g = self.checkpoint.lock().unwrap();
        if checkpoint < *g {
            return Err("checkpoint rolling back".into());
        }
        *g = checkpoint;
        Ok(())
    }
    /// 读取任务全局检查点。
    fn GetGlobalCheckpointForTask(&self, _taskName: &str) -> Result<u64, String> {
        if !self.connect_pd() {
            return Err("pd disconnected".into());
        }
        Ok(*self.checkpoint.lock().unwrap())
    }
    /// 清零任务全局检查点。
    fn ClearV3GlobalCheckpointForTask(&self, _taskName: &str) -> Result<(), String> {
        *self.checkpoint.lock().unwrap() = 0;
        Ok(())
    }
    /// 发送或处理任务暂停。
    fn PauseTask(&self, taskName: &str) -> Result<(), String> {
        if let Some(ch) = self.task_ch.lock().unwrap().as_ref() {
            let _ = ch.send(TaskEvent {
                Type: EventType::EventPause,
                Name: taskName.to_string(),
                Info: None,
                Ranges: Vec::new(),
                Err: None,
            });
        }
        Ok(())
    }
}

/// 支持错误注入、maxVersion 校验与钩子；默认清除首个 locks_below。
impl RegionLockResolver for TestEnv {
    /// 在范围内解决锁；可注入错误或钩子。
    fn ResolveLocksForRange(
        &self,
        maxVersion: u64,
        startKey: &[u8],
        endKey: &[u8],
    ) -> Result<(), String> {
        if let Some(err) = self.scan_locked_err.lock().unwrap().clone() {
            return Err(err);
        }
        let max_ts = self.cluster.max_ts();
        if max_ts != 0 && max_ts != maxVersion {
            return Err(format!(
                "unexpect max version in scan lock, expected {max_ts}, actual {maxVersion}"
            ));
        }
        if let Some(hook) = self.resolve_locks.lock().unwrap().clone() {
            return hook(maxVersion, startKey, endKey);
        }
        // 若存在锁则清除首个 Region（对齐 Go ResolveLocksInOneRegion）。
        let regions = self.cluster.region_list();
        for r in regions {
            if r.locks_below.is_some() {
                self.cluster.set_region_locks_below(r.id, None);
                break;
            }
        }
        let _ = (startKey, endKey);
        Ok(())
    }
}

/// 优先调用注入钩子，否则返回 Command 默认 resolve-lock 间隔。
impl LogBackupFlushIntervalGetter for TestEnv {
    /// 返回 log-backup flush 间隔。
    fn GetLogBackupFlushInterval(&self) -> Result<Duration, String> {
        if let Some(hook) = self.get_log_backup_flush_interval.lock().unwrap().clone() {
            return hook();
        }
        Ok(DefaultCommandConfig().GetResolveLockInterval())
    }
}

/// SharedFakeCluster 的空 StreamMeta：不持久化全局检查点。
impl StreamMeta for SharedFakeCluster {
    fn Begin(&self, _ch: &mut Vec<TaskEvent>) -> Result<(), String> {
        Ok(())
    }
    /// 单调上传任务全局检查点。
    fn UploadV3GlobalCheckpointForTask(
        &self,
        _taskName: &str,
        _checkpoint: u64,
    ) -> Result<(), String> {
        Ok(())
    }
    /// 读取任务全局检查点。
    fn GetGlobalCheckpointForTask(&self, _taskName: &str) -> Result<u64, String> {
        Ok(0)
    }
    /// 清零任务全局检查点。
    fn ClearV3GlobalCheckpointForTask(&self, _taskName: &str) -> Result<(), String> {
        Ok(())
    }
    /// 空实现：忽略暂停。
    fn PauseTask(&self, _taskName: &str) -> Result<(), String> {
        Ok(())
    }
}

/// 空锁解决实现（恒成功）。
impl RegionLockResolver for SharedFakeCluster {
    /// 在范围内解决锁；可注入错误或钩子。
    fn ResolveLocksForRange(
        &self,
        _maxVersion: u64,
        _startKey: &[u8],
        _endKey: &[u8],
    ) -> Result<(), String> {
        Ok(())
    }
}

/// 返回 Command 默认 resolve-lock 间隔。
impl LogBackupFlushIntervalGetter for SharedFakeCluster {
    /// 返回 log-backup flush 间隔。
    fn GetLogBackupFlushInterval(&self) -> Result<Duration, String> {
        Ok(DefaultCommandConfig().GetResolveLockInterval())
    }
}

/// 仅令第一次 GetClient 失败的钩子工厂。
pub fn one_store_failure() -> Arc<dyn Fn(u64) -> Result<(), String> + Send + Sync> {
    let once = AtomicBool::new(false);
    Arc::new(move |_id| {
        if once
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            Err("one store failure".into())
        } else {
            Ok(())
        }
    })
}

/// 生成 `[from,to)` 的六位切分键，便于大规模 Region 布局。
pub fn many_regions(from: i32, to: i32) -> Vec<String> {
    (from..to).map(|i| format!("{i:06}")).collect()
}

/// 为全部 Store 打开 flush 订阅支持。
pub fn install_subscribe_support(c: &SharedFakeCluster) {
    c.cluster.set_support_flush_sub_all(true);
}

/// 仅为前 n 个 Store 打开 flush 订阅支持。
pub fn install_subscribe_support_for_random_n(c: &SharedFakeCluster, n: usize) {
    c.cluster.set_support_flush_sub_n(n);
}
