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

// Batch Coprocessor：把多个 Region 的协处理器任务合并后发往 TiFlash。
//
// 负责任务均衡（连续性分配 / 贪心负载均衡）、存活 store 过滤、存算分离（disaggregated）
// 下的一致性哈希/轮询调度，以及并行响应迭代器 `batchCopIterator`。
// Region 是键空间的分片单位；TiFlash 为列存加速引擎。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::batch_request_sender::{
    Backoffer, BatchError, BatchResponse, BatchResult, CommandType, CoprocessorRegionInfo,
    KeyRange, KeyRanges, RegionInfo, RegionVerId, RpcContext, Store, TableRegions,
};

/// 拉取拓扑信息时的最大退避上限（与 Go fetchTopoMaxBackoff 对齐）。
pub const FETCH_TOPO_MAX_BACKOFF: usize = 20_000;
/// 均衡分数满分；分数越高表示各 store 上 Region 数量越均衡。
pub const MAX_BALANCE_SCORE: i32 = 100;
/// 认为「已均衡」的分数阈值；低于此值可回退到贪心策略。
pub const BALANCE_SCORE_THRESHOLD: i32 = 85;
/// ClosestReplicas 策略下，单节点允许的远程读 Region 数上限。
pub const MAX_REMOTE_READ_COUNT_PER_NODE_FOR_CLOSEST_REPLICAS: usize = 3;
/// TiFlash 超长读超时（1 小时），用于大范围扫描等场景。
pub const TI_FLASH_READ_TIMEOUT_ULTRA_LONG: Duration = Duration::from_secs(3_600);

/// 发往单个 store 的批量协处理器任务：地址、命令、RPC 上下文与 Region/分区表信息。
#[derive(Clone, Debug, Default)]
#[allow(non_camel_case_types, non_snake_case)]
pub struct batchCopTask {
    pub storeAddr: String,
    pub cmdType: CommandType,
    pub ctx: RpcContext,
    pub regionInfos: Vec<RegionInfo>,
    pub PartitionTableRegions: Vec<TableRegions>,
}

impl batchCopTask {
    pub fn get_address(&self) -> &str {
        &self.storeAddr
    }

    /// 任务覆盖的 Region 数量（普通 regionInfos 或分区表 regions 之和）。
    pub fn region_count(&self) -> usize {
        if self.regionInfos.is_empty() {
            self.PartitionTableRegions
                .iter()
                .map(|table| table.regions.len())
                .sum()
        } else {
            self.regionInfos.len()
        }
    }
}

/// `batchCopTask` 的 Go/Rust 风格别名。
pub type BatchCopTask = batchCopTask;

/// 协处理器运行时统计：退避耗时、按原因聚合的 sleep/次数与对端地址。
#[derive(Clone, Debug, Default)]
pub struct CopRuntimeStats {
    pub backoff_time: Duration,
    pub backoff_sleep: HashMap<String, Duration>,
    pub backoff_times: HashMap<String, usize>,
    pub callee_address: String,
    pub read_pool_task_details: Option<crate::pool_task_details::PoolTaskDetails>,
}

/// 批量协处理器响应封装：protobuf 载荷、运行时详情、起始键与错误。
#[derive(Clone, Debug, Default)]
#[allow(non_camel_case_types)]
pub struct batchCopResponse {
    pub pb_resp: Option<BatchResponse>,
    pub detail: Option<CopRuntimeStats>,
    pub start_key: Vec<u8>,
    pub err: Option<BatchError>,
    resp_size: usize,
    pub resp_time: Duration,
}

impl batchCopResponse {
    pub fn get_data(&self) -> &[u8] {
        self.pb_resp
            .as_ref()
            .map(|response| response.data.as_slice())
            .unwrap_or_default()
    }

    pub fn get_start_key(&self) -> &[u8] {
        &self.start_key
    }

    pub fn get_cop_runtime_stats(&self) -> Option<&CopRuntimeStats> {
        self.detail.as_ref()
    }

    /// 惰性计算并缓存响应大致内存占用。
    pub fn mem_size(&mut self) -> usize {
        if self.resp_size == 0 {
            self.resp_size = self.start_key.capacity();
            if self.detail.is_some() {
                self.resp_size += std::mem::size_of::<CopRuntimeStats>();
            }
            if let Some(response) = &self.pb_resp {
                self.resp_size += response.encoded_size();
            }
        }
        self.resp_size
    }

    pub fn response_time(&self) -> Duration {
        self.resp_time
    }
}

/// `batchCopResponse` 别名。
pub type BatchCopResponse = batchCopResponse;

/// 深拷贝 store→task 映射，并为 regionInfos 预留 average_regions 容量。
pub fn deep_copy_store_task_map(
    store_task_map: &HashMap<u64, batchCopTask>,
    average_regions: usize,
) -> HashMap<u64, batchCopTask> {
    store_task_map
        .iter()
        .map(|(&store_id, task)| {
            let mut regions = Vec::with_capacity(task.regionInfos.len() + average_regions);
            regions.extend_from_slice(&task.regionInfos);
            (
                store_id,
                batchCopTask {
                    regionInfos: regions,
                    PartitionTableRegions: Vec::new(),
                    ..task.clone()
                },
            )
        })
        .collect()
}

/// 候选 Region 数加上已分配到各 store 任务中的 Region 总数。
pub fn region_total_count(
    store_tasks: &HashMap<u64, batchCopTask>,
    candidate_region_infos: &[RegionInfo],
) -> usize {
    candidate_region_infos.len()
        + store_tasks
            .values()
            .map(|task| task.regionInfos.len())
            .sum::<usize>()
}

/// 从 store 对应的 Region 下标队列中选出尚未选中的 Region，最多 count 个。
pub fn select_region(
    store_id: u64,
    candidate_region_infos: &[RegionInfo],
    selected: &mut [bool],
    store_id_to_region_index: &mut HashMap<u64, VecDeque<usize>>,
    count: usize,
) -> Vec<RegionInfo> {
    let Some(region_indexes) = store_id_to_region_index.get_mut(&store_id) else {
        return Vec::new();
    };
    let mut result = Vec::with_capacity(count);
    while result.len() < count {
        let Some(index) = region_indexes.pop_front() else {
            break;
        };
        if !selected[index] {
            selected[index] = true;
            result.push(candidate_region_infos[index].clone());
        }
    }
    result
}

