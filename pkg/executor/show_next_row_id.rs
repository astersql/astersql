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

// `SHOW TABLE ... NEXT_ROW_ID` 执行器：列出表上各分配器的下一全局 ID。
//
// 分配器（Allocator）负责为 `_tidb_rowid`、AUTO_INCREMENT、AUTO_RANDOM、SEQUENCE
// 等发号；`next_global_id` 是下一可用全局号，便于运维核对发号进度。

#![allow(non_snake_case)]

use astersql_util_chunk::Chunk;

/// 行号/自增类分配器类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AllocatorType {
    RowId,
    AutoIncrement,
    AutoRandom,
    Sequence,
}

/// 单个分配器在查询时刻的快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AllocatorSnapshot {
    pub allocator_type: AllocatorType,
    /// 下一可用全局 ID。
    pub next_global_id: i64,
    /// 主键是否为整数 handle（可直接作行标识）。
    pub primary_key_is_handle: bool,
    pub auto_increment_column: Option<String>,
    pub primary_key_name: String,
}

/// 按库表名拉取分配器快照的数据源边界。
pub trait NextRowIdSource {
    type Error;

    fn allocators(
        &mut self,
        schema: &str,
        table: &str,
    ) -> Result<Vec<AllocatorSnapshot>, Self::Error>;
}

/// `SHOW TABLE NEXT_ROW_ID` 执行器；一次性写出所有分配器行。
pub struct ShowNextRowIDExec<S: NextRowIdSource> {
    pub source: S,
    pub schema_name: String,
    pub table_name: String,
    pub done: bool,
}

impl<S: NextRowIdSource> ShowNextRowIDExec<S> {
    /// 将各分配器编码为 schema/table/column/next_id/id_type 五行式结果。
    pub fn Next<C>(&mut self, _ctx: C, req: &mut Chunk) -> Result<(), S::Error> {
        req.Reset();
        if self.done {
            return Ok(());
        }

        let allocators = self
            .source
            .allocators(&self.schema_name, &self.table_name)?;
        for allocator in allocators {
            // 按分配器类型决定展示列名与 ID 类型标签。
            let (column_name, id_type) = match allocator.allocator_type {
                AllocatorType::RowId => (
                    if allocator.primary_key_is_handle {
                        allocator.auto_increment_column.unwrap_or_default()
                    } else {
                        "_tidb_rowid".to_owned()
                    },
                    "_TIDB_ROWID",
                ),
                AllocatorType::AutoIncrement => (
                    if allocator.primary_key_is_handle {
                        allocator.auto_increment_column.unwrap_or_default()
                    } else {
                        "_tidb_rowid".to_owned()
                    },
                    "AUTO_INCREMENT",
                ),
                AllocatorType::AutoRandom => (allocator.primary_key_name, "AUTO_RANDOM"),
                AllocatorType::Sequence => (String::new(), "SEQUENCE"),
            };

            req.AppendString(0, &self.schema_name);
            req.AppendString(1, &self.table_name);
            req.AppendString(2, &column_name);
            req.AppendInt64(3, allocator.next_global_id);
            req.AppendString(4, id_type);
        }
        self.done = true;
        Ok(())
    }
}
