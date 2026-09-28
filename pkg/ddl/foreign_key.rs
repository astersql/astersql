// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// 外键（Foreign Key）DDL 相关逻辑。
//
// 本模块负责外键约束的定义校验与 DDL 状态推进，主要内容包括：
// - 外键元数据结构（[`ForeignKeyInfo`]、[`ForeignKeyTable`]、[`ForeignKeyCatalog`]）；
// - 创建/删除外键时的合法性检查（列类型匹配、索引覆盖、临时表/分区表限制等）；
// - 按“在线 Schema 变更”（参考 F1/TiDB 的多阶段 Schema 状态机）推进外键状态：
//   None -> WriteOnly -> WriteReorganization -> Public；
// - 修改列、删除列、删除索引、删除表等 DDL 操作与既有外键的兼容性检查；
// - 生成用于校验存量数据是否满足外键约束的 SQL 语句。
//
// 术语说明：外键是子表列到父表（被引用表）列的引用约束；“覆盖索引”指
// 索引前缀列按顺序完整覆盖外键列，用于加速外键约束检查。

use std::collections::BTreeMap;

use crate::column::{ColumnInfo, ColumnKind, IndexInfo, SchemaState, TableInfo};

/// 引用动作：父表行被删除/更新时对子表引用行采取的策略。
///
/// 对应 SQL 中 `ON DELETE`/`ON UPDATE` 子句的取值。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferentialAction {
    /// 拒绝父表操作（存在引用行时报错）。
    Restrict,
    /// 级联操作：同步删除或更新子表引用行。
    Cascade,
    /// 将子表引用列置为 NULL（要求子表列可空）。
    SetNull,
    /// 不做动作，行为等同 Restrict（MySQL 语义）。
    NoAction,
}

/// 一条外键约束的元数据信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForeignKeyInfo {
    /// 外键在所属表内的唯一 ID（由表的 max_foreign_key_id 分配）。
    pub id: i64,
    /// 外键约束名（大小写不敏感）。
    pub name: String,
    /// 子表参与外键的列名列表（与 referenced_columns 一一对应）。
    pub columns: Vec<String>,
    /// 被引用（父）表所在的库名。
    pub referenced_schema: String,
    /// 被引用（父）表名。
    pub referenced_table: String,
    /// 父表被引用的列名列表。
    pub referenced_columns: Vec<String>,
    /// 父表行删除时的引用动作。
    pub on_delete: ReferentialAction,
    /// 父表行更新时的引用动作。
    pub on_update: ReferentialAction,
    /// 外键元数据版本号。
    pub version: u8,
    /// 当前 Schema 状态（在线 DDL 状态机中的阶段）。
    pub state: SchemaState,
}

/// 参与外键关系的表的描述（表信息 + 外键相关属性）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForeignKeyTable {
    /// 表所在的库名。
    pub schema_name: String,
    /// 是否为临时表（临时表不允许参与外键）。
    pub temporary: bool,
    /// 是否为分区表（分区表不允许参与外键）。
    pub partitioned: bool,
    /// 是否启用 TTL（生存时间自动删除；启用 TTL 的表不能作为外键父表）。
    pub ttl_enabled: bool,
    /// 主键列是否直接作为行句柄（handle，即行的物理定位键）。
    pub primary_key_is_handle: bool,
    /// 表的基础元信息（列、索引等）。
    pub table: TableInfo,
    /// 已分配的最大外键 ID，用于生成新外键 ID。
    pub max_foreign_key_id: i64,
    /// 该表作为子表持有的全部外键约束。
    pub foreign_keys: Vec<ForeignKeyInfo>,
}

