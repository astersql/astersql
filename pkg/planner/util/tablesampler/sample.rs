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

// 物理表采样计划所需的信息结构。
//
// TABLESAMPLE 是 SQL 中按系统/伯努利等方法从基表抽样行的语法；
// 本模块把解析得到的 AST、输出 Schema 与目标分区表打包，供执行计划使用。

use std::sync::Arc;

use expression::Schema;
use parser_ast::TableSample;
use table::PartitionedTable;

/// Information used by a physical table-sample plan.
/// 物理表采样计划使用的信息：采样 AST、完整 Schema、涉及的分区表。
pub struct TableSampleInfo {
    /// 解析器产出的 TABLESAMPLE AST 节点。
    pub AstNode: Option<TableSample>,
    /// 采样结果对应的完整列 Schema。
    pub FullSchema: Option<Schema>,
    /// 需要采样的分区表列表（分区表按分区物理切分存储）。
    pub Partitions: Vec<Arc<dyn PartitionedTable>>,
}

impl TableSampleInfo {
    /// Returns the same estimated memory components as the Go implementation.
    /// 估算本结构占用的近似内存，分量与 Go 实现对齐（指针、切片容量、Schema 等）。
    pub fn MemoryUsage(&self) -> i64 {
        // 指针字段 ×2 + 切片头 + 各 Partition 接口指针容量，再叠加 AST 与 Schema。
        let mut sum = size::SizeOfPointer * 2
            + size::SizeOfSlice
            + self.Partitions.capacity() as i64 * size::SizeOfInterface;
        if self.AstNode.is_some() {
            sum += std::mem::size_of::<TableSample>() as i64;
        }
        if let Some(schema) = &self.FullSchema {
            sum += schema.MemoryUsage();
        }
        sum
    }
}

/// Creates table-sample information and clones the supplied schema.
/// 由可选 AST 节点构造 `TableSampleInfo`；`node` 为 None 时返回 None（无采样）。
pub fn NewTableSampleInfo(
    node: Option<&TableSample>,
    fullSchema: &Schema,
    partitions: Vec<Arc<dyn PartitionedTable>>,
) -> Option<TableSampleInfo> {
    let node = node?;
    Some(TableSampleInfo {
        AstNode: Some(node.clone()),
        FullSchema: Some(fullSchema.Clone()),
        Partitions: partitions,
    })
}
