// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 测试用数据库驱动抽象：值类型、执行/查询、MockDB 与 Scan。
//
// 对齐 Go `database/sql` 风格的 Exec/Query/Prepare/Scan，底层委托
// [`Database`] trait（通常由 mockstore / TestKit 会话实现）。

use std::fmt;
use std::sync::Arc;

use astersql_session::runtime::RuntimeStaleReadState;

use crate::{TestError, TestResult};

/// SQL 绑定参数与结果单元格的统一值枚举。
#[derive(Clone, Debug, PartialEq)]
pub enum DbValue {
    /// SQL NULL，显示为 `<nil>`。
    Null,
    /// 布尔。
    Bool(bool),
    /// 有符号 64 位整数。
    I64(i64),
    /// 无符号 64 位整数。
    U64(u64),
    /// 浮点。
    F64(f64),
    /// 字节串（按 UTF-8 lossy 显示）。
    Bytes(Vec<u8>),
    /// 字符串。
    String(String),
}

impl fmt::Display for DbValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => formatter.write_str("<nil>"),
            Self::Bool(value) => value.fmt(formatter),
            Self::I64(value) => value.fmt(formatter),
            Self::U64(value) => value.fmt(formatter),
            Self::F64(value) => value.fmt(formatter),
            Self::Bytes(value) => formatter.write_str(&String::from_utf8_lossy(value)),
            Self::String(value) => formatter.write_str(value),
        }
    }
}

impl From<&str> for DbValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}
impl From<String> for DbValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}
impl From<i64> for DbValue {
    fn from(value: i64) -> Self {
        Self::I64(value)
    }
}
impl From<i32> for DbValue {
    fn from(value: i32) -> Self {
        Self::I64(value as i64)
    }
}
impl From<u64> for DbValue {
    fn from(value: u64) -> Self {
        Self::U64(value)
    }
}
impl From<f64> for DbValue {
    fn from(value: f64) -> Self {
        Self::F64(value)
    }
}
impl From<Vec<u8>> for DbValue {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(value)
    }
}
impl From<bool> for DbValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

/// DML/DDL 执行结果：影响行数与 last insert id。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExecutionResult {
    /// 影响行数。
    pub affected_rows: u64,
    /// 最近自增插入 ID。
    pub last_insert_id: u64,
}

/// 查询结果：列名与按行的 [`DbValue`] 矩阵。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueryRows {
    /// 列名列表。
    pub columns: Vec<String>,
    /// 数据行。
    pub rows: Vec<Vec<DbValue>>,
}

/// Prepared SELECT result-column metadata exposed by the canonical test session.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PreparedResultField {
    pub database_name: String,
    pub table_name: String,
    pub table_as_name: String,
    pub column_name: String,
    pub column_as_name: String,
}

/// Domain 统计上下文别名，供 ANALYZE 测试取共享状态。
pub type AnalyzeStatsContext = astersql_domain::DomainStatsContext;

impl QueryRows {
    /// 将所有单元格格式化为字符串行（NULL 显示为 `<nil>`）。
    pub fn string_rows(&self) -> Vec<Vec<String>> {
        self.rows
            .iter()
            .map(|row| row.iter().map(ToString::to_string).collect())
            .collect()
    }
}

