// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 表级 DDL 状态机与目录操作的可运行简化模型。
//
// 覆盖建表/删表（Public → WriteOnly → DeleteOnly → None 多阶段 schema 状态）、
// 恢复、截断、跨库重命名、外键引用修正，以及 auto_increment / TiFlash 副本 /
// placement / affinity / Region split policy 等表属性变更。

use std::collections::{BTreeMap, BTreeSet};

/// 表在在线 DDL 中的 schema 可见性状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableState {
    /// 对所有读写公开可见。
    Public,
    /// 仅写入阶段：旧数据只读，新写入走新 schema。
    WriteOnly,
    /// 仅删除阶段：只允许删除操作看到新 schema。
    DeleteOnly,
    /// 元数据已删除，表不再可见。
    None,
}

/// 外键引用的目标库表名。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForeignKeyReference {
    pub schema: String,
    pub table: String,
}

/// TiFlash 列存副本配置：副本数、位置标签与已就绪的物理分区。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TiFlashReplica {
    pub count: u64,
    pub location_labels: Vec<String>,
    pub available_partition_ids: BTreeSet<i64>,
}

/// 简化的表元信息，对应 Go `model.TableInfo` 中与表 DDL 相关的字段子集。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableInfo {
    pub id: i64,
    pub schema_id: i64,
    pub name: String,
    pub state: TableState,
    pub partition_ids: Vec<i64>,
    pub auto_increment_id: i64,
    pub auto_random_id: i64,
    pub auto_id_cache: u64,
    /// The original schema that still owns this table's auto-ID allocator.
    ///
    /// Go leaves this as zero while a table stays in its creation schema. A
    /// cross-schema rename records that schema ID, and moving back clears it.
    /// 仍持有本表 auto-ID 分配器的原始 schema；跨库 rename 时记录，迁回则清零。
    pub auto_id_schema_id: i64,
    pub shard_row_id_bits: u8,
    pub max_shard_row_id_bits: u8,
    pub comment: String,
    pub charset: String,
    pub collation: String,
    pub version: u64,
    pub foreign_keys: Vec<ForeignKeyReference>,
    pub tiflash_replica: Option<TiFlashReplica>,
    pub placement_policy: Option<String>,
    pub attributes: BTreeMap<String, String>,
    pub cached: bool,
    pub affinity: Option<String>,
    pub split_policy: Option<String>,
}

/// 表目录与属性变更过程中的错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TableError {
    AlreadyExists,
    NotFound,
    SchemaNotFound,
    NameTooLong,
    RecoveryConflict,
    InvalidAutoId,
    ShardBitsOverflow,
    InvalidCharsetCollation,
    InvalidReplicaCount,
    PartitionNotFound,
    InvalidVersion,
    InvalidPlacement,
    InvalidAffinity,
    GcSafePointTooNew,
}

/// 重命名语义：`RENAME TABLE` 与 `ALTER TABLE ... RENAME` 的校验差异。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameMode {
    RenameTable,
    AlterTable,
}

impl std::fmt::Display for TableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for TableError {}

/// 已删除但仍可被 recover 的表快照及其 drop 时间戳。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DroppedTable {
    pub table: TableInfo,
    pub drop_ts: u64,
}

/// GC（垃圾回收，清理 MVCC 历史版本）开关与 safe point 控制器。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GcController {
    pub enabled: bool,
    pub safe_point: u64,
}

impl GcController {
    /// 恢复前校验 snapshot 不早于 safe point，并临时关闭 GC；返回先前是否开启。
    pub fn disable_for_recovery(&mut self, snapshot_ts: u64) -> Result<bool, TableError> {
        if snapshot_ts < self.safe_point {
            return Err(TableError::GcSafePointTooNew);
        }
        let was_enabled = self.enabled;
        self.enabled = false;
        Ok(was_enabled)
    }
    /// 按恢复前记录恢复 GC 开关。
    pub fn restore(&mut self, was_enabled: bool) {
        self.enabled = was_enabled;
    }
}

/// 内存表目录：按 schema 存放表，并保留已删除表供 recover。
#[derive(Clone, Default)]
pub struct TableCatalog {
    schemas: BTreeMap<i64, BTreeMap<String, TableInfo>>,
    dropped: BTreeMap<i64, DroppedTable>,
}

impl TableCatalog {
    /// 创建空 schema；已存在则返回 false。
    pub fn create_schema(&mut self, schema_id: i64) -> bool {
        if self.schemas.contains_key(&schema_id) {
            return false;
        }
        self.schemas.insert(schema_id, BTreeMap::new());
        true
    }

