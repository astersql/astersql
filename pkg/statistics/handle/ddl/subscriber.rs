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

// Schema 变更事件订阅者：按 DDL 类型维护统计元数据。
//
// 在创建/截断/删除表或分区、加列、交换分区等 schema 变更后，向统计后端
// 插入伪统计、延迟删除（更新版本）、调整全局行数增量，并可选记录历史统计元数据。
// 对应 Go `statistics/handle/ddl` 中的 subscriber 逻辑。

use std::collections::HashSet;
use std::fmt;

/// 订阅者路径上的错误包装。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// 分区裁剪模式：静态只看分区，动态还需维护全局表统计。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartitionPruneMode {
    Static,
    Dynamic,
}

/// 列元信息（DDL 加列/改列时使用）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColumnInfo {
    pub id: i64,
    pub name: String,
}

/// 单个分区定义：物理 ID 与名称。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PartitionDefinition {
    pub id: i64,
    pub name: String,
}

/// 一组分区定义（增删截断分区事件中的 added/dropped）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PartitionInfo {
    pub definitions: Vec<PartitionDefinition>,
}

/// 表元信息：逻辑表 ID、名称与分区列表。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableInfo {
    pub id: i64,
    pub name: String,
    pub partitions: Vec<PartitionDefinition>,
}

/// DropSchema 使用的精简表信息。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MiniTableInfo {
    pub id: i64,
    pub partitions: Vec<PartitionDefinition>,
}

/// 统计子系统关心的 schema 变更事件枚举（由 DDL notifier 映射而来）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaChangeEvent {
    CreateTable(TableInfo),
    TruncateTable {
        new_table: TableInfo,
        dropped_table: TableInfo,
    },
    DropTable(TableInfo),
    AddColumn {
        table: TableInfo,
        columns: Vec<ColumnInfo>,
    },
    ModifyColumn {
        table: TableInfo,
        columns: Vec<ColumnInfo>,
        analyzed: bool,
    },
    AddTablePartition {
        global_table: TableInfo,
        added: PartitionInfo,
    },
    TruncateTablePartition {
        global_table: TableInfo,
        added: PartitionInfo,
        dropped: PartitionInfo,
    },
    DropTablePartition {
        global_table: TableInfo,
        dropped: PartitionInfo,
    },
    ExchangeTablePartition {
        global_table: TableInfo,
        original_partition: PartitionInfo,
        original_table: TableInfo,
    },
    ReorganizePartition {
        global_table: TableInfo,
        added: PartitionInfo,
        dropped: PartitionInfo,
    },
    AlterTablePartitioning {
        old_single_table_id: i64,
        global_table: TableInfo,
        added: PartitionInfo,
    },
    RemovePartitioning {
        old_table_id: i64,
        new_single_table: TableInfo,
        dropped: PartitionInfo,
    },
    FlashbackCluster,
    AddIndex,
    DropSchema(Vec<MiniTableInfo>),
    Unknown(String),
}