/// 可执行 SQL 的存储/会话抽象（线程安全）。
pub trait Database: Send + Sync + 'static {
    /// Concrete session connection ID, when this database owns a SQL session.
    fn connection_id_for_test(&self) -> Option<u64> {
        None
    }

    /// 执行非查询语句。
    fn execute(&self, sql: &str, arguments: &[DbValue]) -> TestResult<ExecutionResult>;
    /// Execute a statement as an internal session request.
    ///
    /// Backends without a distinct internal-session scope may delegate to
    /// `execute`; canonical session adapters override this to enter restricted
    /// SQL scope for the duration of the statement.
    fn execute_internal(&self, sql: &str, arguments: &[DbValue]) -> TestResult<ExecutionResult> {
        self.execute(sql, arguments)
    }
    /// 执行查询并返回行集。
    fn query(&self, sql: &str, arguments: &[DbValue]) -> TestResult<QueryRows>;
    /// Execute a query through the internal-session scope.
    fn query_internal(&self, sql: &str, arguments: &[DbValue]) -> TestResult<QueryRows> {
        self.query(sql, arguments)
    }

    /// Prepare one statement and return its result-column metadata.
    fn prepare_statement(&self, _sql: &str) -> TestResult<(u64, Vec<PreparedResultField>)> {
        Err(TestError::new(
            "database does not expose prepared statement metadata",
        ))
    }

    /// Execute one statement previously returned by [`Database::prepare_statement`].
    fn execute_prepared_statement(
        &self,
        _statement_id: u64,
        _arguments: &[DbValue],
    ) -> TestResult<ExecutionResult> {
        Err(TestError::new(
            "database does not expose prepared statement execution",
        ))
    }

    /// Drop one statement previously returned by [`Database::prepare_statement`].
    fn drop_prepared_statement(&self, _statement_id: u64) -> TestResult {
        Err(TestError::new(
            "database does not expose prepared statement metadata",
        ))
    }

    /// 打开派生会话；默认无。
    fn create_session(&self) -> TestResult<Option<Arc<dyn Database>>> {
        Ok(None)
    }

    /// 关闭底层资源；默认空操作。
    fn close(&self) -> TestResult {
        Ok(())
    }

    /// 可选的 ANALYZE 统计上下文。
    fn analyze_stats_context(&self) -> Option<AnalyzeStatsContext> {
        None
    }

    /// Session MemTracker child count for leak assertions (Go GetChildrenForTest).
    /// 会话 MemTracker 子节点数，供泄漏断言（对应 Go GetChildrenForTest）。
    fn mem_tracker_children_for_test(&self) -> usize {
        0
    }

    /// Session root MemTracker bytes consumed after the latest statement.
    fn mem_tracker_bytes_consumed_for_test(&self) -> i64 {
        0
    }

    /// Latest statement MemTracker peak, matching Go MaxConsumed.
    fn mem_tracker_max_consumed_for_test(&self) -> i64 {
        0
    }

    /// Latest statement CTE temporary-file peak bytes.
    fn disk_tracker_max_consumed_for_test(&self) -> i64 {
        0
    }

    /// Whether statement-scoped CTE materialization state has been released.
    fn cte_storage_map_is_empty_for_test(&self) -> bool {
        true
    }

    /// Optimize one SELECT in the concrete session and return its root plan ID.
    fn optimize_root_plan_id_for_test(&self, _sql: &str) -> TestResult<i32> {
        Err(TestError::new(
            "database does not expose optimizer plan IDs",
        ))
    }

    /// Derive IndexMerge candidates before physical costing and return the Go-compatible digest.
    fn index_merge_path_digest_for_test(&self, _sql: &str) -> TestResult<String> {
        Err(TestError::new(
            "database does not expose IndexMerge candidate paths",
        ))
    }

    /// Effective (memory quota, max execution time) hints from the latest statement.
    fn last_statement_hints_for_test(&self) -> (i64, u64) {
        (0, 0)
    }

    /// Latest statement OOM fallback priority after finished actions are skipped.
    fn statement_oom_action_priority_for_test(&self) -> Option<i64> {
        None
    }

    /// Latest alternative-logical-plan signals: (decorrelated Apply, same-order IndexJoin).
    fn alternative_logical_plan_signals_for_test(&self) -> Option<(bool, bool)> {
        None
    }

    /// Planner statistics load statuses recorded by the latest session statements.
    fn stats_load_statuses_for_test(&self) -> Vec<(i64, i64, bool, String)> {
        Vec::new()
    }

    /// Return the session's previous statement trace ID.
    fn prev_trace_id_for_test(&self) -> Vec<u8> {
        Vec::new()
    }

    /// Clear the session's previous statement trace ID.
    fn reset_prev_trace_id_for_test(&self) -> TestResult {
        Ok(())
    }

    /// Active transaction isolation observed by the concrete session.
    fn transaction_isolation_for_test(&self) -> Option<String> {
        None
    }

    /// Concrete transaction debug string, matching the production formatter.
    fn transaction_debug_string_for_test(&self) -> Option<String> {
        None
    }

    /// Point-read cache entries retained by the active transaction snapshot.
    fn snapshot_cache_size_for_test(&self) -> usize {
        0
    }

    /// Toggle the session row encoder used by subsequent DML writes.
    fn set_row_encoder_enabled_for_test(&self, _enabled: bool) -> TestResult {
        Err(TestError::new("database does not expose row encoder state"))
    }

    /// Latest MySQL OK-packet message exposed by the concrete session.
    fn last_message_for_test(&self) -> String {
        String::new()
    }

    /// SQL text stored in Go's `sessionctx.QueryString` slot.
    fn query_string_for_test(&self) -> String {
        String::new()
    }

    /// Set MySQL client capability flags for this session.
    fn set_client_capability_for_test(&self, _capability: u32) -> TestResult {
        Err(TestError::new(
            "database does not expose client capability state",
        ))
    }

    /// Apply the client handshake collation to this session.
    fn set_connection_collation_for_test(&self, _collation: u8) -> TestResult {
        Err(TestError::new(
            "database does not expose connection collation state",
        ))
    }

    fn stale_read_state_for_test(&self) -> Option<RuntimeStaleReadState> {
        None
    }

    /// Enable or disable the session-local inspection-table snapshot cache.
    fn set_inspection_table_cache_enabled_for_test(&self, _enabled: bool) -> TestResult {
        Err(TestError::new(
            "database does not expose inspection table cache state",
        ))
    }

    /// Return the cached row count for one inspection table.
    fn inspection_table_cache_row_count_for_test(&self, _table: &str) -> TestResult<Option<usize>> {
        Err(TestError::new(
            "database does not expose inspection table cache state",
        ))
    }

    /// Mutate one cached inspection-table cell.
    fn set_inspection_table_cache_value_for_test(
        &self,
        _table: &str,
        _row: usize,
        _column: &str,
        _value: String,
    ) -> TestResult {
        Err(TestError::new(
            "database does not expose inspection table cache state",
        ))
    }

    /// Flush this session's index-usage samples to the node collector.
    fn report_usage_stats_for_test(&self) -> TestResult {
        Ok(())
    }

    /// Authenticate this concrete test session as an existing account.
    fn authenticate_user_for_test(&self, _username: &str, _hostname: &str) -> TestResult {
        Err(TestError::new(
            "database does not expose test-session authentication",
        ))
    }

    /// Inject a one-shot commit failure into this exact test session.
    fn inject_next_dml_commit_error_for_test(&self, _message: String) -> TestResult {
        Err(TestError::new(
            "database does not expose DML commit failure injection",
        ))
    }

    /// Install retryInfo AUTO_INCREMENT IDs on a concrete test session.
    fn set_retry_auto_increment_ids_for_test(&self, _ids: Vec<u64>) -> TestResult {
        Err(TestError::new(
            "database does not expose retry AUTO_INCREMENT state",
        ))
    }
}

