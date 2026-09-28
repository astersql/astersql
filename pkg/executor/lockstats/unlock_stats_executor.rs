// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// UNLOCK STATS 执行器：解除表或分区上的统计锁定。
//
// 解析路径与 LOCK STATS 对称；分区解锁时使用用户原始拼写（`CIStr.O`），
// 与锁定侧使用小写名的行为刻意区分。

use std::sync::Arc;

use crate::lock_stats_executor::{
    Error, Result, Runtime, TableName, populatePartitionIDAndNames, populateTableAndPartitionIDs,
};

/// UNLOCK STATS 算子。
pub struct UnlockExec {
    pub runtime: Arc<dyn Runtime>,
    pub Tables: Vec<TableName>,
}
impl UnlockExec {
    /// 打开算子。
    pub fn Open(&mut self) -> Result<()> {
        Ok(())
    }
    /// 关闭算子。
    pub fn Close(&mut self) -> Result<()> {
        Ok(())
    }
    /// 执行解锁：分区级走 RemoveLockedPartitions，否则 RemoveLockedTables。
    pub fn Next(&mut self) -> Result<()> {
        let handle = self
            .runtime
            .StatsHandle()
            .ok_or_else(|| Error("Unlock Stats: handle is nil".into()))?;
        if self.Tables.is_empty() {
            return Err(Error("Unlock Stats: table should not empty ".into()));
        }
        let info_schema = self.runtime.InfoSchema();
        let message = if self.onlyUnlockPartitions() {
            let table = &self.Tables[0];
            let (table_id, partitions) =
                populatePartitionIDAndNames(table, &table.PartitionNames, info_schema.as_ref())?;
            // Unlock preserves the original user spelling, unlike lock's lower-case name.
            // 解锁保留用户原始大小写表名，与锁定侧小写名不同
            handle.RemoveLockedPartitions(
                table_id,
                &format!("{}.{}", table.Schema.O, table.Name.O),
                &partitions,
            )?
        } else {
            handle.RemoveLockedTables(&populateTableAndPartitionIDs(
                &self.Tables,
                info_schema.as_ref(),
            )?)?
        };
        if !message.is_empty() {
            self.runtime.AppendWarning(Error(message));
        }
        Ok(())
    }
    /// 是否为单表 + 显式分区列表的分区级解锁。
    pub fn onlyUnlockPartitions(&self) -> bool {
        self.Tables.len() == 1 && !self.Tables[0].PartitionNames.is_empty()
    }
}