/// 外键 DDL 操作可能产生的各类错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ForeignKeyError {
    /// 同表内外键名称重复。
    DuplicateName(String),
    /// 按名称/ID 找不到对应外键。
    ForeignKeyNotFound(String),
    /// 无法打开父表（父表不存在）。
    CannotOpenParent(String),
    /// 外键定义非法（列数不匹配、自引用同列、虚拟生成列等）。
    CannotAddForeignKey,
    /// 临时表不允许参与外键。
    TemporaryTable,
    /// 启用 TTL 的表不能作为外键父表。
    TtlParent,
    /// 分区表不允许参与外键。
    PartitionedTable,
    /// 找不到指定的列（或表）。
    ColumnNotFound(String),
    /// 子表列与父表列类型不兼容（类型/符号/长度/字符集不一致）。
    IncompatibleColumns(String, String),
    /// 缺少能覆盖外键列的索引。
    MissingIndex(String),
    /// 存量数据违反外键约束（存在无法匹配父表的子表行）。
    ExistingRowsViolate(String),
    /// 父表被其他表的外键引用，禁止删除。
    ParentIsReferenced {
        /// 被引用的父表名。
        table: String,
        /// 引用该表的外键名。
        foreign_key: String,
        /// 持有该外键的子表名。
        child_table: String,
    },
    /// 索引被外键依赖，不能删除。
    IndexNeeded(String),
    /// 列被外键依赖，不能删除（列名，外键名）。
    ColumnNeeded(String, String),
    /// 外键处于非法的 Schema 状态，无法继续推进。
    InvalidState(SchemaState),
}

/// 外键目录：以 (库名, 表名) 小写形式为键的表集合，模拟系统表目录。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ForeignKeyCatalog {
    /// 是否启用外键功能（对应 foreign_key_checks 相关的全局开关）。
    pub enabled: bool,
    /// (小写库名, 小写表名) -> 表描述 的映射。
    pub tables: BTreeMap<(String, String), ForeignKeyTable>,
}

impl ForeignKeyCatalog {
    /// 注册一张表，键统一转小写以实现大小写不敏感查找。
    pub fn add_table(&mut self, table: ForeignKeyTable) {
        self.tables.insert(
            (
                table.schema_name.to_ascii_lowercase(),
                table.table.name.to_ascii_lowercase(),
            ),
            table,
        );
    }

    /// 按库名/表名（大小写不敏感）查找表。
    pub fn table(&self, schema: &str, table: &str) -> Option<&ForeignKeyTable> {
        self.tables
            .get(&(schema.to_ascii_lowercase(), table.to_ascii_lowercase()))
    }

    /// 按库名/表名（大小写不敏感）查找表的可变引用。
    pub fn table_mut(&mut self, schema: &str, table: &str) -> Option<&mut ForeignKeyTable> {
        self.tables
            .get_mut(&(schema.to_ascii_lowercase(), table.to_ascii_lowercase()))
    }

    /// 收集所有引用指定表（作为父表）的外键，返回 (子表, 外键) 列表。
    ///
    /// 遍历目录中每张表的外键，筛选出被引用库/表与入参匹配的项。
    pub fn referred_foreign_keys(
        &self,
        schema: &str,
        table: &str,
    ) -> Vec<(&ForeignKeyTable, &ForeignKeyInfo)> {
        self.tables
            .values()
            .flat_map(|child| {
                child.foreign_keys.iter().filter_map(move |foreign_key| {
                    (foreign_key.referenced_schema.eq_ignore_ascii_case(schema)
                        && foreign_key.referenced_table.eq_ignore_ascii_case(table))
                    .then_some((child, foreign_key))
                })
            })
            .collect()
    }
}

/// 按名称（大小写不敏感）在表中查找列。
fn find_column<'a>(table: &'a TableInfo, name: &str) -> Option<&'a ColumnInfo> {
    table
        .columns
        .iter()
        .find(|column| column.name.eq_ignore_ascii_case(name))
}

