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

// Notifier 事件存储与会话事务抽象。
//
// 提供共享逻辑表实现 `TableStore`：事件按 JSON 持久化，并在 Begin/Commit
// 边界上先 validate 再 apply，使 handler 副作用与 `processedByFlag` 更新原子提交。

use crate::SchemaChange;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock, Mutex};

/// Notifier 存储与会话操作可能返回的错误。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 尚未就绪，调用方应稍后重试。
    #[error("not ready, retry later")]
    NotReadyRetryLater,
    /// 带消息的业务/测试注入错误。
    #[error("{0}")]
    Message(String),
    /// JSON 编解码失败。
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// DDL 内部会话或 SQL 执行失败。
    #[error(transparent)]
    Session(#[from] ddl_session::SessionError),
}

impl Error {
    /// 是否为“稍后重试”类错误。
    pub fn is_not_ready(&self) -> bool {
        matches!(self, Self::NotReadyRetryLater)
    }
}

/// 再导出 SQL 参数与结果行，供真实 DDL session 的 handler 使用。
pub use ddl_session::{Row as SqlRow, SqlValue};

trait SqlSessionBackend: Send + Sync {
    fn begin(&self, pessimistic: bool) -> Result<(), Error>;
    fn commit(&self) -> Result<(), Error>;
    fn rollback(&self);
    fn execute(&self, query: &str, arguments: &[SqlValue]) -> Result<Vec<SqlRow>, Error>;
}

struct DdlSessionBackend {
    session: Arc<ddl_session::Session>,
}

impl SqlSessionBackend for DdlSessionBackend {
    fn begin(&self, pessimistic: bool) -> Result<(), Error> {
        let context = ddl_session::ExecutionContext::default();
        if pessimistic {
            self.session.begin_pessimistic(&context)?;
        } else {
            self.session.begin(&context)?;
        }
        Ok(())
    }

    fn commit(&self) -> Result<(), Error> {
        self.session
            .commit(&ddl_session::ExecutionContext::default())?;
        Ok(())
    }

    fn rollback(&self) {
        self.session.rollback();
    }

    fn execute(&self, query: &str, arguments: &[SqlValue]) -> Result<Vec<SqlRow>, Error> {
        let context = ddl_session::ExecutionContext {
            request_source: ddl_session::RequestSource::Ddl,
        };
        let Some(mut rows) = self
            .session
            .context()
            .execute_internal(&context, query, arguments)?
        else {
            return Ok(Vec::new());
        };
        let mut result = Vec::new();
        let drain_result = (|| {
            loop {
                let batch = rows.drain(1024)?;
                if batch.is_empty() {
                    break;
                }
                result.extend(batch);
            }
            Ok::<(), ddl_session::SessionError>(())
        })();
        let close_result = rows.close();
        if let Err(error) = drain_result {
            return Err(error.into());
        }
        close_result?;
        Ok(result)
    }
}

/// 事务内挂起的操作：提交前统一校验，提交时再真正落库。
trait TransactionOperation: Send {
    fn validate(&self) -> Result<(), Error>;
    fn apply(self: Box<Self>);
}

/// Session 内部可变状态：事务标志、悲观事务、失败注入与挂起操作队列。
#[derive(Default)]
struct SessionState {
    in_transaction: bool,
    pessimistic: bool,
    fail_next_begin: Option<String>,
    fail_next_commit: Option<String>,
    operations: Vec<Box<dyn TransactionOperation>>,
}

/// 通知器 handler 与 store 适配器使用的事务边界。
///
/// 具体 SQL 适配器可将这些方法映射到真实 session；内置表存储采用
/// “先 validate 再 apply” 协议，保证 handler 副作用与 processed 位图原子提交。
/// Transaction boundary used by notifier handlers and store adapters.
///
/// A concrete SQL adapter can map these methods to its session implementation.
/// The bundled table store uses the same validation-before-apply protocol so
/// handler side effects and the processed bit commit atomically.
#[derive(Clone)]
pub struct Session {
    state: Arc<Mutex<SessionState>>,
    committed_effects: Arc<Mutex<Vec<String>>>,
    sql: Option<Arc<dyn SqlSessionBackend>>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(SessionState::default())),
            committed_effects: Arc::new(Mutex::new(Vec::new())),
            sql: None,
        }
    }
}

