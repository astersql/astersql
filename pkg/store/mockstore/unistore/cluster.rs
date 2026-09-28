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

// unistore 模拟集群元数据与一次性 RPC 延迟调度。
//
// 对应 Go 侧 Cluster：Region（键空间分片）元数据变更委托给 `MockRegionManager`；
// 另支持按 `(start_ts, region_id)` 注册一次性延迟，用于确定性编排事务测试时序。

use crate::tikv::mock_region::{MockRegionManager, Peer, Region, RegionEpoch, RegionError, Store};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// 延迟事件查找键：事务开始时间戳与 Region ID 的组合。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct DelayKey {
    /// 事务 start_ts（MVCC 读取快照时间戳）。
    start_ts: u64,
    /// Region 标识。
    region_id: u64,
}

/// Cluster delegates metadata mutation to the canonical MockRegionManager and
/// adds one-shot RPC delays used to deterministically order transaction tests.
///
/// 模拟集群：委托 MockRegionManager 管理 Region/Store，并提供一次性 RPC 延迟。
pub struct Cluster {
    /// Region/Peer/Store 元数据管理器。
    region_manager: Arc<MockRegionManager>,
    /// 按 DelayKey 注册的一次性延迟，消费后即移除。
    delay_events: Mutex<HashMap<DelayKey, Duration>>,
}

impl Cluster {
    /// 用给定的 Region 管理器构造集群包装。
    pub fn new(region_manager: Arc<MockRegionManager>) -> Self {
        Self {
            region_manager,
            delay_events: Mutex::new(HashMap::new()),
        }
    }

    /// 返回内部 MockRegionManager 的克隆引用。
    pub fn region_manager(&self) -> Arc<MockRegionManager> {
        Arc::clone(&self.region_manager)
    }

    /// 为指定 `(start_ts, region_id)` 注册一次性 sleep 延迟。
    pub fn schedule_delay(&self, start_ts: u64, region_id: u64, duration: Duration) {
        self.delay_events
            .lock()
            .expect("delay-event lock poisoned")
            .insert(
                DelayKey {
                    start_ts,
                    region_id,
                },
                duration,
            );
    }

    /// 若存在匹配的延迟事件则取出并 sleep；不存在则立即返回。
    pub fn handle_delay(&self, start_ts: u64, region_id: u64) {
        // 取出即消费，保证同一键只延迟一次。
        let duration = self
            .delay_events
            .lock()
            .expect("delay-event lock poisoned")
            .remove(&DelayKey {
                start_ts,
                region_id,
            });
        if let Some(duration) = duration {
            thread::sleep(duration);
        }
    }

    /// 按 Go `SplitRaw` 契约编码原始键后拆分 Region。
    pub fn split_raw(
        &self,
        region_id: u64,
        new_region_id: u64,
        raw_key: Vec<u8>,
        peer_ids: &[u64],
        _leader_peer_id: u64,
    ) -> Result<Region, RegionError> {
        self.region_manager
            .split(region_id, new_region_id, encode_bytes(&raw_key), peer_ids)
    }

    /// 对用户键做 `encode_bytes` 后再拆分 Region。
    pub fn split(
        &self,
        region_id: u64,
        new_region_id: u64,
        key: &[u8],
        peer_ids: &[u64],
        _leader_peer_id: u64,
    ) -> Result<Region, RegionError> {
        self.region_manager
            .split(region_id, new_region_id, encode_bytes(key), peer_ids)
    }

    /// Mirrors MockRegionManager.SplitKeys once the caller supplies the keys in
    /// the selected range. The quotient/remainder distribution is identical to
    /// Go and produces `count` regions whenever enough keys exist.
    ///
    /// 在 `[start, end)` 内按商/余均匀选取切分点，尽量生成 `count` 个 Region。
    pub fn split_keys(
        &self,
        start: &[u8],
        end: &[u8],
        count: usize,
        data_keys: &[Vec<u8>],
    ) -> Result<Vec<Region>, RegionError> {
        if count <= 1 {
            return Ok(Vec::new());
        }
        // 过滤落在目标范围内的数据键，再按商余分配切分位置。
        let mut keys = data_keys
            .iter()
            .filter(|key| key.as_slice() >= start && (end.is_empty() || key.as_slice() < end))
            .cloned()
            .collect::<Vec<_>>();
        // Go collects these keys through an ordered Badger iterator. Keep that
        // storage-order contract even when the Rust caller supplies an
        // unordered snapshot of the data keys.
        keys.sort();
        let quotient = keys.len() / count;
        let mut remainder = keys.len() % count;
        let mut index = 0;
        let mut splits = Vec::new();
        while index < keys.len() {
            let mut entries = quotient;
            if remainder > 0 {
                remainder -= 1;
                entries += 1;
            }
            index += entries;
            if index < keys.len() {
                splits.push(encode_bytes(&keys[index]));
            }
        }
        self.region_manager.split_keys(splits)
    }