/// 根据最大/最小 Region 数与连续块大小计算均衡分数。
pub fn balance_score(
    max_region_count: usize,
    min_region_count: usize,
    balance_continuous_region_count: usize,
) -> i32 {
    if min_region_count == 0 {
        return i32::MIN;
    }
    let unbalanced = max_region_count.saturating_sub(min_region_count);
    if unbalanced <= balance_continuous_region_count {
        return MAX_BALANCE_SCORE;
    }
    MAX_BALANCE_SCORE - (unbalanced * 100 / min_region_count) as i32
}

/// 分数是否达到均衡阈值。
pub fn is_balance(score: i32) -> bool {
    score >= BALANCE_SCORE_THRESHOLD
}

/// 检查各 store 任务的 Region 数量均衡度，返回分数与描述信息。
pub fn check_batch_cop_task_balance(
    store_tasks: &HashMap<u64, batchCopTask>,
    balance_continuous_region_count: usize,
) -> (i32, Vec<String>) {
    if store_tasks.is_empty() {
        return (0, Vec::new());
    }
    let max = store_tasks
        .values()
        .map(|task| task.regionInfos.len())
        .max()
        .unwrap_or_default();
    let min = store_tasks
        .values()
        .map(|task| task.regionInfos.len())
        .min()
        .unwrap_or_default();
    let details = store_tasks
        .iter()
        .map(|(id, task)| {
            format!(
                "storeID {id} storeAddr {} regionCount {}",
                task.storeAddr,
                task.regionInfos.len()
            )
        })
        .collect();
    (
        balance_score(max, min, balance_continuous_region_count),
        details,
    )
}

pub(crate) fn prefer_contiguous_tasks(
    greedy_score: i32,
    contiguous_tasks: Option<Vec<batchCopTask>>,
) -> Option<Vec<batchCopTask>> {
    if !is_balance(greedy_score) {
        contiguous_tasks
    } else {
        None
    }
}

/// Assigns ordered regions in fixed-size runs so TiFlash receives nearby ranges.
/// The caller can fall back to the greedy balancer when the returned score is low.
/// 按有序 Region 以固定大小游程分配，使 TiFlash 收到邻近 range；分数过低时可回退。
pub fn balance_batch_cop_task_with_continuity(
    store_task_map: &HashMap<u64, batchCopTask>,
    candidate_region_infos: &[RegionInfo],
    balance_continuous_region_count: usize,
) -> (Option<Vec<batchCopTask>>, i32) {
    if candidate_region_infos.len() < 500 || store_task_map.is_empty() {
        return (None, 0);
    }
    let region_count = region_total_count(store_task_map, candidate_region_infos);
    let mut store_tasks = deep_copy_store_task_map(
        store_task_map,
        candidate_region_infos.len() / store_task_map.len(),
    );
    let mut indexes: HashMap<u64, VecDeque<usize>> = HashMap::new();
    for (index, region) in candidate_region_infos.iter().enumerate() {
        for store_id in &region.AllStores {
            indexes.entry(*store_id).or_default().push_back(index);
        }
    }
    let mut selected = vec![false; candidate_region_infos.len()];
    loop {
        let mut selected_this_round = 0;
        for (store_id, task) in &mut store_tasks {
            let picked = select_region(
                *store_id,
                candidate_region_infos,
                &mut selected,
                &mut indexes,
                balance_continuous_region_count,
            );
            selected_this_round += picked.len();
            task.regionInfos.extend(picked);
        }
        let assigned = store_tasks
            .values()
            .map(|task| task.regionInfos.len())
            .sum::<usize>();
        if assigned >= region_count {
            break;
        }
        if selected_this_round == 0 {
            return (None, 0);
        }
    }

    let (score, _) = check_batch_cop_task_balance(&store_tasks, balance_continuous_region_count);
    let tasks: Vec<_> = store_tasks
        .into_values()
        .filter(|task| !task.regionInfos.is_empty())
        .collect();
    if tasks
        .iter()
        .map(|task| task.regionInfos.len())
        .sum::<usize>()
        != region_count
    {
        return (None, 0);
    }
    (Some(tasks), score)
}

