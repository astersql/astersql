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

// 物理算子基础工具的单元测试。
//
// 覆盖下推 Limit 克隆、运行时过滤（Runtime Filter）ID 生成、虚拟列谓词拆分，
// 以及空分区信息（PhysPlanPartInfo）的计划缓存克隆与内存占用。

use types::metadata::NameSlice;

use crate::{
    PhysPlanPartInfo, PushedDownLimit, RuntimeFilterIDGenerator, SplitSelCondsWithVirtualColumn,
    emptyPartitionInfoSize, pushedDownLimitSize,
};

/// 下推 Limit 的 Clone 应保留 Offset/Count，且 MemoryUsage 等于空结构常量。
#[test]
fn pushed_down_limit_clone_preserves_bounds() {
    let limit = PushedDownLimit {
        Offset: 3,
        Count: 17,
    };

    assert_eq!(*limit.Clone(), limit);
    assert_eq!(limit.MemoryUsage(), pushedDownLimitSize);
}

/// Runtime Filter ID 从给定起点单调递增，保证同一查询内 ID 唯一。
#[test]
fn runtime_filter_ids_are_monotonic() {
    let mut ids = RuntimeFilterIDGenerator::New(41);

    assert_eq!(ids.GetNextID(), 41);
    assert_eq!(ids.GetNextID(), 42);
}

/// 空选择条件下拆分结果两侧均应为空，行为保持稳定。
#[test]
fn empty_virtual_condition_split_is_stable() {
    let (ordinary, virtual_columns) = SplitSelCondsWithVirtualColumn(&[]);

    assert!(ordinary.is_empty());
    assert!(virtual_columns.is_empty());
}

/// 空分区信息克隆后各字段仍为空，且内存占用等于空结构常量。
#[test]
fn empty_partition_info_clones_independently() {
    let info = PhysPlanPartInfo {
        PruningConds: Vec::new(),
        PartitionNames: Vec::new(),
        Columns: Vec::new(),
        ColumnNames: NameSlice(Vec::new()),
    };
    let cloned = info.CloneForPlanCache();

    assert!(cloned.GetPruningConds().is_empty());
    assert!(cloned.GetPartitionNames().is_empty());
    assert!(cloned.GetColumns().is_empty());
    assert_eq!(info.MemoryUsage(), emptyPartitionInfoSize);
}