/// 预编译语句：将 SQL 与参数转发到 [`Database`]。
///
/// 与 Go driver 的 `NumInput() == -1` 保持一致，参数个数与 SQL 词法由底层
/// prepared-statement/session 实现校验，避免把字符串或注释里的 `?` 当作参数。
#[derive(Clone)]
pub struct PreparedStatement {
    database: Arc<dyn Database>,
    sql: Arc<str>,
}

impl PreparedStatement {
    /// 保存 SQL，并沿用底层会话的参数解析语义。
    pub fn new(database: Arc<dyn Database>, sql: impl Into<Arc<str>>) -> Self {
        Self {
            database,
            sql: sql.into(),
        }
    }

    /// 交由底层会话校验并执行。
    pub fn execute(&self, arguments: &[DbValue]) -> TestResult<ExecutionResult> {
        self.database.execute(&self.sql, arguments)
    }

    /// 交由底层会话校验并查询。
    pub fn query(&self, arguments: &[DbValue]) -> TestResult<QueryRows> {
        self.database.query(&self.sql, arguments)
    }

    /// 返回原始 SQL 文本。
    pub fn sql(&self) -> &str {
        &self.sql
    }
}

/// 薄封装：持有 [`Database`] 并提供 prepare/execute/query。
#[derive(Clone)]
pub struct DbDriver {
    database: Arc<dyn Database>,
}

