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

// Delete Range（删除区间）工具模块。
//
// DDL（数据定义语言，如 DROP TABLE / DROP INDEX / TRUNCATE 等）删除大量数据时，
// 不会逐行删除，而是把待清理的数据范围（key 区间）记录到 `gc_delete_range`
// 系统表中，由后台 GC（垃圾回收）任务异步物理删除。
// 每条删除区间记录需要一个在同一 DDL 任务内唯一的 element id（元素编号）
// 来区分不同的删除对象（某个物理表、某个索引等）。
// 本模块提供 element id 的分配器。

use std::collections::BTreeMap;

/// 「物理表 ID + 索引 ID」组合键。
///
/// physical_id 指物理表的 ID：普通表即表 ID，分区表则是各分区的 ID；
/// index_id 是该物理表上某个索引的 ID。二者共同唯一确定一段索引数据的删除区间。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct TableIndexId {
    /// 物理表 ID（普通表的表 ID，或分区表中某个分区的 ID）。
    pub physical_id: i64,
    /// 索引 ID。
    pub index_id: i64,
}

/// element id（元素编号）分配器。
///
/// 在同一个 DDL 任务内，为每个删除对象（索引数据或整个物理表数据）
/// 分配从 1 开始递增的唯一编号；对同一对象重复请求会返回已分配的编号（幂等）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ElementIdAllocator {
    /// 已为「物理表 + 索引」组合分配的编号。
    index_ids: BTreeMap<TableIndexId, i64>,
    /// 已为物理表整体数据分配的编号。
    physical_ids: BTreeMap<i64, i64>,
}

impl ElementIdAllocator {
    /// 为指定物理表上的某个索引分配（或复用）element id。
    pub fn alloc_for_index_id(&mut self, physical_id: i64, index_id: i64) -> i64 {
        let key = TableIndexId {
            physical_id,
            index_id,
        };
        // 若该组合已分配过，直接返回原编号，保证幂等。
        if let Some(id) = self.index_ids.get(&key) {
            return *id;
        }
        // 新编号 = 两张映射表已分配总数 + 1，从 1 开始递增。
        let next = (self.physical_ids.len() + self.index_ids.len() + 1) as i64;
        self.index_ids.insert(key, next);
        next
    }

    /// 为指定物理表整体数据分配（或复用）element id。
    pub fn alloc_for_physical_id(&mut self, physical_id: i64) -> i64 {
        // 若该物理表已分配过，直接返回原编号，保证幂等。
        if let Some(id) = self.physical_ids.get(&physical_id) {
            return *id;
        }
        // 新编号 = 两张映射表已分配总数 + 1，与索引编号共享同一递增序列。
        let next = (self.physical_ids.len() + self.index_ids.len() + 1) as i64;
        self.physical_ids.insert(physical_id, next);
        next
    }
}
