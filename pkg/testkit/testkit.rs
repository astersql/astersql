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

// TestKit：单测用 SQL 会话封装，对齐 Go `testkit.TestKit`。
//
// 在 mock/真实存储上创建会话，提供 MustExec/MustQuery、错误断言，
// 以及通过 `EXPLAIN` 检查执行计划（物理算子树）是否包含索引、Point_Get 等。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::db_driver::{
    AnalyzeStatsContext, Database, DbValue, ExecutionResult, PreparedResultField,
    PreparedStatement, QueryRows,
};
use crate::result::Result;
use crate::{TestError, TestResult};
use astersql_session::runtime::RuntimeStaleReadState;

/// 全局递增的连接 ID，模拟会话 ConnectionID 分配。
static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

/// 测试工具箱：持有存储与会话，缓存最近一次执行结果与失败注释。
#[derive(Clone)]
pub struct TestKit {
    store: Arc<dyn Database>,
    database: Arc<dyn Database>,
    connection_id: u64,
    comments: Vec<String>,
    last_result: ExecutionResult,
}

/// Session handle exposing Go-shaped Session()/GetSessionVars()/MemTracker APIs
/// while still delegating Database methods such as `close()`.
/// 会话句柄：暴露与 Go 同形的 Session/GetSessionVars/MemTracker API，并委托 `close()` 等 Database 方法。
#[derive(Clone)]
pub struct TestSession {
    database: Arc<dyn Database>,
}

/// SessionVars view used by testkit leak assertions.
/// SessionVars 视图，供 testkit 内存泄漏类断言使用。
#[derive(Clone, Copy)]
pub struct TestSessionVars {
    mem_tracker_children: usize,
    mem_tracker_bytes_consumed: i64,
    mem_tracker_max_consumed: i64,
    disk_tracker_max_consumed: i64,
    cte_storage_map_is_empty: bool,
}

/// Effective statement hints observed on the concrete execution path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TestStatementHints {
    pub MemQuotaQuery: i64,
    pub MaxExecutionTime: u64,
}

/// MemTracker view whose GetChildrenForTest length mirrors Go.
/// MemTracker 视图；`GetChildrenForTest` 长度与 Go 侧一致。
#[derive(Clone, Copy)]
pub struct TestMemTracker {
    children: usize,
    bytes_consumed: i64,
    max_consumed: i64,
}

/// DiskTracker view exposing the latest statement spill peak.
#[derive(Clone, Copy)]
pub struct TestDiskTracker {
    max_consumed: i64,
}

impl TestSession {
    /// Parse, build and optimize one SELECT on this session and return its real root plan ID.
    pub fn OptimizeRootPlanIDForTest(&self, sql: &str) -> TestResult<i32> {
        self.database.optimize_root_plan_id_for_test(sql)
    }

    /// Return the pre-cost IndexMerge candidate digest used by the Go casetest.
    pub fn IndexMergePathDigestForTest(&self, sql: &str) -> TestResult<String> {
        self.database.index_merge_path_digest_for_test(sql)
    }

    /// Return effective hints captured while executing the latest statement.
    pub fn LastStatementHintsForTest(&self) -> TestStatementHints {
        let (MemQuotaQuery, MaxExecutionTime) = self.database.last_statement_hints_for_test();
        TestStatementHints {
            MemQuotaQuery,
            MaxExecutionTime,
        }
    }

    /// 返回最近一条语句的 MySQL OK-packet 消息。
    pub fn LastMessage(&self) -> String {
        self.database.last_message_for_test()
    }

    /// Return the SQL stored in Go's `sessionctx.QueryString` slot.
    pub fn QueryString(&self) -> String {
        self.database.query_string_for_test()
    }

    /// Set MySQL client capability flags on this exact session.
    pub fn SetClientCapability(&self, capability: u32) -> TestResult {
        self.database.set_client_capability_for_test(capability)
    }

    /// Apply the MySQL handshake collation to this exact session.
    pub fn SetConnectionCollationForTest(&self, collation: u8) -> TestResult {
        self.database.set_connection_collation_for_test(collation)
    }

    /// Prepare SQL through the canonical session and return field metadata.
    pub fn PrepareStmt(&self, sql: &str) -> TestResult<(u64, Vec<PreparedResultField>)> {
        self.database.prepare_statement(sql)
    }

