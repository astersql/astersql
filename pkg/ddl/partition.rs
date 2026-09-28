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

// 表分区（partition）元信息构建、校验与 DDL 辅助逻辑。
//
// 分区把一张逻辑表按 RANGE / LIST / HASH / KEY 等策略切成多个物理分片，
// 便于数据管理与裁剪。本模块负责：根据建表选项生成 `PartitionInfo`、
// 校验边界与列类型、增删截断重组分区、exchange（与普通表互换物理 ID）、
// TiFlash 副本可用性检查，以及把分区定义格式化为 SHOW CREATE 片段。

use std::collections::{BTreeMap, BTreeSet};

use crate::index::{ColumnInfo, IndexInfo};

/// RANGE 分区上界关键字 MAXVALUE 的字面量。
pub const PARTITION_MAX_VALUE: &str = "MAXVALUE";
/// 单表允许的最大分区数（与 MySQL/TiDB 上限对齐）。
pub const PARTITION_COUNT_LIMIT: usize = 8192;

/// 分区策略类型。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PartitionType {
    #[default]
    None,
    Range,
    List,
    Hash,
    Key,
}

/// 分区边界或 LIST 取值中的单个常量。
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum PartitionValue {
    MinValue,
    Null,
    Int(i64),
    UInt(u64),
    String(String),
    MaxValue,
    Default,
}

impl PartitionValue {
    /// 格式化为 SQL 字面量（含转义与非 ASCII 的 `_binary 0x...`）。
    fn sql(&self) -> String {
        match self {
            Self::MinValue => "MINVALUE".to_owned(),
            Self::Null => "NULL".to_owned(),
            Self::Int(value) => value.to_string(),
            Self::UInt(value) => value.to_string(),
            Self::String(value) if value.is_ascii() => {
                format!("'{}'", value.replace('\'', "''"))
            }
            Self::String(value) => format!("_binary 0x{}", encode_hex(value.as_bytes())),
            Self::MaxValue => PARTITION_MAX_VALUE.to_owned(),
            Self::Default => "DEFAULT".to_owned(),
        }
    }
}

/// 单个分区定义：名称、边界/取值、注释与放置策略。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartitionDefinition {
    pub id: i64,
    pub name: String,
    /// RANGE 的 VALUES LESS THAN 上界元组。
    pub less_than: Vec<PartitionValue>,
    /// LIST 的 VALUES IN 取值集合。
    pub in_values: Vec<Vec<PartitionValue>>,
    pub comment: String,
    pub placement_policy: Option<String>,
}

/// 表级分区元信息，含进行中的 adding/dropping 定义（DDL 状态机中间态）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartitionInfo {
    pub partition_type: PartitionType,
    pub enabled: bool,
    pub expression: String,
    pub columns: Vec<String>,
    /// KEY 分区未显式指定列时，从主键推导后置为 true。
    pub empty_columns: bool,
    pub number: usize,
    pub definitions: Vec<PartitionDefinition>,
    pub adding_definitions: Vec<PartitionDefinition>,
    pub dropping_definitions: Vec<PartitionDefinition>,
    pub new_table_id: Option<i64>,
}

/// 临时表类型；临时表不允许建分区。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TemporaryTableType {
    #[default]
    None,
    Local,
    Global,
}

/// 带分区元信息的表快照，供本模块各校验/变更函数使用。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartitionedTableInfo {
    pub id: i64,
    pub name: String,
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
    pub partition: Option<PartitionInfo>,
    pub temporary_type: TemporaryTableType,
    /// 已确认 TiFlash 副本可用的分区物理 ID 列表。
    pub tiflash_available_partition_ids: Vec<i64>,
}

/// 分区相关错误码集合。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PartitionError {
    UnsupportedType,
    SubpartitionUnsupported,
    NoPartitions,
    TooManyPartitions,
    DuplicatePartitionName(String),
    DuplicatePartitionColumn(String),
    PartitionColumnNotFound(String),
    InvalidPartitionColumnType(String),
    WrongValues,
    RangeNotIncreasing,
    MaxValueNotLast,
    DuplicateListValue(Vec<PartitionValue>),
    DefaultPartitionNotLast,
    PartitionNotFound(String),
    DropAllPartitions,
    TemporaryTablePartition,
    UniqueKeyMissingPartitionColumns(String),
    PartialIndexUnsupported,
    PlacementPolicyMismatch,
    ExchangeSchemaMismatch,
    InvalidExpression,
    ReplicaUnavailable,
    IdCountMismatch,
}

/// CREATE TABLE ... PARTITION BY ... 的解析结果选项。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartitionOptions {
    pub partition_type: PartitionType,
    pub expression: String,
    pub columns: Vec<String>,
    pub number: usize,
    pub definitions: Vec<PartitionDefinition>,
    pub has_subpartition: bool,
    pub linear: bool,
}

