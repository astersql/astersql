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

// TiDB Driver 实现：基于会话运行时管理预处理语句与文本执行。
//
// `TiDBDriver` 通过 `TiDBStore` 创建会话；`TiDBContext` 持有语句表，
// 支持 Prepare/Execute、沙箱密码策略，以及会话状态编解码迁移。

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Eq, PartialEq)]
/// Driver 层简易错误消息包装。
pub struct Error(pub String);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Debug, PartialEq)]
/// 预处理参数 / 结果单元格取值。
pub enum Expression {
    /// SQL NULL。
    Null,
    /// 有符号整数。
    Signed(i64),
    /// 无符号整数。
    Unsigned(u64),
    /// 浮点值。
    Float(f64),
    /// 字节/字符串载荷。
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 结果列元数据：名称与 MySQL 列类型字节。
pub struct ColumnInfo {
    pub name: String,
    pub column_type: u8,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 会话级 SQL 警告（级别 + 消息）。
pub struct SqlWarning {
    pub level: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 语句执行统计：次数与影响行数。
pub struct StatementStats {
    pub executions: u64,
    pub affected_rows: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 可序列化的预处理语句元信息（会话迁移用）。
pub struct PreparedStmtInfo {
    pub stmt_text: String,
    pub stmt_db: String,
    pub name: String,
    pub param_types: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 会话状态快照：当前已 prepare 的语句映射。
pub struct SessionStates {
    pub prepared_stmts: HashMap<u32, PreparedStmtInfo>,
}

#[derive(Clone, Debug)]
/// Prepare 成功后的返回：语句 ID、参数个数、列信息与库名。
pub struct PrepareResult {
    pub id: u32,
    pub param_count: usize,
    pub columns: Vec<ColumnInfo>,
    pub database: String,
}

/// 结果集：关闭时释放底层资源。
pub trait ResultSet: Send {
    fn close(&mut self) -> Result<(), Error>;
}
/// 游标结果集：关闭迭代器并上报 RU（资源单位）增量。
pub trait CursorResultSet: ResultSet {
    fn close_iterator(&mut self);
    fn report_ru_delta(&mut self);
}
/// 行容器：可分离内存跟踪器并关闭。
pub trait RowContainer: Send {
    fn detach_trackers(&mut self);
    fn close(&mut self) -> Result<(), Error>;
}

/// TiDB 会话运行时：配置连接、Prepare/Execute、警告与库切换等。
pub trait TiDBSessionRuntime: Send + Sync {
    fn configure(&self, connection_id: u64, capability: u32, collation: u8) -> Result<(), Error>;
    fn prepare(&self, sql: &str) -> Result<PrepareResult, Error>;
    fn execute_prepared(
        &self,
        id: u32,
        args: &[Expression],
    ) -> Result<Option<Box<dyn ResultSet>>, Error>;
    fn drop_prepared(&self, id: u32) -> Result<(), Error>;
    fn execute_statement(
        &self,
        statement: &str,
        non_transactional: bool,
    ) -> Result<Option<Box<dyn ResultSet>>, Error>;
    fn field_list(&self, table: &str) -> Result<Vec<ColumnInfo>, Error>;
    fn warnings(&self) -> Vec<SqlWarning>;
    fn statement_stats(&self) -> StatementStats;
    fn sandbox_mode(&self) -> bool;
    fn restricted_sql(&self) -> bool;
    fn close(&self) -> Result<(), Error>;
    fn prepared_metadata(&self) -> Result<HashMap<u32, PreparedStmtInfo>, Error>;
    fn next_prepared_id(&self) -> u32;
    fn set_next_prepared_id(&self, id: u32);
    fn current_db(&self) -> String;
    fn set_current_db(&self, database: &str);
    fn prepare_named(&self, name: &str, sql: &str) -> Result<(), Error>;
}

/// 存储抽象：为每个连接创建独立会话运行时。
pub trait TiDBStore: Send + Sync {
    fn create_session(&self) -> Result<Arc<dyn TiDBSessionRuntime>, Error>;
}

/// TiDB Driver：持有 Store，用于 OpenCtx 打开连接上下文。
pub struct TiDBDriver {
    store: Arc<dyn TiDBStore>,
}
/// 用给定 Store 构造 TiDBDriver。
pub fn NewTiDBDriver(store: Arc<dyn TiDBStore>) -> TiDBDriver {
    TiDBDriver { store }
}

/// 单连接上下文：会话运行时 + 本地语句表。
pub struct TiDBContext {
    /// 会话运行时句柄。
    pub session: Arc<dyn TiDBSessionRuntime>,
    /// 本地语句 ID → 语句对象。
    stmts: Mutex<HashMap<u32, Arc<Mutex<TiDBStatement>>>>,
}

/// 单个预处理语句：参数绑定、游标结果集与行容器状态。
pub struct TiDBStatement {
    id: u32,
    num_params: usize,
    bound_params: Vec<Option<Vec<u8>>>,
    params_type: Vec<u8>,
    runtime: Arc<dyn TiDBSessionRuntime>,
    result_set: Option<Box<dyn CursorResultSet>>,
    row_container: Option<Box<dyn RowContainer>>,
    sql: String,
    has_active_cursor: bool,
}

impl TiDBStatement {
    /// 返回语句 ID（协议侧为 i32）。
    pub fn ID(&self) -> i32 {
        self.id as i32
    }
    /// 用给定参数执行已 prepare 的语句。
    pub fn Execute(&self, args: &[Expression]) -> Result<Option<Box<dyn ResultSet>>, Error> {
        self.runtime.execute_prepared(self.id, args)
    }
    /// COM_STMT_SEND_LONG_DATA：向指定参数槽追加字节。
    pub fn AppendParam(&mut self, param_id: usize, data: &[u8]) -> Result<(), Error> {
        let slot = self
            .bound_params
            .get_mut(param_id)
            .ok_or_else(|| Error("wrong arguments to stmt_send_longdata".into()))?;
        // 首次绑定时分配缓冲区，后续 SEND_LONG_DATA 追加。
        let value = slot.get_or_insert_with(Vec::new);
        value.extend_from_slice(data);
        Ok(())
    }
    /// 参数个数。
    pub fn NumParams(&self) -> usize {
        self.num_params
    }
    /// 已绑定的长数据参数槽。
    pub fn BoundParams(&self) -> &[Option<Vec<u8>>] {
        &self.bound_params
    }
    /// 设置参数类型字节序列（协议侧 params_type）。
    pub fn SetParamsType(&mut self, params_type: Vec<u8>) {
        self.params_type = params_type;
    }
    /// 读取参数类型字节序列。
    pub fn GetParamsType(&self) -> &[u8] {
        &self.params_type
    }
    /// 保存游标结果集（FETCH 场景）。
    pub fn StoreResultSet(&mut self, result_set: Box<dyn CursorResultSet>) {
        self.result_set = Some(result_set);
    }
    /// 取得可变游标结果集引用。
    pub fn GetResultSet(&mut self) -> Option<&mut (dyn CursorResultSet + '_)> {
        match self.result_set.as_mut() {
            Some(value) => Some(value.as_mut()),
            None => None,
        }
    }
    /// 重置绑定参数与游标，并关闭行容器。
    pub fn Reset(&mut self) -> Result<(), Error> {
        // 清空长数据绑定并关闭游标/行容器。
        self.bound_params.fill(None);
        self.has_active_cursor = false;
        if let Some(mut result_set) = self.result_set.take() {
            result_set.report_ru_delta();
            result_set.close_iterator();
        }
        if let Some(mut container) = self.row_container.take() {
            container.detach_trackers();
            container.close()?;
        }
        Ok(())
    }
    /// 关闭游标与行容器后从会话侧 drop；不施加 Reset 的绑定参数/游标状态副作用。
    pub fn Close(&mut self) -> Result<(), Error> {
        if let Some(result_set) = self.result_set.as_mut() {
            result_set.report_ru_delta();
            result_set.close_iterator();
        }
        if let Some(container) = self.row_container.as_mut() {
            container.detach_trackers();
            container.close()?;
        }
        self.runtime.drop_prepared(self.id)
    }
    /// 是否存在未取完的游标。
    pub fn GetCursorActive(&self) -> bool {
        self.has_active_cursor
    }
    /// 标记游标是否活跃。
    pub fn SetCursorActive(&mut self, active: bool) {
        self.has_active_cursor = active;
    }
    /// 保存行容器。
    pub fn StoreRowContainer(&mut self, container: Box<dyn RowContainer>) {
        self.row_container = Some(container);
    }
    /// 取得可变行容器引用。
    pub fn GetRowContainer(&mut self) -> Option<&mut (dyn RowContainer + '_)> {
        match self.row_container.as_mut() {
            Some(value) => Some(value.as_mut()),
            None => None,
        }
    }
    /// 原始 SQL 文本。
    pub fn SQL(&self) -> &str {
        &self.sql
    }
}

impl TiDBDriver {
    /// 创建会话并配置连接能力位与校对规则，返回 TiDBContext。
    pub fn OpenCtx(
        &self,
        conn_id: u64,
        capability: u32,
        collation: u8,
    ) -> Result<TiDBContext, Error> {
        let session = self.store.create_session()?;
        session.configure(conn_id, capability, collation)?;
        Ok(TiDBContext {
            session,
            stmts: Mutex::new(HashMap::new()),
        })
    }
}

impl TiDBContext {
    /// 读取会话语句统计。
    pub fn GetStmtStats(&self) -> StatementStats {
        self.session.statement_stats()
    }
    /// 读取会话警告列表。
    pub fn GetWarnings(&self) -> Vec<SqlWarning> {
        self.session.warnings()
    }
    /// 警告条数（截断到 u16）。
    pub fn WarningCount(&self) -> u16 {
        self.session.warnings().len().min(u16::MAX as usize) as u16
    }
    /// 沙箱模式：仅允许改密相关语句，否则要求先改密码。
    pub fn checkSandBoxMode(&self, statement: &str) -> Result<(), Error> {
        // 沙箱且未放宽 SQL 时，只允许 SET PASSWORD / ALTER USER。
        if self.session.sandbox_mode() && !self.session.restricted_sql() {
            let normalized = statement.trim_start().to_ascii_uppercase();
            let has_keyword_prefix = |prefix: &str| {
                normalized.strip_prefix(prefix).is_some_and(|rest| {
                    rest.chars()
                        .next()
                        .is_none_or(|ch| !(ch.is_ascii_alphanumeric() || ch == '_'))
                })
            };
            if !(has_keyword_prefix("SET PASSWORD") || has_keyword_prefix("ALTER USER")) {
                return Err(Error("must change password".into()));
            }
        }
        Ok(())
    }
    /// 先做沙箱检查，再执行文本语句（可标记非事务路径）。
    pub fn ExecuteStmt(
        &self,
        statement: &str,
        non_transactional: bool,
    ) -> Result<Option<Box<dyn ResultSet>>, Error> {
        self.checkSandBoxMode(statement)?;
        self.session.execute_statement(statement, non_transactional)
    }
    /// 关闭全部本地语句后关闭会话；与 Go 的 terror.Call/Session.Close 一样忽略清理错误。
    pub fn Close(&self) -> Result<(), Error> {
        let statements: Vec<_> = self
            .stmts
            .lock()
            .map_err(|_| Error("statement lock poisoned".into()))?
            .drain()
            .map(|(_, stmt)| stmt)
            .collect();
        for statement in statements {
            if let Ok(mut statement) = statement.lock() {
                let _ = statement.Close();
            }
        }
        let _ = self.session.close();
        Ok(())
    }
    /// COM_FIELD_LIST：返回表列信息。
    pub fn FieldList(&self, table: &str) -> Result<Vec<ColumnInfo>, Error> {
        self.session.field_list(table)
    }
    /// 按语句 ID 查找本地预处理语句。
    pub fn GetStatement(&self, stmt_id: i32) -> Option<Arc<Mutex<TiDBStatement>>> {
        u32::try_from(stmt_id)
            .ok()
            .and_then(|id| self.stmts.lock().ok()?.get(&id).cloned())
    }
    /// Prepare SQL，登记本地语句，返回语句句柄、结果列与参数占位列。
    pub fn Prepare(
        &self,
        sql: &str,
    ) -> Result<(Arc<Mutex<TiDBStatement>>, Vec<ColumnInfo>, Vec<ColumnInfo>), Error> {
        let prepared = self.session.prepare(sql)?;
        // 参数占位列：类型 252（BLOB）占位，个数与 param_count 一致。
        let params = vec![
            ColumnInfo {
                name: String::new(),
                column_type: 252
            };
            prepared.param_count
        ];
        let statement = Arc::new(Mutex::new(TiDBStatement {
            id: prepared.id,
            num_params: prepared.param_count,
            bound_params: vec![None; prepared.param_count],
            params_type: Vec::new(),
            runtime: self.session.clone(),
            result_set: None,
            row_container: None,
            sql: sql.to_owned(),
            has_active_cursor: false,
        }));
        self.stmts
            .lock()
            .map_err(|_| Error("statement lock poisoned".into()))?
            .insert(prepared.id, statement.clone());
        Ok((statement, prepared.columns, params))
    }
    /// 编码会话状态；若仍有绑定参数或未取完游标则拒绝迁移。
    pub fn EncodeSessionStates(&self) -> Result<SessionStates, Error> {
        let mut states = SessionStates {
            prepared_stmts: self.session.prepared_metadata()?,
        };
        for (id, statement) in self
            .stmts
            .lock()
            .map_err(|_| Error("statement lock poisoned".into()))?
            .iter()
        {
            let statement = statement
                .lock()
                .map_err(|_| Error("statement lock poisoned".into()))?;
            // 仍有绑定参数或未取完游标时不可迁移会话。
            if statement.BoundParams().iter().any(Option::is_some) {
                return Err(Error(
                    "cannot migrate session: prepared statements have bound params".into(),
                ));
            }
            if statement.GetCursorActive() {
                return Err(Error(
                    "cannot migrate session: prepared statements have unfetched rows".into(),
                ));
            }
            states
                .prepared_stmts
                .get_mut(id)
                .ok_or_else(|| Error(format!("prepared statement {id} not found")))?
                .param_types = statement.GetParamsType().to_vec();
        }
        Ok(states)
    }
    /// 按快照重建预处理语句，结束后恢复 next_id 与当前库。
    pub fn DecodeSessionStates(&self, states: &SessionStates) -> Result<(), Error> {
        if states.prepared_stmts.is_empty() {
            return Ok(());
        }
        // 重建前保存 next_id 与当前库，失败或成功后都恢复。
        let saved_id = self.session.next_prepared_id();
        let saved_db = self.session.current_db();
        let result = (|| {
            for (id, info) in &states.prepared_stmts {
                // 将 next_id 设为 id-1，使随后 Prepare 得到期望 id。
                self.session.set_next_prepared_id(id.wrapping_sub(1));
                self.session.set_current_db(&info.stmt_db);
                if info.name.is_empty() {
                    let (statement, _, _) = self.Prepare(&info.stmt_text)?;
                    statement
                        .lock()
                        .map_err(|_| Error("statement lock poisoned".into()))?
                        .SetParamsType(info.param_types.clone());
                } else {
                    self.session.prepare_named(&info.name, &info.stmt_text)?;
                }
            }
            Ok(())
        })();
        self.session.set_next_prepared_id(saved_id.wrapping_sub(1));
        self.session.set_current_db(&saved_db);
        result
    }
}
