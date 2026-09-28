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

//! PiTR filter helpers ported from `br/pkg/utils/filter.go`.
//!
//! PiTR 恢复用的库表/分区跟踪器，以及带系统库排除的 schema/table 过滤器封装。
//! TrackTableId 会同时登记 db_id，保证后续 ContainsDB 对隐式库也成立。
//! 名称跟踪与 id 跟踪并存，覆盖元数据尚未分配 id 的阶段。

use std::collections::{HashMap, HashSet};

use astersql_br_pkg_logutil::{Field, log};
use astersql_util_table_filter::Filter;

use crate::schema::{IsSysDB, StripTempDBPrefixIfNeeded};

/// Tracks database/table/partition ids and names selected for PiTR restore.
/// PiTR 选中对象的 id/名称集合；TableIdToDBIds 支持同表 id 映射多库（极端/测试场景）。
#[derive(Clone, Debug, Default)]
pub struct PiTRIdTracker {
    pub DBIds: HashSet<i64>,
    pub TableIdToDBIds: HashMap<i64, HashSet<i64>>,
    pub PartitionIds: HashSet<i64>,
    pub DBNameToTableNames: HashMap<String, HashSet<String>>,
}

/// Creates an empty tracker.
/// 构造空跟踪器。
pub fn NewPiTRIdTracker() -> PiTRIdTracker {
    PiTRIdTracker::default()
}

impl PiTRIdTracker {
    /// 登记表及其所属库；日志便于排查过滤漏选。
    pub fn TrackTableId(&mut self, db_id: i64, table_id: i64) {
        log::L().Info(
            "tracking table id",
            [Field::int("dbID", db_id), Field::int("tableID", table_id)],
        );
        self.DBIds.insert(db_id);
        self.TableIdToDBIds
            .entry(table_id)
            .or_default()
            .insert(db_id);
    }

    /// 登记分区 id（物理 id），供分区级 PiTR 过滤。
    pub fn TrackPartitionId(&mut self, partition_id: i64) {
        self.PartitionIds.insert(partition_id);
    }

    /// 仅登记库 id（无表时也需保留库级选中）。
    pub fn AddDB(&mut self, db_id: i64) {
        log::L().Info("tracking db id", [Field::int("dbID", db_id)]);
        self.DBIds.insert(db_id);
    }

    /// 同时匹配库 id 与表 id，避免跨库同 table_id 误中。
    pub fn ContainsDBAndTableId(&self, db_id: i64, table_id: i64) -> bool {
        self.TableIdToDBIds
            .get(&table_id)
            .is_some_and(|db_ids| db_ids.contains(&db_id))
    }

    /// 仅按表 id 查询是否曾被跟踪（不区分所属库）。
    /// 与 ContainsDBAndTableId 不同：跨库同 table_id 也会命中。
    pub fn ContainsTableId(&self, table_id: i64) -> bool {
        self.TableIdToDBIds.contains_key(&table_id)
    }

    /// 分区是否在选中集合中。
    pub fn ContainsPartitionId(&self, partition_id: i64) -> bool {
        self.PartitionIds.contains(&partition_id)
    }

    /// 库是否在选中集合中。
    pub fn ContainsDB(&self, db_id: i64) -> bool {
        self.DBIds.contains(&db_id)
    }

    /// 按名称跟踪（元数据尚未有 id 时的兜底路径）。
    pub fn TrackTableName(&mut self, db_name: String, table_name: String) {
        log::L().Info(
            "tracking table name",
            [
                Field::string("dbName", db_name.clone()),
                Field::string("tableName", table_name.clone()),
            ],
        );
        self.DBNameToTableNames
            .entry(db_name)
            .or_default()
            .insert(table_name);
    }

    /// 只读访问名称映射，供上层序列化/日志。
    pub fn GetDBNameToTableName(&self) -> &HashMap<String, HashSet<String>> {
        &self.DBNameToTableNames
    }
}

/// Matches schema through table-filter while honoring system DB exclusions.
/// 去掉临时库前缀后再匹配；`with_sys=false` 时系统库一律排除。
pub fn MatchSchema(filter: &dyn Filter, schema: &str, with_sys: bool) -> bool {
    let schema = StripTempDBPrefixIfNeeded(schema);
    if IsSysDB(&schema) && !with_sys {
        return false;
    }
    filter.MatchSchema(&schema)
}

/// Matches table through table-filter while honoring system DB exclusions.
/// 表级匹配同样先剥临时前缀并尊重系统库开关。
pub fn MatchTable(filter: &dyn Filter, schema: &str, table: &str, with_sys: bool) -> bool {
    let schema = StripTempDBPrefixIfNeeded(schema);
    if IsSysDB(&schema) && !with_sys {
        return false;
    }
    filter.MatchTable(&schema, table)
}