/// 根据建表分区选项填充 `table.partition`，返回警告列表（如忽略 LINEAR）。
pub fn build_table_partition_info(
    table: &mut PartitionedTableInfo,
    options: Option<PartitionOptions>,
) -> Result<Vec<String>, PartitionError> {
    let Some(options) = options else {
        return Ok(Vec::new());
    };
    if options.has_subpartition
        && matches!(
            options.partition_type,
            PartitionType::Hash | PartitionType::Key
        )
    {
        return Err(PartitionError::SubpartitionUnsupported);
    }
    let mut warnings = Vec::new();
    if options.linear {
        warnings.push("LINEAR partitioning is treated as non-linear".to_owned());
    }
    if options.partition_type == PartitionType::None {
        warnings.push("unsupported partition type; table remains unpartitioned".to_owned());
        return Ok(warnings);
    }
    check_add_partition_on_temporary_mode(table)?;
    check_partition_count(options.number.max(options.definitions.len()))?;
    let mut info = PartitionInfo {
        partition_type: options.partition_type,
        enabled: true,
        expression: options.expression,
        columns: options.columns,
        empty_columns: false,
        number: options.number,
        ..PartitionInfo::default()
    };
    // KEY 未指定列时回退到主键列。
    if info.partition_type == PartitionType::Key && info.columns.is_empty() {
        if let Some(primary) = table.indexes.iter().find(|index| index.primary) {
            info.columns = primary
                .columns
                .iter()
                .map(|column| column.name.clone())
                .collect();
            info.empty_columns = true;
        }
    }
    check_partition_columns(table, &info)?;
    info.definitions = build_partition_definitions(&info, options.definitions)?;
    check_partition_definition_constraints(&info)?;
    check_partition_keys_constraints(table, &info)?;
    if table.indexes.iter().any(|index| index.condition.is_some()) {
        return Err(PartitionError::PartialIndexUnsupported);
    }
    info.number = info.definitions.len();
    table.partition = Some(info);
    Ok(warnings)
}

/// 按分区类型补齐或校验定义列表（HASH/KEY 可按 number 自动生成 p0..pn）。
pub fn build_partition_definitions(
    info: &PartitionInfo,
    definitions: Vec<PartitionDefinition>,
) -> Result<Vec<PartitionDefinition>, PartitionError> {
    let result = match info.partition_type {
        PartitionType::Hash | PartitionType::Key => {
            if info.number == 0 {
                return Err(PartitionError::NoPartitions);
            }
            let mut result = Vec::with_capacity(info.number);
            for index in 0..info.number {
                result.push(definitions.get(index).cloned().unwrap_or_else(|| {
                    PartitionDefinition {
                        name: format!("p{index}"),
                        ..PartitionDefinition::default()
                    }
                }));
            }
            result
        }
        PartitionType::Range => {
            if definitions.is_empty() {
                return Err(PartitionError::NoPartitions);
            }
            for definition in &definitions {
                if definition.less_than.is_empty() || !definition.in_values.is_empty() {
                    return Err(PartitionError::WrongValues);
                }
            }
            definitions
        }
        PartitionType::List => {
            if definitions.is_empty() {
                return Err(PartitionError::NoPartitions);
            }
            for definition in &definitions {
                if !definition.less_than.is_empty() {
                    return Err(PartitionError::WrongValues);
                }
            }
            definitions
        }
        PartitionType::None => return Err(PartitionError::UnsupportedType),
    };
    check_partition_names_unique(&result)?;
    Ok(result)
}

/// 按类型校验分区定义的取值约束（RANGE 递增、LIST 唯一等）。
pub fn check_partition_definition_constraints(info: &PartitionInfo) -> Result<(), PartitionError> {
    check_partition_names_unique(&info.definitions)?;
    match info.partition_type {
        PartitionType::Range => check_range_partition_values(&info.definitions),
        PartitionType::List => check_list_partition_values(&info.definitions),
        PartitionType::Hash | PartitionType::Key => {
            if info.definitions.is_empty() {
                Err(PartitionError::NoPartitions)
            } else {
                Ok(())
            }
        }
        PartitionType::None => Ok(()),
    }
}

/// 检查 RANGE 上界严格递增，且 MAXVALUE 只能出现在最后一个分区。
pub fn check_range_partition_values(
    definitions: &[PartitionDefinition],
) -> Result<(), PartitionError> {
    let mut previous: Option<&[PartitionValue]> = None;
    for (index, definition) in definitions.iter().enumerate() {
        if definition
            .less_than
            .iter()
            .any(|value| *value == PartitionValue::Null)
        {
            return Err(PartitionError::WrongValues);
        }
        if definition
            .less_than
            .iter()
            .any(|value| *value == PartitionValue::MaxValue)
            && index + 1 != definitions.len()
        {
            return Err(PartitionError::MaxValueNotLast);
        }
        // 相邻分区上界必须严格小于关系（字典序比较元组）。
        if let Some(previous) = previous {
            if compare_partition_tuple(previous, &definition.less_than) != std::cmp::Ordering::Less
            {
                return Err(PartitionError::RangeNotIncreasing);
            }
        }
        previous = Some(&definition.less_than);
    }
    Ok(())
}