    /// 删除 schema 并返回其中所有表。
    pub fn drop_schema(&mut self, schema_id: i64) -> Result<Vec<TableInfo>, TableError> {
        self.schemas
            .remove(&schema_id)
            .map(|tables| tables.into_values().collect())
            .ok_or(TableError::SchemaNotFound)
    }

    /// 判断 schema 是否存在。
    pub fn schema_exists(&self, schema_id: i64) -> bool {
        self.schemas.contains_key(&schema_id)
    }

    /// 列出 schema 下所有表名。
    pub fn table_names(&self, schema_id: i64) -> Result<Vec<String>, TableError> {
        let schema = self
            .schemas
            .get(&schema_id)
            .ok_or(TableError::SchemaNotFound)?;
        Ok(schema.values().map(|table| table.name.clone()).collect())
    }

    /// 插入表；同名已存在则报 AlreadyExists。
    pub fn insert(&mut self, table: TableInfo) -> Result<(), TableError> {
        let schema = self.schemas.entry(table.schema_id).or_default();
        let key = table.name.to_ascii_lowercase();
        if schema.contains_key(&key) {
            return Err(TableError::AlreadyExists);
        }
        schema.insert(key, table);
        Ok(())
    }

    /// 按 schema 与表名查找表（大小写不敏感）。
    pub fn get(&self, schema_id: i64, name: &str) -> Result<&TableInfo, TableError> {
        self.schemas
            .get(&schema_id)
            .and_then(|schema| schema.get(&name.to_ascii_lowercase()))
            .ok_or(TableError::NotFound)
    }

    /// 按 schema 与表名可变查找表。
    pub fn get_mut(&mut self, schema_id: i64, name: &str) -> Result<&mut TableInfo, TableError> {
        self.schemas
            .get_mut(&schema_id)
            .and_then(|schema| schema.get_mut(&name.to_ascii_lowercase()))
            .ok_or(TableError::NotFound)
    }

    /// 推进删表状态机一步；到达 None 时移入 dropped 并记录 drop_ts。
    pub fn drop_table_step(
        &mut self,
        schema_id: i64,
        name: &str,
        drop_ts: u64,
    ) -> Result<TableState, TableError> {
        let key = name.to_ascii_lowercase();
        let table = self
            .schemas
            .get_mut(&schema_id)
            .and_then(|schema| schema.get_mut(&key))
            .ok_or(TableError::NotFound)?;
        // Public → WriteOnly → DeleteOnly → None，与在线 DDL 删表状态机一致。
        let next = match table.state {
            TableState::Public => TableState::WriteOnly,
            TableState::WriteOnly => TableState::DeleteOnly,
            TableState::DeleteOnly | TableState::None => TableState::None,
        };
        table.state = next;
        if next == TableState::None {
            let table = self
                .schemas
                .get_mut(&schema_id)
                .unwrap()
                .remove(&key)
                .unwrap();
            self.dropped
                .insert(table.id, DroppedTable { table, drop_ts });
        }
        Ok(next)
    }

    /// 从 dropped 恢复表：临时关 GC，冲突时回滚 GC 状态。
    pub fn recover_table(
        &mut self,
        table_id: i64,
        gc: &mut GcController,
    ) -> Result<bool, TableError> {
        let dropped = self
            .dropped
            .get(&table_id)
            .ok_or(TableError::NotFound)?
            .clone();
        let was_enabled = gc.disable_for_recovery(dropped.drop_ts)?;
        let schema = self.schemas.entry(dropped.table.schema_id).or_default();
        let key = dropped.table.name.to_ascii_lowercase();
        // 目标名已存在则恢复失败，并还原 GC 开关。
        if schema.contains_key(&key) {
            gc.restore(was_enabled);
            return Err(TableError::RecoveryConflict);
        }
        let mut table = self.dropped.remove(&table_id).unwrap().table;
        table.state = TableState::Public;
        schema.insert(key, table);
        gc.restore(was_enabled);
        Ok(true)
    }

    /// 截断表：换新 table/partition ID，重置 auto ID 与 TiFlash 可用分区。
    pub fn truncate_table(
        &mut self,
        schema_id: i64,
        name: &str,
        new_table_id: i64,
        new_partition_ids: Vec<i64>,
    ) -> Result<Vec<i64>, TableError> {
        let table = self.get_mut(schema_id, name)?;
        let mut old_ids = vec![table.id];
        old_ids.extend(table.partition_ids.iter().copied());
        table.id = new_table_id;
        table.partition_ids = new_partition_ids;
        table.auto_increment_id = 0;
        table.auto_random_id = 0;
        if let Some(replica) = table.tiflash_replica.as_mut() {
            replica.available_partition_ids.clear();
        }
        table.version = table.version.saturating_add(1);
        Ok(old_ids)
    }

