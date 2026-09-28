// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// DM（Data Migration，数据迁移）用的内存 SchemaTracker。
//
// 在不访问真实集群的情况下，按 DDL 语义维护 `InfoStore` 中的库/表元数据，
// 支持建删库表、增删改列与索引、分区增减、重命名等。表达式索引会生成
// Hidden（隐藏）生成列；批量建表失败时保留此前已成功创建的表。

use crate::{Error, InfoStore, NewInfoStore, ast, model};

#[derive(Clone)]
/// 创建表的规格：目标库、表定义与 IF NOT EXISTS。
pub struct CreateTableSpec {
    /// 目标库名。
    pub schema: ast::CIStr,
    /// 待创建的表定义。
    pub table: model::TableInfo,
    /// 为 true 时表已存在不报错。
    pub if_not_exists: bool,
}
#[derive(Clone)]
/// 创建索引的规格。
pub struct CreateIndexSpec {
    /// 目标库名。
    pub schema: ast::CIStr,
    /// 目标表名。
    pub table: ast::CIStr,
    /// 索引定义。
    pub index: IndexSpec,
    /// 为 true 时索引已存在不报错。
    pub if_not_exists: bool,
}
#[derive(Clone)]
/// 索引名称、列/表达式部件以及唯一性、可见性。
pub struct IndexSpec {
    /// 索引名。
    pub name: ast::CIStr,
    /// 索引键部件（列或表达式）。
    pub parts: Vec<IndexPart>,
    /// 是否唯一索引。
    pub unique: bool,
    /// 是否对优化器不可见（invisible index）。
    pub invisible: bool,
}
#[derive(Clone)]
/// 索引的一个键部件。
pub enum IndexPart {
    /// 普通列；length 为前缀长度，-1 表示整列。
    Column { name: ast::CIStr, length: isize },
    /// 表达式索引：会物化为隐藏生成列后再建索引。
    Expression(String),
}
#[derive(Clone)]
/// 新增列在表中的相对位置。
pub enum ColumnPosition {
    /// 追加到末尾。
    None,
    /// 放到第一列。
    First,
    /// 放到指定列之后。
    After(ast::CIStr),
}
#[derive(Clone)]
/// ALTER TABLE 可执行的单步操作。
pub enum AlterOperation {
    /// 添加列。
    AddColumn {
        column: model::ColumnInfo,
        if_not_exists: bool,
        position: ColumnPosition,
    },
    /// 删除列（同时从索引中剔除该列）。
    DropColumn { name: ast::CIStr, if_exists: bool },
    /// 重命名列，并同步更新索引列名。
    RenameColumn { old: ast::CIStr, new: ast::CIStr },
    /// 用新定义替换已有列（MODIFY COLUMN）。
    ModifyColumn {
        old: ast::CIStr,
        column: model::ColumnInfo,
    },
    /// 添加索引。
    AddIndex {
        index: IndexSpec,
        if_not_exists: bool,
    },
    /// 删除索引；表达式索引还会清理隐藏列。
    DropIndex { name: ast::CIStr, if_exists: bool },
    /// 重命名索引。
    RenameIndex { old: ast::CIStr, new: ast::CIStr },
    /// 修改索引可见性。
    SetIndexVisibility { name: ast::CIStr, invisible: bool },
    /// 以 PRIMARY 为名创建主键索引。
    CreatePrimaryKey(Vec<ast::CIStr>),
    /// 设置表注释。
    SetComment(String),
    /// 设置表及各列的字符集与排序规则。
    SetCharset { charset: String, collate: String },
    /// 添加分区定义。
    AddPartitions(Vec<model::PartitionDefinition>),
    /// 按名删除分区。
    DropPartitions(Vec<ast::CIStr>),
}
#[derive(Clone)]
/// ALTER TABLE 规格：目标表与操作列表。
pub struct AlterTableSpec {
    /// 目标库名。
    pub schema: ast::CIStr,
    /// 目标表名。
    pub table: ast::CIStr,
    /// 按序应用的变更操作。
    pub operations: Vec<AlterOperation>,
}