impl Session {
    /// 用真实 DDL 内部会话构造 notifier 会话。
    pub fn FromDDLSession(session: Arc<ddl_session::Session>) -> Self {
        Self {
            sql: Some(Arc::new(DdlSessionBackend { session })),
            ..Self::default()
        }
    }

    /// 在真实 DDL session 上执行 SQL；内存测试会话未配置 backend 时返回错误。
    pub fn ExecuteSQL(&self, query: &str, arguments: &[SqlValue]) -> Result<Vec<SqlRow>, Error> {
        self.sql
            .as_ref()
            .ok_or_else(|| Error::Message("SQL session backend is not configured".to_owned()))?
            .execute(query, arguments)
    }

    /// 开启乐观事务。
    pub fn Begin(&self) -> Result<(), Error> {
        self.begin(false)
    }
    /// 开启悲观事务（pessimistic：持锁直至提交/回滚）。
    pub fn BeginPessimistic(&self) -> Result<(), Error> {
        self.begin(true)
    }
    /// 内部：清空挂起操作并进入事务；可注入下一次 Begin 失败。
    fn begin(&self, pessimistic: bool) -> Result<(), Error> {
        let mut state = self.state.lock().expect("session mutex poisoned");
        if let Some(error) = state.fail_next_begin.take() {
            return Err(Error::Message(error));
        }
        if let Some(sql) = &self.sql {
            sql.begin(pessimistic)?;
        }
        state.operations.clear();
        state.in_transaction = true;
        state.pessimistic = pessimistic;
        Ok(())
    }

    /// 提交事务：先对全部挂起操作 validate，成功后再逐个 apply。
    pub fn Commit(&self) -> Result<(), Error> {
        let operations = {
            let mut state = self.state.lock().expect("session mutex poisoned");
            if !state.in_transaction {
                return Ok(());
            }
            if let Some(error) = state.fail_next_commit.take() {
                if let Some(sql) = &self.sql {
                    sql.rollback();
                }
                state.operations.clear();
                state.in_transaction = false;
                state.pessimistic = false;
                return Err(Error::Message(error));
            }
            // 提交前统一校验，任一步失败则整笔事务不落库。
            for operation in &state.operations {
                operation.validate()?;
            }
            if let Some(sql) = &self.sql {
                if let Err(error) = sql.commit() {
                    state.operations.clear();
                    state.in_transaction = false;
                    state.pessimistic = false;
                    return Err(error);
                }
            }
            state.in_transaction = false;
            state.pessimistic = false;
            std::mem::take(&mut state.operations)
        };
        // 锁外执行 apply，避免长时间持有 session mutex。
        for operation in operations {
            operation.apply();
        }
        Ok(())
    }

    /// 回滚：丢弃挂起操作并退出事务。
    pub fn Rollback(&self) {
        if let Some(sql) = &self.sql {
            sql.rollback();
        }
        let mut state = self.state.lock().expect("session mutex poisoned");
        state.operations.clear();
        state.in_transaction = false;
        state.pessimistic = false;
    }

    /// 当前是否处于悲观事务中。
    pub fn IsPessimistic(&self) -> bool {
        self.state
            .lock()
            .expect("session mutex poisoned")
            .pessimistic
    }
    /// 测试注入：下一次 Begin 返回给定错误消息。
    pub fn FailNextBegin(&self, message: impl Into<String>) {
        self.state
            .lock()
            .expect("session mutex poisoned")
            .fail_next_begin = Some(message.into());
    }
    /// 测试注入：下一次 Commit 返回给定错误消息。
    pub fn FailNextCommit(&self, message: impl Into<String>) {
        self.state
            .lock()
            .expect("session mutex poisoned")
            .fail_next_commit = Some(message.into());
    }

    /// 在当前事务中暂存一条 handler 可见副作用。
    ///
    /// Stages a handler-visible effect in the current transaction.
    pub fn StageEffect(&self, value: impl Into<String>) -> Result<(), Error> {
        self.stage(Box::new(EffectOperation {
            target: self.committed_effects.clone(),
            value: value.into(),
        }))
    }
    /// 返回已提交的副作用列表（测试观察用）。
    pub fn CommittedEffects(&self) -> Vec<String> {
        self.committed_effects
            .lock()
            .expect("effects mutex poisoned")
            .clone()
    }

