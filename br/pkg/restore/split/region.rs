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

//! RegionInfo and interior-key helpers, matching `region.go`.
//! Region 元数据包装与“内部键”判定，供 split/scatter 决策使用。

use crate::stubs::metapb;

/// RegionInfo includes a region and the leader of the region.
/// 聚合 Region、Leader 及 pending/down peers，便于日志与重试判断。
#[derive(Clone, Debug, Default)]
pub struct RegionInfo {
    pub Region: Option<metapb::Region>,
    pub Leader: Option<metapb::Peer>,
    pub PendingPeers: Vec<metapb::Peer>,
    pub DownPeers: Vec<metapb::Peer>,
}

impl RegionInfo {
    /// ContainsInterior returns whether the region contains the given key, and also
    /// that the key does not fall on the boundary (start key) of the region.
    /// 内部键：严格大于 start，且在 end 之前（空 end 表示无上界）。
    /// 边界键留给相邻 region，避免在 start 上重复分裂。
    pub fn ContainsInterior(&self, key: &[u8]) -> bool {
        let (start, end): (&[u8], &[u8]) = match &self.Region {
            Some(region) => (region.GetStartKey(), region.GetEndKey()),
            // Go protobuf getters return empty byte slices for a nil *Region.
            None => (&[], &[]),
        };
        key > start && beforeEnd(key, end)
    }

    /// ToZapFields returns a display string for the RegionInfo (nil-safe via Option).
    /// 空 Region 返回空串，避免日志侧解引用 panic。
    pub fn ToZapFields(region: Option<&RegionInfo>) -> String {
        match region.and_then(|r| r.Region.as_ref()) {
            Some(r) => crate::stubs::logutil::Region(r),
            None => String::new(),
        }
    }
}

/// beforeEnd: empty end means unbounded.
/// 空 end 视为正无穷，与 PD region 约定一致。
pub fn beforeEnd(key: &[u8], end: &[u8]) -> bool {
    key < end || end.is_empty()
}
