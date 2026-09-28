// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 临时表 InfoSchema（信息模式）与会话表目录（对应 Go temptable infoschema）。
//
// 维护会话内本地临时表的元数据（库/表名到 Table），并可将本地表挂到
// SessionExtendedInfoSchema，使 `table_by_id` 优先命中临时表再回落基线 InfoSchema。

use crate::interceptor::MemBuffer;
use std::any::Any;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::sync::{Arc, Mutex, Once, RwLock};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
/// 大小写不敏感字符串：保留原文，比较/索引用 Unicode 小写。
pub struct CiString {
    original: String,
    lower: String,
}

impl CiString {
    /// 由任意字符串构造。
    pub fn new(value: impl Into<String>) -> Self {
        let original = value.into();
        let lower = original.to_lowercase();
        Self { original, lower }
    }

    /// 返回原始大小写形式。
    pub fn original(&self) -> &str {
        &self.original
    }

    /// 返回用于查找的小写形式。
    pub fn lower(&self) -> &str {
        &self.lower
    }
}

impl Display for CiString {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.original)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 临时表类型：无 / 全局 / 本地（会话私有）。
pub enum TempTableType {
    #[default]
    None,
    Global,
    Local,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// DDL Schema 状态简化枚举；Public 表示对用户可见可用。
pub enum SchemaState {
    #[default]
    None,
    Public,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 库（schema）元信息。
pub struct DbInfo {
    pub id: i64,
    pub name: CiString,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 临时表元信息子集。
pub struct TableInfo {
    pub id: i64,
    pub db_id: i64,
    pub name: CiString,
    pub temp_table_type: TempTableType,
    pub state: SchemaState,
    pub has_auto_id: bool,
}

#[derive(Clone)]
/// 运行时表对象：元数据与是否具备自增分配器。
pub struct Table {
    metadata: Arc<TableInfo>,
    auto_id_allocator: bool,
}

impl Table {
    /// 由元数据构造 Table。
    pub fn from_metadata(metadata: TableInfo) -> Self {
        let auto_id_allocator = metadata.has_auto_id;
        Self {
            metadata: Arc::new(metadata),
            auto_id_allocator,
        }
    }

    /// 返回共享元数据。
    pub fn metadata(&self) -> Arc<TableInfo> {
        Arc::clone(&self.metadata)
    }

    /// 是否需要自增 ID 分配器。
    pub fn has_auto_id_allocator(&self) -> bool {
        self.auto_id_allocator
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 临时表路径上的错误集合。
pub enum TempTableError {
    TableNotExists(String),
    TableAlreadyExists(String),
    SchemaNotExists(i64),
    NormalTableSessionRead(i64),
    KeyNotExist,
    Store(String),
    Iterator(String),
}

impl Display for TempTableError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TableNotExists(name) => write!(f, "table {name} does not exist"),
            Self::TableAlreadyExists(name) => write!(f, "table {name} already exists"),
            Self::SchemaNotExists(id) => write!(f, "schema {id} does not exist"),
            Self::NormalTableSessionRead(id) => {
                write!(f, "cannot get normal table {id} key from session")
            }
            Self::KeyNotExist => write!(f, "key does not exist"),
            Self::Store(error) => write!(f, "store error: {error}"),
            Self::Iterator(error) => write!(f, "iterator error: {error}"),
        }
    }
}

impl std::error::Error for TempTableError {}

#[derive(Default)]
/// 会话内本地临时表目录：按名、按 ID、以及所属库索引。
pub struct SessionTables {
    tables_by_name: RwLock<HashMap<(String, String), Arc<Table>>>,
    tables_by_id: RwLock<HashMap<i64, Arc<Table>>>,
    schemas: RwLock<HashMap<String, Arc<DbInfo>>>,
}

impl SessionTables {
    /// 空目录。
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册表；同名已存在则 TableAlreadyExists。
    pub fn add_table(
        &self,
        database: Arc<DbInfo>,
        table: Arc<Table>,
    ) -> Result<(), TempTableError> {
        let metadata = table.metadata();
        if metadata.db_id != 0 {
            assert_eq!(
                database.id, metadata.db_id,
                "table DB ID must match its containing database"
            );
        }
        let key = (
            database.name.lower().to_owned(),
            metadata.name.lower().to_owned(),
        );
        let mut names = self.tables_by_name.write().unwrap();
        if names.contains_key(&key) {
            return Err(TempTableError::TableAlreadyExists(format!(
                "{}.{}",
                database.name, metadata.name
            )));
        }
        if self.tables_by_id.read().unwrap().contains_key(&metadata.id) {
            return Err(TempTableError::TableAlreadyExists(format!(
                "{}.{}",
                database.name, metadata.name
            )));
        }
        self.schemas
            .write()
            .unwrap()
            .entry(database.name.lower().to_owned())
            .or_insert(database);
        self.tables_by_id
            .write()
            .unwrap()
            .insert(metadata.id, Arc::clone(&table));
        names.insert(key, table);
        Ok(())
    }