/// 统计存储/缓存后端抽象：订阅者只依赖此 trait，便于测试注入。
pub trait StatsBackend {
    /// 当前会话的分区裁剪模式。
    fn prune_mode(&mut self) -> Result<PartitionPruneMode, Error>;
    /// 是否启用历史统计（historical stats）。
    fn historical_stats_enabled(&mut self) -> Result<bool, Error>;
    /// 指定物理表的统计缓存是否已初始化。
    fn cache_initialized(&self, physical_id: i64) -> bool;
    /// 为物理表插入伪表级统计，返回写入用的 start_ts。
    fn insert_table_stats(&mut self, table: &TableInfo, physical_id: i64) -> Result<u64, Error>;
    /// 为物理表插入列级伪统计。
    fn insert_column_stats(
        &mut self,
        physical_id: i64,
        columns: &[ColumnInfo],
    ) -> Result<u64, Error>;
    /// 更新 stats_meta 版本（延迟删除语义）。
    fn update_stats_meta_version(&mut self, physical_id: i64) -> Result<u64, Error>;
    /// 记录历史统计元数据。
    fn record_historical_stats_meta(
        &mut self,
        physical_id: i64,
        start_ts: u64,
    ) -> Result<(), Error>;
    /// 读取物理表的 (count, modify_count)。
    fn stats_meta(&mut self, physical_id: i64) -> Result<Option<(i64, i64)>, Error>;
    /// 当前被锁定统计的表集合。
    fn locked_tables(&mut self) -> Result<HashSet<i64>, Error>;
    /// 取当前事务 start_ts（时间戳）。
    fn start_ts(&mut self) -> Result<u64, Error>;
    /// 锁定表路径下的增量 upsert（允许结果为负）。
    fn update_locked_delta(
        &mut self,
        table_id: i64,
        start_ts: u64,
        count_delta: i64,
        modify_delta: i64,
    ) -> Result<(), Error>;
    /// 非锁定路径写回绝对 count / modify_count（下限钳制为 0）。
    fn write_stats_meta(
        &mut self,
        table_id: i64,
        start_ts: u64,
        count: i64,
        modify_count: i64,
    ) -> Result<(), Error>;
    /// 对全局表应用行数/修改数增量（删/截断分区时）。
    fn apply_global_delta(
        &mut self,
        table_id: i64,
        start_ts: u64,
        count: i64,
        delta: i64,
        locked: bool,
    ) -> Result<(), Error>;
    /// 将全局统计从旧表 ID 迁移到新表 ID。
    fn change_global_stats_id(&mut self, old_id: i64, new_id: i64) -> Result<(), Error>;
    /// Flashback 后刷新全部统计版本。
    fn update_all_stats_versions(&mut self) -> Result<(), Error>;
    /// 按表 ID 查 schema 名（日志字段用）。
    fn schema_name(&self, table_id: i64) -> Option<String>;
    /// best-effort 路径下记录被忽略的事件错误。
    fn warn_ignored_event_error(&mut self, _event: &SchemaChangeEvent, _error: &Error) {}
}

/// Schema 变更订阅者：持有统计后端并分发事件。
pub struct Subscriber<B> {
    backend: B,
}

impl<B: StatsBackend> Subscriber<B> {
    /// 用给定后端构造订阅者。
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    /// 只读访问后端。
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// 可变访问后端。
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// 按事件类型更新统计；多数路径在全部物理 ID 上插入或延迟删除。
    pub fn handle(&mut self, change: &SchemaChangeEvent) -> Result<(), Error> {
        match change {
            SchemaChangeEvent::CreateTable(table) => {
                for id in self.physical_ids(table)? {
                    self.insert_stats_for_physical_id(table, id)?;
                }
            }
            SchemaChangeEvent::TruncateTable {
                new_table,
                dropped_table,
            } => {
                // 新表插入伪统计，旧表仅推进版本（延迟删除）。
                for id in self.physical_ids(new_table)? {
                    self.insert_stats_for_physical_id(new_table, id)?;
                }
                for id in self.physical_ids(dropped_table)? {
                    self.delayed_delete_stats_for_physical_id(id)?;
                }
            }
            SchemaChangeEvent::DropTable(table) => {
                for id in self.physical_ids(table)? {
                    self.delayed_delete_stats_for_physical_id(id)?;
                }
            }
            SchemaChangeEvent::AddColumn { table, columns } => {
                for id in self.physical_ids(table)? {
                    self.insert_stats_for_columns(id, columns)?;
                }
            }
            SchemaChangeEvent::ModifyColumn {
                table,
                columns,
                analyzed,
            } => {
                // 改列时若已随 DDL 分析完成，则无需再插入伪列统计。
                if *analyzed {
                    return Ok(());
                }
                for id in self.physical_ids(table)? {
                    self.insert_stats_for_columns(id, columns)?;
                }
            }
            SchemaChangeEvent::AddTablePartition {
                global_table,
                added,
            } => {
                for definition in &added.definitions {
                    self.insert_stats_for_physical_id(global_table, definition.id)?;
                }
            }
            SchemaChangeEvent::TruncateTablePartition {
                global_table,
                added,
                dropped,
            } => {
                for definition in &added.definitions {
                    self.insert_stats_for_physical_id(global_table, definition.id)?;
                }
                self.update_global_stats_for_truncate_partition(global_table, dropped)?;
                for definition in &dropped.definitions {
                    self.delayed_delete_stats_for_physical_id(definition.id)?;
                }
                return Ok(());
            }
            SchemaChangeEvent::DropTablePartition {
                global_table,
                dropped,
            } => {
                self.update_global_stats_for_drop_partition(global_table, dropped)?;
                for definition in &dropped.definitions {
                    self.delayed_delete_stats_for_physical_id(definition.id)?;
                }
                return Ok(());
            }
            SchemaChangeEvent::ExchangeTablePartition {
                global_table,
                original_partition,
                original_table,
            } => {
                self.update_global_stats_for_exchange_partition(
                    global_table,
                    original_partition,
                    original_table,
                )?;
                return Ok(());
            }
            SchemaChangeEvent::ReorganizePartition {
                global_table,
                added,
                dropped,
            } => {
                for definition in &added.definitions {
                    self.insert_stats_for_physical_id(global_table, definition.id)?;
                }
                for definition in &dropped.definitions {
                    self.delayed_delete_stats_for_physical_id(definition.id)?;
                }
                return Ok(());
            }
            SchemaChangeEvent::AlterTablePartitioning {
                old_single_table_id,
                global_table,
                added,
            } => {
                for definition in &added.definitions {
                    self.insert_stats_for_physical_id(global_table, definition.id)?;
                }
                // 普通表改分区表：全局统计 ID 从旧单表迁到新全局表。
                self.backend
                    .change_global_stats_id(*old_single_table_id, global_table.id)?;
                return Ok(());
            }
            SchemaChangeEvent::RemovePartitioning {
                old_table_id,
                new_single_table,
                dropped,
            } => {
                self.backend
                    .change_global_stats_id(*old_table_id, new_single_table.id)?;
                for definition in &dropped.definitions {
                    self.delayed_delete_stats_for_physical_id(definition.id)?;
                }
            }
            SchemaChangeEvent::FlashbackCluster => self.backend.update_all_stats_versions()?,
            // 加索引当前不改表级 stats_meta。
            SchemaChangeEvent::AddIndex => {}
            SchemaChangeEvent::DropSchema(tables) => {
                for table in tables {
                    for partition in &table.partitions {
                        let _ = self.delayed_delete_stats_for_physical_id(partition.id);
                    }
                    let _ = self.delayed_delete_stats_for_physical_id(table.id);
                }
            }
            SchemaChangeEvent::Unknown(action) => {
                return Err(Error(format!("unhandled schema change event: {action}")));
            }
        }
        Ok(())
    }

