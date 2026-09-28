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

// 分区表重组字段交换测试工具。
//
// 在分区重组（reorganize partition）过程中，需要在新旧 `PartitionedTable`
// 实例间交换定义、表达式与双写分区集合；本模块用 `Any` 动态下转型对齐
// Go 的 `table.Table` 类型断言行为。

use crate::partition::PartitionedTable;
use std::any::Any;
use std::mem;

/// Swaps every field participating in partition reorganization. The dynamic
/// boundary preserves Go's `table.Table` type-assertion behaviour.
///
/// 交换参与分区重组的全部字段；任一侧无法下转为 `PartitionedTable` 则返回 false。
pub fn swap_reorg_part_fields(src: &mut dyn Any, dst: &mut dyn Any) -> bool {
    let Some(src) = src.downcast_mut::<PartitionedTable>() else {
        return false;
    };
    let Some(dst) = dst.downcast_mut::<PartitionedTable>() else {
        return false;
    };
    // 依次交换定义、分区表达式、物理分区列表及重组/双写集合。
    mem::swap(&mut src.definitions, &mut dst.definitions);
    mem::swap(&mut src.expression, &mut dst.expression);
    mem::swap(&mut src.partitions, &mut dst.partitions);
    mem::swap(
        &mut src.reorganize_partitions,
        &mut dst.reorganize_partitions,
    );
    mem::swap(
        &mut src.double_write_partitions,
        &mut dst.double_write_partitions,
    );
    true
}