/// Balances regions using the same two-stage policy as Go: fixed assignments first,
/// then either continuity-aware assignment or the weighted greedy fallback.
/// 两阶段均衡：先固定分配，再按连续性或加权贪心把候选 Region 分到各 store。
pub fn balance_batch_cop_task(
    alive_stores: &[Store],
    original_tasks: Vec<batchCopTask>,
    balance_with_continuity: bool,
    balance_continuous_region_count: usize,
    all_region_infos: Vec<RegionInfo>,
) -> Vec<batchCopTask> {
    if original_tasks.is_empty() {
        return original_tasks;
    }
    let mut store_tasks: HashMap<u64, batchCopTask> = alive_stores
        .iter()
        .map(|store| {
            (
                store.id,
                batchCopTask {
                    storeAddr: store.address.clone(),
                    cmdType: original_tasks[0].cmdType,
                    ctx: RpcContext {
                        address: store.address.clone(),
                        store: Some(store.clone()),
                        ..RpcContext::default()
                    },
                    ..batchCopTask::default()
                },
            )
        })
        .collect();
    let mut candidates: HashMap<u64, HashMap<RegionVerId, RegionInfo>> = HashMap::new();
    let mut candidate_regions = Vec::new();
    let mut total_candidate_entries = 0usize;

    for region in all_region_infos {
        let valid: Vec<u64> = region
            .AllStores
            .iter()
            .copied()
            .filter(|id| store_tasks.contains_key(id))
            .collect();
        match valid.as_slice() {
            [] => return original_tasks,
            [only] => store_tasks
                .get_mut(only)
                .expect("validated store disappeared")
                .regionInfos
                .push(region),
            _ => {
                for store_id in valid {
                    let old = candidates
                        .entry(store_id)
                        .or_default()
                        .insert(region.Region, region.clone());
                    if old.is_some() {
                        return original_tasks;
                    }
                    total_candidate_entries += 1;
                }
                candidate_regions.push(region);
            }
        }
    }

    let mut contiguous = None;
    let mut contiguous_score = 0;
    if balance_with_continuity {
        (contiguous, contiguous_score) = balance_batch_cop_task_with_continuity(
            &store_tasks,
            &candidate_regions,
            balance_continuous_region_count,
        );
        if is_balance(contiguous_score)
            && let Some(tasks) = contiguous
        {
            return tasks;
        }
    }

    let mut remaining = candidate_regions.len();
    let find_next_store = |preferred: Option<&[u64]>,
                           candidates: &HashMap<u64, HashMap<RegionVerId, RegionInfo>>,
                           tasks: &HashMap<u64, batchCopTask>,
                           average: f64|
     -> Option<u64> {
        let ids: Vec<u64> = preferred
            .map(|ids| ids.to_vec())
            .unwrap_or_else(|| candidates.keys().copied().collect());
        ids.into_iter()
            .filter(|id| candidates.contains_key(id))
            .min_by(|left, right| {
                let left_weight =
                    candidates[left].len() as f64 / average + tasks[left].regionInfos.len() as f64;
                let right_weight = candidates[right].len() as f64 / average
                    + tasks[right].regionInfos.len() as f64;
                left_weight
                    .partial_cmp(&right_weight)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| left.cmp(right))
            })
    };

    let mut next_store = if remaining == 0 {
        None
    } else {
        find_next_store(
            None,
            &candidates,
            &store_tasks,
            total_candidate_entries as f64 / remaining as f64,
        )
    };
    while remaining > 0 {
        let Some(store_id) = next_store else {
            return original_tasks;
        };
        let Some((&key, region)) = candidates[&store_id].iter().next() else {
            return original_tasks;
        };
        let region = region.clone();
        store_tasks
            .get_mut(&store_id)
            .expect("candidate store disappeared")
            .regionInfos
            .push(region.clone());
        remaining -= 1;
        for id in &region.AllStores {
            if let Some(store_candidates) = candidates.get_mut(id)
                && store_candidates.remove(&key).is_some()
            {
                total_candidate_entries -= 1;
            }
            if candidates.get(id).is_some_and(HashMap::is_empty) {
                candidates.remove(id);
            }
        }
        next_store = if remaining == 0 {
            None
        } else {
            find_next_store(
                Some(&region.AllStores),
                &candidates,
                &store_tasks,
                total_candidate_entries as f64 / remaining as f64,
            )
        };
    }

    if let Some(contiguous_tasks) = contiguous {
        let (greedy_score, _) =
            check_batch_cop_task_balance(&store_tasks, balance_continuous_region_count);
        if let Some(tasks) = prefer_contiguous_tasks(greedy_score, Some(contiguous_tasks)) {
            return tasks;
        }
    }
    store_tasks
        .into_values()
        .filter(|task| !task.regionInfos.is_empty())
        .collect()
}

/// TiFlash Compute 节点调度策略：一致性哈希或轮询。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchPolicy {
    Invalid,
    RoundRobin,
    ConsistentHash,
}

/// 副本读策略：全部副本或就近副本（ClosestReplicas）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplicaReadPolicy {
    AllReplicas,
    ClosestReplicas,
    ClosestAdaptive,
}

impl ReplicaReadPolicy {
    pub fn is_all_replicas(self) -> bool {
        self == Self::AllReplicas
    }

    pub fn is_closest_replicas(self) -> bool {
        self == Self::ClosestReplicas
    }
}

/// 存活 store 及其 id 列表的打包结果。
#[derive(Clone, Debug, Default)]
pub struct AliveStoresBundle {
    pub stores_in_all_zones: Vec<Store>,
    pub store_ids_in_all_zones: HashSet<u64>,
    pub stores_in_tidb_zone: Vec<Store>,
    pub store_ids_in_tidb_zone: HashSet<u64>,
}

/// 按 TiFlash 副本读策略过滤可用 store 列表。
pub fn filter_all_stores_according_to_tiflash_replica_read(
    all_stores: &[u64],
    alive_stores: &AliveStoresBundle,
    policy: ReplicaReadPolicy,
) -> (Vec<u64>, bool) {
    if policy.is_all_replicas() {
        return (
            all_stores
                .iter()
                .copied()
                .filter(|id| alive_stores.store_ids_in_all_zones.contains(id))
                .collect(),
            false,
        );
    }
    let local: Vec<u64> = all_stores
        .iter()
        .copied()
        .filter(|id| alive_stores.store_ids_in_tidb_zone.contains(id))
        .collect();
    if !local.is_empty() {
        return (local, false);
    }
    match policy {
        ReplicaReadPolicy::ClosestAdaptive => (
            all_stores
                .iter()
                .copied()
                .filter(|id| alive_stores.store_ids_in_all_zones.contains(id))
                .collect(),
            true,
        ),
        ReplicaReadPolicy::ClosestReplicas => (
            alive_stores
                .store_ids_in_tidb_zone
                .iter()
                .copied()
                .collect(),
            true,
        ),
        ReplicaReadPolicy::AllReplicas => unreachable!(),
    }
}

/// 收集 Region 信息中实际用到的全部 TiFlash store id。
pub fn get_all_used_tiflash_stores(
    all_tiflash_stores: &[Store],
    used_store_ids: &HashSet<u64>,
) -> Vec<Store> {
    all_tiflash_stores
        .iter()
        .filter(|store| used_store_ids.contains(&store.id))
        .cloned()
        .collect()
}