    /// 按库名+表名移除并同步 ID 索引。
    pub fn remove_table(&self, schema: &CiString, name: &CiString) -> Option<Arc<Table>> {
        let table = self
            .tables_by_name
            .write()
            .unwrap()
            .remove(&(schema.lower().to_owned(), name.lower().to_owned()))?;
        self.tables_by_id
            .write()
            .unwrap()
            .remove(&table.metadata().id);
        if !self
            .tables_by_name
            .read()
            .unwrap()
            .keys()
            .any(|(schema_name, _)| schema_name == schema.lower())
        {
            self.schemas.write().unwrap().remove(schema.lower());
        }
        Some(table)
    }

    /// 按大小写不敏感的库名+表名查找。
    pub fn table_by_name(&self, schema: &CiString, name: &CiString) -> Option<Arc<Table>> {
        self.tables_by_name
            .read()
            .unwrap()
            .get(&(schema.lower().to_owned(), name.lower().to_owned()))
            .cloned()
    }

    /// 按库名+表名判断表是否存在。
    pub fn table_exists(&self, schema: &CiString, name: &CiString) -> bool {
        self.table_by_name(schema, name).is_some()
    }

    /// 按 table ID 查找。
    pub fn table_by_id(&self, id: i64) -> Option<Arc<Table>> {
        self.tables_by_id.read().unwrap().get(&id).cloned()
    }

    /// 按库 ID 查找 DbInfo。
    pub fn schema_by_id(&self, id: i64) -> Option<Arc<DbInfo>> {
        self.schemas
            .read()
            .unwrap()
            .values()
            .find(|database| database.id == id)
            .cloned()
    }

    /// 会话临时表数量。
    pub fn count(&self) -> usize {
        self.tables_by_id.read().unwrap().len()
    }

    /// 会话是否尚无任何临时表。
    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }
}

/// InfoSchema 最小接口：按 ID 取表、是否含临时表。
pub trait InfoSchema: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn table_by_id(&self, id: i64) -> Option<Arc<Table>>;
    fn has_temporary_table(&self) -> bool;
}

#[derive(Default)]
/// 内存版 InfoSchema，用于测试或轻量挂载。
pub struct MemoryInfoSchema {
    tables: RwLock<HashMap<i64, Arc<Table>>>,
}

impl MemoryInfoSchema {
    /// 插入/覆盖一张表。
    pub fn insert(&self, table: Arc<Table>) {
        self.tables
            .write()
            .unwrap()
            .insert(table.metadata().id, table);
    }
}

impl InfoSchema for MemoryInfoSchema {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn table_by_id(&self, id: i64) -> Option<Arc<Table>> {
        self.tables.read().unwrap().get(&id).cloned()
    }

    fn has_temporary_table(&self) -> bool {
        self.tables
            .read()
            .unwrap()
            .values()
            .any(|table| table.metadata().temp_table_type != TempTableType::None)
    }
}

