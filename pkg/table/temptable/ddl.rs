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

// 本地临时表 DDL（对应 Go `pkg/table/temptable` 中 DDL 路径）。
//
// 在会话内创建/删除/截断 LOCAL TEMPORARY TABLE：分配全局 table ID、
// 注册到会话 SessionTables，并清理 MemBuffer 中该表前缀下的键值。

use crate::infoschema::{
    CiString, DbInfo, SchemaState, SessionVarsProvider, Table, TableInfo, TempTableError,
    ensure_local_temporary_tables, get_local_temporary_tables,
};
use crate::interceptor::{MemBuffer, Retriever, encode_table_prefix};
use std::sync::Arc;

/// 存储抽象：开启事务以获取 MemBuffer，并分配全局 ID。
pub trait Store: Send + Sync {
    /// 开启事务（事务：一组原子读写）；此处主要用于拿到会话 MemBuffer。
    fn begin(&self, start_ts: u64) -> Result<Arc<MemBuffer>, TempTableError>;
    /// 分配全局唯一 table ID，避免与持久化表键前缀冲突。
    fn generate_global_id(&self) -> Result<i64, TempTableError>;
}

/// 会话上下文：提供会话变量与 Store。
pub trait SessionContext: SessionVarsProvider {
    /// 返回底层 Store。
    fn store(&self) -> Arc<dyn Store>;
}

/// 本地临时表 DDL 接口。
pub trait TemporaryTableDdl: Send + Sync {
    /// 在指定库下创建本地临时表并分配 ID。
    fn create_local_temporary_table(
        &self,
        database: Arc<DbInfo>,
        info: &mut TableInfo,
    ) -> Result<(), TempTableError>;
    /// 删除本地临时表元数据并清除其 MemBuffer 记录。
    fn drop_local_temporary_table(
        &self,
        schema: &CiString,
        table_name: &CiString,
    ) -> Result<(), TempTableError>;
    /// 截断：换新 table ID 重建元数据，并清除旧表数据。
    fn truncate_local_temporary_table(
        &self,
        schema: &CiString,
        table_name: &CiString,
    ) -> Result<(), TempTableError>;
}

/// 基于会话上下文的临时表 DDL 实现。
pub struct SessionTemporaryTableDdl {
    context: Arc<dyn SessionContext>,
}

impl SessionTemporaryTableDdl {
    /// 构造 DDL 执行器。
    pub fn new(context: Arc<dyn SessionContext>) -> Self {
        Self { context }
    }

    /// 扫描并删除某 table_id 前缀下全部会话键（先收集再删，避免迭代中变异）。
    fn clear_temporary_table_records(&self, table_id: i64) -> Result<(), TempTableError> {
        let Some(session_data) = get_session_data(self.context.as_ref()) else {
            return Ok(());
        };
        // 表键区间 [prefix(id), prefix(id+1))。
        let table_prefix = encode_table_prefix(table_id);
        let end_key = encode_table_prefix(table_id + 1);
        let mut iterator = session_data.iter(&table_prefix, &end_key)?;
        let mut keys = Vec::with_capacity(16);
        while iterator.valid() {
            let key = iterator.key();
            if !key.starts_with(&table_prefix) {
                break;
            }
            // Collect before deleting: mutating the backing mem-buffer while its
            // iterator is active is unsafe and differs from Go's two-phase loop.
            keys.push(key.to_vec());
            iterator.next()?;
        }
        iterator.close();
        for key in keys {
            session_data.delete_table_key(table_id, &key)?;
        }
        Ok(())
    }
}

// TemporaryTableDdl 实现：创建时 ensure 会话数据；删除/截断前校验存在性。
impl TemporaryTableDdl for SessionTemporaryTableDdl {
    fn create_local_temporary_table(
        &self,
        database: Arc<DbInfo>,
        info: &mut TableInfo,
    ) -> Result<(), TempTableError> {
        ensure_session_data(self.context.as_ref())?;
        info.db_id = database.id;
        let table = new_temporary_table_from_table_info(self.context.as_ref(), info)?;
        ensure_local_temporary_tables(self.context.as_ref()).add_table(database, table)
    }