/// Go `IsIndexPrefixCovered` for foreign-key column sets: names must match in
/// order and prefix-index lengths shorter than the column width are rejected.
///
/// 判断索引是否“前缀覆盖”外键列集合：索引的前若干列必须与外键列按顺序
/// 逐一同名，且不允许使用比列宽更短的前缀索引（前缀索引只索引列值的
/// 前 N 个字符，无法保证完整值的唯一定位）。
fn index_covers_columns(table: &TableInfo, index: &IndexInfo, columns: &[String]) -> bool {
    // 索引列数不足以覆盖外键全部列，直接失败。
    if index.columns.len() < columns.len() {
        return false;
    }
    for (index_column, column_name) in index.columns.iter().zip(columns) {
        // 列名必须按顺序一一对应。
        if !index_column.name.eq_ignore_ascii_case(column_name) {
            return false;
        }
        let Some(column) = find_column(table, column_name) else {
            return false;
        };
        // 前缀索引长度小于列定义长度时不满足覆盖要求。
        if let Some(length) = index_column.length {
            if length < column.field_type.flen {
                return false;
            }
        }
    }
    true
}

/// 判断表中是否存在能覆盖给定外键列集合的索引。
///
/// 外键约束检查需要借助索引快速定位父/子表中的匹配行，因此外键列
/// 必须被某个索引（或作为 handle 的主键）覆盖。
fn table_has_covering_index(table: &ForeignKeyTable, columns: &[String]) -> bool {
    // 特殊情况：单列外键且主键即行句柄时，检查主键索引是否覆盖，
    // 此时还要求该列已处于 Public（对所有事务可见的）状态。
    if table.primary_key_is_handle && columns.len() == 1 {
        if let Some(column) = find_column(&table.table, &columns[0])
            && table
                .table
                .indices
                .iter()
                .any(|index| index.primary && index_covers_columns(&table.table, index, columns))
        {
            return column.state == SchemaState::Public;
        }
    }
    // 一般情况：任意一个索引能前缀覆盖外键列即可。
    table
        .table
        .indices
        .iter()
        .any(|index| index_covers_columns(&table.table, index, columns))
}

/// 判断子表列与父表列类型是否兼容：类型、有无符号必须一致；
/// 字符串类型（Varchar/String）还要求字符集与排序规则（collation）一致。
/// `flen` 是显示宽度/长度，列修改的可接受范围由
/// [`is_acceptable_foreign_key_column_change`] 单独校验。
fn foreign_key_column_types_match(child: &ColumnInfo, parent: &ColumnInfo) -> bool {
    child.field_type.kind == parent.field_type.kind
        && child.field_type.unsigned == parent.field_type.unsigned
        && child.field_type.charset == parent.field_type.charset
        && child.field_type.collation == parent.field_type.collation
}

/// 是否为“虚拟生成列”（由表达式计算、不落盘存储的列）。
/// 虚拟生成列没有物理存储，无法参与外键约束。
fn is_virtual_generated(column: &ColumnInfo) -> bool {
    column.generated && !column.generated_stored
}

/// 检查是否在 NOT NULL 的子表列上使用了 SET NULL 动作。
/// SET NULL 需要把子表列置为 NULL，与 NOT NULL 约束冲突。
fn set_null_on_not_null_child(foreign_key: &ForeignKeyInfo, child_column: &ColumnInfo) -> bool {
    child_column.not_null
        && (matches!(foreign_key.on_delete, ReferentialAction::SetNull)
            || matches!(foreign_key.on_update, ReferentialAction::SetNull))
}

