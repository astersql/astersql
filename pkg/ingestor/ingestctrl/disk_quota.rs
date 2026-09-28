// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 本地 ingest Engine 磁盘配额检查。
//
// 汇总各引擎的磁盘与内存占用，在超过 `quota` 时挑选可淘汰的大型引擎；
// 正在导入（IsImporting）的引擎优先级最低，只计入 `inProgressLargeEngines`。

use crate::{EngineFileSize, EngineId};

/// 提供当前所有引擎文件大小快照的抽象。
pub trait DiskUsage {
    /// 返回各引擎的磁盘/内存占用与导入状态。
    fn EngineFileSizes(&self) -> Vec<EngineFileSize>;
}

/// 磁盘配额检查结果：超额引擎列表、进行中超额数与总占用。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DiskQuotaResult {
    /// 已超额且非导入中的引擎 ID，可供调用方关闭或刷盘。
    pub largeEngines: Vec<EngineId>,
    /// 已超额但仍在导入中的引擎数量（不宜立即淘汰）。
    pub inProgressLargeEngines: usize,
    /// 所有引擎磁盘占用之和。
    pub totalDiskSize: i64,
    /// 所有引擎内存占用之和。
    pub totalMemSize: i64,
}

/// 按占用从小到大累加，超出 `quota` 的引擎记入结果。
///
/// 排序优先把非 importing、占用更大的引擎排到后面，这样累加时后触达的
/// 大引擎更容易成为淘汰候选；importing 引擎触达超额时只增加计数。
pub fn CheckDiskQuota(manager: &dyn DiskUsage, quota: i64) -> DiskQuotaResult {
    let mut sizes = manager.EngineFileSizes();
    // 非导入优先于导入；同组内按 Disk+Mem 升序，使大引擎后被计入超额集合
    sizes.sort_by(|left, right| {
        right.IsImporting.cmp(&left.IsImporting).then_with(|| {
            left.DiskSize
                .wrapping_add(left.MemSize)
                .cmp(&right.DiskSize.wrapping_add(right.MemSize))
        })
    });
    let mut result = DiskQuotaResult::default();
    for size in sizes {
        result.totalDiskSize = result.totalDiskSize.wrapping_add(size.DiskSize);
        result.totalMemSize = result.totalMemSize.wrapping_add(size.MemSize);
        if result.totalDiskSize.wrapping_add(result.totalMemSize) > quota {
            if size.IsImporting {
                result.inProgressLargeEngines += 1;
            } else {
                result.largeEngines.push(size.UUID);
            }
        }
    }
    result
}
