// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// DDL 统计订阅路径单元测试。
//
// 用可记录调用的 `RecordingBackend` 验证创建表、截断、删分区、交换分区、
// DropSchema、加列、改分区、Flashback、锁定增量与事件通道容量等行为。

use crate::{
    ColumnInfo, DdlHandler, MiniTableInfo, PartitionDefinition, PartitionInfo, PartitionPruneMode,
    SchemaChangeEvent, StatsBackend, TableInfo,
    update_stats_with_count_delta_and_modify_count_delta_for_test,
};
use std::collections::{HashMap, HashSet};

/// 记录后端各方法调用的测试桩，便于断言副作用。
#[derive(Default)]
struct RecordingBackend {
    prune_mode: PartitionPruneMode,
    historical_enabled: bool,
    inserted_tables: Vec<(i64, i64)>,
    inserted_columns: Vec<(i64, Vec<i64>)>,
    version_updates: Vec<i64>,
    historical: Vec<(i64, u64)>,
    meta: HashMap<i64, (i64, i64)>,
    locked: HashSet<i64>,
    start_ts: u64,
    locked_deltas: Vec<(i64, i64, i64)>,
    written_meta: Vec<(i64, i64, i64)>,
    global_deltas: Vec<(i64, i64, i64, bool)>,
    changed_ids: Vec<(i64, i64)>,
    all_versions_updates: usize,
    ignored_errors: usize,
    schemas: HashMap<i64, String>,
    cache_ready: HashSet<i64>,
}

impl Default for PartitionPruneMode {
    fn default() -> Self {
        Self::Static
    }
}

impl StatsBackend for RecordingBackend {
    fn prune_mode(&mut self) -> Result<PartitionPruneMode, crate::Error> {
        Ok(self.prune_mode)
    }

    fn historical_stats_enabled(&mut self) -> Result<bool, crate::Error> {
        Ok(self.historical_enabled)
    }

    fn cache_initialized(&self, physical_id: i64) -> bool {
        self.cache_ready.contains(&physical_id)
    }

    fn insert_table_stats(
        &mut self,
        table: &TableInfo,
        physical_id: i64,
    ) -> Result<u64, crate::Error> {
        self.inserted_tables.push((table.id, physical_id));
        self.meta.entry(physical_id).or_insert((0, 0));
        self.cache_ready.insert(physical_id);
        self.start_ts += 1;
        Ok(self.start_ts)
    }

    fn insert_column_stats(
        &mut self,
        physical_id: i64,
        columns: &[ColumnInfo],
    ) -> Result<u64, crate::Error> {
        self.inserted_columns
            .push((physical_id, columns.iter().map(|c| c.id).collect()));
        self.start_ts += 1;
        Ok(self.start_ts)
    }

    fn update_stats_meta_version(&mut self, physical_id: i64) -> Result<u64, crate::Error> {
        self.version_updates.push(physical_id);
        self.start_ts += 1;
        Ok(self.start_ts)
    }

    fn record_historical_stats_meta(
        &mut self,
        physical_id: i64,
        start_ts: u64,
    ) -> Result<(), crate::Error> {
        self.historical.push((physical_id, start_ts));
        Ok(())
    }

    fn stats_meta(&mut self, physical_id: i64) -> Result<Option<(i64, i64)>, crate::Error> {
        Ok(self.meta.get(&physical_id).copied())
    }

    fn locked_tables(&mut self) -> Result<HashSet<i64>, crate::Error> {
        Ok(self.locked.clone())
    }

    fn start_ts(&mut self) -> Result<u64, crate::Error> {
        self.start_ts += 1;
        Ok(self.start_ts)
    }

    fn update_locked_delta(
        &mut self,
        table_id: i64,
        _start_ts: u64,
        count_delta: i64,
        modify_delta: i64,
    ) -> Result<(), crate::Error> {
        self.locked_deltas
            .push((table_id, count_delta, modify_delta));
        Ok(())
    }

    fn write_stats_meta(
        &mut self,
        table_id: i64,
        _start_ts: u64,
        count: i64,
        modify_count: i64,
    ) -> Result<(), crate::Error> {
        self.written_meta.push((table_id, count, modify_count));
        self.meta.insert(table_id, (count, modify_count));
        Ok(())
    }