    /// Go `Cluster.Close` is a no-op for the in-process mock.
    ///
    /// 进程内 mock 的 Close 为空操作。
    pub fn close(&self) {}
}

/// 工厂：由 Region 管理器创建共享的 Cluster。
pub fn newCluster(region_manager: Arc<MockRegionManager>) -> Arc<Cluster> {
    Arc::new(Cluster::new(region_manager))
}

/// 用单 Store、单 Peer、单 Region 引导集群，返回 `(store_id, peer_id, region_id)`。
pub fn BootstrapWithSingleStore(cluster: &Cluster) -> Result<(u64, u64, u64), RegionError> {
    let manager = cluster.region_manager();
    let store_id = manager.alloc_id();
    let region_id = manager.alloc_id();
    let peer_id = manager.alloc_id();
    manager.bootstrap(
        vec![Store {
            id: store_id,
            address: format!("store{store_id}"),
            labels: Vec::new(),
        }],
        Region {
            id: region_id,
            epoch: RegionEpoch {
                conf_ver: 1,
                version: 1,
            },
            peers: vec![Peer {
                id: peer_id,
                store_id,
            }],
            ..Region::default()
        },
    )?;
    Ok((store_id, peer_id, region_id))
}

/// 用多个 Store 引导：同一 Region 上挂多个 Peer，返回 store/peer 列表与首个 leader peer。
pub fn BootstrapWithMultiStores(
    cluster: &Cluster,
    count: usize,
) -> Result<(Vec<u64>, Vec<u64>, u64, u64), RegionError> {
    if count == 0 {
        return Err(RegionError::NotBootstrapped);
    }
    let manager = cluster.region_manager();
    let store_ids = manager.alloc_ids(count);
    let peer_ids = manager.alloc_ids(count);
    let region_id = manager.alloc_id();
    let stores = store_ids
        .iter()
        .map(|id| Store {
            id: *id,
            address: format!("store{id}"),
            labels: Vec::new(),
        })
        .collect();
    let peers = peer_ids
        .iter()
        .zip(&store_ids)
        .map(|(peer, store)| Peer {
            id: *peer,
            store_id: *store,
        })
        .collect();
    manager.bootstrap(
        stores,
        Region {
            id: region_id,
            epoch: RegionEpoch {
                conf_ver: 1,
                version: 1,
            },
            peers,
            ..Region::default()
        },
    )?;
    Ok((store_ids, peer_ids.clone(), region_id, peer_ids[0]))
}

/// 先单 Store 引导，再按 `split_keys` 依次拆分出多个 Region。
pub fn BootstrapWithMultiRegions(
    cluster: &Cluster,
    split_keys: &[Vec<u8>],
) -> Result<(u64, Vec<u64>, Vec<u64>), RegionError> {
    let (store_id, first_peer_id, first_region_id) = BootstrapWithSingleStore(cluster)?;
    let manager = cluster.region_manager();
    let mut region_ids = vec![first_region_id];
    region_ids.extend(manager.alloc_ids(split_keys.len()));
    let mut peer_ids = vec![first_peer_id];
    peer_ids.extend(manager.alloc_ids(split_keys.len()));
    // 自左向右按切分键依次 split，保持与 Go 相同的 Region/Peer 分配顺序。
    for (index, key) in split_keys.iter().enumerate() {
        cluster.split(
            region_ids[index],
            region_ids[index + 1],
            key,
            &[peer_ids[index]],
            peer_ids[index],
        )?;
    }
    Ok((store_id, region_ids, peer_ids))
}

/// TiDB's codec.EncodeBytes: groups of eight bytes followed by a marker whose
/// distance from 0xff is the zero padding count.
///
/// TiDB `codec.EncodeBytes`：每 8 字节一组，末尾标记字节为 `0xff - 填充零个数`。
pub fn encode_bytes(value: &[u8]) -> Vec<u8> {
    const GROUP: usize = 8;
    let mut result = Vec::with_capacity((value.len() / GROUP + 1) * (GROUP + 1));
    for chunk in value.chunks(GROUP) {
        result.extend_from_slice(chunk);
        let padding = GROUP - chunk.len();
        result.resize(result.len() + padding, 0);
        result.push(0xff - padding as u8);
        // 不足一整组时已是最后一块，补零后即可返回。
        if padding > 0 {
            return result;
        }
    }
    // 恰好整组对齐时再追加一组全零与对应标记。
    result.extend_from_slice(&[0; GROUP]);
    result.push(0xff - GROUP as u8);
    result
}