    /// Execute a binary-protocol statement returned by [`TestSession::PrepareStmt`].
    pub fn ExecutePreparedStmt(
        &self,
        statement_id: u64,
        arguments: &[DbValue],
    ) -> TestResult<ExecutionResult> {
        self.database
            .execute_prepared_statement(statement_id, arguments)
    }

    /// Drop a statement returned by [`TestSession::PrepareStmt`].
    pub fn DropPreparedStmt(&self, statement_id: u64) -> TestResult {
        self.database.drop_prepared_statement(statement_id)
    }

    /// 返回会话变量视图（含 MemTracker 子节点计数）。
    pub fn GetSessionVars(&self) -> TestSessionVars {
        TestSessionVars {
            mem_tracker_children: self.database.mem_tracker_children_for_test(),
            mem_tracker_bytes_consumed: self.database.mem_tracker_bytes_consumed_for_test(),
            mem_tracker_max_consumed: self.database.mem_tracker_max_consumed_for_test(),
            disk_tracker_max_consumed: self.database.disk_tracker_max_consumed_for_test(),
            cte_storage_map_is_empty: self.database.cte_storage_map_is_empty_for_test(),
        }
    }

    /// 关闭底层数据库会话。
    pub fn close(&self) -> TestResult {
        self.database.close()
    }

    /// 取出底层 Database 的 Arc 引用。
    pub fn database(&self) -> Arc<dyn Database> {
        Arc::clone(&self.database)
    }

    /// Execute SQL through the session's internal-request path.
    ///
    /// Go's `session.ExecuteInternal` requires a context carrying an internal
    /// request source. Keep that boundary explicit here instead of allowing
    /// callers to label an ordinary external execution as internal.
    pub fn ExecuteInternal(
        &self,
        context: &astersql_kv::Context,
        sql: &str,
        arguments: &[DbValue],
    ) -> TestResult<ExecutionResult> {
        if astersql_kv::GetInternalSourceType(context).is_empty() {
            return Err(TestError::new(
                "ExecuteInternal requires an internal request source",
            ));
        }
        self.database.execute_internal(sql, arguments)
    }

    /// Query SQL through the session's internal-request path.
    pub fn QueryInternal(
        &self,
        context: &astersql_kv::Context,
        sql: &str,
        arguments: &[DbValue],
    ) -> TestResult<QueryRows> {
        if astersql_kv::GetInternalSourceType(context).is_empty() {
            return Err(TestError::new(
                "QueryInternal requires an internal request source",
            ));
        }
        self.database.query_internal(sql, arguments)
    }

    /// Observe the isolation level installed on the concrete transaction.
    pub fn TransactionIsolationForTest(&self) -> Option<String> {
        self.database.transaction_isolation_for_test()
    }

    /// Return the concrete transaction's production debug representation.
    pub fn TransactionDebugStringForTest(&self) -> Option<String> {
        self.database.transaction_debug_string_for_test()
    }

    /// Return the active transaction snapshot's cached point-read count.
    pub fn SnapCacheSize(&self) -> usize {
        self.database.snapshot_cache_size_for_test()
    }

    /// Match Go `SessionVars.RowEncoder.Enable` for row-format regressions.
    pub fn SetRowEncoderEnabledForTest(&self, enabled: bool) -> TestResult {
        self.database.set_row_encoder_enabled_for_test(enabled)
    }

    pub fn StaleReadStateForTest(&self) -> Option<RuntimeStaleReadState> {
        self.database.stale_read_state_for_test()
    }

    /// Enable or disable Go-compatible inspection-table snapshot caching.
    pub fn SetInspectionTableCacheEnabledForTest(&self, enabled: bool) -> TestResult {
        self.database
            .set_inspection_table_cache_enabled_for_test(enabled)
    }

    /// Return the cached row count for one inspection table.
    pub fn InspectionTableCacheRowCountForTest(&self, table: &str) -> TestResult<Option<usize>> {
        self.database
            .inspection_table_cache_row_count_for_test(table)
    }

    /// Mutate one cached inspection-table cell.
    pub fn SetInspectionTableCacheValueForTest(
        &self,
        table: &str,
        row: usize,
        column: &str,
        value: impl Into<String>,
    ) -> TestResult {
        self.database
            .set_inspection_table_cache_value_for_test(table, row, column, value.into())
    }