/// 基于 InfoStore 的内存 schema 跟踪器。
pub struct SchemaTracker {
    /// 底层库/表元数据存储。
    pub InfoStore: InfoStore,
}
/// 按表名大小写模式创建空跟踪器。
pub fn NewSchemaTracker(lower_case_table_names: i32) -> SchemaTracker {
    SchemaTracker {
        InfoStore: NewInfoStore(lower_case_table_names),
    }
}

impl SchemaTracker {
    /// 以默认 utf8mb4/utf8mb4_bin 创建库。
    pub fn CreateSchema(&mut self, name: ast::CIStr, if_not_exists: bool) -> Result<(), Error> {
        self.CreateSchemaWithInfo(
            model::DBInfo {
                Name: name,
                Charset: "utf8mb4".to_owned(),
                Collate: "utf8mb4_bin".to_owned(),
                ..Default::default()
            },
            if_not_exists,
        )
    }
    /// 确保存在名为 test 的库（IF NOT EXISTS）。
    pub fn CreateTestDB(&mut self) {
        let _ = self.CreateSchema(ast::NewCIStr("test"), true);
    }
    /// 用完整 DBInfo 创建库；已存在时按 ignore_on_exist 决定是否报错。
    pub fn CreateSchemaWithInfo(
        &mut self,
        info: model::DBInfo,
        ignore_on_exist: bool,
    ) -> Result<(), Error> {
        if self.InfoStore.SchemaByName(&info.Name).is_some() {
            return if ignore_on_exist {
                Ok(())
            } else {
                Err(Error::DatabaseExists(info.Name.O))
            };
        }
        self.InfoStore.PutSchema(info);
        Ok(())
    }
    /// 更新库的字符集与排序规则。
    pub fn AlterSchema(
        &mut self,
        name: &ast::CIStr,
        charset: String,
        collate: String,
    ) -> Result<(), Error> {
        let mut info = self
            .InfoStore
            .SchemaByName(name)
            .ok_or_else(|| Error::DatabaseNotExists(name.O.clone()))?
            .Clone();
        info.Charset = charset;
        info.Collate = collate;
        self.InfoStore.PutSchema(info);
        Ok(())
    }
    /// 删除库；if_exists 为 true 时库不存在也成功。
    pub fn DropSchema(&mut self, name: &ast::CIStr, if_exists: bool) -> Result<(), Error> {
        if self.InfoStore.DeleteSchema(name) || if_exists {
            Ok(())
        } else {
            Err(Error::DatabaseNotExists(name.O.clone()))
        }
    }

    /// 按规格创建表。
    pub fn CreateTable(&mut self, spec: CreateTableSpec) -> Result<(), Error> {
        self.CreateTableWithInfo(spec.schema, spec.table, spec.if_not_exists)
    }
    /// 写入表定义；已存在时按 ignore_on_exist 处理。
    pub fn CreateTableWithInfo(
        &mut self,
        schema: ast::CIStr,
        mut table: model::TableInfo,
        ignore_on_exist: bool,
    ) -> Result<(), Error> {
        if self.InfoStore.TableByName(&schema, &table.Name).is_ok() {
            return if ignore_on_exist {
                Ok(())
            } else {
                Err(Error::TableExists(schema.O, table.Name.O))
            };
        }
        self.InfoStore.PutTable(schema, table)
    }
    /// 创建视图：当前复用建表路径。
    pub fn CreateView(&mut self, spec: CreateTableSpec) -> Result<(), Error> {
        self.CreateTable(spec)
    }
    /// 删除多张表；任一张不存在且非 if_exists 则失败。
    pub fn DropTable(
        &mut self,
        schema: &ast::CIStr,
        tables: &[ast::CIStr],
        if_exists: bool,
    ) -> Result<(), Error> {
        let mut missing = Vec::new();
        for table in tables {
            let is_base_table = self
                .InfoStore
                .TableByName(schema, table)
                .is_ok_and(model::TableInfo::IsBaseTable);
            if !is_base_table {
                if !if_exists {
                    missing.push(format!("{}.{}", schema.O, table.O));
                }
                continue;
            }
            self.InfoStore.DeleteTable(schema, table)?;
        }
        if missing.is_empty() {
            Ok(())
        } else {
            Err(Error::TableDropExists(missing.join(",")))
        }
    }
    /// 删除视图：复用删表。
    pub fn DropView(
        &mut self,
        schema: &ast::CIStr,
        tables: &[ast::CIStr],
        if_exists: bool,
    ) -> Result<(), Error> {
        let mut missing = Vec::new();
        for table in tables {
            let Ok(info) = self.InfoStore.TableByName(schema, table) else {
                if !if_exists {
                    missing.push(format!("{}.{}", schema.O, table.O));
                }
                continue;
            };
            if !info.IsView() {
                return Err(Error::WrongObject(
                    schema.O.clone(),
                    table.O.clone(),
                    "VIEW",
                ));
            }
            self.InfoStore.DeleteTable(schema, table)?;
        }
        if missing.is_empty() {
            Ok(())
        } else {
            Err(Error::TableDropExists(missing.join(",")))
        }
    }