    /// 事务内则入队；非事务则立即 validate+apply（自动提交语义）。
    fn stage(&self, operation: Box<dyn TransactionOperation>) -> Result<(), Error> {
        let mut state = self.state.lock().expect("session mutex poisoned");
        if state.in_transaction {
            state.operations.push(operation);
            Ok(())
        } else {
            operation.validate()?;
            operation.apply();
            Ok(())
        }
    }
}

/// 将字符串副作用追加到 Session 的已提交列表。
struct EffectOperation {
    target: Arc<Mutex<Vec<String>>>,
    value: String,
}
impl TransactionOperation for EffectOperation {
    fn validate(&self) -> Result<(), Error> {
        Ok(())
    }
    fn apply(self: Box<Self>) {
        self.target
            .lock()
            .expect("effects mutex poisoned")
            .push(self.value);
    }
}

#[derive(Clone)]
/// Session 工厂池：Get 创建、Put 时 Rollback 归还语义。
pub struct SessionPool {
    factory: Arc<dyn Fn() -> Session + Send + Sync>,
}
impl Default for SessionPool {
    fn default() -> Self {
        Self::New(Session::default)
    }
}
impl SessionPool {
    /// 使用自定义工厂创建 Session 池。
    pub fn New(factory: impl Fn() -> Session + Send + Sync + 'static) -> Self {
        Self {
            factory: Arc::new(factory),
        }
    }
    /// 从池中取出（新建）一个 Session。
    pub fn Get(&self) -> Session {
        (self.factory)()
    }
    /// 归还 Session：先 Rollback 清理未提交状态。
    pub fn Put(&self, session: Session) {
        session.Rollback();
    }
}

/// 关闭 List 游标时的回调（通常 Rollback 只读事务）。
pub type CloseFn = Box<dyn FnOnce() + Send>;

/// Schema 变更事件的持久化存储接口。
pub trait Store: Send + Sync {
    /// 插入一条 schema 变更；重复 (ddl_job_id, sub_job_id) 应失败。
    fn Insert(&self, session: &Session, change: &SchemaChange) -> Result<(), Error>;
    /// 在 old→new 校验下更新 processedByFlag（防多 owner 并发覆盖）。
    fn UpdateProcessed(
        &self,
        session: &Session,
        ddl_job_id: i64,
        sub_job_id: i64,
        old_processed_by: u64,
        new_processed_by: u64,
    ) -> Result<(), Error>;
    /// 开启事务删除指定变更并提交。
    fn DeleteAndCommit(
        &self,
        session: &Session,
        ddl_job_id: i64,
        sub_job_id: i64,
    ) -> Result<(), Error>;
    /// 打开有序扫描游标；返回结果集与关闭回调。
    fn List(&self, session: Session) -> (Box<dyn ListResult>, CloseFn);
    /// 当前存储中的变更条数。
    fn Count(&self) -> usize;
}

/// 列表读取结果：向调用方缓冲区填入下一批 SchemaChange。
pub trait ListResult: Send {
    /// 读取至多 `changes.len()` 条；已有槽位通过 overwrite_from 合并。
    fn Read(&mut self, changes: &mut [Option<SchemaChange>]) -> Result<usize, Error>;
}

#[derive(Clone)]
/// 持久化行：事件只保存 JSON 字节，读取时重新反序列化。
struct StoredChange {
    event: Vec<u8>,
    processed_by_flag: u64,
}

#[derive(Default)]
/// 内存表数据：按 (ddl_job_id, sub_job_id) 有序存放序列化行。
struct TableData {
    rows: BTreeMap<(i64, i64), StoredChange>,
}

/// 同一逻辑库表的多个 `OpenTableStore` 句柄共享一份表数据。
static TABLES: LazyLock<Mutex<HashMap<(String, String), Arc<Mutex<TableData>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone)]
/// 内置内存实现的 notifier 表存储。
pub struct TableStore {
    /// 逻辑库名（仅标识用途）。
    pub db: String,
    /// 逻辑表名（仅标识用途）。
    pub table: String,
    data: Arc<Mutex<TableData>>,
}

/// 打开一个逻辑 TableStore；同名库表的句柄共享持久化行。
pub fn OpenTableStore(db: impl Into<String>, table: impl Into<String>) -> Arc<dyn Store> {
    let db = db.into();
    let table = table.into();
    let data = TABLES
        .lock()
        .expect("table registry mutex poisoned")
        .entry((db.clone(), table.clone()))
        .or_insert_with(|| Arc::new(Mutex::new(TableData::default())))
        .clone();
    Arc::new(TableStore { db, table, data })
}