/// 结合存活探测回调，返回存活 store 与其 id。
pub fn get_alive_stores_and_store_ids<F>(
    all_tiflash_stores: &[Store],
    used_store_ids: &HashSet<u64>,
    ttl: Duration,
    policy: ReplicaReadPolicy,
    tidb_zone: &str,
    is_alive: F,
) -> AliveStoresBundle
where
    F: Fn(&str, Duration) -> bool + Sync,
{
    let used = get_all_used_tiflash_stores(all_tiflash_stores, used_store_ids);
    let alive = filter_alive_stores(&used, ttl, is_alive);
    let all_ids = alive.iter().map(|store| store.id).collect();
    let local = if policy.is_all_replicas() {
        Vec::new()
    } else {
        alive
            .iter()
            .filter(|store| {
                store
                    .labels
                    .get("zone")
                    .is_some_and(|zone| zone == tidb_zone)
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    let local_ids = local.iter().map(|store| store.id).collect();
    AliveStoresBundle {
        stores_in_all_zones: alive,
        store_ids_in_all_zones: all_ids,
        stores_in_tidb_zone: local,
        store_ids_in_tidb_zone: local_ids,
    }
}

/// 按存活探测过滤 store 地址列表。
pub fn filter_alive_store_addresses<F>(
    addresses: &[String],
    ttl: Duration,
    is_alive: F,
) -> Vec<String>
where
    F: Fn(&str, Duration) -> bool + Sync,
{
    let indexes = filter_alive_store_indexes(addresses, ttl, is_alive);
    indexes
        .into_iter()
        .map(|index| addresses[index].clone())
        .collect()
}

/// 按存活探测过滤 Store 对象列表。
pub fn filter_alive_stores<F>(stores: &[Store], ttl: Duration, is_alive: F) -> Vec<Store>
where
    F: Fn(&str, Duration) -> bool + Sync,
{
    let addresses: Vec<_> = stores.iter().map(|store| store.address.clone()).collect();
    filter_alive_store_indexes(&addresses, ttl, is_alive)
        .into_iter()
        .map(|index| stores[index].clone())
        .collect()
}

// 返回存活 store 在原地址切片中的下标。
fn filter_alive_store_indexes<F>(stores: &[String], ttl: Duration, is_alive: F) -> Vec<usize>
where
    F: Fn(&str, Duration) -> bool + Sync,
{
    let (sender, receiver) = mpsc::channel();
    thread::scope(|scope| {
        let check = &is_alive;
        for (index, address) in stores.iter().enumerate() {
            let sender = sender.clone();
            scope.spawn(move || {
                if check(address, ttl) {
                    let _ = sender.send(index);
                }
            });
        }
    });
    drop(sender);
    let mut result: Vec<_> = receiver.into_iter().collect();
    result.sort_unstable();
    result
}

// MurmurHash3 32 位摘要，供一致性哈希选节点。
fn murmur3_sum32(bytes: &[u8]) -> u32 {
    let mut hash = 0u32;
    let mut chunks = bytes.chunks_exact(4);
    for chunk in &mut chunks {
        let mut value = u32::from_le_bytes(chunk.try_into().expect("four-byte chunk"));
        value = value.wrapping_mul(0xcc9e_2d51).rotate_left(15);
        value = value.wrapping_mul(0x1b87_3593);
        hash ^= value;
        hash = hash
            .rotate_left(13)
            .wrapping_mul(5)
            .wrapping_add(0xe654_6b64);
    }
    let tail = chunks.remainder();
    let mut value = 0u32;
    if tail.len() >= 3 {
        value ^= u32::from(tail[2]) << 16;
    }
    if tail.len() >= 2 {
        value ^= u32::from(tail[1]) << 8;
    }
    if let Some(first) = tail.first() {
        value ^= u32::from(*first);
        value = value.wrapping_mul(0xcc9e_2d51).rotate_left(15);
        value = value.wrapping_mul(0x1b87_3593);
        hash ^= value;
    }
    hash ^= bytes.len() as u32;
    hash ^= hash >> 16;
    hash = hash.wrapping_mul(0x85eb_ca6b);
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(0xc2b2_ae35);
    hash ^ (hash >> 16)
}

/// 用一致性哈希为 Region 选择 TiFlash Compute RPC 上下文。
pub fn get_tiflash_compute_rpc_context_by_consistent_hash(
    ids: &[RegionVerId],
    stores: &[String],
) -> BatchResult<Vec<RpcContext>> {
    if stores.is_empty() {
        return Err(BatchError::NoAliveStore(
            "cannot dispatch without a TiFlash compute node".to_owned(),
        ));
    }
    Ok(ids
        .iter()
        .map(|id| {
            let address = stores
                .iter()
                .max_by_key(|address| murmur3_sum32(format!("{address}-{}", id.id).as_bytes()))
                .expect("non-empty stores")
                .clone();
            RpcContext {
                region: *id,
                address,
                ..RpcContext::default()
            }
        })
        .collect())
}

static ROUND_ROBIN_SEED: AtomicUsize = AtomicUsize::new(0);

/// 用轮询为 Region 选择 TiFlash Compute RPC 上下文。
pub fn get_tiflash_compute_rpc_context_by_round_robin(
    ids: &[RegionVerId],
    stores: &[String],
) -> BatchResult<Vec<RpcContext>> {
    if stores.is_empty() {
        return Err(BatchError::NoAliveStore(
            "cannot dispatch without a TiFlash compute node".to_owned(),
        ));
    }
    let start = ROUND_ROBIN_SEED.fetch_add(1, Ordering::Relaxed) % stores.len();
    Ok(ids
        .iter()
        .enumerate()
        .map(|(offset, id)| RpcContext {
            region: *id,
            address: stores[(start + offset) % stores.len()].clone(),
            ..RpcContext::default()
        })
        .collect())
}

/// 在存算分离模式下，按调度策略把 Region 编成发往 Compute 节点的任务。
pub fn build_compute_tasks(
    regions: Vec<RegionInfo>,
    stores: &[String],
    policy: DispatchPolicy,
) -> BatchResult<Vec<batchCopTask>> {
    let ids: Vec<_> = regions.iter().map(|region| region.Region).collect();
    let contexts = match policy {
        DispatchPolicy::RoundRobin => get_tiflash_compute_rpc_context_by_round_robin(&ids, stores)?,
        DispatchPolicy::ConsistentHash => {
            get_tiflash_compute_rpc_context_by_consistent_hash(&ids, stores)?
        }
        DispatchPolicy::Invalid => {
            return Err(BatchError::InvalidDispatchPolicy("invalid".to_owned()));
        }
    };
    if contexts.len() != regions.len() {
        return Err(BatchError::OtherResponse(
            "RPC contexts and regions have different lengths".to_owned(),
        ));
    }
    let mut result: Vec<batchCopTask> = Vec::new();
    let mut indexes = HashMap::<String, usize>::new();
    for (region, context) in regions.into_iter().zip(contexts) {
        if let Some(index) = indexes.get(&context.address).copied() {
            result[index].regionInfos.push(region);
        } else {
            let index = result.len();
            indexes.insert(context.address.clone(), index);
            result.push(batchCopTask {
                storeAddr: context.address.clone(),
                cmdType: CommandType::BatchCop,
                ctx: context,
                regionInfos: vec![region],
                PartitionTableRegions: Vec::new(),
            });
        }
    }
    Ok(result)
}

/// 过滤可访问 store 并构建带 AllStores 的 RegionInfo。
pub fn filter_accessible_stores_and_build_region_info(
    mut region: RegionInfo,
    alive_stores: &AliveStoresBundle,
    policy: ReplicaReadPolicy,
    regions_needing_reload: &mut Vec<RegionInfo>,
    regions_in_other_zones: &mut Vec<u64>,
    max_remote_read_count_allowed: usize,
    tidb_zone: &str,
) -> BatchResult<RegionInfo> {
    let (stores, cross_zone) = filter_all_stores_according_to_tiflash_replica_read(
        &region.AllStores,
        alive_stores,
        policy,
    );
    region.AllStores = stores;
    if cross_zone {
        regions_in_other_zones.push(region.Region.id);
        regions_needing_reload.push(region.clone());
        if policy.is_closest_replicas()
            && regions_in_other_zones.len() > max_remote_read_count_allowed
        {
            let examples = regions_in_other_zones
                .iter()
                .take(3)
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(BatchError::RemoteReadLimit(format!(
                "no less than {} region(s) cannot be accessed by TiFlash in zone [{tidb_zone}]: {examples}, etc",
                regions_in_other_zones.len()
            )));
        }
    }
    Ok(region)
}

/// 判断是否可跳过存活探测（例如探测结果仍在 TTL 内）。
pub fn can_skip_check_alive_stores(
    alive_stores: &AliveStoresBundle,
    used_store_ids: &HashSet<u64>,
    policy: ReplicaReadPolicy,
    minimum_replica_count: usize,
    in_test: bool,
) -> bool {
    if in_test {
        return true;
    }
    if policy.is_closest_replicas() {
        return false;
    }
    let dead_store_count = used_store_ids
        .len()
        .saturating_sub(alive_stores.store_ids_in_all_zones.len());
    minimum_replica_count > dead_store_count
}

/// 探测单个 store 是否存活，并更新相关缓存状态。
pub fn check_alive_store(
    alive_stores: &AliveStoresBundle,
    stores_per_region: &[Vec<u64>],
    used_store_ids: &HashSet<u64>,
    policy: ReplicaReadPolicy,
    region_ids: &[RegionVerId],
    minimum_replica_count: usize,
    max_remote_read_count_allowed: usize,
    in_test: bool,
) -> (bool, Vec<RegionVerId>) {
    if can_skip_check_alive_stores(
        alive_stores,
        used_store_ids,
        policy,
        minimum_replica_count,
        in_test,
    ) {
        return (false, Vec::new());
    }
    if alive_stores.store_ids_in_all_zones.is_empty() {
        return (true, region_ids.to_vec());
    }

    let mut invalid = Vec::new();
    let mut remote = Vec::new();
    for (index, stores) in stores_per_region.iter().enumerate() {
        let local = stores
            .iter()
            .any(|id| alive_stores.store_ids_in_tidb_zone.contains(id));
        let anywhere = stores
            .iter()
            .any(|id| alive_stores.store_ids_in_all_zones.contains(id));
        if policy.is_closest_replicas() {
            if !local {
                if anywhere {
                    remote.push(region_ids[index]);
                } else {
                    invalid.push(region_ids[index]);
                }
            }
        } else if !anywhere {
            invalid.push(region_ids[index]);
        }
    }
    if policy.is_closest_replicas() && remote.len() > max_remote_read_count_allowed {
        invalid.extend(remote);
    }
    (!invalid.is_empty(), invalid)
}

/// 将扁平 RegionInfo 列表按分区表物理 id 归并为 TableRegions。
pub fn convert_region_infos_to_partition_table_regions(
    batch_tasks: &mut [batchCopTask],
    partition_ids: &[i64],
) -> BatchResult<()> {
    for task in batch_tasks {
        let mut tables: Vec<_> = partition_ids
            .iter()
            .map(|id| TableRegions {
                physical_table_id: *id,
                regions: Vec::new(),
            })
            .collect();
        for region in &task.regionInfos {
            let index = usize::try_from(region.PartitionIndex)
                .map_err(|_| BatchError::OtherResponse("negative partition index".to_owned()))?;
            let Some(table) = tables.get_mut(index) else {
                return Err(BatchError::OtherResponse(format!(
                    "partition index {index} is out of range"
                )));
            };
            table.regions.push(region.to_coprocessor_region_info());
        }
        tables.retain(|table| !table.regions.is_empty());
        task.PartitionTableRegions = tables;
        task.regionInfos.clear();
    }
    Ok(())
}

/// 构建 Batch Cop 任务所需的数据源：切分 range、拓扑、存活探测、RPC 上下文等。
pub trait BatchTaskSource: Send + Sync {
    fn split_key_ranges(
        &self,
        ranges: &KeyRanges,
        partition_index: i64,
    ) -> BatchResult<Vec<RegionInfo>>;
    fn fetch_topology(&self) -> BatchResult<Vec<String>>;
    fn compute_stores(&self) -> BatchResult<Vec<Store>>;
    fn is_store_alive(&self, address: &str, ttl: Duration) -> bool;
    fn rpc_context(&self, region: RegionVerId, is_mpp: bool) -> BatchResult<Option<RpcContext>>;
    fn all_valid_store_ids(&self, region: RegionVerId, primary_store_id: u64) -> Vec<u64>;
    fn all_tiflash_stores(&self) -> Vec<Store>;
    fn invalidate_region(&self, region: RegionVerId);
}

/// 构建 Batch Cop 任务的选项：均衡参数、副本策略、存算分离与探测 TTL 等。
#[derive(Clone, Debug)]
pub struct BatchBuildOptions {
    pub disaggregated_tiflash: bool,
    pub use_auto_scaler: bool,
    pub is_mpp: bool,
    pub ttl: Duration,
    pub balance_with_continuity: bool,
    pub balance_continuous_region_count: usize,
    pub dispatch_policy: DispatchPolicy,
    pub replica_read_policy: ReplicaReadPolicy,
    pub tidb_zone: Option<String>,
    pub max_retries: usize,
    pub in_test: bool,
}

impl Default for BatchBuildOptions {
    fn default() -> Self {
        Self {
            disaggregated_tiflash: false,
            use_auto_scaler: false,
            is_mpp: false,
            ttl: Duration::ZERO,
            balance_with_continuity: false,
            balance_continuous_region_count: 0,
            dispatch_policy: DispatchPolicy::Invalid,
            replica_read_policy: ReplicaReadPolicy::AllReplicas,
            tidb_zone: None,
            max_retries: 3,
            in_test: false,
        }
    }
}

// 将请求中的全部 key range 按 Region 位置切分。
fn split_all_ranges(
    source: &dyn BatchTaskSource,
    ranges_for_each_physical_table: &[KeyRanges],
) -> BatchResult<Vec<RegionInfo>> {
    let mut partitions = Vec::with_capacity(ranges_for_each_physical_table.len());
    for (index, ranges) in ranges_for_each_physical_table.iter().enumerate() {
        partitions.push(source.split_key_ranges(ranges, index as i64)?);
    }
    if partitions.len() > 1 {
        partitions.sort_by(|left, right| {
            let left_key = left
                .first()
                .and_then(|region| region.Ranges.iter().next())
                .map(|range| range.start.as_slice())
                .unwrap_or_default();
            let right_key = right
                .first()
                .and_then(|region| region.Ranges.iter().next())
                .map(|range| range.start.as_slice())
                .unwrap_or_default();
            left_key.cmp(right_key)
        });
    }
    Ok(partitions.into_iter().flatten().collect())
}

// 存算分离路径：按 Compute 拓扑构建任务。
fn build_disaggregated_tasks(
    source: &dyn BatchTaskSource,
    ranges: &[KeyRanges],
    options: &BatchBuildOptions,
    backoffer: &mut Backoffer,
) -> BatchResult<Vec<batchCopTask>> {
    let regions = split_all_ranges(source, ranges)?;
    let mut attempts = 0;
    let stores = loop {
        attempts += 1;
        let available = if options.use_auto_scaler {
            filter_alive_store_addresses(&source.fetch_topology()?, options.ttl, |address, ttl| {
                source.is_store_alive(address, ttl)
            })
        } else {
            filter_alive_stores(&source.compute_stores()?, options.ttl, |address, ttl| {
                source.is_store_alive(address, ttl)
            })
            .into_iter()
            .map(|store| store.address)
            .collect()
        };
        if !available.is_empty() {
            break available;
        }
        let error = BatchError::NoAliveStore(if options.use_auto_scaler {
            "cannot find an alive TiFlash compute node from autoscaler topology".to_owned()
        } else {
            "tiflash_compute node is unavailable".to_owned()
        });
        if options.in_test || attempts > options.max_retries {
            return Err(error);
        }
        backoffer.backoff(&error)?;
    };
    build_compute_tasks(regions, &stores, options.dispatch_policy)
}

// 存算一体路径：按 TiFlash store 与均衡策略构建任务。
fn build_integrated_tasks(
    source: &dyn BatchTaskSource,
    ranges: &[KeyRanges],
    options: &BatchBuildOptions,
    backoffer: &mut Backoffer,
    append_warning: &mut dyn FnMut(BatchError),
) -> BatchResult<Vec<batchCopTask>> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        let mut regions = split_all_ranges(source, ranges)?;
        let mut contexts = Vec::with_capacity(regions.len());
        let mut stores_per_region = Vec::with_capacity(regions.len());
        let mut used_ids = HashSet::new();
        let mut missing = Vec::new();
        let mut minimum_replica_count = usize::MAX;
        for region in &regions {
            let Some(context) = source.rpc_context(region.Region, options.is_mpp)? else {
                missing.push(region.Region);
                continue;
            };
            let primary_id = context
                .store
                .as_ref()
                .map(|store| store.id)
                .unwrap_or_default();
            let stores = source.all_valid_store_ids(region.Region, primary_id);
            minimum_replica_count = minimum_replica_count.min(stores.len());
            used_ids.extend(stores.iter().copied());
            contexts.push(context);
            stores_per_region.push(stores);
        }
        if !missing.is_empty() || contexts.len() != regions.len() {
            for region in missing {
                source.invalidate_region(region);
            }
            let error =
                BatchError::NoAliveStore("cannot find region with a TiFlash peer".to_owned());
            if attempts > options.max_retries {
                return Err(error);
            }
            backoffer.backoff(&error)?;
            continue;
        }

        let mut policy = options.replica_read_policy;
        let tidb_zone = options.tidb_zone.as_deref().unwrap_or_default();
        if options.tidb_zone.is_none() {
            policy = ReplicaReadPolicy::AllReplicas;
        }
        let alive = get_alive_stores_and_store_ids(
            &source.all_tiflash_stores(),
            &used_ids,
            options.ttl,
            policy,
            tidb_zone,
            |address, ttl| source.is_store_alive(address, ttl),
        );
        let maximum_remote = alive.store_ids_in_tidb_zone.len()
            * MAX_REMOTE_READ_COUNT_PER_NODE_FOR_CLOSEST_REPLICAS;
        let ids: Vec<_> = regions.iter().map(|region| region.Region).collect();
        let (retry, invalid) = check_alive_store(
            &alive,
            &stores_per_region,
            &used_ids,
            policy,
            &ids,
            minimum_replica_count,
            maximum_remote,
            options.in_test,
        );
        if retry {
            for region in invalid {
                source.invalidate_region(region);
            }
            let error =
                BatchError::NoAliveStore("cannot find region with a TiFlash peer".to_owned());
            if attempts > options.max_retries {
                return Err(error);
            }
            backoffer.backoff(&error)?;
            continue;
        }

        let mut cross_zone_ids = Vec::new();
        let mut reload = Vec::new();
        for (index, region) in regions.iter_mut().enumerate() {
            region.AllStores = stores_per_region[index].clone();
            *region = filter_accessible_stores_and_build_region_info(
                region.clone(),
                &alive,
                policy,
                &mut reload,
                &mut cross_zone_ids,
                maximum_remote,
                tidb_zone,
            )?;
        }
        if !cross_zone_ids.is_empty() {
            let examples = cross_zone_ids
                .iter()
                .take(3)
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            append_warning(BatchError::OtherResponse(format!(
                "total {} region(s) cannot be accessed by TiFlash in zone [{tidb_zone}]: {examples}, etc",
                cross_zone_ids.len()
            )));
        }

        let mut by_address = HashMap::<String, batchCopTask>::new();
        for (region, context) in regions.iter().cloned().zip(contexts) {
            by_address
                .entry(context.address.clone())
                .or_insert_with(|| batchCopTask {
                    storeAddr: context.address.clone(),
                    cmdType: CommandType::BatchCop,
                    ctx: context,
                    ..batchCopTask::default()
                })
                .regionInfos
                .push(region);
        }
        let original: Vec<_> = by_address.into_values().collect();
        let relevant_stores: Vec<_> = alive
            .stores_in_all_zones
            .iter()
            .filter(|store| {
                regions
                    .iter()
                    .any(|region| region.AllStores.contains(&store.id))
            })
            .cloned()
            .collect();
        return Ok(balance_batch_cop_task(
            &relevant_stores,
            original,
            options.balance_with_continuity,
            options.balance_continuous_region_count,
            regions,
        ));
    }
}

/// 为非分区表构建 Batch Cop 任务列表。
pub fn build_batch_cop_tasks_for_non_partitioned_table(
    source: &dyn BatchTaskSource,
    ranges: KeyRanges,
    options: &BatchBuildOptions,
    backoffer: &mut Backoffer,
    append_warning: &mut dyn FnMut(BatchError),
) -> BatchResult<Vec<batchCopTask>> {
    if options.disaggregated_tiflash {
        build_disaggregated_tasks(source, &[ranges], options, backoffer)
    } else {
        build_integrated_tasks(source, &[ranges], options, backoffer, append_warning)
    }
}

/// 为分区表构建 Batch Cop 任务（按物理表归并 Region）。
pub fn build_batch_cop_tasks_for_partitioned_table(
    source: &dyn BatchTaskSource,
    ranges: Vec<KeyRanges>,
    partition_ids: &[i64],
    options: &BatchBuildOptions,
    backoffer: &mut Backoffer,
    append_warning: &mut dyn FnMut(BatchError),
) -> BatchResult<Vec<batchCopTask>> {
    if ranges.len() != partition_ids.len() {
        return Err(BatchError::OtherResponse(
            "partition IDs and range groups have different lengths".to_owned(),
        ));
    }
    let mut tasks = if options.disaggregated_tiflash {
        build_disaggregated_tasks(source, &ranges, options, backoffer)?
    } else {
        build_integrated_tasks(source, &ranges, options, backoffer, append_warning)?
    };
    convert_region_infos_to_partition_table_regions(&mut tasks, partition_ids)?;
    Ok(tasks)
}

/// 重试前合并任务内的 key ranges（可选带分区表 id）。
pub fn merge_task_ranges_for_retry(task: &batchCopTask) -> Vec<(Option<i64>, KeyRanges)> {
    if !task.regionInfos.is_empty() {
        let ranges = task
            .regionInfos
            .iter()
            .flat_map(|region| region.Ranges.iter().cloned())
            .collect::<Vec<KeyRange>>();
        return vec![(None, KeyRanges::new(ranges).into_sorted())];
    }
    task.PartitionTableRegions
        .iter()
        .map(|table| {
            let ranges = table
                .regions
                .iter()
                .flat_map(|region| region.ranges.iter().cloned())
                .collect();
            (
                Some(table.physical_table_id),
                KeyRanges::new(ranges).into_sorted(),
            )
        })
        .collect()
}

/// 单个 Batch 任务执行结果（响应或错误相关字段）。
#[derive(Clone, Debug, Default)]
pub struct TaskRunResult {
    pub responses: Vec<BatchResponse>,
    pub retry_tasks: Vec<batchCopTask>,
    pub stats: CopRuntimeStats,
}

/// 执行单个 batchCopTask 的运行器抽象。
pub trait BatchTaskRunner: Send + Sync + 'static {
    fn run_task(&self, task: &batchCopTask) -> BatchResult<TaskRunResult>;
}