/// 检查 LIST 取值不重复，DEFAULT 分区必须位于最后且唯一。
pub fn check_list_partition_values(
    definitions: &[PartitionDefinition],
) -> Result<(), PartitionError> {
    let mut values = BTreeSet::new();
    let mut default_seen = false;
    for (definition_index, definition) in definitions.iter().enumerate() {
        for tuple in &definition.in_values {
            if tuple.len() == 1 && tuple[0] == PartitionValue::Default {
                if default_seen || definition_index + 1 != definitions.len() {
                    return Err(PartitionError::DefaultPartitionNotLast);
                }
                default_seen = true;
                continue;
            }
            if !values.insert(tuple.clone()) {
                return Err(PartitionError::DuplicateListValue(tuple.clone()));
            }
        }
    }
    Ok(())
}

/// 字典序比较两个分区边界元组。
fn compare_partition_tuple(
    left: &[PartitionValue],
    right: &[PartitionValue],
) -> std::cmp::Ordering {
    for (left, right) in left.iter().zip(right) {
        let order = left.cmp(right);
        if order != std::cmp::Ordering::Equal {
            return order;
        }
    }
    left.len().cmp(&right.len())
}

/// 分区名在忽略大小写后必须唯一。
pub fn check_partition_names_unique(
    definitions: &[PartitionDefinition],
) -> Result<(), PartitionError> {
    let mut names = BTreeSet::new();
    for definition in definitions {
        if !names.insert(definition.name.to_lowercase()) {
            return Err(PartitionError::DuplicatePartitionName(
                definition.name.clone(),
            ));
        }
    }
    Ok(())
}

/// 新增分区名不得与已有分区冲突。
pub fn check_add_partition_name_unique(
    existing: &[PartitionDefinition],
    adding: &[PartitionDefinition],
) -> Result<(), PartitionError> {
    let mut names: BTreeSet<String> = existing
        .iter()
        .map(|definition| definition.name.to_lowercase())
        .collect();
    for definition in adding {
        if !names.insert(definition.name.to_lowercase()) {
            return Err(PartitionError::DuplicatePartitionName(
                definition.name.clone(),
            ));
        }
    }
    Ok(())
}

/// 校验 ADD PARTITION 的名称与取值是否可拼接到现有分区定义上。
pub fn check_add_partition_value(
    existing: &PartitionInfo,
    adding: &PartitionInfo,
) -> Result<(), PartitionError> {
    check_add_partition_name_unique(&existing.definitions, &adding.definitions)?;
    match existing.partition_type {
        PartitionType::Range => {
            let Some(last) = existing.definitions.last() else {
                return Ok(());
            };
            // 已有 MAXVALUE 分区时无法再追加 RANGE 分区。
            if last
                .less_than
                .iter()
                .any(|value| *value == PartitionValue::MaxValue)
            {
                return Err(PartitionError::MaxValueNotLast);
            }
            let mut combined = existing.definitions.clone();
            combined.extend(adding.definitions.clone());
            check_range_partition_values(&combined)
        }
        PartitionType::List => {
            let mut combined = existing.definitions.clone();
            combined.extend(adding.definitions.clone());
            check_list_partition_values(&combined)
        }
        PartitionType::Hash | PartitionType::Key => Ok(()),
        PartitionType::None => Err(PartitionError::UnsupportedType),
    }
}

/// 将新分区定义追加到表上，返回新增分区物理 ID。
pub fn add_table_partitions(
    table: &mut PartitionedTableInfo,
    adding: PartitionInfo,
) -> Result<Vec<i64>, PartitionError> {
    let existing = table
        .partition
        .as_mut()
        .ok_or(PartitionError::UnsupportedType)?;
    check_add_partition_value(existing, &adding)?;
    let ids = adding
        .definitions
        .iter()
        .map(|definition| definition.id)
        .collect();
    existing.adding_definitions = adding.definitions.clone();
    existing.definitions.extend(adding.definitions);
    existing.number = existing.definitions.len();
    existing.adding_definitions.clear();
    Ok(ids)
}