    fn drop_local_temporary_table(
        &self,
        schema: &CiString,
        table_name: &CiString,
    ) -> Result<(), TempTableError> {
        let table =
            check_local_temporary_exists_and_return(self.context.as_ref(), schema, table_name)?;
        get_local_temporary_tables(self.context.as_ref())
            .expect("existence check guarantees session tables")
            .remove_table(schema, table_name);
        self.clear_temporary_table_records(table.metadata().id)
    }

    fn truncate_local_temporary_table(
        &self,
        schema: &CiString,
        table_name: &CiString,
    ) -> Result<(), TempTableError> {
        let old_table =
            check_local_temporary_exists_and_return(self.context.as_ref(), schema, table_name)?;
        let old_info = old_table.metadata();
        let mut new_info = (*old_info).clone();
        let new_table = new_temporary_table_from_table_info(self.context.as_ref(), &mut new_info)?;
        let local_tables = get_local_temporary_tables(self.context.as_ref())
            .expect("existence check guarantees session tables");
        let database = local_tables
            .schema_by_id(old_info.db_id)
            .ok_or(TempTableError::SchemaNotExists(old_info.db_id))?;
        local_tables.remove_table(schema, table_name);
        // As in Go, failure to add the replacement happens after removal. The
        // caller sees the AddTable error rather than an implicit rollback.
        local_tables.add_table(database, new_table)?;
        self.clear_temporary_table_records(old_info.id)
    }
}

/// 读取会话上已绑定的临时表 MemBuffer（可能尚未创建）。
pub fn get_session_data(context: &dyn SessionVarsProvider) -> Option<Arc<MemBuffer>> {
    context
        .session_variables()
        .temporary_table_data
        .lock()
        .unwrap()
        .clone()
}

/// 确保会话拥有临时表数据缓冲；缺失时用 StartTS=0 开启事务获得。
pub fn ensure_session_data(context: &dyn SessionContext) -> Result<Arc<MemBuffer>, TempTableError> {
    let variables = context.session_variables();
    let mut data = variables.temporary_table_data.lock().unwrap();
    if data.is_none() {
        // This transaction exists solely to obtain its memory buffer. StartTS=0
        // preserves the special TiKV option used by the Go implementation.
        *data = Some(context.store().begin(0)?);
    }
    Ok(Arc::clone(data.as_ref().unwrap()))
}

/// 从 TableInfo 构造临时表：分配全局 ID 并将 SchemaState 置为 Public。
pub fn new_temporary_table_from_table_info(
    context: &dyn SessionContext,
    table_info: &mut TableInfo,
) -> Result<Arc<Table>, TempTableError> {
    // Local temporary tables use a real globally allocated table ID so their
    // encoded key prefix can never collide with a persistent table.
    table_info.id = context.store().generate_global_id()?;
    table_info.state = SchemaState::Public;
    Ok(Arc::new(Table::from_metadata(table_info.clone())))
}

/// 检查本地临时表是否存在；不存在则返回 TableNotExists。
pub fn check_local_temporary_exists_and_return(
    context: &dyn SessionVarsProvider,
    schema: &CiString,
    table_name: &CiString,
) -> Result<Arc<Table>, TempTableError> {
    let identifier = format!("{schema}.{table_name}");
    get_local_temporary_tables(context)
        .and_then(|tables| tables.table_by_name(schema, table_name))
        .ok_or(TempTableError::TableNotExists(identifier))
}

/// 工厂：返回会话绑定的 TemporaryTableDdl。
pub fn get_temporary_table_ddl(context: Arc<dyn SessionContext>) -> Arc<dyn TemporaryTableDdl> {
    Arc::new(SessionTemporaryTableDdl::new(context))
}