    /// 按规格添加索引。
    pub fn CreateIndex(&mut self, spec: CreateIndexSpec) -> Result<(), Error> {
        self.createIndex(&spec.schema, &spec.table, spec.index, spec.if_not_exists)
    }
    /// 前置结果成功时才写回表定义。
    fn putTableIfNoError(
        &mut self,
        result: Result<(), Error>,
        schema: ast::CIStr,
        table: model::TableInfo,
    ) -> Result<(), Error> {
        result?;
        self.InfoStore.PutTable(schema, table)
    }
    /// 克隆表、添加索引后写回。
    fn createIndex(
        &mut self,
        schema: &ast::CIStr,
        table: &ast::CIStr,
        index: IndexSpec,
        ignore: bool,
    ) -> Result<(), Error> {
        let mut info = self.InfoStore.TableClonedByName(schema, table)?;
        add_index(&mut info, index, ignore)?;
        self.InfoStore.PutTable(schema.clone(), info)
    }
    /// 删除索引。
    pub fn DropIndex(
        &mut self,
        schema: &ast::CIStr,
        table: &ast::CIStr,
        name: &ast::CIStr,
        if_exists: bool,
    ) -> Result<(), Error> {
        match self.dropIndex(schema, table, name, if_exists) {
            Err(Error::DatabaseNotExists(_) | Error::TableNotExists(_, _)) if if_exists => Ok(()),
            result => result,
        }
    }
    /// 克隆表、删除索引后写回。
    fn dropIndex(
        &mut self,
        schema: &ast::CIStr,
        table: &ast::CIStr,
        name: &ast::CIStr,
        if_exists: bool,
    ) -> Result<(), Error> {
        let mut info = self.InfoStore.TableClonedByName(schema, table)?;
        drop_index(&mut info, name, if_exists)?;
        self.InfoStore.PutTable(schema.clone(), info)
    }