/// 校验一条外键定义是否合法。
///
/// 检查项包括：
/// - 父/子表均不能是临时表或分区表，父表不能启用 TTL；
/// - 子表列与父表列数量一致且非空；
/// - 禁止自引用同一组列（表引用自身且列完全相同）；
/// - 每对列不能是虚拟生成列、类型必须兼容、SET NULL 不能落在 NOT NULL 列上；
/// - 父表必须存在覆盖被引用列的索引。
pub fn check_foreign_key_definition(
    parent: &ForeignKeyTable,
    child: &ForeignKeyTable,
    foreign_key: &ForeignKeyInfo,
) -> Result<(), ForeignKeyError> {
    // 临时表 / TTL 父表 / 分区表均不允许参与外键。
    if parent.temporary || child.temporary {
        return Err(ForeignKeyError::TemporaryTable);
    }
    if parent.ttl_enabled {
        return Err(ForeignKeyError::TtlParent);
    }
    if parent.partitioned || child.partitioned {
        return Err(ForeignKeyError::PartitionedTable);
    }
    // 子表列与父表列必须一一对应且不能为空。
    if foreign_key.columns.len() != foreign_key.referenced_columns.len()
        || foreign_key.columns.is_empty()
    {
        return Err(ForeignKeyError::CannotAddForeignKey);
    }
    // 禁止表引用自身且每对列名完全相同的“自引用同列”外键。
    if parent.schema_name.eq_ignore_ascii_case(&child.schema_name)
        && parent.table.name.eq_ignore_ascii_case(&child.table.name)
        && foreign_key
            .columns
            .iter()
            .zip(&foreign_key.referenced_columns)
            .all(|(child, parent)| child.eq_ignore_ascii_case(parent))
    {
        return Err(ForeignKeyError::CannotAddForeignKey);
    }
    // 逐对校验子表列与父表列。
    for (child_name, parent_name) in foreign_key
        .columns
        .iter()
        .zip(&foreign_key.referenced_columns)
    {
        let child_column = find_column(&child.table, child_name)
            .ok_or_else(|| ForeignKeyError::ColumnNotFound(child_name.clone()))?;
        let parent_column = find_column(&parent.table, parent_name)
            .ok_or_else(|| ForeignKeyError::ColumnNotFound(parent_name.clone()))?;
        if is_virtual_generated(child_column) || is_virtual_generated(parent_column) {
            return Err(ForeignKeyError::CannotAddForeignKey);
        }
        if set_null_on_not_null_child(foreign_key, child_column) {
            return Err(ForeignKeyError::CannotAddForeignKey);
        }
        if !foreign_key_column_types_match(child_column, parent_column) {
            return Err(ForeignKeyError::IncompatibleColumns(
                child_name.clone(),
                parent_name.clone(),
            ));
        }
    }
    // 父表必须有能覆盖被引用列的索引，否则约束检查无法高效执行。
    if !table_has_covering_index(parent, &foreign_key.referenced_columns) {
        return Err(ForeignKeyError::MissingIndex(foreign_key.name.clone()));
    }
    Ok(())
}

/// 校验向子表新增外键是否可行。
///
/// 流程：先检查同名外键冲突；若外键功能未启用则直接放行；父表不存在时
/// 依据 `foreign_key_checks`（会话级外键检查开关）决定报错还是忽略；
/// 最后校验外键定义并要求子表也存在覆盖外键列的索引。
pub fn check_add_foreign_key_valid(
    catalog: &ForeignKeyCatalog,
    child_schema: &str,
    child_table: &str,
    foreign_key: &ForeignKeyInfo,
    foreign_key_checks: bool,
) -> Result<(), ForeignKeyError> {
    let child = catalog
        .table(child_schema, child_table)
        .ok_or_else(|| ForeignKeyError::ColumnNotFound(child_table.into()))?;
    if child
        .foreign_keys
        .iter()
        .any(|existing| existing.name.eq_ignore_ascii_case(&foreign_key.name))
    {
        return Err(ForeignKeyError::DuplicateName(foreign_key.name.clone()));
    }
    // 外键功能未启用时只做名字冲突检查。
    if !catalog.enabled {
        return Ok(());
    }
    // 父表不存在：foreign_key_checks 开启则报错，关闭则允许（延迟校验）。
    let Some(parent) = catalog.table(
        &foreign_key.referenced_schema,
        &foreign_key.referenced_table,
    ) else {
        return if foreign_key_checks {
            Err(ForeignKeyError::CannotOpenParent(
                foreign_key.referenced_table.clone(),
            ))
        } else {
            Ok(())
        };
    };
    check_foreign_key_definition(parent, child, foreign_key)?;
    // 子表同样需要覆盖外键列的索引。
    if !table_has_covering_index(child, &foreign_key.columns) {
        return Err(ForeignKeyError::MissingIndex(foreign_key.name.clone()));
    }
    Ok(())
}