impl DbDriver {
    /// 包装给定数据库实现。
    pub fn new(database: Arc<dyn Database>) -> Self {
        Self { database }
    }
    /// 创建预编译语句。
    pub fn prepare(&self, sql: &str) -> PreparedStatement {
        PreparedStatement::new(self.database.clone(), sql)
    }
    /// 直接执行。
    pub fn execute(&self, sql: &str, args: &[DbValue]) -> TestResult<ExecutionResult> {
        self.database.execute(sql, args)
    }
    /// 直接查询。
    pub fn query(&self, sql: &str, args: &[DbValue]) -> TestResult<QueryRows> {
        self.database.query(sql, args)
    }
    /// 关闭底层数据库。
    pub fn close(&self) -> TestResult {
        self.database.close()
    }
}

/// Assignable scan destination used by [`MockRows`] / [`MockRow`], mirroring
/// Go `database/sql` Scan into typed variables.
/// Scan 目标类型：从 [`DbValue`] 赋入类型化变量。
pub trait AssignFromDbValue {
    /// 将 `value` 写入 `self`。
    fn assign_from(&mut self, value: &DbValue) -> TestResult;
}

impl AssignFromDbValue for String {
    fn assign_from(&mut self, value: &DbValue) -> TestResult {
        *self = match value {
            DbValue::Null => {
                return Err(TestError::new("cannot scan NULL into String"));
            }
            DbValue::String(text) => text.clone(),
            other => other.to_string(),
        };
        Ok(())
    }
}

impl AssignFromDbValue for i32 {
    fn assign_from(&mut self, value: &DbValue) -> TestResult {
        *self = match value {
            DbValue::Null => {
                return Err(TestError::new("cannot scan NULL into i32"));
            }
            DbValue::I64(v) => {
                i32::try_from(*v).map_err(|error| TestError::new(error.to_string()))?
            }
            DbValue::U64(v) => {
                i32::try_from(*v).map_err(|error| TestError::new(error.to_string()))?
            }
            DbValue::String(text) => text
                .parse()
                .map_err(|error| TestError::new(format!("scan i32 from {text:?}: {error}")))?,
            other => {
                return Err(TestError::new(format!(
                    "cannot scan i32 from value {other}"
                )));
            }
        };
        Ok(())
    }
}

impl AssignFromDbValue for i64 {
    fn assign_from(&mut self, value: &DbValue) -> TestResult {
        *self = match value {
            DbValue::Null => {
                return Err(TestError::new("cannot scan NULL into i64"));
            }
            DbValue::I64(v) => *v,
            DbValue::U64(v) => {
                i64::try_from(*v).map_err(|error| TestError::new(error.to_string()))?
            }
            DbValue::String(text) => text
                .parse()
                .map_err(|error| TestError::new(format!("scan i64 from {text:?}: {error}")))?,
            other => {
                return Err(TestError::new(format!(
                    "cannot scan i64 from value {other}"
                )));
            }
        };
        Ok(())
    }
}