    /// 顺序应用全部 AlterOperation，再规范化并写回。
    pub fn AlterTable(&mut self, spec: AlterTableSpec) -> Result<(), Error> {
        let mut table = self
            .InfoStore
            .TableClonedByName(&spec.schema, &spec.table)?;
        // 任一步失败则整次 ALTER 失败（调用方看到错误时表可能已部分改动，
        // 测试侧另有原子性场景覆盖）。
        for operation in spec.operations {
            apply_operation(&mut table, operation)?;
        }
        normalize_table(&mut table);
        self.InfoStore.PutTable(spec.schema, table)
    }
    /// 委托给模块级 add_column。
    fn addColumn(
        table: &mut model::TableInfo,
        column: model::ColumnInfo,
        if_not_exists: bool,
        position: ColumnPosition,
    ) -> Result<(), Error> {
        add_column(table, column, if_not_exists, position)
    }
    /// 委托给模块级 drop_column。
    fn dropColumn(
        table: &mut model::TableInfo,
        name: &ast::CIStr,
        if_exists: bool,
    ) -> Result<(), Error> {
        drop_column(table, name, if_exists)
    }
    /// 委托给模块级 rename_column。
    fn renameColumn(
        table: &mut model::TableInfo,
        old: &ast::CIStr,
        new: ast::CIStr,
    ) -> Result<(), Error> {
        rename_column(table, old, new)
    }
    /// 委托给 modify_column（ALTER COLUMN 语义）。
    fn alterColumn(
        table: &mut model::TableInfo,
        old: &ast::CIStr,
        column: model::ColumnInfo,
    ) -> Result<(), Error> {
        modify_column(table, old, column)
    }
    /// 委托给 modify_column（MODIFY COLUMN）。
    fn modifyColumn(
        table: &mut model::TableInfo,
        old: &ast::CIStr,
        column: model::ColumnInfo,
    ) -> Result<(), Error> {
        modify_column(table, old, column)
    }
    /// 委托给 modify_column（CHANGE COLUMN）。
    fn changeColumn(
        table: &mut model::TableInfo,
        old: &ast::CIStr,
        column: model::ColumnInfo,
    ) -> Result<(), Error> {
        modify_column(table, old, column)
    }
    /// 委托给 modify_column 的统一入口。
    fn handleModifyColumn(
        table: &mut model::TableInfo,
        old: &ast::CIStr,
        column: model::ColumnInfo,
    ) -> Result<(), Error> {
        modify_column(table, old, column)
    }
    /// 委托给模块级 rename_index。
    fn renameIndex(
        table: &mut model::TableInfo,
        old: &ast::CIStr,
        new: ast::CIStr,
    ) -> Result<(), Error> {
        rename_index(table, old, new)
    }
    /// 委托给 add_partitions。
    fn addTablePartitions(
        table: &mut model::TableInfo,
        definitions: Vec<model::PartitionDefinition>,
    ) -> Result<(), Error> {
        add_partitions(table, definitions)
    }
    /// 委托给 drop_partitions。
    fn dropTablePartitions(
        table: &mut model::TableInfo,
        names: Vec<ast::CIStr>,
    ) -> Result<(), Error> {
        drop_partitions(table, names)
    }
    /// 以名为 PRIMARY 的唯一索引模拟主键。
    fn createPrimaryKey(
        table: &mut model::TableInfo,
        columns: Vec<ast::CIStr>,
    ) -> Result<(), Error> {
        add_index(
            table,
            IndexSpec {
                name: ast::NewCIStr("PRIMARY"),
                parts: columns
                    .into_iter()
                    .map(|name| IndexPart::Column { name, length: -1 })
                    .collect(),
                unique: true,
                invisible: false,
            },
            false,
        )
    }

    /// 批量重命名表（可跨库）。
    pub fn RenameTable(
        &mut self,
        pairs: Vec<(ast::CIStr, ast::CIStr, ast::CIStr, ast::CIStr)>,
    ) -> Result<(), Error> {
        self.renameTable(pairs)
    }
    /// 先校验目标不存在并收集克隆，再删除旧表、写入新表。
    fn renameTable(
        &mut self,
        pairs: Vec<(ast::CIStr, ast::CIStr, ast::CIStr, ast::CIStr)>,
    ) -> Result<(), Error> {
        // 第一阶段：校验并准备待移动的表副本。
        let mut moved = Vec::new();
        for (old_schema, old_table, new_schema, new_table) in &pairs {
            if self.InfoStore.SchemaByName(new_schema).is_none() {
                return Err(Error::DatabaseNotExists(new_schema.O.clone()));
            }
            if self.InfoStore.TableByName(new_schema, new_table).is_ok() {
                return Err(Error::TableExists(
                    new_schema.O.clone(),
                    new_table.O.clone(),
                ));
            }
            let mut info = self.InfoStore.TableClonedByName(old_schema, old_table)?;
            info.Name = new_table.clone();
            moved.push((
                old_schema.clone(),
                old_table.clone(),
                new_schema.clone(),
                info,
            ));
        }
        // 第二阶段：提交删除与插入。
        for (old_schema, old_table, new_schema, info) in moved {
            self.InfoStore.DeleteTable(&old_schema, &old_table)?;
            self.InfoStore.PutTable(new_schema, info)?;
        }
        Ok(())
    }