/// 解析待删除分区下标；禁止一次删光全部剩余分区。
pub fn check_drop_table_partition(
    info: &PartitionInfo,
    names: &[String],
) -> Result<Vec<usize>, PartitionError> {
    let requested: BTreeSet<String> = names.iter().map(|name| name.to_lowercase()).collect();
    let found: Vec<usize> = info
        .definitions
        .iter()
        .enumerate()
        .filter(|(_, definition)| requested.contains(&definition.name.to_lowercase()))
        .map(|(index, _)| index)
        .collect();
    if found.len() != requested.len() {
        let missing = requested
            .into_iter()
            .find(|name| {
                !info
                    .definitions
                    .iter()
                    .any(|definition| definition.name.eq_ignore_ascii_case(name))
            })
            .unwrap_or_default();
        return Err(PartitionError::PartitionNotFound(missing));
    }
    if found.len() == info.definitions.len() {
        return Err(PartitionError::DropAllPartitions);
    }
    Ok(found)
}

/// 按名删除分区定义，并同步清理 TiFlash 可用 ID。
pub fn drop_table_partitions(
    table: &mut PartitionedTableInfo,
    names: &[String],
) -> Result<Vec<PartitionDefinition>, PartitionError> {
    let info = table
        .partition
        .as_mut()
        .ok_or(PartitionError::UnsupportedType)?;
    check_drop_table_partition(info, names)?;
    let names: BTreeSet<String> = names.iter().map(|name| name.to_lowercase()).collect();
    let mut dropped = Vec::new();
    info.definitions.retain(|definition| {
        if names.contains(&definition.name.to_lowercase()) {
            dropped.push(definition.clone());
            false
        } else {
            true
        }
    });
    info.dropping_definitions = dropped.clone();
    info.number = info.definitions.len();
    remove_tiflash_available_partition_ids(
        table,
        &dropped
            .iter()
            .map(|definition| definition.id)
            .collect::<Vec<_>>(),
    );
    Ok(dropped)
}

/// 截断分区：用新物理 ID 替换旧 ID，返回 (旧定义, 新定义) 对。
pub fn truncate_table_partitions(
    table: &mut PartitionedTableInfo,
    old_ids: &[i64],
    new_ids: &[i64],
) -> Result<(Vec<PartitionDefinition>, Vec<PartitionDefinition>), PartitionError> {
    if old_ids.len() != new_ids.len() {
        return Err(PartitionError::IdCountMismatch);
    }
    let info = table
        .partition
        .as_mut()
        .ok_or(PartitionError::UnsupportedType)?;
    let replacement: BTreeMap<i64, i64> = old_ids
        .iter()
        .copied()
        .zip(new_ids.iter().copied())
        .collect();
    let mut old = Vec::new();
    let mut new = Vec::new();
    for definition in &mut info.definitions {
        if let Some(new_id) = replacement.get(&definition.id) {
            old.push(definition.clone());
            definition.id = *new_id;
            new.push(definition.clone());
        }
    }
    if old.len() != old_ids.len() {
        return Err(PartitionError::PartitionNotFound(String::new()));
    }
    remove_tiflash_available_partition_ids(table, old_ids);
    Ok((old, new))
}

/// 重组分区：删除指定分区并插入新定义，整体重新校验约束。
pub fn reorganize_partitions(
    table: &mut PartitionedTableInfo,
    dropped_names: &[String],
    adding: Vec<PartitionDefinition>,
) -> Result<(Vec<PartitionDefinition>, Vec<PartitionDefinition>), PartitionError> {
    let info = table
        .partition
        .as_mut()
        .ok_or(PartitionError::UnsupportedType)?;
    let drop_set: BTreeSet<String> = dropped_names
        .iter()
        .map(|name| name.to_lowercase())
        .collect();
    let dropped: Vec<PartitionDefinition> = info
        .definitions
        .iter()
        .filter(|definition| drop_set.contains(&definition.name.to_lowercase()))
        .cloned()
        .collect();
    if dropped.len() != drop_set.len() {
        return Err(PartitionError::PartitionNotFound(dropped_names.join(",")));
    }
    let remaining: Vec<PartitionDefinition> = info
        .definitions
        .iter()
        .filter(|definition| !drop_set.contains(&definition.name.to_lowercase()))
        .cloned()
        .collect();
    check_add_partition_name_unique(&remaining, &adding)?;
    // 先在候选定义上完整校验，再写回正式 definitions。
    let mut candidate = remaining;
    candidate.extend(adding.clone());
    let validation = PartitionInfo {
        definitions: candidate.clone(),
        ..info.clone()
    };
    check_partition_definition_constraints(&validation)?;
    info.dropping_definitions = dropped.clone();
    info.adding_definitions = adding.clone();
    info.definitions = candidate;
    info.number = info.definitions.len();
    info.dropping_definitions.clear();
    info.adding_definitions.clear();
    Ok((dropped, adding))
}