/// 事务内插入：validate 查重，apply 写入 BTreeMap。
struct InsertOperation {
    data: Arc<Mutex<TableData>>,
    key: (i64, i64),
    change: StoredChange,
}
impl TransactionOperation for InsertOperation {
    fn validate(&self) -> Result<(), Error> {
        if self
            .data
            .lock()
            .expect("table mutex poisoned")
            .rows
            .contains_key(&self.key)
        {
            return Err(Error::Message(format!(
                "duplicate schema change ({}, {})",
                self.key.0, self.key.1
            )));
        }
        Ok(())
    }
    fn apply(self: Box<Self>) {
        self.data
            .lock()
            .expect("table mutex poisoned")
            .rows
            .insert(self.key, self.change);
    }
}

/// 事务内更新 processedByFlag：validate 校验旧值匹配。
struct UpdateOperation {
    data: Arc<Mutex<TableData>>,
    key: (i64, i64),
    old: u64,
    new: u64,
}
impl TransactionOperation for UpdateOperation {
    fn validate(&self) -> Result<(), Error> {
        let data = self.data.lock().expect("table mutex poisoned");
        if data
            .rows
            .get(&self.key)
            .is_none_or(|row| row.processed_by_flag != self.old)
        {
            return Err(Error::Message(format!(
                "failed to update processed_by_flag, maybe the row has been updated by other owner. ddl_job_id: {}, sub_job_id: {}",
                self.key.0, self.key.1
            )));
        }
        Ok(())
    }
    fn apply(self: Box<Self>) {
        if let Some(row) = self
            .data
            .lock()
            .expect("table mutex poisoned")
            .rows
            .get_mut(&self.key)
        {
            row.processed_by_flag = self.new;
        }
    }
}

/// 事务内删除指定 (ddl_job_id, sub_job_id) 行。
struct DeleteOperation {
    data: Arc<Mutex<TableData>>,
    key: (i64, i64),
}
impl TransactionOperation for DeleteOperation {
    fn validate(&self) -> Result<(), Error> {
        Ok(())
    }
    fn apply(self: Box<Self>) {
        self.data
            .lock()
            .expect("table mutex poisoned")
            .rows
            .remove(&self.key);
    }
}