/// 外键 DDL 单步推进后的结果。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ForeignKeyOutcome {
    /// 推进后外键所处的 Schema 状态。
    pub schema_state: SchemaState,
    /// 推进后的全局 Schema 版本号（每次变更递增，用于通知各节点刷新元数据）。
    pub schema_version: i64,
    /// DDL 任务是否已完成（到达 Public 或回滚/删除完毕）。
    pub finished: bool,
    /// 是否为回滚完成。
    pub rollback_done: bool,
}

/// 推进“创建外键”DDL 状态机一步。
///
/// 在线 Schema 变更把外键创建拆成多个阶段，避免长时间锁表：
/// - None：先做合法性检查，然后分配外键 ID 并进入 WriteOnly（外键仅在
///   写入时生效，尚不对读可见）；
/// - WriteOnly -> WriteReorganization：此阶段前需校验存量数据满足约束
///   （`existing_rows_valid`），否则报错；
/// - WriteReorganization -> Public：外键对所有事务完全可见，任务结束。
///
/// `rolling_back` 为 true 时执行回滚：直接从子表移除同名外键。
/// 每步推进都会自增 `schema_version`。
#[allow(clippy::too_many_arguments)]
pub fn advance_create_foreign_key(
    catalog: &mut ForeignKeyCatalog,
    child_schema: &str,
    child_table: &str,
    foreign_key: &mut ForeignKeyInfo,
    foreign_key_checks: bool,
    existing_rows_valid: bool,
    schema_version: &mut i64,
    rolling_back: bool,
) -> Result<ForeignKeyOutcome, ForeignKeyError> {
    // 回滚路径：从子表移除该外键并结束任务。
    if rolling_back {
        let child = catalog
            .table_mut(child_schema, child_table)
            .ok_or_else(|| ForeignKeyError::ColumnNotFound(child_table.into()))?;
        child
            .foreign_keys
            .retain(|existing| !existing.name.eq_ignore_ascii_case(&foreign_key.name));
        *schema_version += 1;
        return Ok(ForeignKeyOutcome {
            schema_state: SchemaState::None,
            schema_version: *schema_version,
            finished: true,
            rollback_done: true,
        });
    }
    // 初始阶段：校验后分配 ID，写入子表元数据并进入 WriteOnly。
    if foreign_key.state == SchemaState::None {
        check_add_foreign_key_valid(
            catalog,
            child_schema,
            child_table,
            foreign_key,
            foreign_key_checks,
        )?;
        let child = catalog
            .table_mut(child_schema, child_table)
            .ok_or_else(|| ForeignKeyError::ColumnNotFound(child_table.into()))?;
        child.max_foreign_key_id += 1;
        foreign_key.id = child.max_foreign_key_id;
        foreign_key.state = SchemaState::WriteOnly;
        child.foreign_keys.push(foreign_key.clone());
        *schema_version += 1;
        return Ok(ForeignKeyOutcome {
            schema_state: SchemaState::WriteOnly,
            schema_version: *schema_version,
            finished: false,
            rollback_done: false,
        });
    }
    let child = catalog
        .table_mut(child_schema, child_table)
        .ok_or_else(|| ForeignKeyError::ColumnNotFound(child_table.into()))?;
    // 后续阶段：按已存储的状态决定下一个状态。
    let stored = child
        .foreign_keys
        .iter_mut()
        .find(|existing| existing.id == foreign_key.id)
        .ok_or_else(|| ForeignKeyError::ForeignKeyNotFound(foreign_key.name.clone()))?;
    let next = match stored.state {
        SchemaState::WriteOnly => {
            // 进入重组阶段前需确认存量数据满足外键约束。
            if foreign_key_checks && !existing_rows_valid {
                return Err(ForeignKeyError::ExistingRowsViolate(
                    foreign_key.name.clone(),
                ));
            }
            SchemaState::WriteReorganization
        }
        SchemaState::WriteReorganization => SchemaState::Public,
        state => return Err(ForeignKeyError::InvalidState(state)),
    };
    stored.state = next;
    foreign_key.state = next;
    *schema_version += 1;
    Ok(ForeignKeyOutcome {
        schema_state: next,
        schema_version: *schema_version,
        finished: next == SchemaState::Public,
        rollback_done: false,
    })
}