// 向迭代器通道投递一条响应或结束信号。
fn send_iterator_response(
    sender: &mpsc::SyncSender<batchCopResponse>,
    finish: &AtomicBool,
    mut response: batchCopResponse,
) -> bool {
    loop {
        if finish.load(Ordering::Acquire) {
            return false;
        }
        match sender.try_send(response) {
            Ok(()) => return true,
            Err(mpsc::TrySendError::Full(value)) => {
                response = value;
                thread::yield_now();
            }
            Err(mpsc::TrySendError::Disconnected(_)) => return false,
        }
    }
}

/// Parallel response iterator. Every initial task gets one worker and retries are
/// appended to that worker's queue, matching the dynamic Go `for idx < len(tasks)` loop.
/// 并行响应迭代器：每个初始任务一个 worker，重试任务再拉起 worker。
#[allow(non_camel_case_types)]
pub struct batchCopIterator {
    tasks: Vec<batchCopTask>,
    runner: Arc<dyn BatchTaskRunner>,
    response_sender: Option<mpsc::SyncSender<batchCopResponse>>,
    response_receiver: mpsc::Receiver<batchCopResponse>,
    finish: Arc<AtomicBool>,
    killed: Arc<AtomicU32>,
    workers: Vec<JoinHandle<()>>,
    started: bool,
}

