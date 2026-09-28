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

// 分析优先队列对 DDL（数据定义语言）schema 变更事件的处理。
//
// DDL 可能使统计作业失效或需要重建（如加索引、截断/删除表或分区）。
// 队列未初始化且开启了 auto-analyze 时返回可重试错误，避免丢失事件。

use crate::queue::AnalysisPriorityQueue;

/// 队列尚未就绪时的可重试错误信息（对应 Go `ErrNotReadyRetryLater`）。
pub const ERR_NOT_READY_RETRY_LATER: &str = "priority queue is not ready; retry later";

/// Schema 变更动作类型，映射自 TiDB DDL Action。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaChangeAction {
    /// 添加索引：需重建该表作业以纳入新索引分析。
    AddIndex,
    /// 截断表：旧表 ID 作业删除，新表 ID 重建。
    TruncateTable,
    /// 删除表：从队列移除作业。
    DropTable,
    /// 截断分区。
    TruncateTablePartition,
    /// 删除分区。
    DropTablePartition,
    /// 交换分区（与普通表互换）。
    ExchangeTablePartition,
    /// 重组分区。
    ReorganizePartition,
    /// 变更分区定义。
    AlterTablePartitioning,
    /// 移除分区化（变回非分区表）。
    RemovePartitioning,
    /// 删除整个 Schema（库）：清理库内相关表作业。
    DropSchema,
    /// 其他未关心的 DDL，忽略。
    Other,
}

/// 封装一次 schema 变更通知所需的表 ID 信息。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SchemaChangeEvent {
    /// 变更动作；None 时按 Other 处理。
    pub action: Option<SchemaChangeAction>,
    /// 主表（或新表）ID。
    pub table_id: i64,
    /// 截断/重组等场景下的旧表 ID。
    pub old_table_id: Option<i64>,
    /// 受影响的其它表 ID（如 DropSchema 下的多表）。
    pub affected_table_ids: Vec<i64>,
    /// `ADD INDEX` 是否已由 DDL 阶段完成统计分析；已分析时无需创建队列作业。
    pub added_index_analyzed: bool,
}

impl AnalysisPriorityQueue {
    /// 根据 DDL 事件类型分发到具体处理函数。
    ///
    /// 未初始化时：若 `run_auto_analyze` 为真则报错以便上层重试，否则静默忽略。
    pub fn HandleDDLEvent(
        &self,
        run_auto_analyze: bool,
        event: &SchemaChangeEvent,
    ) -> Result<(), String> {
        if !self.IsInitialized() {
            return if run_auto_analyze {
                Err(ERR_NOT_READY_RETRY_LATER.to_owned())
            } else {
                Ok(())
            };
        }
        let result = match event.action.unwrap_or(SchemaChangeAction::Other) {
            SchemaChangeAction::AddIndex => self.HandleAddIndexEvent(event),
            SchemaChangeAction::TruncateTable => self.HandleTruncateTableEvent(event),
            SchemaChangeAction::DropTable => self.HandleDropTableEvent(event),
            SchemaChangeAction::TruncateTablePartition => {
                self.HandleTruncateTablePartitionEvent(event)
            }
            SchemaChangeAction::DropTablePartition => self.HandleDropTablePartitionEvent(event),
            SchemaChangeAction::ExchangeTablePartition => {
                self.HandleExchangeTablePartitionEvent(event)
            }
            SchemaChangeAction::ReorganizePartition => self.HandleReorganizePartitionEvent(event),
            SchemaChangeAction::AlterTablePartitioning => {
                self.HandleAlterTablePartitioningEvent(event)
            }
            SchemaChangeAction::RemovePartitioning => self.HandleRemovePartitioningEvent(event),
            SchemaChangeAction::DropSchema => self.HandleDropSchemaEvent(event),
            SchemaChangeAction::Other => Ok(()),
        };
        // Go intentionally logs handler failures and acknowledges the notifier event: without a
        // retry limit, returning these errors would retry the same DDL forever. The readiness error
        // above is the sole retryable result.
        let _ = result;
        Ok(())
    }

    /// 按表 ID 从队列删除作业（若存在）。
    pub fn GetAndDeleteJob(&self, table_id: i64) -> Result<(), String> {
        self.DeleteByTableID(table_id)
    }

    /// 删除旧作业后按当前元数据重建并推入队列。
    pub fn RecreateAndPushJobForTable(&self, table_id: i64) -> Result<(), String> {
        self.RecreateAndPushJob(table_id)
    }

