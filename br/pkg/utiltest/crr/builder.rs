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

//! Region 布局构建器：以选项链拼出连续、可关闭的假集群边界。
//! 对应 Go `br/pkg/utiltest/crr/builder.go`，供 CRR 单测构造拓扑。
//! 约束：首 region 起键为空、末 region 止键为空、相邻边界首尾相接。
//! store id 不可为 0；已关闭布局不可再追加。
//! 选项失败时整条链短路，不留下半截无效布局。
//! 选项失败立即短路；成功路径返回深拷贝，避免测试间共享可变状态。

use crate::types::RegionBoundary;

/// RegionLayoutOption appends or transforms boundaries while preserving a valid layout.
/// 选项闭包：输入当前边界列表，返回变换后列表或错误。
pub type RegionLayoutOption =
    Box<dyn Fn(Vec<RegionBoundary>) -> Result<Vec<RegionBoundary>, String> + Send + Sync>;

/// BuildRegionLayout applies options in order and returns a validated region layout.
/// 顺序应用选项后校验：非空且最后一条 EndKey 为空（布局已闭合）。
pub fn BuildRegionLayout(opts: Vec<RegionLayoutOption>) -> Result<Vec<RegionBoundary>, String> {
    let mut boundaries = Vec::new();
    // 按调用方给定顺序折叠选项，中间态可尚未闭合。
    for opt in opts {
        boundaries = opt(boundaries)?;
    }
    if boundaries.is_empty() {
        return Err("region layout must contain at least one region".into());
    }
    // 空 EndKey 表示覆盖到正无穷，与 Go 假 PD 约定一致。
    if !boundaries[boundaries.len() - 1].EndKey.is_empty() {
        return Err("last region must end with empty key".into());
    }
    Ok(cloneRegionBoundaries(&boundaries))
}

/// StoreIDRange returns continuous store IDs: [start, start+count).
/// count<=0 返回空向量（对齐 Go nil/空切片语义）。
pub fn StoreIDRange(start: u64, count: i32) -> Vec<u64> {
    if count <= 0 {
        return Vec::new();
    }
    let mut ids = Vec::with_capacity(count as usize);
    // 生成连续 store id，便于轮询分配 leader。
    for i in 0..count as u64 {
        // Go's uint64 arithmetic wraps modulo 2^64; make that explicit in debug builds too.
        ids.push(start.wrapping_add(i));
    }
    ids
}

/// AddRegion appends one continuous region to the current layout.
/// 追加单段 [startKey, endKey)，leader 落在 storeID。
pub fn AddRegion(startKey: &str, endKey: &str, storeID: u64) -> RegionLayoutOption {
    let startKey = startKey.to_string();
    let endKey = endKey.to_string();
    Box::new(move |boundaries| {
        appendRegion(boundaries, startKey.as_bytes(), endKey.as_bytes(), storeID)
    })
}

/// AddRegionsBySplitKeys appends continuous regions split by splitKeys.
/// It auto-assigns store IDs in round-robin order and closes with an empty end key.
/// 若布局未空，从末段 EndKey 继续；已闭合（EndKey 空）则报错。
pub fn AddRegionsBySplitKeys(splitKeys: Vec<String>, storeIDs: Vec<u64>) -> RegionLayoutOption {
    Box::new(move |boundaries| {
        if storeIDs.is_empty() {
            // 无 store 无法放置 leader，与 Go 校验一致。
            return Err("store ids must not be empty".into());
        }

        let mut startKey = Vec::new();
        let mut storeIndex = 0usize;
        let mut result = cloneRegionBoundaries(&boundaries);
        if !result.is_empty() {
            let last = &result[result.len() - 1];
            if last.EndKey.is_empty() {
                // 已覆盖到 +∞，无法再切分追加。
                return Err("cannot append regions after layout is already closed".into());
            }
            // Go strings retain arbitrary bytes, so keep the boundary in byte form instead of
            // passing it through a lossy UTF-8 conversion.
            startKey = last.EndKey.clone();
            // 续接时 store 轮询起点按已有 region 数取模。
            storeIndex = result.len() % storeIDs.len();
        }

        for splitKey in &splitKeys {
            result = appendRegion(
                result,
                &startKey,
                splitKey.as_bytes(),
                storeIDs[storeIndex % storeIDs.len()],
            )?;
            startKey = splitKey.as_bytes().to_vec();
            storeIndex += 1;
        }
        // 最后一段以空 EndKey 闭合整条键空间。
        appendRegion(
            result,
            &startKey,
            b"",
            storeIDs[storeIndex % storeIDs.len()],
        )
    })
}