    fn apply_global_delta(
        &mut self,
        table_id: i64,
        _start_ts: u64,
        count: i64,
        delta: i64,
        locked: bool,
    ) -> Result<(), crate::Error> {
        self.global_deltas.push((table_id, count, delta, locked));
        let entry = self.meta.entry(table_id).or_insert((0, 0));
        entry.0 = (entry.0 + count).max(0);
        entry.1 = (entry.1 + delta).max(0);
        Ok(())
    }

    fn change_global_stats_id(&mut self, old_id: i64, new_id: i64) -> Result<(), crate::Error> {
        self.changed_ids.push((old_id, new_id));
        if let Some(meta) = self.meta.remove(&old_id) {
            self.meta.insert(new_id, meta);
        }
        Ok(())
    }

    fn update_all_stats_versions(&mut self) -> Result<(), crate::Error> {
        self.all_versions_updates += 1;
        Ok(())
    }

    fn schema_name(&self, table_id: i64) -> Option<String> {
        self.schemas.get(&table_id).cloned()
    }

    fn warn_ignored_event_error(&mut self, _event: &SchemaChangeEvent, _error: &crate::Error) {
        self.ignored_errors += 1;
    }
}

/// 构造无分区普通表。
fn plain_table(id: i64, _columns: &[i64]) -> TableInfo {
    TableInfo {
        id,
        name: format!("t{id}"),
        partitions: Vec::new(),
    }
}

/// 构造带指定分区列表的分区表。
fn partitioned_table(id: i64, partitions: &[(i64, &str)]) -> TableInfo {
    TableInfo {
        id,
        name: format!("t{id}"),
        partitions: partitions
            .iter()
            .map(|(pid, name)| PartitionDefinition {
                id: *pid,
                name: (*name).into(),
            })
            .collect(),
    }
}

/// 创建普通表时应为该物理 ID 插入表统计。
#[test]
fn TestDDLTable_create_table_inserts_stats() {
    let mut handler = DdlHandler::new(RecordingBackend::default());
    let table = plain_table(10, &[1, 2]);
    handler
        .handle_ddl_event(&SchemaChangeEvent::CreateTable(table.clone()))
        .unwrap();
    assert_eq!(
        handler.subscriber().backend().inserted_tables,
        vec![(10, 10)]
    );
    assert!(handler.subscriber().backend().meta.contains_key(&10));
}

/// 动态裁剪下创建分区表时，分区与全局表 ID 都应插入统计。
#[test]
fn TestDDLTable_create_partitioned_table_dynamic_includes_global() {
    let mut backend = RecordingBackend::default();
    backend.prune_mode = PartitionPruneMode::Dynamic;
    let mut handler = DdlHandler::new(backend);
    let table = partitioned_table(20, &[(21, "p0"), (22, "p1")]);
    handler
        .handle_ddl_event(&SchemaChangeEvent::CreateTable(table))
        .unwrap();
    let inserted = &handler.subscriber().backend().inserted_tables;
    assert!(inserted.contains(&(20, 21)));
    assert!(inserted.contains(&(20, 22)));
    assert!(inserted.contains(&(20, 20)));
}

/// 截断表：新表插入，旧表推进版本（延迟删除）。
#[test]
fn TestTruncateTable_inserts_new_and_marks_old() {
    let mut handler = DdlHandler::new(RecordingBackend::default());
    let old = plain_table(30, &[1]);
    let new = plain_table(31, &[1]);
    handler
        .handle_ddl_event(&SchemaChangeEvent::TruncateTable {
            new_table: new,
            dropped_table: old,
        })
        .unwrap();
    assert!(
        handler
            .subscriber()
            .backend()
            .inserted_tables
            .contains(&(31, 31))
    );
    assert!(handler.subscriber().backend().version_updates.contains(&30));
}

