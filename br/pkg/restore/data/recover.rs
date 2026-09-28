// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Recovery plan helpers (from `br/pkg/restore/data/recover.go`).
//!
//! 模块职责：从各 store 收集的 Region peer 中选出恢复代表、校验键空间连续性，
//! 并按 store 负载分数选定 leader，供上层 `Recovery` 生成下发计划。
//! 对应 Go `br/pkg/restore/data/recover.go`；键比较依赖本包 `key` 前缀编码。
//! 约束：不发起网络 IO，纯内存排序与区间树裁决；tombstone peer 不得进入有效集。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Deref;

use crate::key::{PrefixEndKey, PrefixStartKey, keyCmp, keyEq};
use crate::stubs::berrors;
use crate::stubs::log;
use crate::stubs::recovpb::RegionMeta;
use crate::stubs::{Error, Result};

/// RecoverRegion embeds RegionMeta plus the peer's store id (Go embedding).
///
/// 对齐 Go 嵌入式结构：对外既可当 RegionMeta 读字段，又携带 StoreId 供选主。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoverRegion {
    pub RegionMeta: RegionMeta,
    pub StoreId: u64,
}

// Deref 到 RegionMeta，模拟 Go 嵌入字段提升，避免处处写 `.RegionMeta.`。
impl Deref for RecoverRegion {
    type Target = RegionMeta;
    fn deref(&self) -> &RegionMeta {
        &self.RegionMeta
    }
}

/// RecoverRegionInfo is the per-region candidate after peer sorting.
///
/// 每个 region 只保留一条代表信息；Start/End 已做 Prefix* 规范化，供一致性检查。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoverRegionInfo {
    pub RegionId: u64,
    pub RegionVersion: u64,
    pub StartKey: Vec<u8>,
    pub EndKey: Vec<u8>,
    pub TombStone: bool,
}

/// SortRecoverRegions sorts peers per region by log term → last index → commit
/// index (descending), then sorts region infos by version descending.
///
/// 先在 region 内按 raft 进度降序排 peer，取 first 为代表；再按 RegionVersion 降序
/// 输出。版本高的优先参与后续重叠消解（split/merge 场景）。
pub fn SortRecoverRegions(
    regions: &mut HashMap<u64, Vec<RecoverRegion>>,
) -> Vec<RecoverRegionInfo> {
    let mut regionInfos = Vec::with_capacity(regions.len());

    for (regionId, peers) in regions.iter_mut() {
        // term → last_index → commit_index 三级降序，与 Go sort.Slice 比较键一致。
        peers.sort_by(|a, b| {
            b.GetLastLogTerm()
                .cmp(&a.GetLastLogTerm())
                .then_with(|| b.GetLastIndex().cmp(&a.GetLastIndex()))
                .then_with(|| b.GetCommitIndex().cmp(&a.GetCommitIndex()))
        });

        // Go immediately indexes peers[0]: an empty peer list violates the
        // collected-region invariant and must not be silently omitted.
        let peer = &peers[0];
        // 代表 peer 的键写入前必须 Prefix*，否则与树图键空间不一致。
        regionInfos.push(RecoverRegionInfo {
            RegionId: *regionId,
            RegionVersion: peer.Version,
            StartKey: PrefixStartKey(&peer.StartKey),
            EndKey: PrefixEndKey(&peer.EndKey),
            TombStone: peer.Tombstone,
        });
    }

    // 全局按 epoch version 降序，高版本 region 在重叠时优先保留。
    regionInfos.sort_by(|a, b| b.RegionVersion.cmp(&a.RegionVersion));
    regionInfos
}