impl batchCopIterator {
    pub fn new(tasks: Vec<batchCopTask>, runner: Arc<dyn BatchTaskRunner>) -> Self {
        let (sender, receiver) = mpsc::sync_channel(2_048);
        Self {
            tasks,
            runner,
            response_sender: Some(sender),
            response_receiver: receiver,
            finish: Arc::new(AtomicBool::new(false)),
            killed: Arc::new(AtomicU32::new(0)),
            workers: Vec::new(),
            started: false,
        }
    }

    pub fn killed_signal(&self) -> Arc<AtomicU32> {
        Arc::clone(&self.killed)
    }

    /// 为每个初始任务启动 worker 线程并开始拉取响应。
    pub fn run(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        let sender = self
            .response_sender
            .as_ref()
            .expect("sender exists before run")
            .clone();
        for task in std::mem::take(&mut self.tasks) {
            let sender = sender.clone();
            let runner = Arc::clone(&self.runner);
            let finish = Arc::clone(&self.finish);
            self.workers.push(thread::spawn(move || {
                let mut tasks = VecDeque::from([task]);
                while let Some(task) = tasks.pop_front() {
                    if finish.load(Ordering::Acquire) {
                        break;
                    }
                    match runner.run_task(&task) {
                        Ok(result) => {
                            tasks.extend(result.retry_tasks);
                            for response in result.responses {
                                if !response.retry_regions.is_empty() {
                                    continue;
                                }
                                let value = batchCopResponse {
                                    pb_resp: Some(response),
                                    detail: Some(result.stats.clone()),
                                    ..batchCopResponse::default()
                                };
                                if !send_iterator_response(&sender, &finish, value) {
                                    return;
                                }
                            }
                        }
                        Err(error) => {
                            let _ = send_iterator_response(
                                &sender,
                                &finish,
                                batchCopResponse {
                                    err: Some(error),
                                    detail: Some(CopRuntimeStats::default()),
                                    ..batchCopResponse::default()
                                },
                            );
                            break;
                        }
                    }
                }
            }));
        }
        self.response_sender.take();
    }