/// 删除子表上的指定外键（按名称，大小写不敏感）。
///
/// 外键删除是单阶段操作：直接从元数据移除并递增 Schema 版本；
/// 找不到同名外键时返回 [`ForeignKeyError::ForeignKeyNotFound`]。
pub fn drop_foreign_key(
    catalog: &mut ForeignKeyCatalog,
    child_schema: &str,
    child_table: &str,
    foreign_key_name: &str,
    schema_version: &mut i64,
    rolling_back: bool,
) -> Result<ForeignKeyOutcome, ForeignKeyError> {
    let child = catalog
        .table_mut(child_schema, child_table)
        .ok_or_else(|| ForeignKeyError::ColumnNotFound(child_table.into()))?;
    let before = child.foreign_keys.len();
    child
        .foreign_keys
        .retain(|foreign_key| !foreign_key.name.eq_ignore_ascii_case(foreign_key_name));
    if child.foreign_keys.len() == before {
        return Err(ForeignKeyError::ForeignKeyNotFound(foreign_key_name.into()));
    }
    *schema_version += 1;
    Ok(ForeignKeyOutcome {
        schema_state: SchemaState::None,
        schema_version: *schema_version,
        finished: true,
        rollback_done: rolling_back,
    })
}

/// 判断修改列类型后是否仍能满足外键约束要求。
///
/// 规则：整数类型的修改总是允许；否则新列长度不能小于关联列或原列的
/// 长度（缩短长度可能截断数据，破坏外键匹配）。
pub fn is_acceptable_foreign_key_column_change(
    new_column: &ColumnInfo,
    original_column: &ColumnInfo,
    related_column: &ColumnInfo,
) -> bool {
    // 整数类型间的变更被认为是安全的。
    if new_column.field_type.kind == ColumnKind::Integer {
        return true;
    }
    // 新列长度不能小于对端关联列或原列的长度。
    if new_column.field_type.flen < related_column.field_type.flen
        || new_column.field_type.flen < original_column.field_type.flen
    {
        return false;
    }
    if new_column.field_type.kind == ColumnKind::Integer
        && (new_column.field_type.flen != original_column.field_type.flen
            || new_column.field_type.decimal != original_column.field_type.decimal)
    {
        return false;
    }
    true
}