    /// 无额外校验的单表重命名（内部直接调用 `rename_table_inner`）。
    pub fn rename_table(
        &mut self,
        old_schema: i64,
        old_name: &str,
        new_schema: i64,
        new_name: &str,
    ) -> Result<(), TableError> {
        self.rename_table_inner(old_schema, old_name, new_schema, new_name)
    }

    /// 按 RenameMode 校验后再重命名；AlterTable 允许同库同名仅改大小写。
    pub fn rename_table_checked(
        &mut self,
        mode: RenameMode,
        old_schema: i64,
        old_name: &str,
        new_schema: i64,
        new_name: &str,
    ) -> Result<(), TableError> {
        if new_name.chars().count() > 64 {
            return Err(TableError::NameTooLong);
        }
        let old_key = old_name.to_ascii_lowercase();
        let new_key = new_name.to_ascii_lowercase();

        match mode {
            RenameMode::RenameTable => {
                if self
                    .schemas
                    .get(&new_schema)
                    .is_some_and(|destination| destination.contains_key(&new_key))
                {
                    return Err(TableError::AlreadyExists);
                }
                let source_exists = self
                    .schemas
                    .get(&old_schema)
                    .is_some_and(|schema| schema.contains_key(&old_key));
                if !source_exists {
                    return Err(TableError::NotFound);
                }
                if !self.schemas.contains_key(&new_schema) {
                    return Err(TableError::SchemaNotFound);
                }
            }
            RenameMode::AlterTable => {
                if !self
                    .schemas
                    .get(&old_schema)
                    .is_some_and(|schema| schema.contains_key(&old_key))
                {
                    return Err(TableError::NotFound);
                }
                let destination = self
                    .schemas
                    .get(&new_schema)
                    .ok_or(TableError::SchemaNotFound)?;
                // 同库仅大小写变化：就地改名并 bump version。
                if old_schema == new_schema && old_key == new_key {
                    let table = self.get_mut(old_schema, old_name)?;
                    table.name = new_name.to_owned();
                    table.version = table.version.saturating_add(1);
                    return Ok(());
                }
                if destination.contains_key(&new_key) {
                    return Err(TableError::AlreadyExists);
                }
            }
        }
        self.rename_table_inner(old_schema, old_name, new_schema, new_name)
    }

    /// 真正执行搬迁：维护 auto_id_schema_id，并修正其它表上的外键引用表名。
    fn rename_table_inner(
        &mut self,
        old_schema: i64,
        old_name: &str,
        new_schema: i64,
        new_name: &str,
    ) -> Result<(), TableError> {
        let old_key = old_name.to_ascii_lowercase();
        let new_key = new_name.to_ascii_lowercase();
        if self
            .schemas
            .get(&new_schema)
            .is_some_and(|schema| schema.contains_key(&new_key))
        {
            return Err(TableError::AlreadyExists);
        }
        let mut table = self
            .schemas
            .get_mut(&old_schema)
            .and_then(|schema| schema.remove(&old_key))
            .ok_or(TableError::NotFound)?;
        let old_table_name = table.name.clone();
        // 首次跨库搬走时记录原 schema 作为 auto-ID 归属；迁回则清零。
        if table.auto_id_schema_id == 0 && new_schema != old_schema {
            table.auto_id_schema_id = old_schema;
        }
        if new_schema == table.auto_id_schema_id {
            table.auto_id_schema_id = 0;
        }
        table.schema_id = new_schema;
        table.name = new_name.to_string();
        table.version = table.version.saturating_add(1);
        self.schemas
            .entry(new_schema)
            .or_default()
            .insert(new_key, table);
        // 同步更新其它表外键中对本表旧名的引用。
        for schema in self.schemas.values_mut() {
            for dependent in schema.values_mut() {
                for reference in &mut dependent.foreign_keys {
                    if reference.table.eq_ignore_ascii_case(&old_table_name) {
                        reference.table = new_name.to_string();
                    }
                }
            }
        }
        Ok(())
    }

    /// 批量重命名：先在克隆目录上试跑，成功后再提交，保证原子性。
    pub fn rename_tables(
        &mut self,
        renames: &[(i64, String, i64, String)],
    ) -> Result<(), TableError> {
        let mut staged = self.clone();
        staged.rename_tables_inner(renames)?;
        *self = staged;
        Ok(())
    }