/// 交换分区与普通表的物理 ID；需 schema 一致且行数据通过分区边界校验。
pub fn exchange_table_partition(
    partitioned: &mut PartitionedTableInfo,
    standalone: &mut PartitionedTableInfo,
    partition_name: &str,
    validate_records: impl Fn(i64, &PartitionDefinition) -> bool,
) -> Result<(), PartitionError> {
    check_exchange_partition_schema(partitioned, standalone)?;
    let info = partitioned
        .partition
        .as_mut()
        .ok_or(PartitionError::UnsupportedType)?;
    let definition = info
        .definitions
        .iter_mut()
        .find(|definition| definition.name.eq_ignore_ascii_case(partition_name))
        .ok_or_else(|| PartitionError::PartitionNotFound(partition_name.to_owned()))?;
    if definition.placement_policy.is_some()
        && definition.placement_policy
            != standalone
                .partition
                .as_ref()
                .and_then(|info| info.definitions.first())
                .and_then(|definition| definition.placement_policy.clone())
    {
        return Err(PartitionError::PlacementPolicyMismatch);
    }
    if !validate_records(standalone.id, definition) {
        return Err(PartitionError::ExchangeSchemaMismatch);
    }
    std::mem::swap(&mut definition.id, &mut standalone.id);
    Ok(())
}

/// 校验交换双方的列/索引结构兼容（普通表不得已有分区）。
pub fn check_exchange_partition_schema(
    partitioned: &PartitionedTableInfo,
    standalone: &PartitionedTableInfo,
) -> Result<(), PartitionError> {
    if standalone.partition.is_some()
        || partitioned.columns.len() != standalone.columns.len()
        || partitioned.indexes.len() != standalone.indexes.len()
    {
        return Err(PartitionError::ExchangeSchemaMismatch);
    }
    for (left, right) in partitioned.columns.iter().zip(&standalone.columns) {
        if !left.name.eq_ignore_ascii_case(&right.name)
            || left.column_type != right.column_type
            || left.generated != right.generated
            || left.stored != right.stored
        {
            return Err(PartitionError::ExchangeSchemaMismatch);
        }
    }
    for (left, right) in partitioned.indexes.iter().zip(&standalone.indexes) {
        if left.unique != right.unique
            || left.primary != right.primary
            || left.kind != right.kind
            || left.columns.len() != right.columns.len()
            || left
                .columns
                .iter()
                .zip(&right.columns)
                .any(|(left, right)| !left.name.eq_ignore_ascii_case(&right.name))
        {
            return Err(PartitionError::ExchangeSchemaMismatch);
        }
    }
    Ok(())
}

/// 检查新增分区的副本是否达到期望数量；返回 true 表示尚需等待。
pub fn check_partition_replica(
    replica_count: u64,
    adding: &[PartitionDefinition],
    available_replicas: &BTreeMap<i64, u64>,
) -> Result<bool, PartitionError> {
    if replica_count == 0 {
        return Ok(false);
    }
    for definition in adding {
        match available_replicas.get(&definition.id) {
            Some(count) if *count >= replica_count => {}
            Some(_) => return Ok(true),
            None => return Err(PartitionError::ReplicaUnavailable),
        }
    }
    Ok(false)
}

/// 判断 peer 列表中是否至少有一个 TiFlash 存储节点。
pub fn check_tiflash_peer_store_at_least_one(
    tiflash_store_ids: &BTreeSet<u64>,
    peer_store_ids: &[u64],
) -> bool {
    peer_store_ids
        .iter()
        .any(|store_id| tiflash_store_ids.contains(store_id))
}

/// 从「TiFlash 可用分区 ID」列表中移除指定 ID。
pub fn remove_tiflash_available_partition_ids(table: &mut PartitionedTableInfo, ids: &[i64]) {
    let ids: BTreeSet<i64> = ids.iter().copied().collect();
    table
        .tiflash_available_partition_ids
        .retain(|id| !ids.contains(id));
}

/// 校验分区列存在、不重复且类型允许作为分区键。
pub fn check_partition_columns(
    table: &PartitionedTableInfo,
    info: &PartitionInfo,
) -> Result<(), PartitionError> {
    let mut names = BTreeSet::new();
    for name in &info.columns {
        if !names.insert(name.to_lowercase()) {
            return Err(PartitionError::DuplicatePartitionColumn(name.clone()));
        }
        let column = table
            .columns
            .iter()
            .find(|column| column.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| PartitionError::PartitionColumnNotFound(name.clone()))?;
        if !is_column_type_allowed_as_partitioning_column(info.partition_type, column) {
            return Err(PartitionError::InvalidPartitionColumnType(name.clone()));
        }
    }
    Ok(())
}