    /// 恢复表：DM 跟踪器侧空操作。
    pub fn RecoverTable(&self) -> Result<(), Error> {
        Ok(())
    }
    /// 集群闪回：DM 跟踪器侧空操作。
    pub fn FlashbackCluster(&self) -> Result<(), Error> {
        Ok(())
    }
    /// 恢复库：DM 跟踪器侧空操作。
    pub fn RecoverSchema(&self) -> Result<(), Error> {
        Ok(())
    }
    /// 截断表：跟踪器侧无元数据变更，直接成功。
    pub fn TruncateTable(&self) -> Result<(), Error> {
        Ok(())
    }
    /// 锁表：跟踪器侧空操作。
    pub fn LockTables(&self) -> Result<(), Error> {
        Ok(())
    }
    /// 解锁表：跟踪器侧空操作。
    pub fn UnlockTables(&self) -> Result<(), Error> {
        Ok(())
    }
    /// 修改表模式：跟踪器侧空操作。
    pub fn AlterTableMode(&self) -> Result<(), Error> {
        Ok(())
    }
    /// 清理表锁：跟踪器侧空操作。
    pub fn CleanupTableLock(&self) -> Result<(), Error> {
        Ok(())
    }
    /// 更新副本信息：跟踪器侧空操作。
    pub fn UpdateTableReplicaInfo(&self) -> Result<(), Error> {
        Ok(())
    }
    /// 修复表：DM 跟踪器侧空操作。
    pub fn RepairTable(&self) -> Result<(), Error> {
        Ok(())
    }
    /// 创建序列：DM 跟踪器侧空操作。
    pub fn CreateSequence(&self) -> Result<(), Error> {
        Ok(())
    }
    /// 删除序列：DM 跟踪器侧空操作。
    pub fn DropSequence(&self) -> Result<(), Error> {
        Ok(())
    }
    /// 修改序列：DM 跟踪器侧空操作。
    pub fn AlterSequence(&self) -> Result<(), Error> {
        Ok(())
    }
    /// 创建脱敏策略：DM 跟踪器侧空操作。
    pub fn CreateMaskingPolicy(&self) -> Result<(), Error> {
        Ok(())
    }
    /// 创建放置策略：DM 跟踪器侧空操作。
    pub fn CreatePlacementPolicy(&self) -> Result<(), Error> {
        Ok(())
    }
    /// 删除放置策略：DM 跟踪器侧空操作。
    pub fn DropPlacementPolicy(&self) -> Result<(), Error> {
        Ok(())
    }
    /// 修改放置策略：DM 跟踪器侧空操作。
    pub fn AlterPlacementPolicy(&self) -> Result<(), Error> {
        Ok(())
    }
    /// 创建资源组：DM 跟踪器侧空操作。
    pub fn AddResourceGroup(&self) -> Result<(), Error> {
        Ok(())
    }
    /// 删除资源组：DM 跟踪器侧空操作。
    pub fn DropResourceGroup(&self) -> Result<(), Error> {
        Ok(())
    }
    /// 修改资源组：DM 跟踪器侧空操作。
    pub fn AlterResourceGroup(&self) -> Result<(), Error> {
        Ok(())
    }
    /// 用元信息创建放置策略：DM 跟踪器侧空操作。
    pub fn CreatePlacementPolicyWithInfo(&self) -> Result<(), Error> {
        Ok(())
    }
    /// 刷新元数据：跟踪器侧空操作。
    pub fn RefreshMeta(&self) -> Result<(), Error> {
        Ok(())
    }
    /// 批量建表；按 Go 顺序逐张创建，失败时保留此前已创建的表。
    pub fn BatchCreateTableWithInfo(
        &mut self,
        schema: ast::CIStr,
        tables: Vec<model::TableInfo>,
        ignore: bool,
    ) -> Result<(), Error> {
        for table in tables {
            self.CreateTableWithInfo(schema.clone(), table, ignore)?;
        }
        Ok(())
    }
}