    /// 批量重命名前校验源存在、目标 schema 存在且新名长度合法。
    pub fn rename_tables_checked(
        &mut self,
        renames: &[(i64, String, i64, String)],
    ) -> Result<(), TableError> {
        for (old_schema, old_name, _, _) in renames {
            if !self
                .schemas
                .get(old_schema)
                .is_some_and(|schema| schema.contains_key(&old_name.to_ascii_lowercase()))
            {
                return Err(TableError::NotFound);
            }
        }
        for (_, _, new_schema, new_name) in renames {
            if new_name.chars().count() > 64 {
                return Err(TableError::NameTooLong);
            }
            if !self.schema_exists(*new_schema) {
                return Err(TableError::SchemaNotFound);
            }
        }
        self.rename_tables(renames)
    }

    /// 批量搬迁实现：先全部取出再写回，并处理循环换名与外键引用。
    fn rename_tables_inner(
        &mut self,
        renames: &[(i64, String, i64, String)],
    ) -> Result<(), TableError> {
        let destinations: BTreeSet<(i64, String)> = renames
            .iter()
            .map(|(_, _, schema, name)| (*schema, name.to_ascii_lowercase()))
            .collect();
        // 目标 (schema, name) 不可重复。
        if destinations.len() != renames.len() {
            return Err(TableError::AlreadyExists);
        }
        for (_, _, schema, name) in renames {
            let is_source = renames.iter().any(|(source_schema, source_name, _, _)| {
                source_schema == schema && source_name.eq_ignore_ascii_case(name)
            });
            // 目标名若不是某次 rename 的源，则目录中不能已占用。
            if !is_source
                && self
                    .schemas
                    .get(schema)
                    .is_some_and(|tables| tables.contains_key(&name.to_ascii_lowercase()))
            {
                return Err(TableError::AlreadyExists);
            }
        }
        let mut moved = Vec::new();
        for (schema_id, name, _, _) in renames {
            let table = self
                .schemas
                .get_mut(schema_id)
                .and_then(|tables| tables.remove(&name.to_ascii_lowercase()))
                .ok_or(TableError::NotFound)?;
            moved.push(table);
        }
        for (mut table, (_, old_name, new_schema, new_name)) in moved.into_iter().zip(renames) {
            for schema in self.schemas.values_mut() {
                for dependent in schema.values_mut() {
                    for reference in &mut dependent.foreign_keys {
                        if reference.table.eq_ignore_ascii_case(old_name) {
                            reference.table = new_name.clone();
                        }
                    }
                }
            }
            if table.auto_id_schema_id == 0 && *new_schema != table.schema_id {
                table.auto_id_schema_id = table.schema_id;
            }
            if *new_schema == table.auto_id_schema_id {
                table.auto_id_schema_id = 0;
            }
            table.schema_id = *new_schema;
            table.name = new_name.clone();
            table.version = table.version.saturating_add(1);
            self.schemas
                .entry(*new_schema)
                .or_default()
                .insert(new_name.to_ascii_lowercase(), table);
        }
        Ok(())
    }
}

/// 调整 AUTO_INCREMENT 基准；非 force 时不得小于等于当前值。
pub fn rebase_auto_increment(
    table: &mut TableInfo,
    new_base: i64,
    force: bool,
) -> Result<bool, TableError> {
    if new_base < 0 {
        return Err(TableError::InvalidAutoId);
    }
    if !force && new_base <= table.auto_increment_id {
        return Ok(false);
    }
    table.auto_increment_id = new_base;
    Ok(true)
}

/// 调整 AUTO_RANDOM 基准；不得回退或为负。
pub fn rebase_auto_random(table: &mut TableInfo, new_base: i64) -> Result<bool, TableError> {
    if new_base < table.auto_random_id || new_base < 0 {
        return Err(TableError::InvalidAutoId);
    }
    let changed = new_base != table.auto_random_id;
    table.auto_random_id = new_base;
    Ok(changed)
}

/// 修改 auto ID 缓存大小，返回是否发生变化。
pub fn alter_auto_id_cache(table: &mut TableInfo, cache: u64) -> bool {
    let changed = table.auto_id_cache != cache;
    table.auto_id_cache = cache;
    changed
}

/// 修改 shard_row_id_bits（行 ID 高位分片位数，用于打散写入热点）。
///
/// 与 Go `onShardRowID` 一致：调低当前位数时保留历史最大值，只有调高时才推进该值。
pub fn alter_shard_row_id_bits(table: &mut TableInfo, bits: u8) -> Result<bool, TableError> {
    if bits > 15 {
        return Err(TableError::ShardBitsOverflow);
    }
    let changed = table.shard_row_id_bits != bits;
    table.shard_row_id_bits = bits;
    table.max_shard_row_id_bits = table.max_shard_row_id_bits.max(bits);
    Ok(changed)
}