/// 按分区类型判断列类型是否可用作分区键（如 KEY 禁止 Blob/JSON 等）。
pub fn is_column_type_allowed_as_partitioning_column(
    partition_type: PartitionType,
    column: &ColumnInfo,
) -> bool {
    use crate::index::ColumnType;
    match partition_type {
        PartitionType::Key => !matches!(
            column.column_type,
            ColumnType::Blob | ColumnType::Text | ColumnType::Json | ColumnType::Vector
        ),
        PartitionType::Range | PartitionType::List if column.generated && !column.stored => false,
        PartitionType::Range | PartitionType::List => matches!(
            column.column_type,
            ColumnType::TinyInt
                | ColumnType::SmallInt
                | ColumnType::MediumInt
                | ColumnType::Int
                | ColumnType::BigInt
                | ColumnType::Date
                | ColumnType::DateTime
                | ColumnType::Timestamp
                | ColumnType::Char(_)
                | ColumnType::VarChar(_)
                | ColumnType::Binary(_)
                | ColumnType::VarBinary(_)
        ),
        PartitionType::Hash => matches!(
            column.column_type,
            ColumnType::TinyInt
                | ColumnType::SmallInt
                | ColumnType::MediumInt
                | ColumnType::Int
                | ColumnType::BigInt
                | ColumnType::Date
                | ColumnType::DateTime
                | ColumnType::Timestamp
                | ColumnType::Duration
                | ColumnType::Year
        ),
        PartitionType::None => true,
    }
}

/// 非全局唯一索引必须包含全部分区列（MySQL 分区表约束）。
pub fn check_partition_keys_constraints(
    table: &PartitionedTableInfo,
    info: &PartitionInfo,
) -> Result<(), PartitionError> {
    let partition_columns = extract_partition_columns(info, table)?;
    for index in &table.indexes {
        if index.unique && !index.global {
            let index_columns: BTreeSet<String> = index
                .columns
                .iter()
                .map(|column| column.name.to_lowercase())
                .collect();
            if partition_columns
                .iter()
                .any(|column| !index_columns.contains(&column.to_lowercase()))
            {
                return Err(PartitionError::UniqueKeyMissingPartitionColumns(
                    index.name.clone(),
                ));
            }
        }
    }
    Ok(())
}

/// 取出分区列：优先用显式 columns，否则从表达式中启发式解析标识符。
pub fn extract_partition_columns(
    info: &PartitionInfo,
    table: &PartitionedTableInfo,
) -> Result<Vec<String>, PartitionError> {
    if !info.columns.is_empty() {
        return Ok(info.columns.clone());
    }
    let expression = info.expression.to_lowercase();
    let mut result = Vec::new();
    for column in &table.columns {
        let name = column.name.to_lowercase();
        if expression
            .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            .any(|token| token == name)
        {
            result.push(column.name.clone());
        }
    }
    if result.is_empty() && !expression.trim().is_empty() {
        return Err(PartitionError::InvalidExpression);
    }
    Ok(result)
}

/// 按等间隔生成 RANGE 分区定义（用于 INTERVAL 语法展开）。
pub fn generate_partition_definitions_from_interval(
    start: i64,
    end: i64,
    interval: i64,
    first_id: i64,
) -> Result<Vec<PartitionDefinition>, PartitionError> {
    if interval <= 0 || start >= end {
        return Err(PartitionError::WrongValues);
    }
    let mut definitions = Vec::new();
    let mut bound = start.saturating_add(interval);
    while bound < end {
        let index = definitions.len();
        definitions.push(PartitionDefinition {
            id: first_id.saturating_add(index as i64),
            name: format!("p{index}"),
            less_than: vec![PartitionValue::Int(bound)],
            ..PartitionDefinition::default()
        });
        if definitions.len() >= PARTITION_COUNT_LIMIT {
            return Err(PartitionError::TooManyPartitions);
        }
        let next = bound.saturating_add(interval);
        if next <= bound {
            return Err(PartitionError::WrongValues);
        }
        bound = next;
    }
    definitions.push(PartitionDefinition {
        id: first_id.saturating_add(definitions.len() as i64),
        name: format!("p{}", definitions.len()),
        less_than: vec![PartitionValue::Int(end)],
        ..PartitionDefinition::default()
    });
    Ok(definitions)
}

/// 拒绝含非确定性函数、子查询等非法分区表达式。
pub fn check_partition_expression_allowed(expression: &str) -> Result<(), PartitionError> {
    let lower = expression.to_lowercase();
    let forbidden = [
        "select",
        "rand(",
        "uuid(",
        "now(",
        "current_timestamp",
        "window",
        "over(",
        "@",
    ];
    if expression.trim().is_empty()
        || forbidden.iter().any(|token| lower.contains(token))
        || lower.contains("/")
    {
        return Err(PartitionError::InvalidExpression);
    }
    Ok(())
}