    /// 需要写入统计的物理 ID 列表：无分区则仅表 ID；动态裁剪时额外包含全局表 ID。
    pub fn physical_ids(&mut self, table: &TableInfo) -> Result<Vec<i64>, Error> {
        if table.partitions.is_empty() {
            return Ok(vec![table.id]);
        }
        let mut ids = table
            .partitions
            .iter()
            .map(|part| part.id)
            .collect::<Vec<_>>();
        if self.backend.prune_mode()? == PartitionPruneMode::Dynamic {
            ids.push(table.id);
        }
        Ok(ids)
    }

    /// 插入表级伪统计并按需记录历史元数据。
    fn insert_stats_for_physical_id(&mut self, table: &TableInfo, id: i64) -> Result<(), Error> {
        let start_ts = self.backend.insert_table_stats(table, id)?;
        self.record_historical_stats_meta(id, start_ts)
    }

    /// 插入列级伪统计并按需记录历史元数据。
    fn insert_stats_for_columns(&mut self, id: i64, columns: &[ColumnInfo]) -> Result<(), Error> {
        let start_ts = self.backend.insert_column_stats(id, columns)?;
        self.record_historical_stats_meta(id, start_ts)
    }

    /// 延迟删除：只推进 stats_meta 版本，真实清理由后续 GC 完成。
    fn delayed_delete_stats_for_physical_id(&mut self, id: i64) -> Result<(), Error> {
        let start_ts = self.backend.update_stats_meta_version(id)?;
        self.record_historical_stats_meta(id, start_ts)
    }

    /// 在启用历史统计且缓存已初始化时写入历史元数据。
    fn record_historical_stats_meta(&mut self, id: i64, start_ts: u64) -> Result<(), Error> {
        if start_ts == 0
            || !self.backend.historical_stats_enabled()?
            || !self.backend.cache_initialized(id)
        {
            return Ok(());
        }
        self.backend.record_historical_stats_meta(id, start_ts)
    }

    /// 删除分区后从全局行数扣减被删分区的 count。
    fn update_global_stats_for_drop_partition(
        &mut self,
        table: &TableInfo,
        dropped: &PartitionInfo,
    ) -> Result<(), Error> {
        let count = self.partition_count(dropped)?;
        if count == 0 {
            return Ok(());
        }
        let locked = self.backend.locked_tables()?.contains(&table.id);
        let start_ts = self.backend.start_ts()?;
        self.backend
            .apply_global_delta(table.id, start_ts, count, -count, locked)
    }