    /// 阻塞取下一条响应；通道关闭则返回 None。
    pub fn next(&mut self) -> BatchResult<Option<batchCopResponse>> {
        if !self.started {
            self.run();
        }
        loop {
            if self.finish.load(Ordering::Acquire) {
                return Ok(None);
            }
            if self.killed.load(Ordering::Acquire) != 0 {
                return Err(BatchError::QueryInterrupted);
            }
            match self.response_receiver.recv_timeout(Duration::from_secs(3)) {
                Ok(response) => {
                    if let Some(error) = response.err.clone() {
                        return Err(error);
                    }
                    return Ok(Some(response));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(None),
            }
        }
    }

    /// 标记关闭并等待 worker 结束。
    pub fn close(&mut self) {
        if !self.finish.swap(true, Ordering::AcqRel) {
            self.response_sender.take();
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

impl Drop for batchCopIterator {
    fn drop(&mut self) {
        self.close();
    }
}

/// `batchCopIterator` 别名。
pub type BatchCopIterator = batchCopIterator;

/// 将单次 BatchResponse 转为迭代器可用的 batchCopResponse。
pub fn handle_batch_cop_response(response: BatchResponse) -> BatchResult<Option<batchCopResponse>> {
    if !response.other_error.is_empty() {
        return Err(BatchError::OtherResponse(response.other_error));
    }
    if !response.retry_regions.is_empty() {
        return Ok(None);
    }
    Ok(Some(batchCopResponse {
        pb_resp: Some(response),
        detail: Some(CopRuntimeStats::default()),
        ..batchCopResponse::default()
    }))
}

/// 处理流式 Batch Cop 响应序列。
pub fn handle_streamed_batch_cop_response(
    responses: impl IntoIterator<Item = BatchResult<BatchResponse>>,
    backoffer: &mut Backoffer,
) -> BatchResult<Vec<batchCopResponse>> {
    let mut result = Vec::new();
    for response in responses {
        match response {
            Ok(response) => {
                if let Some(response) = handle_batch_cop_response(response)? {
                    result.push(response);
                }
            }
            Err(BatchError::Cancelled) => return Err(BatchError::ServerTimeout),
            Err(error) => {
                backoffer.backoff(&error)?;
                return Err(BatchError::ServerTimeout);
            }
        }
    }
    Ok(result)
}

/// Go 风格导出名：连续性均衡。
#[allow(non_snake_case)]
pub fn balanceBatchCopTaskWithContinuity(
    store_task_map: &HashMap<u64, batchCopTask>,
    candidate_region_infos: &[RegionInfo],
    balance_continuous_region_count: usize,
) -> (Option<Vec<batchCopTask>>, i32) {
    balance_batch_cop_task_with_continuity(
        store_task_map,
        candidate_region_infos,
        balance_continuous_region_count,
    )
}

/// Go 风格导出名：两阶段均衡。
#[allow(non_snake_case)]
pub fn balanceBatchCopTask(
    alive_stores: &[Store],
    original_tasks: Vec<batchCopTask>,
    balance_with_continuity: bool,
    balance_continuous_region_count: usize,
    all_region_infos: Vec<RegionInfo>,
) -> Vec<batchCopTask> {
    balance_batch_cop_task(
        alive_stores,
        original_tasks,
        balance_with_continuity,
        balance_continuous_region_count,
        all_region_infos,
    )
}

/// Go 风格常量别名。
#[allow(non_upper_case_globals)]
pub const fetchTopoMaxBackoff: usize = FETCH_TOPO_MAX_BACKOFF;
/// Go 风格常量别名。
#[allow(non_upper_case_globals)]
pub const maxBalanceScore: i32 = MAX_BALANCE_SCORE;
/// Go 风格常量别名。
#[allow(non_upper_case_globals)]
pub const balanceScoreThreshold: i32 = BALANCE_SCORE_THRESHOLD;
/// Go 风格常量别名。
#[allow(non_upper_case_globals)]
pub const TiFlashReadTimeoutUltraLong: Duration = TI_FLASH_READ_TIMEOUT_ULTRA_LONG;

/// Go 风格函数别名：计算均衡分数。
#[allow(non_snake_case)]
pub fn balanceScore(maximum: usize, minimum: usize, continuous: usize) -> i32 {
    balance_score(maximum, minimum, continuous)
}

/// Go 风格函数别名：是否已均衡。
#[allow(non_snake_case)]
pub fn isBalance(score: i32) -> bool {
    is_balance(score)
}

/// Go 风格函数别名：RegionInfo → TableRegions。
#[allow(non_snake_case)]
pub fn convertRegionInfosToPartitionTableRegions(
    tasks: &mut [batchCopTask],
    partition_ids: &[i64],
) -> BatchResult<()> {
    convert_region_infos_to_partition_table_regions(tasks, partition_ids)
}

#[allow(dead_code)]
fn _assert_region_wire_type(_: CoprocessorRegionInfo) {}