/// 为 RANGE 某分区构造 exchange validation 的违规行条件及绑定参数。
pub fn build_check_condition_for_range(
    info: &PartitionInfo,
    index: usize,
) -> Result<(String, Vec<PartitionValue>), PartitionError> {
    let definition = info
        .definitions
        .get(index)
        .ok_or_else(|| PartitionError::PartitionNotFound(index.to_string()))?;
    let expression = if info.columns.is_empty() {
        info.expression.clone()
    } else {
        format!("({})", info.columns.join(","))
    };
    let null_check = if info.columns.is_empty() {
        format!("{expression} IS NULL")
    } else {
        info.columns
            .iter()
            .map(|column| format!("{column} IS NULL"))
            .collect::<Vec<_>>()
            .join(" OR ")
    };
    let upper_is_max = definition
        .less_than
        .iter()
        .all(|value| *value == PartitionValue::MaxValue);
    if index == 0 {
        if upper_is_max {
            Ok(("FALSE".to_owned(), Vec::new()))
        } else {
            Ok((
                format!(
                    "{expression} >= {}",
                    tuple_placeholders(&definition.less_than)
                ),
                definition.less_than.clone(),
            ))
        }
    } else {
        let lower_values = info.definitions[index - 1].less_than.clone();
        let lower = tuple_placeholders(&lower_values);
        let mut values = lower_values;
        if upper_is_max {
            Ok((format!("{expression} < {lower} OR {null_check}"), values))
        } else {
            let upper = tuple_placeholders(&definition.less_than);
            values.extend(definition.less_than.clone());
            Ok((
                format!("{expression} < {lower} OR {expression} >= {upper} OR {null_check}"),
                values,
            ))
        }
    }
}

/// 为 LIST 某分区构造 exchange validation 的 null-safe 违规行条件。
pub fn build_check_condition_for_list(
    info: &PartitionInfo,
    index: usize,
) -> Result<String, PartitionError> {
    let definition = info
        .definitions
        .get(index)
        .ok_or_else(|| PartitionError::PartitionNotFound(index.to_string()))?;
    if definition
        .in_values
        .iter()
        .any(|tuple| tuple == &[PartitionValue::Default])
    {
        let preceding: Vec<String> = info
            .definitions
            .iter()
            .take(index)
            .flat_map(|definition| &definition.in_values)
            .map(|tuple| list_tuple_match(info, tuple))
            .collect();
        return Ok(if preceding.is_empty() {
            "FALSE".to_owned()
        } else {
            preceding.join(" OR ")
        });
    }
    let matches = definition
        .in_values
        .iter()
        .map(|tuple| list_tuple_match(info, tuple))
        .collect::<Vec<_>>()
        .join(" OR ");
    Ok(format!("NOT ({matches})"))
}

fn list_tuple_match(info: &PartitionInfo, tuple: &[PartitionValue]) -> String {
    if info.columns.is_empty() {
        tuple
            .iter()
            .map(|value| format!("({}) <=> {}", info.expression, value.sql()))
            .collect::<Vec<_>>()
            .join(" AND ")
    } else {
        info.columns
            .iter()
            .zip(tuple)
            .map(|(column, value)| format!("`{}` <=> {}", column.replace('`', "``"), value.sql()))
            .collect::<Vec<_>>()
            .join(" AND ")
    }
}

/// 生成占位符元组字符串，如 `?` 或 `(?,?,?)`。
fn tuple_placeholders(values: &[PartitionValue]) -> String {
    if values.len() == 1 {
        "?".to_owned()
    } else {
        format!("({})", vec!["?"; values.len()].join(","))
    }
}

/// 将分区信息格式化为 CREATE TABLE 尾部的 PARTITION BY 子句。
pub fn append_partition_info(info: Option<&PartitionInfo>) -> String {
    let Some(info) = info else {
        return String::new();
    };
    // HASH/KEY 且全是默认名 pN、无注释/策略时，使用简写 PARTITIONS n 形式。
    let default_definitions = matches!(
        info.partition_type,
        PartitionType::Hash | PartitionType::Key
    ) && info.definitions.iter().enumerate().all(
        |(index, definition)| {
            definition.name == format!("p{index}")
                && definition.comment.is_empty()
                && definition.placement_policy.is_none()
        },
    );
    if default_definitions {
        return match info.partition_type {
            PartitionType::Hash => {
                format!(
                    "\nPARTITION BY HASH ({}) PARTITIONS {}",
                    info.expression, info.number
                )
            }
            PartitionType::Key => format!(
                "\nPARTITION BY KEY ({}) PARTITIONS {}",
                if info.empty_columns {
                    String::new()
                } else {
                    info.columns.join(",")
                },
                info.number
            ),
            _ => unreachable!(),
        };
    }
    let head = if !info.columns.is_empty() {
        let columns = if info.empty_columns {
            String::new()
        } else {
            info.columns.join(",")
        };
        match info.partition_type {
            PartitionType::Key => format!("\nPARTITION BY KEY ({columns})\n("),
            PartitionType::Range => format!("\nPARTITION BY RANGE COLUMNS({columns})\n("),
            PartitionType::List => format!("\nPARTITION BY LIST COLUMNS({columns})\n("),
            _ => format!("\nPARTITION BY {:?} ({columns})\n(", info.partition_type),
        }
    } else {
        format!(
            "\nPARTITION BY {} ({})\n(",
            partition_type_name(info.partition_type),
            info.expression
        )
    };
    format!("{head}{})", append_partition_definitions(info))
}