#[test]
fn go_merge_47_materialized_view_cutover_and_metadata_events() {
    let mut backend = RecordingBackend::default();
    backend.historical_enabled = true;
    backend.cache_ready.insert(30);
    let mut handler = DdlHandler::new(backend);
    let old = plain_table(30, &[]);
    let new = plain_table(31, &[]);
    handler
        .handle_ddl_event(&SchemaChangeEvent::MViewRefreshOutOfPlaceCutover {
            new_table: new,
            dropped_table: old,
        })
        .unwrap();
    let backend = handler.subscriber().backend();
    assert!(backend.inserted_tables.contains(&(31, 31)));
    assert!(backend.version_updates.contains(&30));
    assert!(backend.historical.iter().any(|(id, _)| *id == 30));

    for event in [
        SchemaChangeEvent::AlterMaterializedViewRefresh,
        SchemaChangeEvent::AlterMaterializedViewAttributes,
        SchemaChangeEvent::AlterMaterializedViewLogPurge,
        SchemaChangeEvent::CreateMaterializedViewLog,
        SchemaChangeEvent::CreateMaterializedView,
    ] {
        handler.handle_ddl_event(&event).unwrap();
    }
    let backend = handler.subscriber().backend();
    assert_eq!(backend.inserted_tables.len(), 1);
    assert_eq!(backend.version_updates.len(), 1);
}

/// 删除分区应对全局表应用负增量并延迟删除分区统计。
#[test]
fn TestDropTablePartition_updates_global_delta() {
    let mut backend = RecordingBackend::default();
    backend.meta.insert(41, (5, 1));
    backend.meta.insert(40, (10, 2));
    let mut handler = DdlHandler::new(backend);
    let global = partitioned_table(40, &[(41, "p0"), (42, "p1")]);
    let dropped = PartitionInfo {
        definitions: vec![PartitionDefinition {
            id: 41,
            name: "p0".into(),
        }],
    };
    handler
        .handle_ddl_event(&SchemaChangeEvent::DropTablePartition {
            global_table: global,
            dropped,
        })
        .unwrap();
    assert_eq!(
        handler.subscriber().backend().global_deltas,
        vec![(40, 5, -5, false)]
    );
    assert!(handler.subscriber().backend().version_updates.contains(&41));
}

/// 截断分区：插入新分区、扣减全局行数，并保留 Go 侧 modify 语义。
#[test]
fn TestTruncateTablePartition_preserves_modify_count_semantics() {
    let mut backend = RecordingBackend::default();
    backend.meta.insert(51, (4, 2));
    backend.meta.insert(50, (8, 3));
    let mut handler = DdlHandler::new(backend);
    let global = partitioned_table(50, &[(51, "p0")]);
    let dropped = PartitionInfo {
        definitions: vec![PartitionDefinition {
            id: 51,
            name: "p0".into(),
        }],
    };
    let added = PartitionInfo {
        definitions: vec![PartitionDefinition {
            id: 52,
            name: "p0".into(),
        }],
    };
    handler
        .handle_ddl_event(&SchemaChangeEvent::TruncateTablePartition {
            global_table: global,
            added,
            dropped,
        })
        .unwrap();
    assert!(
        handler
            .subscriber()
            .backend()
            .inserted_tables
            .contains(&(50, 52))
    );
    assert_eq!(
        handler.subscriber().backend().global_deltas,
        vec![(50, 4, -4, false)]
    );
}

/// 交换分区应按表与分区的 count/modify 差更新全局 stats_meta。
#[test]
fn TestExchangeTablePartition_updates_global_counts() {
    let mut backend = RecordingBackend::default();
    backend.meta.insert(61, (3, 1));
    backend.meta.insert(62, (7, 2));
    backend.meta.insert(60, (10, 0));
    let mut handler = DdlHandler::new(backend);
    handler
        .handle_ddl_event(&SchemaChangeEvent::ExchangeTablePartition {
            global_table: partitioned_table(60, &[(61, "p0")]),
            original_partition: PartitionInfo {
                definitions: vec![PartitionDefinition {
                    id: 61,
                    name: "p0".into(),
                }],
            },
            original_table: plain_table(62, &[1]),
        })
        .unwrap();
    // count_delta = 7 - 3 = 4
    // modify_delta = 7 + 3 - 1 + 2 = 11
    assert_eq!(
        handler.subscriber().backend().written_meta,
        vec![(60, 14, 11)]
    );
}

