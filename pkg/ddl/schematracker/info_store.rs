// Copyright 2022 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// Schema 跟踪器用的内存信息模式存储（InfoStore）。
//
// 以库名、表名为键缓存 `DBInfo` / `TableInfo`，供 DM（Data Migration，数据迁移）
// 等场景在内存中模拟 DDL 对元数据的影响。键的大小写策略由
// `lowerCaseTableNames` 控制（0 用原始大小写，非 0 用小写键）。

use crate::{Error, ast, model};
use std::collections::HashMap;

/// 从外部 InfoSchema 批量拉取库表定义的数据源抽象。
pub trait InfoSchemaSource {
    /// 返回全部数据库（schema）元信息。
    fn AllSchemas(&self) -> Vec<model::DBInfo>;
    /// 返回指定库下的全部表定义。
    fn SchemaTableInfos(&self, schema: &ast::CIStr) -> Result<Vec<model::TableInfo>, Error>;
}

/// 内存中的库/表元数据仓库。
#[derive(Clone)]
pub struct InfoStore {
    /// 表名大小写模式：0 保留原始大小写作键，非 0 使用小写键。
    pub lowerCaseTableNames: i32,
    /// 库名键 → 数据库信息。
    dbs: HashMap<String, model::DBInfo>,
    /// 库名键 →（表名键 → 表信息）。
    tables: HashMap<String, HashMap<String, model::TableInfo>>,
}

/// 创建空的 `InfoStore`。
pub fn NewInfoStore(lower_case_table_names: i32) -> InfoStore {
    InfoStore {
        lowerCaseTableNames: lower_case_table_names,
        dbs: HashMap::new(),
        tables: HashMap::new(),
    }
}

impl InfoStore {
    /// 从 InfoSchema 数据源灌入全部库表。
    pub fn InitFromIS(&mut self, source: &dyn InfoSchemaSource) -> Result<(), Error> {
        for db in source.AllSchemas() {
            let name = db.Name.clone();
            self.PutSchema(db);
            let tables = source.SchemaTableInfos(&name)?;
            for table in tables {
                self.PutTable(name.clone(), table)?;
            }
        }
        Ok(())
    }

    /// 按 `lowerCaseTableNames` 将 CIStr（大小写不敏感字符串）转为查找键。
    fn ciStr2Key(&self, name: &ast::CIStr) -> String {
        if self.lowerCaseTableNames == 0 {
            name.O.clone()
        } else {
            name.L.clone()
        }
    }

    /// 按库名查找数据库信息。
    pub fn SchemaByName(&self, name: &ast::CIStr) -> Option<&model::DBInfo> {
        self.dbs.get(&self.ciStr2Key(name))
    }
    /// 写入或覆盖数据库信息，并确保对应表映射桶存在。
    pub fn PutSchema(&mut self, db_info: model::DBInfo) {
        let key = self.ciStr2Key(&db_info.Name);
        self.dbs.insert(key.clone(), db_info);
        self.tables.entry(key).or_default();
    }
    /// 删除库及其下全部表映射；库不存在时返回 false。
    pub fn DeleteSchema(&mut self, name: &ast::CIStr) -> bool {
        let key = self.ciStr2Key(name);
        if self.dbs.remove(&key).is_none() {
            return false;
        }
        self.tables.remove(&key);
        true
    }
    /// 按库名与表名查找表信息；任一层缺失则返回对应错误。
    pub fn TableByName(
        &self,
        schema: &ast::CIStr,
        table: &ast::CIStr,
    ) -> Result<&model::TableInfo, Error> {
        let schema_key = self.ciStr2Key(schema);
        let tables = self
            .tables
            .get(&schema_key)
            .ok_or_else(|| Error::DatabaseNotExists(schema.O.clone()))?;
        tables
            .get(&self.ciStr2Key(table))
            .ok_or_else(|| Error::TableNotExists(schema.O.clone(), table.O.clone()))
    }
    /// 返回表信息的深拷贝，便于后续就地修改后再写回。
    pub fn TableClonedByName(
        &self,
        schema: &ast::CIStr,
        table: &ast::CIStr,
    ) -> Result<model::TableInfo, Error> {
        Ok(self.TableByName(schema, table)?.Clone())
    }
    /// 在已存在的库下写入或覆盖表信息。
    pub fn PutTable(&mut self, schema: ast::CIStr, table: model::TableInfo) -> Result<(), Error> {
        let schema_key = self.ciStr2Key(&schema);
        let table_key = self.ciStr2Key(&table.Name);
        self.tables
            .get_mut(&schema_key)
            .ok_or_else(|| Error::DatabaseNotExists(schema.O.clone()))?
            .insert(table_key, table);
        Ok(())
    }
    /// 删除指定表；库或表不存在时返回错误。
    pub fn DeleteTable(&mut self, schema: &ast::CIStr, table: &ast::CIStr) -> Result<(), Error> {
        let schema_key = self.ciStr2Key(schema);
        let table_key = self.ciStr2Key(table);
        let tables = self
            .tables
            .get_mut(&schema_key)
            .ok_or_else(|| Error::DatabaseNotExists(schema.O.clone()))?;
        tables
            .remove(&table_key)
            .ok_or_else(|| Error::TableNotExists(schema.O.clone(), table.O.clone()))?;
        Ok(())
    }
    /// 返回当前所有库名键。
    pub fn AllSchemaNames(&self) -> Vec<String> {
        self.dbs.keys().cloned().collect()
    }
    /// 返回指定库下所有表名键。
    pub fn AllTableNamesOfSchema(&self, schema: &ast::CIStr) -> Result<Vec<String>, Error> {
        let key = self.ciStr2Key(schema);
        Ok(self
            .tables
            .get(&key)
            .ok_or_else(|| Error::DatabaseNotExists(schema.O.clone()))?
            .keys()
            .cloned()
            .collect())
    }
}

/// 对 `InfoStore` 的适配器，提供与 Go InfoSchema 风格接近的查询接口。
pub struct InfoStoreAdaptor<'a> {
    /// 被包装的 InfoStore 引用。
    pub inner: &'a InfoStore,
}
impl InfoStoreAdaptor<'_> {
    /// 按库名查找，返回 `(可选信息, 是否存在)`。
    pub fn SchemaByName(&self, schema: &ast::CIStr) -> (Option<&model::DBInfo>, bool) {
        let value = self.inner.SchemaByName(schema);
        (value, value.is_some())
    }
    /// 判断库下是否存在指定表。
    pub fn TableExists(&self, schema: &ast::CIStr, table: &ast::CIStr) -> bool {
        self.inner.TableByName(schema, table).is_ok()
    }
    /// 按名查找表并包装为 `TableHandle`（持有克隆的 TableInfo）。
    pub fn TableByName(
        &self,
        schema: &ast::CIStr,
        table: &ast::CIStr,
    ) -> Result<TableHandle, Error> {
        Ok(TableHandle(self.inner.TableByName(schema, table)?.Clone()))
    }
    /// 按名返回表信息引用。
    pub fn TableInfoByName(
        &self,
        schema: &ast::CIStr,
        table: &ast::CIStr,
    ) -> Result<&model::TableInfo, Error> {
        self.inner.TableByName(schema, table)
    }
}
/// 持有一份表信息克隆的句柄，便于调用方按值使用。
#[derive(Clone)]
pub struct TableHandle(pub model::TableInfo);