/// 逐个分区定义格式化为 `PARTITION \`name\` VALUES ...` 列表。
pub fn append_partition_definitions(info: &PartitionInfo) -> String {
    info.definitions
        .iter()
        .map(|definition| {
            let mut output = format!("PARTITION `{}`", definition.name.replace('`', "``"));
            match info.partition_type {
                PartitionType::Range => output.push_str(&format!(
                    " VALUES LESS THAN ({})",
                    definition
                        .less_than
                        .iter()
                        .map(PartitionValue::sql)
                        .collect::<Vec<_>>()
                        .join(",")
                )),
                PartitionType::List => {
                    if definition.in_values.is_empty()
                        || definition
                            .in_values
                            .iter()
                            .any(|tuple| tuple == &[PartitionValue::Default])
                    {
                        output.push_str(" DEFAULT");
                    } else {
                        let values = definition
                            .in_values
                            .iter()
                            .map(|tuple| {
                                if tuple.len() == 1 {
                                    tuple[0].sql()
                                } else {
                                    format!(
                                        "({})",
                                        tuple
                                            .iter()
                                            .map(PartitionValue::sql)
                                            .collect::<Vec<_>>()
                                            .join(",")
                                    )
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(",");
                        output.push_str(&format!(" VALUES IN ({values})"));
                    }
                }
                _ => {}
            }
            if !definition.comment.is_empty() {
                output.push_str(&format!(
                    " COMMENT '{}'",
                    definition.comment.replace('\'', "''")
                ));
            }
            if let Some(policy) = &definition.placement_policy {
                output.push_str(&format!(
                    " /*T![placement] PLACEMENT POLICY=`{}` */",
                    policy.replace('`', "``")
                ));
            }
            output
        })
        .collect::<Vec<_>>()
        .join(",\n ")
}

/// 分区类型枚举转 SQL 关键字。
fn partition_type_name(partition_type: PartitionType) -> &'static str {
    match partition_type {
        PartitionType::None => "NONE",
        PartitionType::Range => "RANGE",
        PartitionType::List => "LIST",
        PartitionType::Hash => "HASH",
        PartitionType::Key => "KEY",
    }
}

/// 校验分区数量在 (0, PARTITION_COUNT_LIMIT] 内。
pub fn check_partition_count(count: usize) -> Result<(), PartitionError> {
    if count == 0 {
        Err(PartitionError::NoPartitions)
    } else if count > PARTITION_COUNT_LIMIT {
        Err(PartitionError::TooManyPartitions)
    } else {
        Ok(())
    }
}

/// 临时表禁止添加分区。
pub fn check_add_partition_on_temporary_mode(
    table: &PartitionedTableInfo,
) -> Result<(), PartitionError> {
    if table.temporary_type != TemporaryTableType::None {
        Err(PartitionError::TemporaryTablePartition)
    } else {
        Ok(())
    }
}

/// 收集表上全部分区物理 ID。
pub fn partition_ids(table: &PartitionedTableInfo) -> Vec<i64> {
    table.partition.as_ref().map_or_else(Vec::new, |info| {
        info.definitions
            .iter()
            .map(|definition| definition.id)
            .collect()
    })
}

/// 生成放置规则用的分区路径 ID（schema/table/partition 层级）。
pub fn partition_rule_ids(database: &str, table: &PartitionedTableInfo) -> Vec<String> {
    table.partition.as_ref().map_or_else(Vec::new, |info| {
        info.definitions
            .iter()
            .map(|definition| {
                format!(
                    "schema/{}/table/{}/partition/{}",
                    database.to_lowercase(),
                    table.name.to_lowercase(),
                    definition.name.to_lowercase()
                )
            })
            .collect()
    })
}

/// TRUNCATE TABLE 时为所有分区重新分配物理 ID，返回旧 ID 列表。
pub fn truncate_table_by_reassign_partition_ids(
    table: &mut PartitionedTableInfo,
    new_ids: &[i64],
) -> Result<Vec<i64>, PartitionError> {
    let info = table
        .partition
        .as_mut()
        .ok_or(PartitionError::UnsupportedType)?;
    if info.definitions.len() != new_ids.len() {
        return Err(PartitionError::IdCountMismatch);
    }
    let old = info
        .definitions
        .iter()
        .map(|definition| definition.id)
        .collect();
    for (definition, id) in info.definitions.iter_mut().zip(new_ids) {
        definition.id = *id;
    }
    Ok(old)
}

/// 字节序列转小写十六进制字符串。
fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}