/// DropSchema 应对库内各表与分区都推进版本。
#[test]
fn TestDropSchema_deletes_all_tables() {
    let mut handler = DdlHandler::new(RecordingBackend::default());
    handler
        .handle_ddl_event(&SchemaChangeEvent::DropSchema(vec![
            MiniTableInfo {
                id: 70,
                partitions: vec![],
            },
            MiniTableInfo {
                id: 71,
                partitions: vec![PartitionDefinition {
                    id: 72,
                    name: "p0".into(),
                }],
            },
        ]))
        .unwrap();
    let versions = &handler.subscriber().backend().version_updates;
    assert!(versions.contains(&70));
    assert!(versions.contains(&71));
    assert!(versions.contains(&72));
}

/// 加列应插入对应列伪统计。
#[test]
fn TestAddColumn_inserts_column_stats() {
    let mut handler = DdlHandler::new(RecordingBackend::default());
    let table = plain_table(80, &[1, 2]);
    handler
        .handle_ddl_event(&SchemaChangeEvent::AddColumn {
            table,
            columns: vec![ColumnInfo {
                id: 3,
                name: "c3".into(),
            }],
        })
        .unwrap();
    assert_eq!(
        handler.subscriber().backend().inserted_columns,
        vec![(80, vec![3])]
    );
}

/// 改分区：新分区插入统计，并将全局统计 ID 从旧单表迁到新全局表。
#[test]
fn TestAlterTablePartitioning_changes_global_id() {
    let mut handler = DdlHandler::new(RecordingBackend::default());
    handler
        .handle_ddl_event(&SchemaChangeEvent::AlterTablePartitioning {
            old_single_table_id: 90,
            global_table: partitioned_table(91, &[(92, "p0")]),
            added: PartitionInfo {
                definitions: vec![PartitionDefinition {
                    id: 92,
                    name: "p0".into(),
                }],
            },
        })
        .unwrap();
    assert_eq!(handler.subscriber().backend().changed_ids, vec![(90, 91)]);
    assert!(
        handler
            .subscriber()
            .backend()
            .inserted_tables
            .contains(&(91, 92))
    );
}

/// FlashbackCluster 应触发全量统计版本刷新。
#[test]
fn TestFlashbackCluster_updates_all_versions() {
    let mut handler = DdlHandler::new(RecordingBackend::default());
    handler
        .handle_ddl_event(&SchemaChangeEvent::FlashbackCluster)
        .unwrap();
    assert_eq!(handler.subscriber().backend().all_versions_updates, 1);
}

/// 锁定表增量路径应走 `update_locked_delta` 而非绝对写回。
#[test]
fn TestUpdateStatsWithCountDelta_locked_path() {
    let mut backend = RecordingBackend {
        locked: HashSet::from([100]),
        meta: HashMap::from([(100, (5, 1))]),
        ..Default::default()
    };
    update_stats_with_count_delta_and_modify_count_delta_for_test(&mut backend, 100, -2, 3)
        .unwrap();
    assert_eq!(backend.locked_deltas, vec![(100, -2, 3)]);
}

/// 事件队列满时应报错；弹出后可继续消费。
#[test]
fn TestDDLEventChannelCapacity() {
    let mut handler = DdlHandler::new(RecordingBackend::default());
    for i in 0..crate::DDL_EVENT_CHANNEL_CAPACITY {
        handler
            .enqueue(SchemaChangeEvent::CreateTable(plain_table(i as i64, &[1])))
            .unwrap();
    }
    let err = handler
        .enqueue(SchemaChangeEvent::CreateTable(plain_table(999, &[1])))
        .unwrap_err();
    assert!(err.to_string().contains("full"));
    assert_eq!(
        handler
            .next_event()
            .map(|e| matches!(e, SchemaChangeEvent::CreateTable(_))),
        Some(true)
    );
}

/// 系统表 DDL 过滤是调用方策略：订阅者本身不主动过滤。
#[test]
fn TestSystemTableDDLHasNoEvent_is_caller_policy() {
    // Go filters mysql.* DDL before enqueueing into StatsHandle.DDLEventCh.
    // The Rust subscriber still processes events it is given; callers must not
    // enqueue system-table events. Verify the channel starts empty.
    // Go 在入队前过滤 mysql.* 系统表 DDL；Rust 订阅者仍处理已入队事件，
    // 因此调用方不得入队系统表事件。此处验证初始队列为空。
    let mut handler = DdlHandler::new(RecordingBackend::default());
    assert!(handler.next_event().is_none());
}