    /// 加索引后重建该表作业。
    pub fn HandleAddIndexEvent(&self, event: &SchemaChangeEvent) -> Result<(), String> {
        if event.added_index_analyzed {
            return Ok(());
        }
        self.RecreateAndPushJobForTable(event.table_id)
    }

    /// 截断表：删除被替换的旧表及其静态分区作业；新表尚无统计，无需重建。
    pub fn HandleTruncateTableEvent(&self, event: &SchemaChangeEvent) -> Result<(), String> {
        self.GetAndDeleteJob(event.old_table_id.unwrap_or(event.table_id))?;
        for table_id in &event.affected_table_ids {
            self.GetAndDeleteJob(*table_id)?;
        }
        Ok(())
    }

    /// 删表：仅移除队列中的作业。
    pub fn HandleDropTableEvent(&self, event: &SchemaChangeEvent) -> Result<(), String> {
        self.GetAndDeleteJob(event.table_id)?;
        for table_id in &event.affected_table_ids {
            self.GetAndDeleteJob(*table_id)?;
        }
        Ok(())
    }

    /// 截断分区：删除旧分区及全局表作业，只重建仍存在的全局表。
    pub fn HandleTruncateTablePartitionEvent(
        &self,
        event: &SchemaChangeEvent,
    ) -> Result<(), String> {
        self.delete_partitions_and_recreate_global(event)
    }

    /// 删除分区：重建相关作业。
    pub fn HandleDropTablePartitionEvent(&self, event: &SchemaChangeEvent) -> Result<(), String> {
        self.delete_partitions_and_recreate_global(event)
    }

    /// 交换分区：双方表统计均需刷新。
    pub fn HandleExchangeTablePartitionEvent(
        &self,
        event: &SchemaChangeEvent,
    ) -> Result<(), String> {
        for table_id in &event.affected_table_ids {
            self.GetAndDeleteJob(*table_id)?;
        }
        if let Some(non_partitioned_table_id) = event.old_table_id {
            self.GetAndDeleteJob(non_partitioned_table_id)?;
        }
        self.GetAndDeleteJob(event.table_id)?;
        self.RecreateAndPushJobForTable(event.table_id)?;
        // The exchanged partition ID becomes the new non-partitioned table ID.
        if let Some(exchanged_partition_id) = event.affected_table_ids.first() {
            self.RecreateAndPushJobForTable(*exchanged_partition_id)?;
        }
        Ok(())
    }

    /// 重组分区。
    pub fn HandleReorganizePartitionEvent(&self, event: &SchemaChangeEvent) -> Result<(), String> {
        self.delete_partitions_and_recreate_global(event)
    }

    /// 变更分区定义。
    pub fn HandleAlterTablePartitioningEvent(
        &self,
        event: &SchemaChangeEvent,
    ) -> Result<(), String> {
        if let Some(old_single_table_id) = event.old_table_id {
            self.GetAndDeleteJob(old_single_table_id)?;
        }
        self.GetAndDeleteJob(event.table_id)?;
        self.RecreateAndPushJobForTable(event.table_id)
    }

    /// 移除分区化。
    pub fn HandleRemovePartitioningEvent(&self, event: &SchemaChangeEvent) -> Result<(), String> {
        for table_id in &event.affected_table_ids {
            self.GetAndDeleteJob(*table_id)?;
        }
        if let Some(old_global_table_id) = event.old_table_id {
            self.GetAndDeleteJob(old_global_table_id)?;
        }
        self.RecreateAndPushJobForTable(event.table_id)
    }

    /// 删库：删除主表及 affected_table_ids 中所有作业。
    pub fn HandleDropSchemaEvent(&self, event: &SchemaChangeEvent) -> Result<(), String> {
        self.GetAndDeleteJob(event.table_id)?;
        for table_id in &event.affected_table_ids {
            self.GetAndDeleteJob(*table_id)?;
        }
        Ok(())
    }

    /// 删除失效的静态分区及动态全局表作业，再只重建仍存在的全局表作业。
    fn delete_partitions_and_recreate_global(
        &self,
        event: &SchemaChangeEvent,
    ) -> Result<(), String> {
        for table_id in &event.affected_table_ids {
            self.GetAndDeleteJob(*table_id)?;
        }
        self.GetAndDeleteJob(event.table_id)?;
        self.RecreateAndPushJobForTable(event.table_id)?;
        Ok(())
    }
}