impl Store for TableStore {
    /// 实现：序列化检查后把 InsertOperation 挂到 session。
    fn Insert(&self, session: &Session, change: &SchemaChange) -> Result<(), Error> {
        // 在持久化边界先 MarshalJSON，与 Go 表存储在序列化失败时的行为对齐；
        // Marshal at the persistence boundary, matching the Go table store's
        // serialization failure behavior and persist the exact JSON representation.
        if session.sql.is_some() {
            return InsertSchemaChangeSQL(&self.db, &self.table, change, |query, args| {
                session.ExecuteSQL(query, args).map(|_| ())
            });
        }
        let event = change.event.MarshalJSON()?;
        session.stage(Box::new(InsertOperation {
            data: self.data.clone(),
            key: (change.ddlJobID, change.subJobID),
            change: StoredChange {
                event,
                processed_by_flag: 0,
            },
        }))
    }
    /// 实现：挂起 UpdateOperation，由 Commit 原子应用。
    fn UpdateProcessed(
        &self,
        session: &Session,
        ddl_job_id: i64,
        sub_job_id: i64,
        old_processed_by: u64,
        new_processed_by: u64,
    ) -> Result<(), Error> {
        if session.sql.is_some() {
            let select = format!(
                "SELECT processed_by_flag FROM {}.{} WHERE ddl_job_id = %? AND sub_job_id = %? FOR UPDATE",
                self.db, self.table
            );
            let rows = session.ExecuteSQL(
                &select,
                &[SqlValue::Integer(ddl_job_id), SqlValue::Integer(sub_job_id)],
            )?;
            let matches_old = rows.len() == 1
                && sql_u64(&rows[0], 0).is_ok_and(|value| value == old_processed_by);
            if !matches_old {
                return Err(update_conflict(ddl_job_id, sub_job_id));
            }
            let update = format!(
                "UPDATE {}.{} SET processed_by_flag = %? WHERE ddl_job_id = %? AND sub_job_id = %?",
                self.db, self.table
            );
            session.ExecuteSQL(
                &update,
                &[
                    SqlValue::Unsigned(new_processed_by),
                    SqlValue::Integer(ddl_job_id),
                    SqlValue::Integer(sub_job_id),
                ],
            )?;
            return Ok(());
        }
        session.stage(Box::new(UpdateOperation {
            data: self.data.clone(),
            key: (ddl_job_id, sub_job_id),
            old: old_processed_by,
            new: new_processed_by,
        }))
    }
    /// 实现：Begin → 挂起删除 → Commit；失败则 Rollback。
    fn DeleteAndCommit(
        &self,
        session: &Session,
        ddl_job_id: i64,
        sub_job_id: i64,
    ) -> Result<(), Error> {
        if session.sql.is_some() {
            session.Begin()?;
            let query = format!(
                "DELETE FROM {}.{} WHERE ddl_job_id = %? AND sub_job_id = %?",
                self.db, self.table
            );
            if let Err(error) = session.ExecuteSQL(
                &query,
                &[SqlValue::Integer(ddl_job_id), SqlValue::Integer(sub_job_id)],
            ) {
                session.Rollback();
                return Err(error);
            }
            if let Err(error) = session.Commit() {
                session.Rollback();
                return Err(error);
            }
            return Ok(());
        }
        session.Begin()?;
        if let Err(error) = session.stage(Box::new(DeleteOperation {
            data: self.data.clone(),
            key: (ddl_job_id, sub_job_id),
        })) {
            session.Rollback();
            return Err(error);
        }
        session.Commit()
    }
    /// 实现：返回带游标的 TableListResult，Close 时 Rollback。
    fn List(&self, session: Session) -> (Box<dyn ListResult>, CloseFn) {
        if session.sql.is_some() {
            let result = SqlListResult {
                db: self.db.clone(),
                table: self.table.clone(),
                cursor: (0, 0),
                started: false,
                session: session.clone(),
            };
            return (Box::new(result), Box::new(move || session.Rollback()));
        }
        let result = TableListResult {
            data: self.data.clone(),
            cursor: (0, 0),
            started: false,
            session: session.clone(),
        };
        (Box::new(result), Box::new(move || session.Rollback()))
    }
    /// 实现：返回当前行数。
    fn Count(&self) -> usize {
        self.data.lock().expect("table mutex poisoned").rows.len()
    }
}

fn update_conflict(ddl_job_id: i64, sub_job_id: i64) -> Error {
    Error::Message(format!(
        "failed to update processed_by_flag, maybe the row has been updated by other owner. ddl_job_id: {ddl_job_id}, sub_job_id: {sub_job_id}"
    ))
}

fn sql_value<'a>(row: &'a SqlRow, index: usize) -> Result<&'a SqlValue, Error> {
    row.values
        .get(index)
        .ok_or_else(|| Error::Message(format!("SQL row is missing column {index}")))
}

fn sql_i64(row: &SqlRow, index: usize) -> Result<i64, Error> {
    match sql_value(row, index)? {
        SqlValue::Integer(value) => Ok(*value),
        SqlValue::Unsigned(value) => i64::try_from(*value)
            .map_err(|_| Error::Message(format!("column {index} does not fit i64"))),
        value => Err(Error::Message(format!(
            "column {index} is not an integer: {value:?}"
        ))),
    }
}

fn sql_u64(row: &SqlRow, index: usize) -> Result<u64, Error> {
    match sql_value(row, index)? {
        SqlValue::Unsigned(value) => Ok(*value),
        SqlValue::Integer(value) => {
            u64::try_from(*value).map_err(|_| Error::Message(format!("column {index} is negative")))
        }
        value => Err(Error::Message(format!(
            "column {index} is not an unsigned integer: {value:?}"
        ))),
    }
}

fn sql_bytes(row: &SqlRow, index: usize) -> Result<&[u8], Error> {
    match sql_value(row, index)? {
        SqlValue::Bytes(value) => Ok(value),
        SqlValue::String(value) => Ok(value.as_bytes()),
        value => Err(Error::Message(format!(
            "column {index} is not JSON bytes: {value:?}"
        ))),
    }
}