/// CheckConsistencyAndValidPeer resolves overlapping regions (split/merge during
/// backup) via an ordered map, then requires remaining ranges to be adjacent
/// and non-tombstone.
///
/// 用 BTreeMap 模拟 Go treemap：遇到与已选区间重叠的候选直接跳过（高版本已先插入）。
/// 最终要求剩余区间从空 Start 起首尾相接，且无一为 tombstone。
pub fn CheckConsistencyAndValidPeer(regionInfos: Vec<RecoverRegionInfo>) -> Result<HashSet<u64>> {
    let mut treeMap: BTreeMap<Vec<u8>, RecoverRegionInfo> = BTreeMap::new();

    for p in regionInfos {
        // Ceiling: least key >= StartKey.
        // 若 ceiling 落在本区间内（含同 StartKey），说明与已选区间重叠，丢弃当前。
        if let Some((fk, _)) = treeMap.range(p.StartKey.clone()..).next() {
            if keyEq(fk, &p.StartKey) || keyCmp(fk, &p.EndKey) < 0 {
                continue;
            }
        }

        // Floor: greatest key <= StartKey; overlap if floor.end > start.
        // floor 的 EndKey 越过本 StartKey 同样视为重叠，对齐 Go 区间树逻辑。
        if let Some((_, fv)) = treeMap.range(..=p.StartKey.clone()).next_back() {
            if keyCmp(&fv.EndKey, &p.StartKey) > 0 {
                continue;
            }
        }

        treeMap.insert(p.StartKey.clone(), p);
    }

    let mut validPeers = HashSet::new();
    let mut prevEndKey = PrefixStartKey(&[]);
    let mut prevRegion = 0_u64;
    for (key, v) in treeMap.iter() {
        // 有效恢复 peer 不得为 tombstone，否则后续 raft 恢复无意义。
        if v.TombStone {
            log::Error("validPeer shouldn't be tombstone");
            return Err(Error::Annotatef(
                berrors::ErrRestoreInvalidPeer(),
                "Peer shouldn't be tombstone",
            ));
        }

        // 前一 EndKey 必须等于当前 StartKey，缺口或乱序均报 invalid region range。
        if !keyEq(&prevEndKey, key) {
            log::Error("regions are not adjacent");
            return Err(Error::Annotatef(
                berrors::ErrInvalidRange(),
                "invalid region range",
            ));
        }

        prevEndKey = v.EndKey.clone();
        // prevRegion 保留与 Go 调试变量对称；当前仅推进游标与收集 ID。
        prevRegion = v.RegionId;
        let _ = prevRegion;
        validPeers.insert(v.RegionId);
    }
    Ok(validPeers)
}

/// LeaderCandidates selects peers that share the leader's raft log state.
///
/// 假定 `peers` 已按 SortRecoverRegions 排好序，peers[0] 为日志最前的 leader 基线；
/// 仅保留 term/index/commit 三者全等的候选，供负载均衡二次选择。
pub fn LeaderCandidates(peers: &[RecoverRegion]) -> Result<Vec<RecoverRegion>> {
    // 无副本 region 无法恢复，错误类型对齐 Go ErrRestoreRegionWithoutPeer。
    if peers.is_empty() {
        return Err(Error::Annotatef(
            berrors::ErrRestoreRegionWithoutPeer(),
            "invalid region range",
        ));
    }

    // peers[0] 已是 raft 进度最优者，作为候选集合的基线 leader。
    let leader = peers[0].clone();
    let mut candidates = Vec::with_capacity(peers.len());
    candidates.push(leader.clone());
    for peer in peers.iter().skip(1) {
        // 三者全等才可并列竞选，避免把落后副本选进 leader 候选。
        if peer.LastLogTerm == leader.LastLogTerm
            && peer.LastIndex == leader.LastIndex
            && peer.CommitIndex == leader.CommitIndex
        {
            log::Debug("leader candidate");
            candidates.push(peer.clone());
        }
    }
    Ok(candidates)
}

/// SelectRegionLeader picks the candidate on the store with the lowest balance score.
///
/// 在候选 peer 中选 StoreId 对应 balance 分数最低者，分散各 store 的 leader 数量；
/// 缺省分数按 0 处理。调用方需保证 `peers` 非空。
pub fn SelectRegionLeader(
    storeBalanceScore: &HashMap<u64, i32>,
    peers: &[RecoverRegion],
) -> RecoverRegion {
    // 初始取候选首个；随后仅在严格更低分数时替换，保持与 Go 相同的稳定偏好。
    let mut leader = peers[0].clone();
    let mut minLeaderStore = *storeBalanceScore.get(&leader.StoreId).unwrap_or(&0);

    for peer in peers.iter().skip(1) {
        let score = *storeBalanceScore.get(&peer.StoreId).unwrap_or(&0);
        log::Debug("leader candidate");
        // 分数相等不切换，避免无意义抖动。
        if score < minLeaderStore {
            minLeaderStore = score;
            leader = peer.clone();
        }
    }
    leader
}
