// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// Region 树的测试用简易实现。
//
// Region 是 TiKV 的数据分片单位（按 key 范围划分）。本模块模拟 PD（Placement Driver，
// 集群元数据与调度中心）中的 Region 树：维护若干 Region，支持按重叠替换与按 key
// 范围扫描，供单测对照 Go 侧行为。

use kvproto::metapb;

/// Region is a mock of PD's core.RegionInfo for tests.
/// 测试用 Region 信息：包含元数据 Meta 与可选的 Leader Peer。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Region {
    /// Region 的 metapb 元数据（起止 key、epoch、peers 等）。
    pub Meta: metapb::Region,
    /// 当前 Leader Peer；无 Leader 时为 None。
    pub Leader: Option<metapb::Peer>,
}

/// NewRegionInfo returns a new RegionInfo.
/// 由 meta 与 leader 构造一个新的 Region。
pub fn NewRegionInfo(meta: metapb::Region, leader: Option<metapb::Peer>) -> Region {
    Region {
        Meta: meta,
        Leader: leader,
    }
}

/// RegionTree is a simple mock of PD's region tree.
/// 简易 Region 树：用向量保存全部 Region，不维护真正的区间树结构。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RegionTree {
    /// 当前已登记的全部 Region。
    pub Regions: Vec<Region>,
}

impl RegionTree {
    /// SetRegion removes every old overlapping region and appends the new one.
    /// 写入 Region：先删除所有与之 key 范围重叠的旧 Region，再追加新 Region。
    pub fn SetRegion(&mut self, region: Region) {
        self.Regions.retain(|existing| !overlap(existing, &region));
        self.Regions.push(region);
    }

    /// ScanRange returns intersecting regions in start-key order.
    ///
    /// This preserves the Go implementation's actual `limit == 0` unlimited
    /// check; negative values therefore produce no rows.
    /// 按 start-key 排序后，返回与 `[startKey, endKey)` 相交的 Region。
    /// `limit == 0` 表示不限制条数；负数则不返回任何结果（对齐 Go 实现）。
    pub fn ScanRange(&mut self, startKey: Vec<u8>, endKey: Vec<u8>, limit: i32) -> Vec<Region> {
        // 先按 start_key 排序，保证扫描结果有序。
        self.Regions
            .sort_by(|left, right| left.Meta.start_key.cmp(&right.Meta.start_key));
        // 用临时 Region 作为查询区间的枢轴，复用 overlap 判断。
        let pivot = NewRegionInfo(
            metapb::Region {
                start_key: startKey,
                end_key: endKey,
                ..Default::default()
            },
            None,
        );
        self.Regions
            .iter()
            .filter(|region| {
                overlap(region, &pivot) && (limit == 0 || (limit > 0 && (limit as usize) > 0))
            })
            .take(if limit > 0 {
                limit as usize
            } else if limit == 0 {
                usize::MAX
            } else {
                0
            })
            .cloned()
            .collect()
    }
}

/// 判断两个 Region 的 key 范围是否重叠。
/// 空 end_key 表示正无穷（覆盖到 key 空间末尾）。
fn overlap(a: &Region, b: &Region) -> bool {
    // b 完全在 a 左侧：b.end <= a.start
    if !b.Meta.end_key.is_empty() && b.Meta.end_key <= a.Meta.start_key {
        return false;
    }
    // a 完全在 b 左侧：a.end <= b.start
    if !a.Meta.end_key.is_empty() && a.Meta.end_key <= b.Meta.start_key {
        return false;
    }
    true
}