    /// 截断分区后扣减全局行数；modify_count 语义与 Go 一致由后端处理。
    fn update_global_stats_for_truncate_partition(
        &mut self,
        table: &TableInfo,
        dropped: &PartitionInfo,
    ) -> Result<(), Error> {
        let count = self.partition_count(dropped)?;
        if count == 0 {
            return Ok(());
        }
        let locked = self.backend.locked_tables()?.contains(&table.id);
        let start_ts = self.backend.start_ts()?;
        // Go deliberately leaves the global modify count unchanged here.
        // Go 侧此处故意不单独改全局 modify_count 语义，由 apply_global_delta 处理。
        self.backend
            .apply_global_delta(table.id, start_ts, count, -count, locked)
    }

    /// 汇总一组分区的行数。
    fn partition_count(&mut self, partitions: &PartitionInfo) -> Result<i64, Error> {
        let mut count = 0;
        for definition in &partitions.definitions {
            count += self
                .backend
                .stats_meta(definition.id)?
                .map_or(0, |meta| meta.0);
        }
        Ok(count)
    }

    /// 交换分区：用普通表与分区的 count/modify 差更新全局表。
    fn update_global_stats_for_exchange_partition(
        &mut self,
        global_table: &TableInfo,
        partition: &PartitionInfo,
        original_table: &TableInfo,
    ) -> Result<(), Error> {
        let definition = partition
            .definitions
            .first()
            .ok_or_else(|| Error("exchange partition has no definition".into()))?;
        let (partition_count, partition_modify) =
            self.backend.stats_meta(definition.id)?.unwrap_or((0, 0));
        let (table_count, table_modify) = self
            .backend
            .stats_meta(original_table.id)?
            .unwrap_or((0, 0));
        let count_delta = table_count - partition_count;
        let modify_delta = table_count + partition_count - partition_modify + table_modify;
        if count_delta == 0 && modify_delta == 0 {
            return Ok(());
        }
        update_stats_with_count_delta_and_modify_count_delta(
            &mut self.backend,
            global_table.id,
            count_delta,
            modify_delta,
        )
    }
}

/// 按 count/modify 增量更新表级 stats_meta；锁定表走允许为负的 upsert。
pub fn update_stats_with_count_delta_and_modify_count_delta(
    backend: &mut impl StatsBackend,
    table_id: i64,
    count_delta: i64,
    modify_count_delta: i64,
) -> Result<(), Error> {
    let locked = backend.locked_tables()?.contains(&table_id);
    let start_ts = backend.start_ts()?;
    if locked {
        // Locked-table rows are allowed to become negative, exactly like the Go upsert.
        // 锁定表允许 count/modify 变为负值，与 Go upsert 行为一致。
        return backend.update_locked_delta(table_id, start_ts, count_delta, modify_count_delta);
    }
    let (count, modify_count) = backend.stats_meta(table_id)?.unwrap_or((0, 0));
    backend.write_stats_meta(
        table_id,
        start_ts,
        (count + count_delta).max(0),
        (modify_count + modify_count_delta).max(0),
    )
}

/// schema 名缺失时日志中的占位字符串。
pub const SCHEMA_NOT_FOUND: &str = "Not Found";

/// 构造交换分区相关日志字段（键值对列表）。
pub fn exchange_partition_log_fields(
    backend: &impl StatsBackend,
    global_table: &TableInfo,
    partition: &PartitionDefinition,
    original_table: &TableInfo,
    count_delta: i64,
    modify_delta: i64,
    partition_count: i64,
    partition_modify: i64,
    table_count: i64,
    table_modify: i64,
) -> Vec<(String, String)> {
    vec![
        (
            "globalTableSchema".into(),
            backend
                .schema_name(global_table.id)
                .unwrap_or_else(|| SCHEMA_NOT_FOUND.into()),
        ),
        ("globalTableID".into(), global_table.id.to_string()),
        ("globalTableName".into(), global_table.name.clone()),
        ("countDelta".into(), count_delta.to_string()),
        ("modifyCountDelta".into(), modify_delta.to_string()),
        ("partitionID".into(), partition.id.to_string()),
        ("partitionName".into(), partition.name.clone()),
        ("partitionCount".into(), partition_count.to_string()),
        ("partitionModifyCount".into(), partition_modify.to_string()),
        ("tableID".into(), original_table.id.to_string()),
        ("tableName".into(), original_table.name.clone()),
        ("tableCount".into(), table_count.to_string()),
        ("tableModifyCount".into(), table_modify.to_string()),
    ]
}