    /// Flush the current session's collected index usage.
    pub fn ReportUsageStats(&self) -> TestResult {
        self.database.report_usage_stats_for_test()
    }

    /// Authenticate this TestKit session as an existing account.
    pub fn AuthenticateUserForTest(
        &self,
        identity: &astersql_parser_auth::parser::auth::auth::UserIdentity,
    ) -> TestResult {
        self.database
            .authenticate_user_for_test(&identity.username, &identity.hostname)
    }

    /// Inject a one-shot commit failure into this exact TestKit session.
    pub fn InjectNextDmlCommitErrorForTest(&self, message: impl Into<String>) -> TestResult {
        self.database
            .inject_next_dml_commit_error_for_test(message.into())
    }

    /// Install the AUTO_INCREMENT IDs reused by the next retrying statement.
    pub fn SetRetryAutoIncrementIDsForTest(&self, ids: Vec<u64>) -> TestResult {
        self.database.set_retry_auto_increment_ids_for_test(ids)
    }

    /// 返回最近语句清理 finished OOM action 后的 fallback 优先级。
    pub fn StatementOOMActionPriorityForTest(&self) -> Option<i64> {
        self.database.statement_oom_action_priority_for_test()
    }

    /// 返回最近一轮替代逻辑计划的解相关与同序 IndexJoin 信号。
    pub fn AlternativeLogicalPlanSignalsForTest(&self) -> Option<(bool, bool)> {
        self.database.alternative_logical_plan_signals_for_test()
    }

    /// 返回规划器记录的列/索引统计加载状态。
    pub fn StatsLoadStatusesForTest(&self) -> Vec<(i64, i64, bool, String)> {
        self.database.stats_load_statuses_for_test()
    }

    /// Return the previous statement trace ID stored by the canonical session.
    pub fn PrevTraceIDForTest(&self) -> Vec<u8> {
        self.database.prev_trace_id_for_test()
    }

    /// Clear the previous statement trace ID before a persistence assertion.
    pub fn ResetPrevTraceIDForTest(&self) -> TestResult {
        self.database.reset_prev_trace_id_for_test()
    }
}

impl TestSessionVars {
    /// 返回内存追踪器视图。
    pub fn MemTracker(&self) -> TestMemTracker {
        TestMemTracker {
            children: self.mem_tracker_children,
            bytes_consumed: self.mem_tracker_bytes_consumed,
            max_consumed: self.mem_tracker_max_consumed,
        }
    }

    /// 返回最近语句的 CTE 磁盘跟踪器视图。
    pub fn DiskTracker(&self) -> TestDiskTracker {
        TestDiskTracker {
            max_consumed: self.disk_tracker_max_consumed,
        }
    }

    /// 对应 Go `StmtCtx.CTEStorageMap == nil` 的清理断言。
    pub fn CTEStorageMapIsEmpty(&self) -> bool {
        self.cte_storage_map_is_empty
    }
}

impl TestMemTracker {
    /// 按子节点数量返回占位指针切片，长度对齐 Go 的 GetChildrenForTest。
    pub fn GetChildrenForTest(&self) -> Vec<*mut ()> {
        vec![std::ptr::null_mut(); self.children]
    }

    /// 返回会话根 MemTracker 当前记账字节数。
    pub fn BytesConsumed(&self) -> i64 {
        self.bytes_consumed
    }

    /// 返回最近语句 MemTracker 峰值消费字节数。
    pub fn MaxConsumed(&self) -> i64 {
        self.max_consumed
    }
}

impl TestDiskTracker {
    /// 返回最近语句 CTE 临时文件的峰值字节数。
    pub fn MaxConsumed(&self) -> i64 {
        self.max_consumed
    }
}