/// 检查修改列（MODIFY/CHANGE COLUMN）是否与既有外键冲突。
///
/// 双向检查：
/// 1. 若该列是本表某外键的子表列，需与父表对应列保持类型兼容；
/// 2. 若该列被其他表的外键引用（本表作为父表），需与各子表对应列兼容。
pub fn check_modify_column_with_foreign_key(
    catalog: &ForeignKeyCatalog,
    schema: &str,
    table_name: &str,
    original: &ColumnInfo,
    new_column: &ColumnInfo,
) -> Result<(), ForeignKeyError> {
    if !catalog.enabled {
        return Ok(());
    }
    let table = catalog
        .table(schema, table_name)
        .ok_or_else(|| ForeignKeyError::ColumnNotFound(table_name.into()))?;
    // 方向 1：本表作为子表，该列参与的外键需要与父表列保持兼容。
    for foreign_key in &table.foreign_keys {
        for (offset, child_name) in foreign_key.columns.iter().enumerate() {
            if child_name.eq_ignore_ascii_case(&original.name) {
                let parent = catalog
                    .table(
                        &foreign_key.referenced_schema,
                        &foreign_key.referenced_table,
                    )
                    .ok_or_else(|| {
                        ForeignKeyError::CannotOpenParent(foreign_key.referenced_table.clone())
                    })?;
                let related = find_column(&parent.table, &foreign_key.referenced_columns[offset])
                    .ok_or_else(|| {
                    ForeignKeyError::ColumnNotFound(foreign_key.referenced_columns[offset].clone())
                })?;
                if !foreign_key_column_types_match(new_column, related)
                    || !is_acceptable_foreign_key_column_change(new_column, original, related)
                {
                    return Err(ForeignKeyError::IncompatibleColumns(
                        new_column.name.clone(),
                        related.name.clone(),
                    ));
                }
            }
        }
    }
    // 方向 2：本表作为父表，被引用列需要与各子表列保持兼容。
    for (child, foreign_key) in catalog.referred_foreign_keys(schema, table_name) {
        for (offset, parent_name) in foreign_key.referenced_columns.iter().enumerate() {
            if parent_name.eq_ignore_ascii_case(&original.name) {
                let related =
                    find_column(&child.table, &foreign_key.columns[offset]).ok_or_else(|| {
                        ForeignKeyError::ColumnNotFound(foreign_key.columns[offset].clone())
                    })?;
                if !foreign_key_column_types_match(new_column, related)
                    || !is_acceptable_foreign_key_column_change(new_column, original, related)
                {
                    return Err(ForeignKeyError::IncompatibleColumns(
                        related.name.clone(),
                        new_column.name.clone(),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// 检查表是否被其他表的外键引用（用于 DROP TABLE 等操作前的防护）。
///
/// `ignored_tables` 用于排除同一批被删除的表（例如 DROP 多个表时，
/// 互相引用的表可以一起删除）。外键功能或 foreign_key_checks 关闭时跳过。
pub fn check_table_has_foreign_key_referred(
    catalog: &ForeignKeyCatalog,
    schema: &str,
    table: &str,
    ignored_tables: &[(String, String)],
    foreign_key_checks: bool,
) -> Result<(), ForeignKeyError> {
    if !catalog.enabled || !foreign_key_checks {
        return Ok(());
    }
    if let Some((child, foreign_key)) = catalog
        .referred_foreign_keys(schema, table)
        .into_iter()
        .find(|(child, _)| {
            !ignored_tables.iter().any(|(schema, table)| {
                child.schema_name.eq_ignore_ascii_case(schema)
                    && child.table.name.eq_ignore_ascii_case(table)
            })
        })
    {
        return Err(ForeignKeyError::ParentIsReferenced {
            table: table.into(),
            foreign_key: foreign_key.name.clone(),
            child_table: child.table.name.clone(),
        });
    }
    Ok(())
}

/// 检查删除指定索引是否会导致某个外键失去覆盖索引（用于 DROP INDEX 防护）。
///
/// 收集本表所有外键列以及引用本表的外键被引用列，若待删索引覆盖了某组
/// 外键列，且其余索引均无法覆盖该组列，则该索引不可删除。
pub fn check_index_needed_in_foreign_key(
    catalog: &ForeignKeyCatalog,
    schema: &str,
    table_name: &str,
    index_id: i64,
) -> Result<(), ForeignKeyError> {
    let table = catalog
        .table(schema, table_name)
        .ok_or_else(|| ForeignKeyError::ColumnNotFound(table_name.into()))?;
    let index = table
        .table
        .indices
        .iter()
        .find(|index| index.id == index_id)
        .ok_or_else(|| ForeignKeyError::IndexNeeded(index_id.to_string()))?;
    // 删除该索引后剩余的索引集合。
    let remaining: Vec<&IndexInfo> = table
        .table
        .indices
        .iter()
        .filter(|candidate| candidate.id != index_id)
        .collect();
    // 收集所有需要索引覆盖的外键列集合（本表外键列 + 被引用列）。
    let mut required_columns: Vec<&[String]> = table
        .foreign_keys
        .iter()
        .map(|foreign_key| foreign_key.columns.as_slice())
        .collect();
    for (_, foreign_key) in catalog.referred_foreign_keys(schema, table_name) {
        required_columns.push(foreign_key.referenced_columns.as_slice());
    }
    // 若待删索引覆盖某组外键列且没有其他索引可替代，则禁止删除。
    for columns in required_columns {
        if index_covers_columns(&table.table, index, columns)
            && table.primary_key_is_handle
            && columns.len() == 1
            && find_column(&table.table, &columns[0]).is_some()
        {
            continue;
        }
        if index_covers_columns(&table.table, index, columns)
            && !remaining
                .iter()
                .any(|candidate| index_covers_columns(&table.table, candidate, columns))
        {
            return Err(ForeignKeyError::IndexNeeded(index.name.clone()));
        }
    }
    Ok(())
}

/// 检查删除列是否会破坏外键（用于 DROP COLUMN 防护）。
///
/// 若该列出现在本表任一外键的子表列中，或被其他表外键引用为父表列，
/// 则不能删除，返回 [`ForeignKeyError::ColumnNeeded`]。
pub fn check_drop_column_with_foreign_key(
    catalog: &ForeignKeyCatalog,
    schema: &str,
    table_name: &str,
    column_name: &str,
) -> Result<(), ForeignKeyError> {
    let table = catalog
        .table(schema, table_name)
        .ok_or_else(|| ForeignKeyError::ColumnNotFound(table_name.into()))?;
    if let Some(foreign_key) = table.foreign_keys.iter().find(|foreign_key| {
        foreign_key
            .columns
            .iter()
            .any(|column| column.eq_ignore_ascii_case(column_name))
    }) {
        return Err(ForeignKeyError::ColumnNeeded(
            column_name.into(),
            foreign_key.name.clone(),
        ));
    }
    for (_, foreign_key) in catalog.referred_foreign_keys(schema, table_name) {
        if foreign_key
            .referenced_columns
            .iter()
            .any(|column| column.eq_ignore_ascii_case(column_name))
        {
            return Err(ForeignKeyError::ColumnNeeded(
                column_name.into(),
                foreign_key.name.clone(),
            ));
        }
    }
    Ok(())
}

/// 构造校验存量数据是否满足外键约束的 SQL。
///
/// 生成形如 `SELECT 1 FROM 子表 WHERE 外键列非空 AND (外键列) NOT IN
/// (SELECT 被引用列 FROM 父表) LIMIT 1` 的语句：查到任意一行即说明
/// 存在违反外键约束的数据（NULL 值按 SQL 语义不参与外键检查）。
pub fn build_foreign_key_check_sql(
    schema: &str,
    table: &str,
    foreign_key: &ForeignKeyInfo,
) -> String {
    // 拼接各列的 IS NOT NULL 条件与反引号包裹的列名列表。
    let non_null = foreign_key
        .columns
        .iter()
        .map(|column| format!("`{column}` IS NOT NULL"))
        .collect::<Vec<_>>()
        .join(" AND ");
    let child_columns = foreign_key
        .columns
        .iter()
        .map(|column| format!("`{column}`"))
        .collect::<Vec<_>>()
        .join(",");
    let parent_columns = foreign_key
        .referenced_columns
        .iter()
        .map(|column| format!("`{column}`"))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "SELECT 1 FROM `{schema}`.`{table}` WHERE {non_null} AND ({child_columns}) NOT IN (SELECT {parent_columns} FROM `{}`.`{}`) LIMIT 1",
        foreign_key.referenced_schema, foreign_key.referenced_table
    )
}
