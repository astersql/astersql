// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 统计子系统会话工具：系统变量同步、受限 SQL 执行与事务包装。
//
// 从会话池取出上下文后刷新 ANALYZE/分区剪枝相关全局变量，再执行统计读写；
// 可选 `FLAG_WRAP_TXN` 用悲观事务包裹回调，失败时回滚并保留原始错误。

use std::fmt::{Display, Formatter};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// 统计元历史来源：ANALYZE 收集。
pub const STATS_META_HISTORY_SOURCE_ANALYZE: &str = "analyze";
/// 统计元历史来源：加载已有统计。
pub const STATS_META_HISTORY_SOURCE_LOAD_STATS: &str = "load stats";
/// 统计元历史来源：刷盘写出。
pub const STATS_META_HISTORY_SOURCE_FLUSH_STATS: &str = "flush stats";
/// 统计元历史来源：DDL/ schema 变更。
pub const STATS_META_HISTORY_SOURCE_SCHEMA_CHANGE: &str = "schema change";
/// 统计元历史来源：扩展统计。
pub const STATS_META_HISTORY_SOURCE_EXTENDED_STATS: &str = "extended stats";

/// `call_with_sctx` 标志：用事务包裹回调。
pub const FLAG_WRAP_TXN: i32 = 0;
/// 索引列未指定前缀长度时的哨兵值。
pub const UNSPECIFIED_LENGTH: i32 = -1;

/// Go `util.ExecRows`'s failpoint name.
pub const EXEC_ROWS_TIMEOUT_FAILPOINT: &str =
    "github.com/pingcap/tidb/pkg/statistics/handle/util/ExecRowsTimeout";

/// Go `failpoint.Inject("ExecRowsTimeout", ...)` at the top of `util.ExecRows`.
///
/// Every restricted statistics query goes through this hook so tests can make
/// the statistics reader fail the same way a real query timeout would.
pub fn ExecRowsTimeout() -> Result<(), String> {
    if astersql_testkit_testfailpoint::eval_bool(EXEC_ROWS_TIMEOUT_FAILPOINT) {
        return Err("inject timeout error".to_owned());
    }
    Ok(())
}

/// 全局变量名：是否异步合并分区全局统计。
pub const TIDB_ENABLE_ASYNC_MERGE_GLOBAL_STATS: &str = "tidb_enable_async_merge_global_stats";
/// 全局变量名：ANALYZE 分区并发度。
pub const TIDB_ANALYZE_PARTITION_CONCURRENCY: &str = "tidb_analyze_partition_concurrency";
/// 全局变量名：ANALYZE 统计版本（Version1/Version2）。
pub const TIDB_ANALYZE_VERSION: &str = "tidb_analyze_version";
/// 全局变量名：是否启用历史统计。
pub const TIDB_ENABLE_HISTORICAL_STATS: &str = "tidb_enable_historical_stats";
/// 全局变量名：分区剪枝模式（static/dynamic）。
pub const TIDB_PARTITION_PRUNE_MODE: &str = "tidb_partition_prune_mode";
/// 全局变量名：ANALYZE 是否基于快照。
pub const TIDB_ENABLE_ANALYZE_SNAPSHOT: &str = "tidb_enable_analyze_snapshot";
/// 全局变量名：ANALYZE 跳过的列类型列表。
pub const TIDB_ANALYZE_SKIP_COLUMN_TYPES: &str = "tidb_analyze_skip_column_types";
/// 全局变量名：合并时是否跳过缺失分区统计。
pub const TIDB_SKIP_MISSING_PARTITION_STATS: &str = "tidb_skip_missing_partition_stats";
/// 全局变量名：锁等待超时（秒），内部会换算为毫秒。
pub const INNODB_LOCK_WAIT_TIMEOUT: &str = "innodb_lock_wait_timeout";
/// 全局变量名：会话时区。
pub const TIME_ZONE: &str = "time_zone";
/// Go `variable.ParseAnalyzeSkipColumnTypes` accepts only these column types.
const ANALYZE_SKIP_ALLOWED_COLUMN_TYPES: &[&str] = &[
    "json",
    "text",
    "mediumtext",
    "longtext",
    "blob",
    "mediumblob",
    "longblob",
];

#[derive(Clone, Debug, Eq, PartialEq)]
/// 统计会话工具层错误。
pub enum StatsError {
    GlobalVariable { name: String, message: String },
    ParseVariable { name: String, value: String },
    Sql(String),
    Transaction(String),
    PoolClosed,
}