impl AssignFromDbValue for f64 {
    fn assign_from(&mut self, value: &DbValue) -> TestResult {
        *self = match value {
            DbValue::Null => return Err(TestError::new("cannot scan NULL into f64")),
            DbValue::F64(v) => *v,
            DbValue::I64(v) => *v as f64,
            DbValue::U64(v) => *v as f64,
            DbValue::String(text) => text
                .parse()
                .map_err(|error| TestError::new(format!("scan f64 from {text:?}: {error}")))?,
            other => {
                return Err(TestError::new(format!(
                    "cannot scan f64 from value {other}"
                )));
            }
        };
        Ok(())
    }
}

impl AssignFromDbValue for bool {
    fn assign_from(&mut self, value: &DbValue) -> TestResult {
        *self = match value {
            DbValue::Null => return Err(TestError::new("cannot scan NULL into bool")),
            DbValue::Bool(v) => *v,
            DbValue::String(text) => text
                .parse()
                .map_err(|error| TestError::new(format!("scan bool from {text:?}: {error}")))?,
            other => {
                return Err(TestError::new(format!(
                    "cannot scan bool from value {other}"
                )));
            }
        };
        Ok(())
    }
}

impl AssignFromDbValue for Vec<u8> {
    fn assign_from(&mut self, value: &DbValue) -> TestResult {
        *self = match value {
            DbValue::Null => return Err(TestError::new("cannot scan NULL into Vec<u8>")),
            DbValue::Bytes(bytes) => bytes.clone(),
            DbValue::String(text) => text.as_bytes().to_vec(),
            other => other.to_string().into_bytes(),
        };
        Ok(())
    }
}

/// `database/sql`-like DB facade over a TestKit store (Go CreateMockDB).
/// 仿 `database/sql` 的 DB 门面，会话惰性打开自 TestKit store。
pub struct MockDB {
    /// 底层 store。
    store: Arc<dyn Database>,
    /// 缓存的派生会话。
    session: std::sync::Mutex<Option<Arc<dyn Database>>>,
}

/// CreateMockDB creates a mock DB that opens sessions from the TestKit store.
/// 从 TestKit store 创建会惰性打开会话的 MockDB。
pub fn CreateMockDB(store: Arc<dyn Database>) -> MockDB {
    MockDB {
        store,
        session: std::sync::Mutex::new(None),
    }
}

impl MockDB {
    /// 确保已有会话：已有则复用，否则 `create_session` 或回退到 store。
    fn ensure_session(&self) -> TestResult<Arc<dyn Database>> {
        let mut guard = self.session.lock().expect("mock db session lock poisoned");
        if let Some(session) = guard.as_ref() {
            return Ok(Arc::clone(session));
        }
        let session = self
            .store
            .create_session()?
            .unwrap_or_else(|| Arc::clone(&self.store));
        *guard = Some(Arc::clone(&session));
        Ok(session)
    }

    /// 无参 Exec。
    pub fn Exec(&self, sql: &str) -> TestResult<ExecutionResult> {
        self.ensure_session()?.execute(sql, &[])
    }

    /// 无参 Query，返回可迭代的 [`MockRows`]。
    pub fn Query(&self, sql: &str) -> TestResult<MockRows> {
        let rows = self.ensure_session()?.query(sql, &[])?;
        Ok(MockRows::new(rows))
    }

    /// 预编译并绑定到当前会话。
    pub fn Prepare(&self, sql: &str) -> TestResult<MockStmt> {
        let database = self.ensure_session()?;
        Ok(MockStmt {
            statement: PreparedStatement::new(Arc::clone(&database), sql),
            database,
        })
    }

    /// 关闭并释放缓存会话。
    pub fn Close(&self) -> TestResult {
        if let Some(session) = self
            .session
            .lock()
            .expect("mock db session lock poisoned")
            .take()
        {
            session.close()?;
        }
        Ok(())
    }
}

/// 游标式行迭代器：`Next` / `Scan` / `Close` / `Err`。
pub struct MockRows {
    rows: Vec<Vec<DbValue>>,
    /// 下一行下标；`Next` 成功后指向已消费位置+1。
    index: usize,
    closed: bool,
    err: Option<TestError>,
}