fn overwrite_slot(slot: &mut Option<SchemaChange>, decoded: SchemaChange) {
    if let Some(existing) = slot.as_mut() {
        existing.event.overwrite_from(&decoded.event);
        existing.ddlJobID = decoded.ddlJobID;
        existing.subJobID = decoded.subJobID;
        existing.processedByFlag = decoded.processedByFlag;
    } else {
        *slot = Some(decoded);
    }
}

struct SqlListResult {
    db: String,
    table: String,
    cursor: (i64, i64),
    started: bool,
    session: Session,
}

impl ListResult for SqlListResult {
    fn Read(&mut self, changes: &mut [Option<SchemaChange>]) -> Result<usize, Error> {
        if !self.started {
            self.session.Begin()?;
            self.started = true;
        }
        let query = format!(
            "SELECT ddl_job_id, sub_job_id, schema_change, processed_by_flag FROM {}.{} WHERE (ddl_job_id, sub_job_id) > (%?, %?) ORDER BY ddl_job_id, sub_job_id LIMIT %?",
            self.db, self.table
        );
        let rows = self.session.ExecuteSQL(
            &query,
            &[
                SqlValue::Integer(self.cursor.0),
                SqlValue::Integer(self.cursor.1),
                SqlValue::Integer(changes.len() as i64),
            ],
        )?;
        if rows.len() > changes.len() {
            return Err(Error::Message(format!(
                "SQL List returned {} rows for a {}-slot buffer",
                rows.len(),
                changes.len()
            )));
        }
        for (slot, row) in changes.iter_mut().zip(&rows) {
            let mut event = crate::SchemaChangeEvent::default();
            event.UnmarshalJSON(sql_bytes(row, 2)?)?;
            overwrite_slot(
                slot,
                SchemaChange {
                    ddlJobID: sql_i64(row, 0)?,
                    subJobID: sql_i64(row, 1)?,
                    event,
                    processedByFlag: sql_u64(row, 3)?,
                },
            );
        }
        if let Some(last) = rows.last() {
            self.cursor = (sql_i64(last, 0)?, sql_i64(last, 1)?);
        }
        Ok(rows.len())
    }
}

/// 基于 BTreeMap 游标的批量 List 实现。
struct TableListResult {
    data: Arc<Mutex<TableData>>,
    cursor: (i64, i64),
    started: bool,
    session: Session,
}
impl ListResult for TableListResult {
    fn Read(&mut self, changes: &mut [Option<SchemaChange>]) -> Result<usize, Error> {
        // 首次 Read 时开启只读事务，CloseFn 负责 Rollback。
        if !self.started {
            self.session.Begin()?;
            self.started = true;
        }
        let rows: Vec<((i64, i64), StoredChange)> = self
            .data
            .lock()
            .expect("table mutex poisoned")
            .rows
            .range((
                std::ops::Bound::Excluded(self.cursor),
                std::ops::Bound::Unbounded,
            ))
            .take(changes.len())
            .map(|(key, row)| (*key, row.clone()))
            .collect();
        for (slot, (key, row)) in changes.iter_mut().zip(&rows) {
            let mut event = crate::SchemaChangeEvent::default();
            event.UnmarshalJSON(&row.event)?;
            let decoded = SchemaChange {
                ddlJobID: key.0,
                subJobID: key.1,
                event,
                processedByFlag: row.processed_by_flag,
            };
            // 复用槽位：合并事件字段并刷新 job id / 处理位图。
            overwrite_slot(slot, decoded);
        }
        if let Some((key, _)) = rows.last() {
            self.cursor = *key;
        }
        Ok(rows.len())
    }
}

/// Execute the same SQL insert on a borrowed worker transaction. The callback
/// must use its active session; this function never begins or commits a transaction.
pub fn InsertSchemaChangeSQL(
    db: &str,
    table: &str,
    change: &SchemaChange,
    execute: impl FnOnce(&str, &[SqlValue]) -> Result<(), Error>,
) -> Result<(), Error> {
    let event = change.event.MarshalJSON()?;
    let query = format!(
        "INSERT INTO {db}.{table} (ddl_job_id, sub_job_id, schema_change, processed_by_flag) VALUES (%?, %?, %?, 0)"
    );
    execute(
        &query,
        &[
            SqlValue::Integer(change.ddlJobID),
            SqlValue::Integer(change.subJobID),
            SqlValue::Bytes(event),
        ],
    )
}