/// 包装基线 InfoSchema，并可选挂载会话本地临时表。
pub struct SessionExtendedInfoSchema {
    base: Arc<dyn InfoSchema>,
    local_temporary_tables_once: Once,
    local_temporary_tables: Mutex<Option<Arc<SessionTables>>>,
}

impl SessionExtendedInfoSchema {
    /// 构造并立即挂载本地表目录。
    pub fn new(base: Arc<dyn InfoSchema>, local: Arc<SessionTables>) -> Self {
        Self {
            base,
            // Go's struct literal initializes LocalTemporaryTables without
            // consuming LocalTemporaryTablesOnce. The first later attachment
            // may therefore replace it; subsequent attachments are ignored.
            local_temporary_tables_once: Once::new(),
            local_temporary_tables: Mutex::new(Some(local)),
        }
    }

    /// 仅首次成功设置本地表目录。
    pub fn attach_once(&self, local: Arc<SessionTables>) {
        self.local_temporary_tables_once.call_once(|| {
            *self.local_temporary_tables.lock().unwrap() = Some(local);
        });
    }

    /// 返回不含本地临时表的新扩展层。
    pub fn detach_temporary_table_info_schema(&self) -> Arc<dyn InfoSchema> {
        Arc::new(Self {
            base: Arc::clone(&self.base),
            local_temporary_tables_once: Once::new(),
            local_temporary_tables: Mutex::new(None),
        })
    }
}

impl InfoSchema for SessionExtendedInfoSchema {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn table_by_id(&self, id: i64) -> Option<Arc<Table>> {
        self.local_temporary_tables
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|tables| tables.table_by_id(id))
            .or_else(|| self.base.table_by_id(id))
    }

    fn has_temporary_table(&self) -> bool {
        self.local_temporary_tables
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|tables| !tables.is_empty())
            || self.base.has_temporary_table()
    }
}

#[derive(Default)]
/// 会话变量中与临时表相关的字段：表目录与数据缓冲。
pub struct SessionVariables {
    pub local_temporary_tables: Mutex<Option<Arc<SessionTables>>>,
    pub temporary_table_data: Mutex<Option<Arc<MemBuffer>>>,
}

/// 提供会话变量的访问入口。
pub trait SessionVarsProvider: Send + Sync {
    fn session_variables(&self) -> Arc<SessionVariables>;
}

/// 取得会话本地临时表目录（可能尚未创建）。
pub fn get_local_temporary_tables(context: &dyn SessionVarsProvider) -> Option<Arc<SessionTables>> {
    context
        .session_variables()
        .local_temporary_tables
        .lock()
        .unwrap()
        .clone()
}

/// 确保本地临时表目录存在并返回。
pub fn ensure_local_temporary_tables(context: &dyn SessionVarsProvider) -> Arc<SessionTables> {
    let variables = context.session_variables();
    let mut tables = variables.local_temporary_tables.lock().unwrap();
    Arc::clone(tables.get_or_insert_with(|| Arc::new(SessionTables::new())))
}

/// 将本地临时表挂到 InfoSchema；若已是扩展类型则 attach_once，否则新建包装。
pub fn attach_local_temporary_table_info_schema(
    context: &dyn SessionVarsProvider,
    info_schema: Arc<dyn InfoSchema>,
) -> Arc<dyn InfoSchema> {
    let Some(local) = get_local_temporary_tables(context) else {
        return info_schema;
    };
    if let Some(extended) = info_schema
        .as_any()
        .downcast_ref::<SessionExtendedInfoSchema>()
    {
        extended.attach_once(local);
        return info_schema;
    }
    Arc::new(SessionExtendedInfoSchema::new(info_schema, local))
}

/// 若为 SessionExtendedInfoSchema 则返回无本地临时表的新扩展层，否则原样返回。
pub fn detach_local_temporary_table_info_schema(
    info_schema: Arc<dyn InfoSchema>,
) -> Arc<dyn InfoSchema> {
    info_schema
        .as_any()
        .downcast_ref::<SessionExtendedInfoSchema>()
        .map(SessionExtendedInfoSchema::detach_temporary_table_info_schema)
        .unwrap_or(info_schema)
}