impl TestKit {
    /// 基于存储创建 TestKit：优先 `create_session`，失败则回退为直接使用传入的 database。
    pub fn new(database: Arc<dyn Database>) -> Self {
        // Go imports planner for its package-level initialization, which installs
        // expression.BuildSimpleExpr before TestKit executes SQL. Rust has no
        // package init hook, so establish the same idempotent boundary here.
        astersql_planner_core::InstallPlannerExpressionFactory()
            .expect("install planner expression factory for TestKit");
        let store = database.clone();
        // 尝试从 store 派生独立会话；若实现未提供会话则沿用原 database。
        let database = database
            .create_session()
            .unwrap_or_else(|error| panic!("create TestKit database session: {error}"))
            .unwrap_or(database);
        Self {
            store,
            database,
            connection_id: NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed),
            comments: Vec::new(),
            last_result: ExecutionResult::default(),
        }
    }

    /// Go 风格构造函数别名。
    pub fn NewTestKit(database: Arc<dyn Database>) -> Self {
        Self::new(database)
    }

    /// 返回底层 KV/存储句柄（非会话）。
    pub fn Store(&self) -> Arc<dyn Database> {
        Arc::clone(&self.store)
    }

    pub fn StaleReadStateForTest(&self) -> RuntimeStaleReadState {
        self.database
            .stale_read_state_for_test()
            .expect("database does not expose concrete stale-read state")
    }

    /// 返回当前测试会话句柄。
    pub fn Session(&self) -> TestSession {
        TestSession {
            database: Arc::clone(&self.database),
        }
    }

    /// 返回本 TestKit 分配的连接 ID。
    pub fn ConnectionID(&self) -> u64 {
        self.database
            .connection_id_for_test()
            .unwrap_or(self.connection_id)
    }

    /// 获取 ANALYZE 统计信息上下文（若会话支持）。
    pub fn AnalyzeStatsContext(&self) -> Option<AnalyzeStatsContext> {
        self.database.analyze_stats_context()
    }

    /// 追加失败诊断注释，会随 panic 文案一并输出。
    pub fn AddComment(&mut self, comment: impl Into<String>) {
        self.comments.push(comment.into());
    }
    /// 清空已登记的诊断注释。
    pub fn ClearComment(&mut self) {
        self.comments.clear();
    }

    /// 准备（PREPARE）一条 SQL，返回可绑定参数的语句句柄。
    pub fn Prepare(&self, sql: &str) -> PreparedStatement {
        PreparedStatement::new(self.database.clone(), sql)
    }

    /// 执行语句并缓存 `ExecutionResult`（影响行数、last insert id 等）。
    pub fn Exec(&mut self, sql: &str, arguments: Vec<DbValue>) -> TestResult<ExecutionResult> {
        let result = self.database.execute(sql, &arguments)?;
        self.last_result = result;
        Ok(result)
    }

    /// 执行查询并返回结果行。
    pub fn Query(&self, sql: &str, arguments: Vec<DbValue>) -> TestResult<QueryRows> {
        self.database.query(sql, &arguments)
    }

    /// 执行必须成功的语句；失败则 panic 并附带注释。
    pub fn MustExec(&mut self, sql: &str, arguments: Vec<DbValue>) {
        if let Err(error) = self.Exec(sql, arguments) {
            panic!("{}", self.failure(sql, &error));
        }
    }

    /// 查询必须成功；将行转为可 Check 的 Result，并附带注释。
    pub fn MustQuery(&self, sql: &str, arguments: Vec<DbValue>) -> Result {
        let rows = self
            .Query(sql, arguments)
            .unwrap_or_else(|error| panic!("{}", self.failure(sql, &error)));
        Result::with_comment(rows.string_rows(), self.comments.join("\n"))
    }

    /// 执行应失败的语句并返回错误；若成功则 panic。
    pub fn ExecToErr(&mut self, sql: &str) -> TestError {
        self.Exec(sql, Vec::new())
            .expect_err("statement unexpectedly succeeded")
    }

    /// 查询应失败并返回错误；若成功则 panic。
    pub fn QueryToErr(&self, sql: &str) -> TestError {
        self.Query(sql, Vec::new())
            .expect_err("query unexpectedly succeeded")
    }

    /// Execute a statement that must fail; retained as the explicit Go
    /// `MustExecToErr` assertion form for callers that do not need the error.
    pub fn MustExecToErr(&mut self, sql: &str) {
        let _ = self.ExecToErr(sql);
    }

    /// Query a statement that must fail; retained as the explicit Go
    /// `MustQueryToErr` assertion form.
    pub fn MustQueryToErr(&self, sql: &str) {
        let _ = self.QueryToErr(sql);
    }

    /// 断言执行错误消息与期望完全相等。
    pub fn MustGetErrMsg(&mut self, sql: &str, message: &str) {
        assert_eq!(self.ExecToErr(sql).message(), message, "sql={sql:?}");
    }

    /// 断言执行错误消息包含给定片段。
    pub fn MustContainErrMsg(&mut self, sql: &str, fragment: &str) {
        let error = self.ExecToErr(sql);
        assert!(
            error.message().contains(fragment),
            "sql={sql:?}, error={error}"
        );
    }

    /// 通过 EXPLAIN 断言执行计划中包含指定索引名。
    pub fn MustUseIndex(&self, sql: &str, index: &str) {
        let plan = self.MustQuery(&format!("explain {sql}"), Vec::new());
        let marker = format!("index:{index}");
        assert!(
            plan.Rows()
                .iter()
                .any(|row| row.get(3).is_some_and(|value| value.contains(&marker))),
            "index not used: sql={sql:?}, index={index:?}, plan={:?}",
            plan.Rows()
        );
    }

    /// 通过 EXPLAIN 断言执行计划未使用 Index 类算子。
    pub fn MustNoIndexUsed(&self, sql: &str) {
        let plan = self.MustQuery(&format!("explain {sql}"), Vec::new());
        assert!(
            !plan
                .Rows()
                .iter()
                .any(|row| row.get(3).is_some_and(|value| value.contains("index:"))),
            "index is used: sql={sql:?}, plan={:?}",
            plan.Rows()
        );
    }

    /// 断言计划含 Point_Get（点查：主键/唯一键等值直达）后执行原查询。
    pub fn MustPointGet(&self, sql: &str, arguments: Vec<DbValue>) -> Result {
        let plan = self.MustQuery(&format!("explain {sql}"), arguments.clone());
        let rows = plan.Rows();
        assert_eq!(
            rows.len(),
            1,
            "Point_Get plan must contain one row: {rows:?}"
        );
        assert!(
            rows[0]
                .first()
                .is_some_and(|value| value.contains("Point_Get")),
            "plan is not Point_Get: {rows:?}"
        );
        self.MustQuery(sql, arguments)
    }

    /// 校验最近一次 Exec 的影响行数与 last insert id。
    pub fn CheckExecResult(&self, affected_rows: u64, insert_id: u64) {
        assert_eq!(affected_rows, self.last_result.affected_rows);
        assert_eq!(insert_id, self.last_result.last_insert_id);
    }

    /// 判断 EXPLAIN 输出的算子 ID 列是否包含指定算子名。
    pub fn HasPlan(&self, sql: &str, operator: &str) -> bool {
        self.MustQuery(&format!("explain {sql}"), Vec::new())
            .Rows()
            .iter()
            .any(|row| row.first().is_some_and(|value| value.contains(operator)))
    }

    /// 判断 EXPLAIN 的算子 info 是否包含关键字。
    ///
    /// Go 的标准 EXPLAIN 把 operator info 放在第 5 列；Rust 的紧凑计划树省略
    /// estRows，因此对应信息位于第 4 列。
    pub fn HasKeywordInOperatorInfo(&self, sql: &str, keyword: &str) -> bool {
        self.MustQuery(&format!("explain {sql}"), Vec::new())
            .Rows()
            .iter()
            .any(|row| {
                row.get(4)
                    .or_else(|| (row.len() == 4).then(|| &row[3]))
                    .is_some_and(|value| value.contains(keyword))
            })
    }

    /// `HasKeywordInOperatorInfo` 的否定形式。
    pub fn NotHasKeywordInOperatorInfo(&self, sql: &str, keyword: &str) -> bool {
        !self.HasKeywordInOperatorInfo(sql, keyword)
    }

    /// 组装失败文案：SQL、错误，以及可选的注释块。
    fn failure(&self, sql: &str, error: &TestError) -> String {
        let comments = self.comments.join("\n");
        format!(
            "sql={sql:?}: {error}{}",
            if comments.is_empty() {
                String::new()
            } else {
                format!("\n{comments}")
            }
        )
    }
}

/// 包级 Go 风格工厂：等价于 `TestKit::new`。
pub fn NewTestKit(database: Arc<dyn Database>) -> TestKit {
    TestKit::new(database)
}