/// AddRoundRobinRegions appends regionCount continuous regions with generated split keys.
/// Generated keys are k01, k02 ...; stores are assigned round-robin.
/// 必须从空布局开始；regionCount==1 时直接 [,) 单 region。
pub fn AddRoundRobinRegions(regionCount: i32, storeIDs: Vec<u64>) -> RegionLayoutOption {
    Box::new(move |boundaries| {
        if regionCount <= 0 {
            return Err("region count must be greater than zero".into());
        }
        if storeIDs.is_empty() {
            return Err("store ids must not be empty".into());
        }
        if !boundaries.is_empty() {
            return Err("AddRoundRobinRegions must start from an empty layout".into());
        }
        if regionCount == 1 {
            // 单 region 覆盖全键空间，无需生成中间 split。
            return appendRegion(Vec::new(), b"", b"", storeIDs[0]);
        }

        // 分割键宽度至少 2，保证 k01 风格与 Go 测试向量一致。
        let mut width = format!("{}", regionCount - 1).len();
        if width < 2 {
            width = 2;
        }
        let mut splitKeys = Vec::with_capacity((regionCount - 1) as usize);
        for i in 1..regionCount {
            splitKeys.push(format!("k{i:0width$}"));
        }
        AddRegionsBySplitKeys(splitKeys, storeIDs.clone())(boundaries)
    })
}

// 追加一段并校验连续性、store id、区间有序；成功返回深拷贝后的新列表。
fn appendRegion(
    boundaries: Vec<RegionBoundary>,
    startKey: &[u8],
    endKey: &[u8],
    storeID: u64,
) -> Result<Vec<RegionBoundary>, String> {
    if storeID == 0 {
        // 0 在 PD/测试中常表示无效 store，直接拒绝。
        return Err("store id must not be zero".into());
    }

    let start = startKey.to_vec();
    let end = endKey.to_vec();
    if boundaries.is_empty() {
        // 首段必须从 -∞（空 start）开始，避免键空间空洞。
        if !start.is_empty() {
            return Err("first region must start from empty key".into());
        }
    } else {
        let prev = &boundaries[boundaries.len() - 1];
        // 字节级相等才算连续，避免 UTF-8 显示差异掩盖断档。
        if prev.EndKey != start {
            return Err(format!(
                "region boundary is not continuous: prev end {:?}, next start {:?}",
                String::from_utf8_lossy(&prev.EndKey),
                String::from_utf8_lossy(&start)
            ));
        }
    }
    // 非空 end 时要求 start < end（字典序）。
    if !end.is_empty() && start.as_slice() >= end.as_slice() {
        return Err(format!(
            "invalid region range [{:?}, {:?})",
            String::from_utf8_lossy(&start),
            String::from_utf8_lossy(&end)
        ));
    }

    let mut result = cloneRegionBoundaries(&boundaries);
    result.push(RegionBoundary {
        StartKey: start,
        EndKey: end,
        StoreID: storeID,
    });
    Ok(result)
}

// 深拷贝边界切片，避免选项链共享可变缓冲。
// 深拷贝边界避免调用方持有旧切片别名。
fn cloneRegionBoundaries(boundaries: &[RegionBoundary]) -> Vec<RegionBoundary> {
    boundaries
        .iter()
        .map(|b| RegionBoundary {
            StartKey: b.StartKey.clone(),
            EndKey: b.EndKey.clone(),
            StoreID: b.StoreID,
        })
        .collect()
}