impl MockRows {
    fn new(query: QueryRows) -> Self {
        Self {
            rows: query.rows,
            index: 0,
            closed: false,
            err: None,
        }
    }

    /// 前进到下一行；关闭或出错时返回 false。
    pub fn Next(&mut self) -> bool {
        if self.closed || self.err.is_some() {
            return false;
        }
        if self.index >= self.rows.len() {
            return false;
        }
        self.index += 1;
        true
    }

    /// 将当前行扫描进 `dests`（须先成功 `Next`）。
    pub fn Scan(&mut self, dests: &mut [&mut dyn AssignFromDbValue]) -> TestResult {
        if self.index == 0 || self.index > self.rows.len() {
            return Err(TestError::new("Scan called without a current row"));
        }
        let row = &self.rows[self.index - 1];
        if dests.len() != row.len() {
            return Err(TestError::new(format!(
                "Scan expected {} destinations, got {}",
                row.len(),
                dests.len()
            )));
        }
        for (dest, value) in dests.iter_mut().zip(row.iter()) {
            dest.assign_from(value)?;
        }
        Ok(())
    }

    /// 标记关闭，后续 `Next` 返回 false。
    pub fn Close(&mut self) {
        self.closed = true;
    }

    /// 返回累计错误（若有）。
    pub fn Err(&self) -> TestResult {
        match &self.err {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }
}

/// 预编译语句句柄，支持 `QueryRow`。
pub struct MockStmt {
    database: Arc<dyn Database>,
    statement: PreparedStatement,
}

impl MockStmt {
    /// 查询并取第一行；无行时保留 `ErrNoRows` 语义供 `Scan` 返回错误。
    pub fn QueryRow<A: IntoQueryArgs>(&self, arguments: A) -> MockRow {
        match self.statement.query(&arguments.into_query_args()) {
            Ok(mut rows) => {
                let row = rows.rows.into_iter().next();
                MockRow {
                    row: row.ok_or_else(|| TestError::new("sql: no rows in result set")),
                }
            }
            Err(error) => MockRow { row: Err(error) },
        }
    }

    /// 关闭语句（当前为空操作，保留 database 引用防告警）。
    pub fn Close(&self) -> TestResult {
        let _ = &self.database;
        Ok(())
    }
}

/// 单行查询结果，可 `Scan`。
pub struct MockRow {
    row: TestResult<Vec<DbValue>>,
}

/// 参数集合 accepted by `MockStmt::QueryRow`, covering both the common
/// single-argument form and the variadic Go `database/sql` form.
pub trait IntoQueryArgs {
    fn into_query_args(self) -> Vec<DbValue>;
}

impl IntoQueryArgs for DbValue {
    fn into_query_args(self) -> Vec<DbValue> {
        vec![self]
    }
}

macro_rules! impl_single_query_arg {
    ($($type:ty),+ $(,)?) => {
        $(impl IntoQueryArgs for $type {
            fn into_query_args(self) -> Vec<DbValue> {
                vec![DbValue::from(self)]
            }
        })+
    };
}

impl_single_query_arg!(&str, String, i32, i64, u64, f64, bool);

impl IntoQueryArgs for Vec<DbValue> {
    fn into_query_args(self) -> Vec<DbValue> {
        self
    }
}

impl MockRow {
    /// 将行扫描进目标变量。
    pub fn Scan(&self, dests: &mut [&mut dyn AssignFromDbValue]) -> TestResult {
        let row = self.row.as_ref().map_err(Clone::clone)?;
        if dests.len() != row.len() {
            return Err(TestError::new(format!(
                "Scan expected {} destinations, got {}",
                row.len(),
                dests.len()
            )));
        }
        for (dest, value) in dests.iter_mut().zip(row.iter()) {
            dest.assign_from(value)?;
        }
        Ok(())
    }
}
