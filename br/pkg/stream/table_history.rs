// Copyright 2026 AsterSQL.
// Copyright 2022-present PingCAP, Inc.
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

//! 日志备份表/库名历史管理：对齐 Go `br/pkg/stream/table_history.go`。
//! 每个表 ID 仅保留「首次」与「最新」两条 `TableLocationInfo`，供 PITR 映射与展示。
//! 分区与普通表共用 `addHistory`；库名按 commit ts 单调更新，旧 ts 不回写。
//! 回调 `OnDatabaseInfo`/`OnTableInfo` 供 meta KV 解析侧增量喂入。

// tableNameHistory 的双槽设计避免保存完整 rename 链。
// dbTimestamps 与 dbIdToName 必须同步更新。
// OnTableInfo 分区循环使用同一 commitTs 与表名。
use std::collections::HashMap;

use crate::stubs::TableSimpleInfo;

/// 表或分区在某一时刻的库/名/父子关系快照。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableLocationInfo {
    pub DbID: i64,
    pub TableName: String,
    // 为真时 ParentTableID 指向所属逻辑表。
    pub IsPartition: bool,
    pub ParentTableID: i64,
    pub Timestamp: u64,
}

/// 维护表 ID → [首次, 最新] 位置，以及库 ID → 名称（按最新 ts）。
pub struct LogBackupTableHistoryManager {
    // [0]=首次观察到的位置，[1]=时间戳最大的位置。
    tableNameHistory: HashMap<i64, [TableLocationInfo; 2]>,
    dbIdToName: HashMap<i64, String>,
    // 与 dbIdToName 同步，决定是否接受新库名。
    dbTimestamps: HashMap<i64, u64>,
}

/// 空管理器构造入口，对齐 Go `NewTableHistoryManager`。
pub fn NewTableHistoryManager() -> LogBackupTableHistoryManager {
    LogBackupTableHistoryManager {
        tableNameHistory: HashMap::new(),
        dbIdToName: HashMap::new(),
        dbTimestamps: HashMap::new(),
    }
}

impl LogBackupTableHistoryManager {
    /// 记录普通表（非分区）历史；ParentTableID 固定为 0。
    pub fn AddTableHistory(&mut self, tableId: i64, tableName: &str, dbID: i64, ts: u64) {
        let locationInfo = TableLocationInfo {
            DbID: dbID,
            TableName: tableName.to_string(),
            IsPartition: false,
            ParentTableID: 0,
            Timestamp: ts,
        };
        self.addHistory(tableId, locationInfo);
    }

    /// 记录分区历史；`parentTableID` 为逻辑表 ID。
    pub fn AddPartitionHistory(
        &mut self,
        partitionID: i64,
        tableName: &str,
        dbID: i64,
        parentTableID: i64,
        ts: u64,
    ) {
        let locationInfo = TableLocationInfo {
            DbID: dbID,
            TableName: tableName.to_string(),
            IsPartition: true,
            ParentTableID: parentTableID,
            Timestamp: ts,
        };
        self.addHistory(partitionID, locationInfo);
    }

    // 首次插入时两端相同；之后仅当 ts >= 当前最新才覆盖 [1]，保留 [0]。
    fn addHistory(&mut self, id: i64, locationInfo: TableLocationInfo) {
        match self.tableNameHistory.get(&id) {
            None => {
                self.tableNameHistory
                    .insert(id, [locationInfo.clone(), locationInfo]);
            }
            Some(existing) => {
                if locationInfo.Timestamp >= existing[1].Timestamp {
                    self.tableNameHistory
                        .insert(id, [existing[0].clone(), locationInfo]);
                }
            }
        }
    }

    /// 按 ts 单调更新库名；更旧的 commit 不会覆盖已有映射。
    pub fn RecordDBIdToName(&mut self, dbId: i64, dbName: &str, ts: u64) {
        let should = match self.dbTimestamps.get(&dbId) {
            None => true,
            Some(existingTs) => ts >= *existingTs,
        };
        if should {
            self.dbIdToName.insert(dbId, dbName.to_string());
            self.dbTimestamps.insert(dbId, ts);
        }
    }

    /// 只读访问表历史双槽映射。
    pub fn GetTableHistory(&self) -> &HashMap<i64, [TableLocationInfo; 2]> {
        &self.tableNameHistory
    }

    /// 按库 ID 查最新名称；未知库返回 None。
    pub fn GetDBNameByID(&self, dbId: i64) -> Option<&str> {
        self.dbIdToName.get(&dbId).map(|s| s.as_str())
    }

    /// 返回全部库名映射（含任务过程中新出现的库）。
    pub fn GetNewlyCreatedDBHistory(&self) -> &HashMap<i64, String> {
        &self.dbIdToName
    }

    /// Meta 解析回调：转发到 `RecordDBIdToName`。
    pub fn OnDatabaseInfo(&mut self, dbId: i64, dbName: &str, ts: u64) {
        self.RecordDBIdToName(dbId, dbName, ts);
    }

    /// Meta 解析回调：先记逻辑表，再为每个分区 ID 记分区历史。
    pub fn OnTableInfo(
        &mut self,
        dbID: i64,
        tableId: i64,
        tableSimpleInfo: &TableSimpleInfo,
        commitTs: u64,
    ) {
        self.AddTableHistory(tableId, &tableSimpleInfo.Name, dbID, commitTs);
        for &partitionId in &tableSimpleInfo.PartitionIds {
            self.AddPartitionHistory(partitionId, &tableSimpleInfo.Name, dbID, tableId, commitTs);
        }
    }
}