/// 修改表注释。
pub fn alter_comment(table: &mut TableInfo, comment: impl Into<String>) -> bool {
    let comment = comment.into();
    let changed = table.comment != comment;
    table.comment = comment;
    changed
}

/// 修改字符集与校对规则；校对名须匹配字符集前缀（binary 特例除外）。
pub fn alter_charset_and_collation(
    table: &mut TableInfo,
    charset: &str,
    collation: &str,
) -> Result<bool, TableError> {
    let charset = charset.to_ascii_lowercase();
    let collation = collation.to_ascii_lowercase();
    if !collation.starts_with(&(charset.clone() + "_"))
        && !(charset == "binary" && collation == "binary")
    {
        return Err(TableError::InvalidCharsetCollation);
    }
    let changed = table.charset != charset || table.collation != collation;
    table.charset = charset;
    table.collation = collation;
    Ok(changed)
}

/// 设置或清除 TiFlash 副本；count 为 0 时移除配置。
pub fn set_tiflash_replica(
    table: &mut TableInfo,
    count: u64,
    location_labels: Vec<String>,
) -> Result<(), TableError> {
    if count == 0 {
        table.tiflash_replica = None;
        return Ok(());
    }
    // Go preserves the existing availability when changing a non-zero replica
    // count; only the explicit ResetAvailable job argument clears it.
    let available_partition_ids = table
        .tiflash_replica
        .take()
        .map(|replica| replica.available_partition_ids)
        .unwrap_or_default();
    table.tiflash_replica = Some(TiFlashReplica {
        count,
        location_labels,
        available_partition_ids,
    });
    Ok(())
}

/// 更新某个物理表/分区的 TiFlash 副本就绪状态。
pub fn update_tiflash_replica_status(
    table: &mut TableInfo,
    physical_id: i64,
    available: bool,
) -> Result<bool, TableError> {
    let replica = table
        .tiflash_replica
        .as_mut()
        .ok_or(TableError::InvalidReplicaCount)?;
    if physical_id != table.id && !table.partition_ids.contains(&physical_id) {
        return Err(TableError::PartitionNotFound);
    }
    Ok(if available {
        replica.available_partition_ids.insert(physical_id)
    } else {
        replica.available_partition_ids.remove(&physical_id)
    })
}

/// 单调推进表 version。
pub fn update_table_version(table: &mut TableInfo, version: u64) -> Result<bool, TableError> {
    if version < table.version {
        return Err(TableError::InvalidVersion);
    }
    let changed = version != table.version;
    table.version = version;
    Ok(changed)
}

/// 修改 placement policy（数据放置策略）；空字符串非法。
pub fn alter_placement(
    table: &mut TableInfo,
    placement: Option<String>,
) -> Result<bool, TableError> {
    if placement
        .as_ref()
        .is_some_and(|policy| policy.trim().is_empty())
    {
        return Err(TableError::InvalidPlacement);
    }
    let changed = table.placement_policy != placement;
    table.placement_policy = placement;
    Ok(changed)
}

/// 整体替换表 attributes。
pub fn alter_attributes(table: &mut TableInfo, attributes: BTreeMap<String, String>) -> bool {
    let changed = table.attributes != attributes;
    table.attributes = attributes;
    changed
}

/// 开启或关闭表缓存（cache table）。
pub fn alter_cache(table: &mut TableInfo, cached: bool) -> bool {
    let changed = table.cached != cached;
    table.cached = cached;
    changed
}

/// 修改 affinity（亲和性调度配置）；空字符串非法。
pub fn alter_affinity(table: &mut TableInfo, affinity: Option<String>) -> Result<bool, TableError> {
    if affinity
        .as_ref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(TableError::InvalidAffinity);
    }
    let changed = table.affinity != affinity;
    table.affinity = affinity;
    Ok(changed)
}

/// 修改 Region 预切分策略字符串。
pub fn alter_region_split_policy(table: &mut TableInfo, policy: Option<String>) -> bool {
    let changed = table.split_policy != policy;
    table.split_policy = policy;
    changed
}

/// 返回表的物理 ID 列表：无分区时为 table id，否则为各 partition id。
pub fn table_physical_ids(table: &TableInfo) -> Vec<i64> {
    if table.partition_ids.is_empty() {
        vec![table.id]
    } else {
        table.partition_ids.clone()
    }
}