/// 将单个 AlterOperation 应用到表定义。
fn apply_operation(table: &mut model::TableInfo, operation: AlterOperation) -> Result<(), Error> {
    match operation {
        AlterOperation::AddColumn {
            column,
            if_not_exists,
            position,
        } => add_column(table, column, if_not_exists, position),
        AlterOperation::DropColumn { name, if_exists } => drop_column(table, &name, if_exists),
        AlterOperation::RenameColumn { old, new } => rename_column(table, &old, new),
        AlterOperation::ModifyColumn { old, column } => modify_column(table, &old, column),
        AlterOperation::AddIndex {
            index,
            if_not_exists,
        } => add_index(table, index, if_not_exists),
        AlterOperation::DropIndex { name, if_exists } => drop_index(table, &name, if_exists),
        AlterOperation::RenameIndex { old, new } => rename_index(table, &old, new),
        AlterOperation::SetIndexVisibility { name, invisible } => {
            set_index_visibility(table, &name, invisible)
        }
        AlterOperation::CreatePrimaryKey(columns) => add_index(
            table,
            IndexSpec {
                name: ast::NewCIStr("PRIMARY"),
                parts: columns
                    .into_iter()
                    .map(|name| IndexPart::Column { name, length: -1 })
                    .collect(),
                unique: true,
                invisible: false,
            },
            false,
        ),
        // 仅更新表注释字段。
        AlterOperation::SetComment(comment) => {
            table.Comment = comment;
            Ok(())
        }
        // 同步表级与列级字符集/排序规则。
        AlterOperation::SetCharset { charset, collate } => {
            table.Charset = charset.clone();
            table.Collate = collate.clone();
            for column in &mut table.Columns {
                column.FieldType.SetCharset(charset.clone());
                column.FieldType.SetCollate(collate.clone());
            }
            Ok(())
        }
        AlterOperation::AddPartitions(definitions) => add_partitions(table, definitions),
        AlterOperation::DropPartitions(names) => drop_partitions(table, names),
    }
}
/// 按位置插入新列；已存在时按 ignore 决定是否报错。
fn add_column(
    table: &mut model::TableInfo,
    mut column: model::ColumnInfo,
    ignore: bool,
    position: ColumnPosition,
) -> Result<(), Error> {
    if table.Columns.iter().any(|old| old.Name.L == column.Name.L) {
        return if ignore {
            Ok(())
        } else {
            Err(Error::ColumnExists(column.Name.O))
        };
    }
    column.State = model::StatePublic;
    // 计算插入下标：末尾 / 首位 / 指定列之后。
    let offset = match position {
        ColumnPosition::None => table.Columns.len(),
        ColumnPosition::First => 0,
        ColumnPosition::After(name) => {
            table
                .Columns
                .iter()
                .position(|old| old.Name.L == name.L)
                .ok_or(Error::ColumnNotExists(name.O))?
                + 1
        }
    };
    table.Columns.insert(offset, column);
    normalize_table(table);
    Ok(())
}
/// 删除列，并从各索引中剔除该列；空索引一并移除。
fn drop_column(table: &mut model::TableInfo, name: &ast::CIStr, ignore: bool) -> Result<(), Error> {
    let Some(offset) = table
        .Columns
        .iter()
        .position(|column| column.Name.L == name.L)
    else {
        return if ignore {
            Ok(())
        } else {
            Err(Error::ColumnNotExists(name.O.clone()))
        };
    };
    if table.Columns.len() == 1 {
        return Err(Error::CannotRemoveAllColumns);
    }
    table.Columns.remove(offset);
    // 索引列引用删除列时需要摘除。
    for index in &mut table.Indices {
        index.Columns.retain(|column| column.Name.L != name.L);
    }
    table.Indices.retain(|index| !index.Columns.is_empty());
    normalize_table(table);
    Ok(())
}
/// 重命名列，并同步所有索引中的同名部件。
fn rename_column(
    table: &mut model::TableInfo,
    old: &ast::CIStr,
    new: ast::CIStr,
) -> Result<(), Error> {
    if table.Columns.iter().any(|column| column.Name.L == new.L) {
        return Err(Error::ColumnExists(new.O));
    }
    let column = table
        .Columns
        .iter_mut()
        .find(|column| column.Name.L == old.L)
        .ok_or_else(|| Error::ColumnNotExists(old.O.clone()))?;
    column.Name = new.clone();
    for index in &mut table.Indices {
        for part in &mut index.Columns {
            if part.Name.L == old.L {
                part.Name = new.clone();
            }
        }
    }
    Ok(())
}
/// 用新 ColumnInfo 替换指定列，保留原 offset。
fn modify_column(
    table: &mut model::TableInfo,
    old: &ast::CIStr,
    mut column: model::ColumnInfo,
) -> Result<(), Error> {
    let offset = table
        .Columns
        .iter()
        .position(|item| item.Name.L == old.L)
        .ok_or_else(|| Error::ColumnNotExists(old.O.clone()))?;
    if column.Name.L != old.L
        && table
            .Columns
            .iter()
            .any(|item| item.Name.L == column.Name.L)
    {
        return Err(Error::ColumnExists(column.Name.O));
    }
    let new_name = column.Name.clone();
    column.Offset = offset as isize;
    column.State = model::StatePublic;
    table.Columns[offset] = column;
    for index in &mut table.Indices {
        for part in &mut index.Columns {
            if part.Name.L == old.L {
                part.Name = new_name.clone();
            }
        }
    }
    normalize_table(table);
    Ok(())
}
/// 添加索引；表达式部件会先创建 Hidden 生成列。
fn add_index(table: &mut model::TableInfo, mut spec: IndexSpec, ignore: bool) -> Result<(), Error> {
    if spec.name.L.is_empty() {
        let base = match spec.parts.first() {
            Some(IndexPart::Column { name, .. }) => name.O.clone(),
            _ => "expression_index".to_owned(),
        };
        let mut candidate = ast::NewCIStr(&base);
        let mut suffix = 2;
        while table
            .Indices
            .iter()
            .any(|index| index.Name.L == candidate.L)
        {
            candidate = ast::NewCIStr(&format!("{base}_{suffix}"));
            suffix += 1;
        }
        spec.name = candidate;
    }
    if table
        .Indices
        .iter()
        .any(|index| index.Name.L == spec.name.L)
    {
        return if ignore {
            Ok(())
        } else {
            Err(Error::IndexExists(spec.name.O))
        };
    }
    let mut columns = Vec::new();
    for (position, part) in spec.parts.into_iter().enumerate() {
        match part {
            IndexPart::Column { name, length } => {
                let offset = table
                    .Columns
                    .iter()
                    .position(|column| column.Name.L == name.L)
                    .ok_or_else(|| Error::ColumnNotExists(name.O.clone()))?;
                columns.push(model::IndexColumn {
                    Name: name,
                    Offset: offset as isize,
                    Length: length,
                    ..Default::default()
                });
            }
            // 表达式索引：生成 `_V$_{索引名}_{位置}` 隐藏列。
            IndexPart::Expression(expression) => {
                let name = ast::NewCIStr(&format!("_V$_{}_{}", spec.name.O, position));
                let offset = table.Columns.len();
                table.Columns.push(model::ColumnInfo {
                    Name: name.clone(),
                    Offset: offset as isize,
                    GeneratedExprString: expression,
                    Hidden: true,
                    State: model::StatePublic,
                    ..Default::default()
                });
                columns.push(model::IndexColumn {
                    Name: name,
                    Offset: offset as isize,
                    Length: -1,
                    ..Default::default()
                });
            }
        }
    }
    table.Indices.push(model::IndexInfo {
        Name: spec.name,
        Columns: columns,
        Unique: spec.unique,
        Invisible: spec.invisible,
        State: model::StatePublic,
        ..Default::default()
    });
    Ok(())
}
/// 删除索引，并清理仅被该索引使用的隐藏生成列。
fn drop_index(table: &mut model::TableInfo, name: &ast::CIStr, ignore: bool) -> Result<(), Error> {
    let Some(position) = table
        .Indices
        .iter()
        .position(|index| index.Name.L == name.L)
    else {
        return if ignore {
            Ok(())
        } else {
            Err(Error::IndexNotExists(name.O.clone()))
        };
    };
    let removed = table.Indices.remove(position);
    // 收集被删索引引用的隐藏列名，随后从 Columns 中剔除。
    let hidden: Vec<String> = removed
        .Columns
        .iter()
        .filter_map(|part| table.Columns.get(part.Offset as usize))
        .filter(|column| column.Hidden)
        .map(|column| column.Name.L.clone())
        .collect();
    table
        .Columns
        .retain(|column| !hidden.contains(&column.Name.L));
    normalize_table(table);
    Ok(())
}
/// 重命名索引；新名冲突或旧名不存在时报错。
fn rename_index(
    table: &mut model::TableInfo,
    old: &ast::CIStr,
    new: ast::CIStr,
) -> Result<(), Error> {
    if old.L == new.L {
        return if table.Indices.iter().any(|index| index.Name.L == old.L) {
            Ok(())
        } else {
            Err(Error::IndexNotExists(old.O.clone()))
        };
    }
    if table.Indices.iter().any(|index| index.Name.L == new.L) {
        return Err(Error::IndexExists(new.O));
    }
    let index = table
        .Indices
        .iter_mut()
        .find(|index| index.Name.L == old.L)
        .ok_or_else(|| Error::IndexNotExists(old.O.clone()))?;
    let old_prefix = format!("_V$_{}_", old.O);
    let new_prefix = format!("_V$_{}_", new.O);
    for part in &mut index.Columns {
        let Some(column) = table
            .Columns
            .iter_mut()
            .find(|column| column.Name.L == part.Name.L && column.Hidden)
        else {
            continue;
        };
        if let Some(suffix) = column.Name.O.strip_prefix(&old_prefix) {
            let renamed = ast::NewCIStr(&format!("{new_prefix}{suffix}"));
            column.Name = renamed.clone();
            part.Name = renamed;
        }
    }
    index.Name = new;
    Ok(())
}
/// 修改已有索引的可见性。
fn set_index_visibility(
    table: &mut model::TableInfo,
    name: &ast::CIStr,
    invisible: bool,
) -> Result<(), Error> {
    let index = table
        .Indices
        .iter_mut()
        .find(|index| index.Name.L == name.L)
        .ok_or_else(|| Error::IndexNotExists(name.O.clone()))?;
    index.Invisible = invisible;
    Ok(())
}
/// 向分区表追加分区定义；重名则报错。
fn add_partitions(
    table: &mut model::TableInfo,
    definitions: Vec<model::PartitionDefinition>,
) -> Result<(), Error> {
    let partition = table
        .Partition
        .as_mut()
        .ok_or(Error::PartitionManagementOnNonpartitionedTable)?;
    for definition in definitions {
        if partition
            .Definitions
            .iter()
            .any(|old| old.Name.L == definition.Name.L)
        {
            return Err(Error::TableExists(table.Name.O.clone(), definition.Name.O));
        }
        partition.Definitions.push(definition);
    }
    Ok(())
}
/// 按名删除分区；表非分区或名称不存在时报错。
fn drop_partitions(table: &mut model::TableInfo, names: Vec<ast::CIStr>) -> Result<(), Error> {
    let partition = table
        .Partition
        .as_mut()
        .ok_or(Error::PartitionManagementOnNonpartitionedTable)?;
    for name in names {
        let before = partition.Definitions.len();
        partition
            .Definitions
            .retain(|definition| definition.Name.L != name.L);
        if before == partition.Definitions.len() {
            return Err(Error::PartitionNotExists(name.O));
        }
    }
    Ok(())
}
/// 重算列 Offset，并将 StateNone 的列/索引提升为 Public，校正索引列偏移。
fn normalize_table(table: &mut model::TableInfo) {
    for (offset, column) in table.Columns.iter_mut().enumerate() {
        column.Offset = offset as isize;
        if column.State == model::StateNone {
            column.State = model::StatePublic;
        }
    }
    for index in &mut table.Indices {
        if index.State == model::StateNone {
            index.State = model::StatePublic;
        }
        for part in &mut index.Columns {
            if let Some(offset) = table
                .Columns
                .iter()
                .position(|column| column.Name.L == part.Name.L)
            {
                part.Offset = offset as isize;
            }
        }
    }
}