impl Display for StatsError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GlobalVariable { name, message } => {
                write!(f, "get global variable {name} failed: {message}")
            }
            Self::ParseVariable { name, value } => {
                write!(f, "parse global variable {name}={value} failed")
            }
            Self::Sql(message) => write!(f, "SQL execution failed: {message}"),
            Self::Transaction(message) => write!(f, "transaction failed: {message}"),
            Self::PoolClosed => write!(f, "pool is closed"),
        }
    }
}

impl std::error::Error for StatsError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 内部 SQL 执行来源分类，用于审计/优先级。
pub enum InternalSourceType {
    StatsForegroundPriority,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 受限/内部 SQL 的执行上下文。
pub struct ExecutionContext {
    pub internal_source: InternalSourceType,
}

/// 统计前台优先的默认执行上下文。
pub static STATS_CONTEXT: ExecutionContext = ExecutionContext {
    internal_source: InternalSourceType::StatsForegroundPriority,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 受限 SQL 执行选项。
pub enum ExecOption {
    UseCurrentSession,
}

/// 在当前会话上执行受限 SQL 的选项集。
pub const USE_CURRENT_SESSION_OPTIONS: &[ExecOption] = &[ExecOption::UseCurrentSession];

#[derive(Clone, Debug, Eq, PartialEq)]
/// 受限 SQL 参数/结果单元格取值。
pub enum SqlValue {
    Null,
    Integer(i64),
    Unsigned(u64),
    Float(String),
    Bytes(Vec<u8>),
    String(String),
    StringList(Vec<String>),
    Bool(bool),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 一行查询结果。
pub struct Row {
    pub values: Vec<SqlValue>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 结果集字段元数据。
pub struct ResultField {
    pub database: String,
    pub table: String,
    pub column: String,
}

/// 流式结果集占位（对应 Go `sqlexec.RecordSet`）。
pub trait RecordSet: Send {}

/// 内部 SQL 执行器。
pub trait SqlExecutor: Send + Sync {
    fn execute_internal(
        &self,
        context: &ExecutionContext,
        sql: &str,
        arguments: &[SqlValue],
    ) -> Result<Box<dyn RecordSet>, StatsError>;
}

/// 受限 SQL 执行器：返回物化行与字段信息。
pub trait RestrictedSqlExecutor: Send + Sync {
    fn exec_restricted_sql(
        &self,
        context: &ExecutionContext,
        options: &[ExecOption],
        sql: &str,
        arguments: &[SqlValue],
    ) -> Result<(Vec<Row>, Vec<ResultField>), StatsError>;
}

/// 事务句柄，可取 start_ts（事务开始时间戳）。
pub trait Transaction: Send + Sync {
    fn start_ts(&self) -> u64;
}

/// 读取全局系统变量。
pub trait GlobalVariableAccessor: Send + Sync {
    fn get_global_sys_var(&self, name: &str) -> Result<String, StatsError>;
}

/// 统计相关会话变量缓存（原子/锁保护），由 `update_sctx_vars_for_stats` 刷新。
pub struct SessionVariables {
    pub global_vars_accessor: Arc<dyn GlobalVariableAccessor>,
    enable_async_merge_global_stats: AtomicBool,
    analyze_partition_concurrency: AtomicI64,
    analyze_version: AtomicI64,
    enable_historical_stats: AtomicBool,
    partition_prune_mode: RwLock<String>,
    enable_analyze_snapshot: AtomicBool,
    analyze_skip_column_types: RwLock<Vec<String>>,
    skip_missing_partition_stats: AtomicBool,
    lock_wait_timeout_millis: AtomicI64,
    time_zone: RwLock<String>,
    statement_time_zone: RwLock<String>,
}

impl SessionVariables {
    /// 构造默认未初始化的会话变量缓存。
    pub fn new(global_vars_accessor: Arc<dyn GlobalVariableAccessor>) -> Self {
        Self {
            global_vars_accessor,
            enable_async_merge_global_stats: AtomicBool::new(false),
            analyze_partition_concurrency: AtomicI64::new(0),
            analyze_version: AtomicI64::new(0),
            enable_historical_stats: AtomicBool::new(false),
            partition_prune_mode: RwLock::new(String::new()),
            enable_analyze_snapshot: AtomicBool::new(false),
            analyze_skip_column_types: RwLock::new(Vec::new()),
            skip_missing_partition_stats: AtomicBool::new(false),
            lock_wait_timeout_millis: AtomicI64::new(0),
            time_zone: RwLock::new(String::new()),
            statement_time_zone: RwLock::new(String::new()),
        }
    }

    /// 是否异步合并全局统计。
    pub fn enable_async_merge_global_stats(&self) -> bool {
        self.enable_async_merge_global_stats.load(Ordering::Acquire)
    }

    /// ANALYZE 分区并发度。
    pub fn analyze_partition_concurrency(&self) -> i64 {
        self.analyze_partition_concurrency.load(Ordering::Acquire)
    }

    /// ANALYZE 统计版本号。
    pub fn analyze_version(&self) -> i64 {
        self.analyze_version.load(Ordering::Acquire)
    }

    /// 是否启用历史统计。
    pub fn enable_historical_stats(&self) -> bool {
        self.enable_historical_stats.load(Ordering::Acquire)
    }

    /// 当前分区剪枝模式字符串。
    pub fn partition_prune_mode(&self) -> String {
        self.partition_prune_mode.read().unwrap().clone()
    }

    /// ANALYZE 是否使用快照。
    pub fn enable_analyze_snapshot(&self) -> bool {
        self.enable_analyze_snapshot.load(Ordering::Acquire)
    }

    /// ANALYZE 跳过的列类型（小写）。
    pub fn analyze_skip_column_types(&self) -> Vec<String> {
        self.analyze_skip_column_types.read().unwrap().clone()
    }

    /// 合并时是否跳过缺失分区统计。
    pub fn skip_missing_partition_stats(&self) -> bool {
        self.skip_missing_partition_stats.load(Ordering::Acquire)
    }

    /// 锁等待超时（毫秒）。
    pub fn lock_wait_timeout_millis(&self) -> i64 {
        self.lock_wait_timeout_millis.load(Ordering::Acquire)
    }

    /// 会话时区字符串。
    pub fn time_zone(&self) -> String {
        self.time_zone.read().unwrap().clone()
    }

    /// 语句上下文时区（由 location 同步）。
    pub fn statement_time_zone(&self) -> String {
        self.statement_time_zone.read().unwrap().clone()
    }
}

/// 统计路径使用的会话上下文抽象。
pub trait SessionContext: Send + Sync {
    fn session_variables(&self) -> Arc<SessionVariables>;
    fn transaction(&self, active: bool) -> Result<Arc<dyn Transaction>, StatsError>;
    fn sql_executor(&self) -> Arc<dyn SqlExecutor>;
    fn restricted_sql_executor(&self) -> Arc<dyn RestrictedSqlExecutor>;
    fn mock_restricted_sql_executor(&self) -> Option<Arc<dyn RestrictedSqlExecutor>> {
        None
    }
    fn set_system_variable(&self, name: &str, value: &str) -> Result<(), StatsError>;
    fn location(&self) -> String;
}

/// 会话池：借出会话执行回调并保证归还。
pub trait SessionPool: Send + Sync {
    fn with_session(
        &self,
        callback: &mut dyn FnMut(&dyn SessionContext) -> Result<(), StatsError>,
    ) -> Result<(), StatsError>;
}

/// 将 "1"/"ON" 解析为开启。
fn option_on(value: &str) -> bool {
    value == "1" || value.eq_ignore_ascii_case("ON")
}

/// 解析整数型全局变量，失败时带上变量名。
fn parse_i64(name: &str, value: &str) -> Result<i64, StatsError> {
    value.parse().map_err(|_| StatsError::ParseVariable {
        name: name.to_owned(),
        value: value.to_owned(),
    })
}

/// 通过 accessor 读取全局变量。
fn global(variables: &SessionVariables, name: &str) -> Result<String, StatsError> {
    variables.global_vars_accessor.get_global_sys_var(name)
}

/// 从全局变量刷新统计相关会话缓存，并同步时区到语句上下文。
pub fn update_sctx_vars_for_stats(context: &dyn SessionContext) -> Result<(), StatsError> {
    let variables = context.session_variables();

    let value = global(&variables, TIDB_ENABLE_ASYNC_MERGE_GLOBAL_STATS)?;
    variables
        .enable_async_merge_global_stats
        .store(option_on(&value), Ordering::Release);

    let value = global(&variables, TIDB_ANALYZE_PARTITION_CONCURRENCY)?;
    variables.analyze_partition_concurrency.store(
        parse_i64(TIDB_ANALYZE_PARTITION_CONCURRENCY, &value)?,
        Ordering::Release,
    );

    let value = global(&variables, TIDB_ANALYZE_VERSION)?;
    variables
        .analyze_version
        .store(parse_i64(TIDB_ANALYZE_VERSION, &value)?, Ordering::Release);

    let value = global(&variables, TIDB_ENABLE_HISTORICAL_STATS)?;
    variables
        .enable_historical_stats
        .store(option_on(&value), Ordering::Release);

    let value = global(&variables, TIDB_PARTITION_PRUNE_MODE)?;
    *variables.partition_prune_mode.write().unwrap() = value;

    let value = global(&variables, TIDB_ENABLE_ANALYZE_SNAPSHOT)?;
    variables
        .enable_analyze_snapshot
        .store(option_on(&value), Ordering::Release);

    // Keep the same lower-case, comma-separated whitelist semantics as Go's
    // variable.ParseAnalyzeSkipColumnTypes. Validation normally rejects other
    // values before this path, but parsing still ignores them defensively.
    let value = global(&variables, TIDB_ANALYZE_SKIP_COLUMN_TYPES)?;
    *variables.analyze_skip_column_types.write().unwrap() = value
        .to_ascii_lowercase()
        .split(',')
        .filter(|value| ANALYZE_SKIP_ALLOWED_COLUMN_TYPES.contains(value))
        .map(str::to_owned)
        .collect();

    let value = global(&variables, TIDB_SKIP_MISSING_PARTITION_STATS)?;
    variables
        .skip_missing_partition_stats
        .store(option_on(&value), Ordering::Release);

    let value = global(&variables, INNODB_LOCK_WAIT_TIMEOUT)?;
    variables.lock_wait_timeout_millis.store(
        parse_i64(INNODB_LOCK_WAIT_TIMEOUT, &value)?.saturating_mul(1000),
        Ordering::Release,
    );

    let value = global(&variables, TIME_ZONE)?;
    context.set_system_variable(TIME_ZONE, &value)?;
    *variables.time_zone.write().unwrap() = value;
    *variables.statement_time_zone.write().unwrap() = context.location();
    Ok(())
}

/// 从会话池取上下文、刷新变量后执行回调；与 Go `util.Recover` 一样吞掉普通 panic。
pub fn call_with_sctx<F>(
    pool: &dyn SessionPool,
    callback: F,
    flags: &[i32],
) -> Result<(), StatsError>
where
    F: FnOnce(&dyn SessionContext) -> Result<(), StatsError>,
{
    let mut callback = Some(callback);
    let mut run = |context: &dyn SessionContext| {
        update_sctx_vars_for_stats(context)?;
        let callback = callback
            .take()
            .expect("session pool invoked callback more than once");
        // 按需用悲观事务包裹，保证统计写操作原子性。
        if flags.contains(&FLAG_WRAP_TXN) {
            wrap_txn(context, callback)
        } else {
            callback(context)
        }
    };
    match catch_unwind(AssertUnwindSafe(|| pool.with_session(&mut run))) {
        Ok(result) => result,
        Err(_) => Ok(()),
    }
}

/// 经 `call_with_sctx` 读取当前分区剪枝模式。
pub fn get_current_prune_mode(pool: &dyn SessionPool) -> Result<String, StatsError> {
    let mode = Arc::new(RwLock::new(String::new()));
    let result = Arc::clone(&mode);
    call_with_sctx(
        pool,
        move |context| {
            *result.write().unwrap() = context.session_variables().partition_prune_mode();
            Ok(())
        },
        &[],
    )?;
    let value = mode.read().unwrap().clone();
    Ok(value)
}

/// 按回调结果 COMMIT 或 rollback；回滚失败丢弃，保留原始错误。
fn finish_transaction(
    context: &dyn SessionContext,
    result: Result<(), StatsError>,
) -> Result<(), StatsError> {
    match result {
        Ok(()) => exec_rows(context, "COMMIT", &[]).map(|_| ()),
        Err(original) => {
            // Rollback errors are logged and discarded by Go; the operation's
            // original failure remains authoritative.
            let _ = exec_rows(context, "rollback", &[]);
            Err(original)
        }
    }
}

/// 开启悲观事务执行回调，再根据结果提交或回滚。
pub fn wrap_txn<F>(context: &dyn SessionContext, callback: F) -> Result<(), StatsError>
where
    F: FnOnce(&dyn SessionContext) -> Result<(), StatsError>,
{
    exec_rows(context, "BEGIN PESSIMISTIC", &[])?;
    finish_transaction(context, callback(context))
}

/// 取当前事务 start_ts。
pub fn get_start_ts(context: &dyn SessionContext) -> Result<u64, StatsError> {
    Ok(context.transaction(true)?.start_ts())
}

/// 用默认统计上下文执行内部 SQL，返回流式 RecordSet。
pub fn exec(
    context: &dyn SessionContext,
    sql: &str,
    arguments: &[SqlValue],
) -> Result<Box<dyn RecordSet>, StatsError> {
    exec_with_ctx(&STATS_CONTEXT, context, sql, arguments)
}

/// 指定执行上下文执行内部 SQL。
pub fn exec_with_ctx(
    execution_context: &ExecutionContext,
    context: &dyn SessionContext,
    sql: &str,
    arguments: &[SqlValue],
) -> Result<Box<dyn RecordSet>, StatsError> {
    context
        .sql_executor()
        .execute_internal(execution_context, sql, arguments)
}

static EXEC_ROWS_TIMEOUT: AtomicBool = AtomicBool::new(false);
static IN_TEST: AtomicBool = AtomicBool::new(false);

/// 测试钩子：强制 `exec_rows` 注入超时。
pub fn set_exec_rows_timeout(enabled: bool) {
    EXEC_ROWS_TIMEOUT.store(enabled, Ordering::Release);
}

/// 测试钩子：启用 mock 受限 SQL 执行器路径。
pub fn set_in_test(enabled: bool) {
    IN_TEST.store(enabled, Ordering::Release);
}

/// 执行受限 SQL 并物化行；测试超时开关优先于真实执行。
pub fn exec_rows(
    context: &dyn SessionContext,
    sql: &str,
    arguments: &[SqlValue],
) -> Result<(Vec<Row>, Vec<ResultField>), StatsError> {
    ExecRowsTimeout().map_err(StatsError::Sql)?;
    if EXEC_ROWS_TIMEOUT.load(Ordering::Acquire) {
        return Err(StatsError::Sql("inject timeout error".to_owned()));
    }
    exec_rows_with_ctx(&STATS_CONTEXT, context, sql, arguments)
}

/// 带执行上下文的受限 SQL；测试模式下可走 mock，且固定用 `STATS_CONTEXT`。
pub fn exec_rows_with_ctx(
    execution_context: &ExecutionContext,
    context: &dyn SessionContext,
    sql: &str,
    arguments: &[SqlValue],
) -> Result<(Vec<Row>, Vec<ResultField>), StatsError> {
    if IN_TEST.load(Ordering::Acquire)
        && let Some(mock) = context.mock_restricted_sql_executor()
    {
        // The Go test hook deliberately uses StatsCtx even when the caller
        // supplied another context.
        return mock.exec_restricted_sql(
            &STATS_CONTEXT,
            USE_CURRENT_SESSION_OPTIONS,
            sql,
            arguments,
        );
    }
    context.restricted_sql_executor().exec_restricted_sql(
        execution_context,
        USE_CURRENT_SESSION_OPTIONS,
        sql,
        arguments,
    )
}

/// 使用调用方指定的 ExecOption 执行受限 SQL。
pub fn exec_with_opts(
    context: &dyn SessionContext,
    options: &[ExecOption],
    sql: &str,
    arguments: &[SqlValue],
) -> Result<(Vec<Row>, Vec<ResultField>), StatsError> {
    context
        .restricted_sql_executor()
        .exec_restricted_sql(&STATS_CONTEXT, options, sql, arguments)
}

/// 将物理时长编码为 TiDB 风格时间戳（物理毫秒左移 18 位）。
pub fn duration_to_ts(duration: Duration) -> u64 {
    let physical_millis = duration.as_millis().min(u128::from(u64::MAX)) as u64;
    physical_millis << 18
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 索引列：在表列中的偏移与可选前缀长度。
pub struct IndexColumn {
    pub offset: usize,
    pub length: i32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 索引形状：是否全局索引及其列列表。
pub struct IndexInfo {
    pub global: bool,
    pub columns: Vec<IndexColumn>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 列形状：是否虚拟生成列。
pub struct ColumnInfo {
    pub virtual_generated: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 表形状：列列表，供特殊全局索引判定。
pub struct TableInfo {
    pub columns: Vec<ColumnInfo>,
}

/// 判定是否为“特殊全局索引”：全局且含虚拟生成列或前缀列。
pub fn is_special_global_index(index: &IndexInfo, table: &TableInfo) -> bool {
    if !index.global {
        return false;
    }
    index.columns.iter().any(|index_column| {
        let column = &table.columns[index_column.offset];
        column.virtual_generated || index_column.length != UNSPECIFIED_LENGTH
    })
}
