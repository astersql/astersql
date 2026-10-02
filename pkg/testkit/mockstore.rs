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

// Mock 存储与带 Domain 的分析统计 Store。
//
// 提供两类测试后端：
// - [`MockStore`]：按规范化 SQL 注册期望查询/执行结果的轻量 Mock；
// - [`AnalyzeStatsStore`] / [`AnalyzeSessionDatabase`]：在真实 `Domain` 上
//   为每个会话绑定线程固定的 `ConcreteSession`，供 ANALYZE / 自动分析等路径使用。
use std::any::type_name;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::db_driver::{
    AnalyzeStatsContext, Database, DbValue, ExecutionResult, PreparedResultField, QueryRows,
};
use crate::{NewTestKit, TestError, TestKit, TestResult};
use astersql_domain::domain::CrossKeyspaceCoordinator;
use astersql_domain::{
    AutoAnalyzeExecutor, Domain, DomainConfig, InfoSchemaLoader, KvInfoSchemaLoader, SQLKiller,
};
use astersql_session::runtime::{
    CONCRETE_NULL_VALUE, ConcretePreparedArgument, ConcreteSession, CreateAnalyzeSession,
    RegisterRuntimeTopology, RuntimeReplicaReadRequest, RuntimeSelectRequest,
    RuntimeStaleReadState,
};
use astersql_session::testutil::{TestRecordSet, TestSession};

// Go `bootstrapTables` 中当前 canonical session bootstrap 尚未挂载的统计表。
// 保留 Go `pkg/meta/metadef` 的完整列定义，避免 testkit 的普通 SQL 会话只看到
// restricted-stats KV 行而看不到对应系统表元数据。
const GO_STATS_SYSTEM_TABLES: &[(&str, &str)] = &[
    (
        "stats_fm_sketch",
        r#"CREATE TABLE IF NOT EXISTS mysql.stats_fm_sketch (
            table_id BIGINT(64) NOT NULL,
            is_index TINYINT(2) NOT NULL,
            hist_id BIGINT(64) NOT NULL,
            value LONGBLOB,
            PRIMARY KEY (table_id, is_index, hist_id) CLUSTERED
        )"#,
    ),
    (
        "stats_history",
        r#"CREATE TABLE IF NOT EXISTS mysql.stats_history (
            table_id bigint(64) NOT NULL,
            stats_data longblob NOT NULL,
            seq_no bigint(64) NOT NULL comment 'sequence number of the gzipped data slice',
            version bigint(64) NOT NULL comment 'stats version which corresponding to stats:version in EXPLAIN',
            create_time datetime(6) NOT NULL,
            PRIMARY KEY (table_id, version, seq_no) CLUSTERED,
            KEY table_create_time (table_id, create_time, seq_no),
            KEY idx_create_time (create_time)
        )"#,
    ),
    (
        "stats_meta_history",
        r#"CREATE TABLE IF NOT EXISTS mysql.stats_meta_history (
            table_id bigint(64) NOT NULL,
            modify_count bigint(64) NOT NULL,
            count bigint(64) NOT NULL,
            version bigint(64) NOT NULL comment 'stats version which corresponding to stats:version in EXPLAIN',
            source varchar(40) NOT NULL,
            create_time datetime(6) NOT NULL,
            PRIMARY KEY (table_id, version) CLUSTERED,
            KEY table_create_time (table_id, create_time),
            KEY idx_create_time (create_time)
        )"#,
    ),
    (
        "stats_table_locked",
        r#"CREATE TABLE IF NOT EXISTS mysql.stats_table_locked (
            table_id bigint(64) NOT NULL,
            modify_count bigint(64) NOT NULL DEFAULT 0,
            count bigint(64) NOT NULL DEFAULT 0,
            version bigint(64) UNSIGNED NOT NULL DEFAULT 0,
            PRIMARY KEY (table_id) CLUSTERED
        )"#,
    ),
];

#[derive(Clone, Debug, Eq, PartialEq)]
/// Mock 存储配置：集群 ID、Keyspace、路径与是否启用 Cascades 优化器。
pub struct MockStoreConfig {
    pub cluster_id: u64,
    pub keyspace: Option<String>,
    pub path: Option<String>,
    pub cascades_planner: bool,
}

/// 默认 cluster_id=1，其余关闭/空。
impl Default for MockStoreConfig {
    fn default() -> Self {
        Self {
            cluster_id: 1,
            keyspace: None,
            path: None,
            cascades_planner: false,
        }
    }
}

#[derive(Default)]
/// MockStore 内部可变状态：关闭标志、注册结果与执行历史。
struct StoreState {
    closed: bool,
    query_results: HashMap<String, TestResult<QueryRows>>,
    execution_results: HashMap<String, TestResult<ExecutionResult>>,
    history: Vec<(String, Vec<DbValue>)>,
}

#[derive(Clone, Default)]
/// 轻量 Mock 数据库：按规范化 SQL 返回预注册的查询/执行结果。
pub struct MockStore {
    config: MockStoreConfig,
    state: Arc<Mutex<StoreState>>,
}

impl MockStore {
    /// 使用给定配置构造 MockStore。
    pub fn new(config: MockStoreConfig) -> Self {
        Self {
            config,
            state: Arc::new(Mutex::new(StoreState::default())),
        }
    }

    /// 返回配置引用。
    pub fn config(&self) -> &MockStoreConfig {
        &self.config
    }

    /// 为规范化后的 SQL 注册成功查询结果。
    pub fn expect_query(&self, sql: impl Into<String>, rows: QueryRows) {
        self.state
            .lock()
            .expect("mock store poisoned")
            .query_results
            .insert(normalize_sql(&sql.into()), Ok(rows));
    }

    /// 为规范化后的 SQL 注册查询失败。
    pub fn fail_query(&self, sql: impl Into<String>, error: impl Into<String>) {
        self.state
            .lock()
            .expect("mock store poisoned")
            .query_results
            .insert(normalize_sql(&sql.into()), Err(TestError::new(error)));
    }

    /// 为规范化后的 SQL 注册成功执行结果。
    pub fn expect_execute(&self, sql: impl Into<String>, result: ExecutionResult) {
        self.state
            .lock()
            .expect("mock store poisoned")
            .execution_results
            .insert(normalize_sql(&sql.into()), Ok(result));
    }

    /// 为规范化后的 SQL 注册执行失败。
    pub fn fail_execute(&self, sql: impl Into<String>, error: impl Into<String>) {
        self.state
            .lock()
            .expect("mock store poisoned")
            .execution_results
            .insert(normalize_sql(&sql.into()), Err(TestError::new(error)));
    }

    /// 返回已记录的 (SQL, 参数) 历史快照。
    pub fn history(&self) -> Vec<(String, Vec<DbValue>)> {
        self.state
            .lock()
            .expect("mock store poisoned")
            .history
            .clone()
    }

    /// 清空执行历史。
    pub fn clear_history(&self) {
        self.state
            .lock()
            .expect("mock store poisoned")
            .history
            .clear();
    }
}

/// 将注册表结果暴露为 [`Database`] 接口。
impl Database for MockStore {
    fn execute(&self, sql: &str, arguments: &[DbValue]) -> TestResult<ExecutionResult> {
        let mut state = self.state.lock().expect("mock store poisoned");
        if state.closed {
            return Err(TestError::new("mock store is closed"));
        }
        state.history.push((sql.to_owned(), arguments.to_vec()));
        state
            .execution_results
            .get(&normalize_sql(sql))
            .cloned()
            .unwrap_or(Ok(ExecutionResult::default()))
    }

    fn query(&self, sql: &str, arguments: &[DbValue]) -> TestResult<QueryRows> {
        let mut state = self.state.lock().expect("mock store poisoned");
        if state.closed {
            return Err(TestError::new("mock store is closed"));
        }
        state.history.push((sql.to_owned(), arguments.to_vec()));
        state
            .query_results
            .get(&normalize_sql(sql))
            .cloned()
            .unwrap_or_else(|| {
                Err(TestError::new(format!(
                    "no mock query result registered for {sql:?}"
                )))
            })
    }

    fn close(&self) -> TestResult {
        self.state.lock().expect("mock store poisoned").closed = true;
        Ok(())
    }
}

/// 空白折叠并小写化 SQL，用作期望结果的查找键。
fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// 创建默认配置的 MockStore。
pub fn CreateMockStore() -> Arc<MockStore> {
    Arc::new(MockStore::default())
}

/// Build the Go `TestOption` that selects the requested Cascades planner mode.
pub fn WithCascades(on: bool) -> impl FnOnce(&mut TestKit) {
    move |test_kit| {
        let value = if on { "on" } else { "off" };
        test_kit.MustExec(
            &format!("set @@tidb_enable_cascades_planner = {value}"),
            Vec::new(),
        );
    }
}

/// Run a planner test once with the Cascades planner disabled.
///
/// Go commit `2d30398a5f4977b04044d2493fbc2863788937dd` deliberately removed the
/// second, Cascades-enabled round from this compatibility helper.
pub fn RunTestUnderCascades<F>(test_func: F)
where
    F: FnOnce(&mut TestKit, &str, &str),
{
    let caller = test_callback_caller::<F>();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut test_kit = NewTestKit(store);
    WithCascades(false)(&mut test_kit);
    test_func(&mut test_kit, "off", caller);
}

/// Run a planner test once with its Domain and the Cascades planner disabled.
pub fn RunTestUnderCascadesWithDomain<F>(test_func: F)
where
    F: FnOnce(&mut TestKit, &Arc<Domain>, &str, &str),
{
    let caller = test_callback_caller::<F>();
    let (store, domain) = CreateMockStoreAndDomain();
    let mut test_kit = NewTestKit(store);
    WithCascades(false)(&mut test_kit);
    test_func(&mut test_kit, &domain, "off", caller);
}

/// Run a planner test once with an explicit schema lease and Cascades disabled.
pub fn RunTestUnderCascadesAndDomainWithSchemaLease<F>(schema_lease: Duration, test_func: F)
where
    F: FnOnce(&mut TestKit, &Arc<Domain>, &str, &str),
{
    let caller = test_callback_caller::<F>();
    let (store, domain) = CreateMockStoreAndDomainWithSchemaLease(schema_lease);
    let mut test_kit = NewTestKit(store);
    WithCascades(false)(&mut test_kit);
    test_func(&mut test_kit, &domain, "off", caller);
}

fn test_callback_caller<F>() -> &'static str {
    type_name::<F>()
        .strip_suffix("::{{closure}}")
        .unwrap_or_else(|| type_name::<F>())
        .rsplit("::")
        .next()
        .expect("callback type name must not be empty")
}
/// 按配置创建 MockStore。
pub fn CreateMockStoreWithConfig(config: MockStoreConfig) -> Arc<MockStore> {
    Arc::new(MockStore::new(config))
}

/// 发往会话工作线程的请求：执行 SQL、读 MemTracker 或关闭。
enum SessionRequest {
    Execute {
        sql: String,
        arguments: Vec<DbValue>,
        internal: bool,
        response: mpsc::SyncSender<TestResult<SessionReply>>,
    },
    MemTrackerChildren {
        response: mpsc::SyncSender<usize>,
    },
    MemTrackerBytesConsumed {
        response: mpsc::SyncSender<i64>,
    },
    MemTrackerMaxConsumed {
        response: mpsc::SyncSender<i64>,
    },
    DiskTrackerMaxConsumed {
        response: mpsc::SyncSender<i64>,
    },
    CTEStorageMapIsEmpty {
        response: mpsc::SyncSender<bool>,
    },
    OptimizeRootPlanID {
        sql: String,
        response: mpsc::SyncSender<TestResult<i32>>,
    },
    IndexMergePathDigest {
        sql: String,
        response: mpsc::SyncSender<TestResult<String>>,
    },
    LastStatementHints {
        response: mpsc::SyncSender<(i64, u64)>,
    },
    StatementOOMActionPriority {
        response: mpsc::SyncSender<Option<i64>>,
    },
    AlternativeLogicalPlanSignals {
        response: mpsc::SyncSender<(bool, bool)>,
    },
    StatsLoadStatuses {
        response: mpsc::SyncSender<Vec<(i64, i64, bool, String)>>,
    },
    PrevTraceID {
        response: mpsc::SyncSender<Vec<u8>>,
    },
    ResetPrevTraceID {
        response: mpsc::SyncSender<()>,
    },
    ExpirePessimisticLocks {
        response: mpsc::SyncSender<()>,
    },
    SetPessimisticLockTtl {
        ttl: std::time::Duration,
        response: mpsc::SyncSender<()>,
    },
    InjectDmlCommitError {
        message: String,
        response: mpsc::SyncSender<()>,
    },
    SetRetryAutoIncrementIds {
        ids: Vec<u64>,
        response: mpsc::SyncSender<()>,
    },
    RuntimeDeadlockHistoryCount {
        response: mpsc::SyncSender<usize>,
    },
    ClearRuntimeDeadlockHistory {
        response: mpsc::SyncSender<()>,
    },
    LastReplicaReadRequest {
        response: mpsc::SyncSender<Option<RuntimeReplicaReadRequest>>,
    },
    ClearReplicaReadRequest {
        response: mpsc::SyncSender<()>,
    },
    LastSelectRequest {
        response: mpsc::SyncSender<Option<RuntimeSelectRequest>>,
    },
    ClearSelectRequest {
        response: mpsc::SyncSender<()>,
    },
    TransactionIsolation {
        response: mpsc::SyncSender<String>,
    },
    TransactionDebugString {
        response: mpsc::SyncSender<String>,
    },
    SnapshotCacheSize {
        response: mpsc::SyncSender<usize>,
    },
    SetRowEncoderEnabled {
        enabled: bool,
        response: mpsc::SyncSender<()>,
    },
    LastMessage {
        response: mpsc::SyncSender<String>,
    },
    QueryString {
        response: mpsc::SyncSender<String>,
    },
    SetClientCapability {
        capability: u32,
        response: mpsc::SyncSender<()>,
    },
    SetConnectionCollation {
        collation: u8,
        response: mpsc::SyncSender<TestResult>,
    },
    StaleReadState {
        response: mpsc::SyncSender<RuntimeStaleReadState>,
    },
    SetInspectionTableCacheEnabled {
        enabled: bool,
        response: mpsc::SyncSender<()>,
    },
    InspectionTableCacheRowCount {
        table: String,
        response: mpsc::SyncSender<Option<usize>>,
    },
    SetInspectionTableCacheValue {
        table: String,
        row: usize,
        column: String,
        value: String,
        response: mpsc::SyncSender<TestResult>,
    },
    ReportUsageStats {
        response: mpsc::SyncSender<()>,
    },
    AuthenticateUser {
        username: String,
        hostname: String,
        response: mpsc::SyncSender<TestResult>,
    },
    Prepare {
        sql: String,
        response: mpsc::SyncSender<TestResult<(u64, Vec<PreparedResultField>)>>,
    },
    ExecutePrepared {
        statement_id: u64,
        arguments: Vec<DbValue>,
        response: mpsc::SyncSender<TestResult<ExecutionResult>>,
    },
    DropPrepared {
        statement_id: u64,
        response: mpsc::SyncSender<TestResult>,
    },
    Close,
}

/// 会话执行回复：DML 报告与结果集列表。
struct SessionReply {
    execution: ExecutionResult,
    record_sets: Vec<QueryRows>,
}

/// Pins one canonical `ConcreteSession` to its owning thread. This adapter
/// never parses SQL or owns catalog/statistics state.
///
/// 将会话固定在其所属工作线程上；本适配器不解析 SQL，也不持有目录/统计状态。
pub struct AnalyzeSessionDatabase {
    lifecycle: Mutex<AnalyzeSessionLifecycle>,
    domain: Arc<Domain>,
    killer: Arc<SQLKiller>,
    connection_id: u64,
}

/// 会话工作线程生命周期：请求通道与 JoinHandle。
struct AnalyzeSessionLifecycle {
    requests: Option<mpsc::Sender<SessionRequest>>,
    worker: Option<JoinHandle<()>>,
}

/// RAII：析构时减少活跃会话计数。
struct ActiveSessionGuard(Arc<AtomicUsize>);

/// 会话结束时递减 `active_sessions`。
impl Drop for ActiveSessionGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// AnalyzeStatsStore 内部状态：会话弱引用与自动分析会话。
struct AnalyzeStoreState {
    closed: bool,
    sessions: Vec<Weak<AnalyzeSessionDatabase>>,
    latest_session: Option<Weak<AnalyzeSessionDatabase>>,
    /// Go's auto-analyze worker borrows a session from the internal pool; it is
    /// created on first use and is not part of the user session count.
    /// Go 自动分析 worker 从内部池借用会话；首次使用时创建，不计入用户会话数。
    auto_analyze_session: Option<Arc<AnalyzeSessionDatabase>>,
}

/// Produces independent SQL sessions over one canonical `Domain` and storage.
///
/// 在同一 `Domain` 与存储上产出相互独立的 SQL 会话。
pub struct AnalyzeStatsStore {
    domain: Arc<Domain>,
    runtime_topology: Vec<(u64, String)>,
    active_sessions: Arc<AtomicUsize>,
    next_connection_id: AtomicU64,
    state: Mutex<AnalyzeStoreState>,
}

fn protocol_arguments(arguments: &[DbValue]) -> Vec<ConcretePreparedArgument> {
    arguments
        .iter()
        .map(|argument| match argument {
            DbValue::Null => ConcretePreparedArgument::Null,
            DbValue::Bool(value) => ConcretePreparedArgument::Unsigned(u64::from(*value)),
            DbValue::I64(value) => ConcretePreparedArgument::Signed(*value),
            DbValue::U64(value) => ConcretePreparedArgument::Unsigned(*value),
            DbValue::F64(value) => ConcretePreparedArgument::Float(*value),
            DbValue::Bytes(value) => ConcretePreparedArgument::Bytes(value.clone()),
            DbValue::String(value) => ConcretePreparedArgument::Text(value.clone()),
        })
        .collect()
}

/// 排空 `TestRecordSet` 为列名 + 行值的 [`QueryRows`]。
fn drain_record_set(mut record_set: Box<dyn TestRecordSet>) -> TestResult<QueryRows> {
    let columns = record_set.Columns().to_vec();
    let mut rows = Vec::new();
    loop {
        let row = match record_set.Next() {
            Ok(Some(row)) => row,
            Ok(None) => break,
            Err(error) => {
                // Preserve the read error while releasing the result, just as
                // Go closes its record set even when Next returns an error.
                let _ = record_set.Close();
                return Err(TestError::new(error.to_string()));
            }
        };
        // The session-side compatibility record set uses Go's `<nil>` text
        // for SQL NULL. Restore the typed NULL before exposing database/sql
        // scanning, otherwise scanning NULL into a concrete destination would
        // incorrectly succeed as a string.
        rows.push(
            row.into_iter()
                .map(|value| {
                    if value == "<nil>" || value == CONCRETE_NULL_VALUE {
                        DbValue::Null
                    } else {
                        DbValue::String(value)
                    }
                })
                .collect(),
        );
    }
    record_set
        .Close()
        .map_err(|error| TestError::new(error.to_string()))?;
    Ok(QueryRows { columns, rows })
}

/// 在 `ConcreteSession` 上执行 SQL（无参直执行，有参则 Prepare），并收集结果集与 DML 报告。
fn execute_concrete_session(
    session: &ConcreteSession,
    sql: &str,
    arguments: &[DbValue],
) -> TestResult<SessionReply> {
    let record_sets = if arguments.is_empty() {
        session.Execute(sql)
    } else {
        let (statement_id, _, _) = session
            .prepare_protocol_statement(sql)
            .map_err(|error| TestError::new(error.to_string()))?;
        let record_sets = session
            .execute_protocol_statement(statement_id, &protocol_arguments(arguments))
            .map(|record_sets| {
                record_sets
                    .into_iter()
                    .map(|record_set| Box::new(record_set) as Box<dyn TestRecordSet>)
                    .collect()
            })
            .map_err(|error| TestError::new(error.to_string()))?;
        session
            .close_protocol_statement(statement_id)
            .map_err(|error| TestError::new(error.to_string()))?;
        Ok(record_sets)
    }
    .map_err(|error| TestError::new(error.to_string()))?;
    let record_sets = record_sets
        .into_iter()
        .map(drain_record_set)
        .collect::<TestResult<Vec<_>>>()?;
    let execution = session
        .LastDmlReport()
        .map(|report| ExecutionResult {
            affected_rows: report.AffectedRows,
            last_insert_id: report.LastInsertID,
        })
        .unwrap_or_default();
    Ok(SessionReply {
        execution,
        record_sets,
    })
}

fn create_session_with_schema_loader(
    schema_loader: Arc<dyn InfoSchemaLoader>,
    schema_lease: Duration,
) -> Result<(Arc<Domain>, ConcreteSession), String> {
    let storage = Arc::try_unwrap(
        astersql_store_mockstore_mockstorage::NewMockStorage(
            astersql_store_mockstore_mockstorage::KVStore::NewMemoryWithWallClockTSO(),
            None,
        )
        .map_err(|error| format!("create canonical mock KV storage: {error}"))?,
    )
    .map_err(|_| "canonical mock KV storage retained an unexpected owner".to_owned())?;
    let mut config = DomainConfig::default();
    config.schema_lease = schema_lease;
    config.stats_lease = std::time::Duration::ZERO;
    let domain = Arc::new(Domain::new(storage, schema_loader, config));
    domain
        .init()
        .map_err(|error| format!("initialize canonical Domain: {error}"))?;
    let session = ConcreteSession::new(Arc::clone(&domain));
    Ok((domain, session))
}

impl AnalyzeSessionDatabase {
    /// Go `bootstrapTables` 会创建所有统计系统表；补齐当前 canonical
    /// session bootstrap 未挂载、但 testkit 会通过普通会话查询的四张表。
    fn ensure_go_stats_system_tables(&self) {
        self.execute("CREATE DATABASE IF NOT EXISTS mysql", &[])
            .unwrap_or_else(|error| panic!("bootstrap mysql database for testkit: {error}"));
        for (name, definition) in GO_STATS_SYSTEM_TABLES {
            self.execute(definition, &[]).unwrap_or_else(|error| {
                panic!("bootstrap mysql.{name} for canonical testkit: {error}")
            });
        }
    }

    /// 引导会话：通过 `CreateAnalyzeSession` 创建独立 Domain。
    fn bootstrap(active_sessions: Arc<AtomicUsize>) -> Self {
        Self::spawn(active_sessions, 0, || {
            CreateAnalyzeSession().map_err(|error| error.to_string())
        })
    }

    /// Bootstrap the canonical mock runtime with an explicit schema lease.
    fn bootstrap_with_schema_lease(
        active_sessions: Arc<AtomicUsize>,
        schema_lease: Duration,
    ) -> Self {
        Self::spawn(active_sessions, 0, move || {
            create_session_with_schema_loader(Arc::new(KvInfoSchemaLoader::new()), schema_lease)
        })
    }

    /// Bootstrap a canonical session whose Domain loads shared InfoSchema-v2
    /// snapshots from the same transactional KV metadata.
    fn bootstrap_v2(active_sessions: Arc<AtomicUsize>, cache_capacity: u64) -> Self {
        Self::spawn(active_sessions, 0, move || {
            create_session_with_schema_loader(
                Arc::new(KvInfoSchemaLoader::new_v2(cache_capacity)),
                Duration::ZERO,
            )
        })
    }

    /// 在既有 Domain 上创建非受限会话。
    fn from_domain(
        domain: Arc<Domain>,
        active_sessions: Arc<AtomicUsize>,
        connection_id: u64,
    ) -> Self {
        Self::from_domain_with_scope(domain, active_sessions, connection_id, false)
    }

    /// `restricted` mirrors Go's system session pool: sessions handed to
    /// internal workers run with `SessionVars.InRestrictedSQL` set.
    ///
    /// `restricted` 对齐 Go 系统会话池：交给内部 worker 的会话会设置 `InRestrictedSQL`。
    fn from_domain_with_scope(
        domain: Arc<Domain>,
        active_sessions: Arc<AtomicUsize>,
        connection_id: u64,
        restricted: bool,
    ) -> Self {
        Self::spawn(active_sessions, connection_id, move || {
            let session = ConcreteSession::new(Arc::clone(&domain));
            session.SetInRestrictedSQL(restricted);
            Ok((domain, session))
        })
    }

    /// 在专用线程上创建会话，经通道转发 Execute/MemTracker/Close 请求。
    fn spawn<F>(active_sessions: Arc<AtomicUsize>, connection_id: u64, create_session: F) -> Self
    where
        F: FnOnce() -> Result<(Arc<Domain>, ConcreteSession), String> + Send + 'static,
    {
        let (requests, receiver) = mpsc::channel();
        let (bootstrapped, bootstrap) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let (domain, mut session) = match create_session() {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = bootstrapped.send(Err(error));
                    return;
                }
            };
            session.SetConnectionID(connection_id);
            let killer = session.SQLKiller();
            active_sessions.fetch_add(1, Ordering::AcqRel);
            let _active_session = ActiveSessionGuard(active_sessions);
            if bootstrapped
                .send(Ok((Arc::clone(&domain), killer)))
                .is_err()
            {
                return;
            }
            // 工作线程循环：处理 Execute / MemTracker / Close。
            while let Ok(request) = receiver.recv() {
                match request {
                    SessionRequest::Execute {
                        sql,
                        arguments,
                        internal,
                        response,
                    } => {
                        if internal {
                            session.SetInRestrictedSQL(true);
                        }
                        let result = execute_concrete_session(&session, &sql, &arguments);
                        if internal {
                            session.SetInRestrictedSQL(false);
                        }
                        let _ = response.send(result);
                    }
                    SessionRequest::MemTrackerChildren { response } => {
                        let _ = response.send(session.MemTrackerChildrenForTest());
                    }
                    SessionRequest::MemTrackerBytesConsumed { response } => {
                        let _ = response.send(session.MemTrackerBytesConsumedForTest());
                    }
                    SessionRequest::MemTrackerMaxConsumed { response } => {
                        let _ = response.send(session.MemTrackerMaxConsumedForTest());
                    }
                    SessionRequest::DiskTrackerMaxConsumed { response } => {
                        let _ = response.send(session.DiskTrackerMaxConsumedForTest());
                    }
                    SessionRequest::CTEStorageMapIsEmpty { response } => {
                        let _ = response.send(session.CTEStorageMapIsEmptyForTest());
                    }
                    SessionRequest::OptimizeRootPlanID { sql, response } => {
                        let result = session
                            .OptimizeRootPlanIDForTest(&sql)
                            .map_err(|error| TestError::new(error.to_string()));
                        let _ = response.send(result);
                    }
                    SessionRequest::IndexMergePathDigest { sql, response } => {
                        let result = session
                            .IndexMergePathDigestForTest(&sql)
                            .map_err(|error| TestError::new(error.to_string()));
                        let _ = response.send(result);
                    }
                    SessionRequest::LastStatementHints { response } => {
                        let _ = response.send(session.LastStatementHintsForTest());
                    }
                    SessionRequest::StatementOOMActionPriority { response } => {
                        let _ = response.send(session.StatementOOMActionPriorityForTest());
                    }
                    SessionRequest::AlternativeLogicalPlanSignals { response } => {
                        let _ = response.send(session.AlternativeLogicalPlanSignalsForTest());
                    }
                    SessionRequest::StatsLoadStatuses { response } => {
                        let statuses = session.WithSessionVars(|variables| {
                            variables
                                .StmtCtx
                                .UsedStatsLoadStatus()
                                .into_iter()
                                .map(|((table_id, item_id, is_index), status)| {
                                    (table_id, item_id, is_index, status)
                                })
                                .collect()
                        });
                        let _ = response.send(statuses);
                    }
                    SessionRequest::PrevTraceID { response } => {
                        let _ =
                            response.send(session.WithSessionVars(|vars| vars.PrevTraceIDValue()));
                    }
                    SessionRequest::ResetPrevTraceID { response } => {
                        session.WithSessionVars(|vars| vars.ResetPrevTraceID());
                        let _ = response.send(());
                    }
                    SessionRequest::ExpirePessimisticLocks { response } => {
                        session.ExpirePessimisticLocksForTest();
                        let _ = response.send(());
                    }
                    SessionRequest::SetPessimisticLockTtl { ttl, response } => {
                        session.SetPessimisticLockTTLForTest(ttl);
                        let _ = response.send(());
                    }
                    SessionRequest::InjectDmlCommitError { message, response } => {
                        session.InjectNextDmlCommitError(message);
                        let _ = response.send(());
                    }
                    SessionRequest::SetRetryAutoIncrementIds { ids, response } => {
                        session.SetRetryAutoIncrementIDsForTest(ids);
                        let _ = response.send(());
                    }
                    SessionRequest::RuntimeDeadlockHistoryCount { response } => {
                        let _ = response.send(session.RuntimeDeadlockHistoryCount());
                    }
                    SessionRequest::ClearRuntimeDeadlockHistory { response } => {
                        session.ClearRuntimeDeadlockHistory();
                        let _ = response.send(());
                    }
                    SessionRequest::LastReplicaReadRequest { response } => {
                        let _ = response.send(session.LastReplicaReadRequestForTest());
                    }
                    SessionRequest::ClearReplicaReadRequest { response } => {
                        session.ClearReplicaReadRequestForTest();
                        let _ = response.send(());
                    }
                    SessionRequest::LastSelectRequest { response } => {
                        let _ = response.send(session.LastSelectRequestForTest());
                    }
                    SessionRequest::ClearSelectRequest { response } => {
                        session.ClearSelectRequestForTest();
                        let _ = response.send(());
                    }
                    SessionRequest::TransactionIsolation { response } => {
                        let _ = response.send(session.TransactionIsolation());
                    }
                    SessionRequest::TransactionDebugString { response } => {
                        let _ = response.send(session.TxnDebugStringForTest());
                    }
                    SessionRequest::SnapshotCacheSize { response } => {
                        let _ = response.send(session.SnapCacheSizeForTest());
                    }
                    SessionRequest::SetRowEncoderEnabled { enabled, response } => {
                        session.SetRowEncoderEnabledForTest(enabled);
                        let _ = response.send(());
                    }
                    SessionRequest::LastMessage { response } => {
                        let _ = response.send(session.LastMessage());
                    }
                    SessionRequest::QueryString { response } => {
                        let _ = response.send(session.QueryString());
                    }
                    SessionRequest::SetClientCapability {
                        capability,
                        response,
                    } => {
                        session.SetClientCapability(capability);
                        let _ = response.send(());
                    }
                    SessionRequest::SetConnectionCollation {
                        collation,
                        response,
                    } => {
                        let result = session
                            .SetConnectionCollationForTest(collation)
                            .map_err(|error| TestError::new(error.to_string()));
                        let _ = response.send(result);
                    }
                    SessionRequest::StaleReadState { response } => {
                        let _ = response.send(session.StaleReadStateForTest());
                    }
                    SessionRequest::SetInspectionTableCacheEnabled { enabled, response } => {
                        session.SetInspectionTableCacheEnabledForTest(enabled);
                        let _ = response.send(());
                    }
                    SessionRequest::InspectionTableCacheRowCount { table, response } => {
                        let _ = response.send(session.InspectionTableCacheRowCountForTest(&table));
                    }
                    SessionRequest::SetInspectionTableCacheValue {
                        table,
                        row,
                        column,
                        value,
                        response,
                    } => {
                        let result = session
                            .SetInspectionTableCacheValueForTest(&table, row, &column, value)
                            .map_err(|error| TestError::new(error.to_string()));
                        let _ = response.send(result);
                    }
                    SessionRequest::ReportUsageStats { response } => {
                        let _ = response.send(());
                    }
                    SessionRequest::AuthenticateUser {
                        username,
                        hostname,
                        response,
                    } => {
                        let identity = astersql_parser_auth::parser::auth::auth::UserIdentity {
                            username,
                            hostname,
                            ..Default::default()
                        };
                        let result = session
                            .AuthenticateUserForTest(&identity)
                            .map_err(|error| TestError::new(error.to_string()));
                        let _ = response.send(result);
                    }
                    SessionRequest::Prepare { sql, response } => {
                        let result = session
                            .prepare_protocol_statement(&sql)
                            .map(|(statement_id, _, fields)| {
                                let fields = fields
                                    .into_iter()
                                    .map(|field| PreparedResultField {
                                        database_name: field.db_name.O,
                                        table_name: field.table_name.O,
                                        table_as_name: field.table_as_name.O,
                                        column_name: field.column.Name.O,
                                        column_as_name: field.column_as_name.O,
                                    })
                                    .collect();
                                (statement_id, fields)
                            })
                            .map_err(|error| TestError::new(error.to_string()));
                        let _ = response.send(result);
                    }
                    SessionRequest::ExecutePrepared {
                        statement_id,
                        arguments,
                        response,
                    } => {
                        let result = session
                            .execute_protocol_statement(
                                statement_id,
                                &protocol_arguments(&arguments),
                            )
                            .map_err(|error| TestError::new(error.to_string()))
                            .map(|_| {
                                session
                                    .LastDmlReport()
                                    .map(|report| ExecutionResult {
                                        affected_rows: report.AffectedRows,
                                        last_insert_id: report.LastInsertID,
                                    })
                                    .unwrap_or_default()
                            });
                        let _ = response.send(result);
                    }
                    SessionRequest::DropPrepared {
                        statement_id,
                        response,
                    } => {
                        let result = session
                            .close_protocol_statement(statement_id)
                            .map_err(|error| TestError::new(error.to_string()));
                        let _ = response.send(result);
                    }
                    SessionRequest::Close => break,
                }
            }
        });
        let (domain, killer) = match bootstrap.recv() {
            Ok(Ok(runtime)) => runtime,
            Ok(Err(error)) => {
                let _ = worker.join();
                panic!("bootstrap canonical analyze session: {error}");
            }
            Err(_) => {
                let _ = worker.join();
                panic!("canonical analyze session bootstrap thread exited");
            }
        };
        Self {
            lifecycle: Mutex::new(AnalyzeSessionLifecycle {
                requests: Some(requests),
                worker: Some(worker),
            }),
            domain,
            killer,
            connection_id,
        }
    }

    /// 向工作线程发送 Execute 并等待回复。
    fn request(
        &self,
        sql: &str,
        arguments: &[DbValue],
        internal: bool,
    ) -> TestResult<SessionReply> {
        let (response, result) = mpsc::sync_channel(1);
        {
            let lifecycle = self
                .lifecycle
                .lock()
                .expect("canonical analyze session lifecycle lock poisoned");
            lifecycle
                .requests
                .as_ref()
                .ok_or_else(|| TestError::new("canonical analyze session is closed"))?
                .send(SessionRequest::Execute {
                    sql: sql.to_owned(),
                    arguments: arguments.to_vec(),
                    internal,
                    response,
                })
                .map_err(|_| TestError::new("canonical analyze session is closed"))?;
        }
        result
            .recv()
            .map_err(|_| TestError::new("canonical analyze session dropped its response"))?
    }

    fn prepare_statement_metadata(&self, sql: &str) -> TestResult<(u64, Vec<PreparedResultField>)> {
        let (response, result) = mpsc::sync_channel(1);
        {
            let lifecycle = self
                .lifecycle
                .lock()
                .expect("canonical analyze session lifecycle lock poisoned");
            lifecycle
                .requests
                .as_ref()
                .ok_or_else(|| TestError::new("canonical analyze session is closed"))?
                .send(SessionRequest::Prepare {
                    sql: sql.to_owned(),
                    response,
                })
                .map_err(|_| TestError::new("canonical analyze session is closed"))?;
        }
        result
            .recv()
            .map_err(|_| TestError::new("canonical analyze session dropped its response"))?
    }

    fn drop_prepared_statement_metadata(&self, statement_id: u64) -> TestResult {
        let (response, result) = mpsc::sync_channel(1);
        {
            let lifecycle = self
                .lifecycle
                .lock()
                .expect("canonical analyze session lifecycle lock poisoned");
            lifecycle
                .requests
                .as_ref()
                .ok_or_else(|| TestError::new("canonical analyze session is closed"))?
                .send(SessionRequest::DropPrepared {
                    statement_id,
                    response,
                })
                .map_err(|_| TestError::new("canonical analyze session is closed"))?;
        }
        result
            .recv()
            .map_err(|_| TestError::new("canonical analyze session dropped its response"))?
    }

    /// 向工作线程查询会话 MemTracker 子节点数。
    fn mem_tracker_children(&self) -> TestResult<usize> {
        let (response, result) = mpsc::sync_channel(1);
        {
            let lifecycle = self
                .lifecycle
                .lock()
                .expect("canonical analyze session lifecycle lock poisoned");
            lifecycle
                .requests
                .as_ref()
                .ok_or_else(|| TestError::new("canonical analyze session is closed"))?
                .send(SessionRequest::MemTrackerChildren { response })
                .map_err(|_| TestError::new("canonical analyze session is closed"))?;
        }
        result
            .recv()
            .map_err(|_| TestError::new("canonical analyze session dropped its response"))
    }

    /// 向工作线程查询会话根 MemTracker 当前记账字节数。
    fn mem_tracker_bytes_consumed(&self) -> TestResult<i64> {
        self.send_test_request(|response| SessionRequest::MemTrackerBytesConsumed { response })
    }

    /// 向会话工作线程查询最近语句 tracker 峰值消费字节数。
    fn mem_tracker_max_consumed(&self) -> TestResult<i64> {
        self.send_test_request(|response| SessionRequest::MemTrackerMaxConsumed { response })
    }

    fn disk_tracker_max_consumed(&self) -> TestResult<i64> {
        self.send_test_request(|response| SessionRequest::DiskTrackerMaxConsumed { response })
    }

    fn cte_storage_map_is_empty(&self) -> TestResult<bool> {
        self.send_test_request(|response| SessionRequest::CTEStorageMapIsEmpty { response })
    }

    fn optimize_root_plan_id(&self, sql: &str) -> TestResult<i32> {
        self.send_test_request(|response| SessionRequest::OptimizeRootPlanID {
            sql: sql.to_owned(),
            response,
        })?
    }

    fn index_merge_path_digest(&self, sql: &str) -> TestResult<String> {
        self.send_test_request(|response| SessionRequest::IndexMergePathDigest {
            sql: sql.to_owned(),
            response,
        })?
    }

    fn last_statement_hints(&self) -> TestResult<(i64, u64)> {
        self.send_test_request(|response| SessionRequest::LastStatementHints { response })
    }

    fn statement_oom_action_priority(&self) -> TestResult<Option<i64>> {
        self.send_test_request(|response| SessionRequest::StatementOOMActionPriority { response })
    }

    fn alternative_logical_plan_signals(&self) -> TestResult<(bool, bool)> {
        self.send_test_request(|response| SessionRequest::AlternativeLogicalPlanSignals {
            response,
        })
    }

    fn stats_load_statuses(&self) -> TestResult<Vec<(i64, i64, bool, String)>> {
        self.send_test_request(|response| SessionRequest::StatsLoadStatuses { response })
    }

    fn prev_trace_id(&self) -> TestResult<Vec<u8>> {
        self.send_test_request(|response| SessionRequest::PrevTraceID { response })
    }

    fn reset_prev_trace_id(&self) -> TestResult {
        self.send_test_request(|response| SessionRequest::ResetPrevTraceID { response })
    }

    fn send_test_request<T>(
        &self,
        build: impl FnOnce(mpsc::SyncSender<T>) -> SessionRequest,
    ) -> TestResult<T> {
        let (response, result) = mpsc::sync_channel(1);
        self.lifecycle
            .lock()
            .expect("canonical analyze session lifecycle lock poisoned")
            .requests
            .as_ref()
            .ok_or_else(|| TestError::new("canonical analyze session is closed"))?
            .send(build(response))
            .map_err(|_| TestError::new("canonical analyze session is closed"))?;
        result
            .recv()
            .map_err(|_| TestError::new("canonical analyze session dropped its response"))
    }

    /// Expire this session's pessimistic locks at the next SQL boundary.
    pub fn expire_pessimistic_locks_for_test(&self) -> TestResult {
        self.send_test_request(|response| SessionRequest::ExpirePessimisticLocks { response })
    }

    /// Override this session's managed pessimistic lock TTL.
    pub fn set_pessimistic_lock_ttl_for_test(&self, ttl: std::time::Duration) -> TestResult {
        self.send_test_request(|response| SessionRequest::SetPessimisticLockTtl { ttl, response })
    }

    /// Inject the next autocommit or explicit transaction commit failure.
    pub fn inject_next_dml_commit_error(&self, message: impl Into<String>) -> TestResult {
        let message = message.into();
        self.send_test_request(|response| SessionRequest::InjectDmlCommitError {
            message,
            response,
        })
    }

    /// Install retryInfo AUTO_INCREMENT IDs on this session.
    pub fn set_retry_auto_increment_ids_for_test(&self, ids: Vec<u64>) -> TestResult {
        self.send_test_request(|response| SessionRequest::SetRetryAutoIncrementIds {
            ids,
            response,
        })
    }

    /// Return retained deadlock-history edge count.
    pub fn runtime_deadlock_history_count(&self) -> TestResult<usize> {
        self.send_test_request(|response| SessionRequest::RuntimeDeadlockHistoryCount { response })
    }

    /// Clear retained runtime deadlock history.
    pub fn clear_runtime_deadlock_history(&self) -> TestResult {
        self.send_test_request(|response| SessionRequest::ClearRuntimeDeadlockHistory { response })
    }

    /// Read the latest SQL-to-KV replica decision without executing SQL.
    pub fn last_replica_read_request_for_test(
        &self,
    ) -> TestResult<Option<RuntimeReplicaReadRequest>> {
        self.send_test_request(|response| SessionRequest::LastReplicaReadRequest { response })
    }

    /// Clear the replica observation before one statement matrix case.
    pub fn clear_replica_read_request_for_test(&self) -> TestResult {
        self.send_test_request(|response| SessionRequest::ClearReplicaReadRequest { response })
    }

    /// Read the latest relational SELECT's concrete KV request branches.
    pub fn last_select_request_for_test(&self) -> TestResult<Option<RuntimeSelectRequest>> {
        self.send_test_request(|response| SessionRequest::LastSelectRequest { response })
    }

    /// Clear the relational SELECT request observation.
    pub fn clear_select_request_for_test(&self) -> TestResult {
        self.send_test_request(|response| SessionRequest::ClearSelectRequest { response })
    }

    /// Read the isolation level installed on this session's active transaction.
    pub fn transaction_isolation_for_test(&self) -> TestResult<String> {
        self.send_test_request(|response| SessionRequest::TransactionIsolation { response })
    }

    /// Read the production transaction debug representation on its owner thread.
    pub fn transaction_debug_string_for_test(&self) -> TestResult<String> {
        self.send_test_request(|response| SessionRequest::TransactionDebugString { response })
    }

    pub fn snapshot_cache_size_for_test(&self) -> TestResult<usize> {
        self.send_test_request(|response| SessionRequest::SnapshotCacheSize { response })
    }

    pub fn set_row_encoder_enabled_for_test(&self, enabled: bool) -> TestResult {
        self.send_test_request(|response| SessionRequest::SetRowEncoderEnabled {
            enabled,
            response,
        })
    }

    fn last_message(&self) -> TestResult<String> {
        self.send_test_request(|response| SessionRequest::LastMessage { response })
    }

    fn query_string(&self) -> TestResult<String> {
        self.send_test_request(|response| SessionRequest::QueryString { response })
    }

    fn set_client_capability(&self, capability: u32) -> TestResult {
        self.send_test_request(|response| SessionRequest::SetClientCapability {
            capability,
            response,
        })
    }

    fn set_connection_collation(&self, collation: u8) -> TestResult {
        self.send_test_request(|response| SessionRequest::SetConnectionCollation {
            collation,
            response,
        })?
    }

    pub fn stale_read_state_for_test(&self) -> TestResult<RuntimeStaleReadState> {
        self.send_test_request(|response| SessionRequest::StaleReadState { response })
    }

    pub fn set_inspection_table_cache_enabled_for_test(&self, enabled: bool) -> TestResult {
        self.send_test_request(|response| SessionRequest::SetInspectionTableCacheEnabled {
            enabled,
            response,
        })
    }

    pub fn inspection_table_cache_row_count_for_test(
        &self,
        table: &str,
    ) -> TestResult<Option<usize>> {
        let table = table.to_owned();
        self.send_test_request(|response| SessionRequest::InspectionTableCacheRowCount {
            table,
            response,
        })
    }

    pub fn set_inspection_table_cache_value_for_test(
        &self,
        table: &str,
        row: usize,
        column: &str,
        value: String,
    ) -> TestResult {
        let table = table.to_owned();
        let column = column.to_owned();
        self.send_test_request(|response| SessionRequest::SetInspectionTableCacheValue {
            table,
            row,
            column,
            value,
            response,
        })?
    }

    pub fn report_usage_stats_for_test(&self) -> TestResult {
        self.send_test_request(|response| SessionRequest::ReportUsageStats { response })
    }

    pub fn authenticate_user_for_test(&self, username: &str, hostname: &str) -> TestResult {
        let username = username.to_owned();
        let hostname = hostname.to_owned();
        self.send_test_request(|response| SessionRequest::AuthenticateUser {
            username,
            hostname,
            response,
        })?
    }

    /// 发送 Close 并 join 工作线程（幂等）。
    fn shutdown(&self) -> TestResult {
        let worker = {
            let mut lifecycle = self
                .lifecycle
                .lock()
                .expect("canonical analyze session lifecycle lock poisoned");
            let Some(worker) = lifecycle.worker.take() else {
                return Ok(());
            };
            if let Some(requests) = lifecycle.requests.take() {
                let _ = requests.send(SessionRequest::Close);
            }
            worker
        };
        worker
            .join()
            .map_err(|_| TestError::new("canonical analyze session worker panicked"))
    }

    /// 返回该会话的 SQLKiller（用于注入 Kill 信号）。
    pub fn sql_killer(&self) -> Arc<SQLKiller> {
        Arc::clone(&self.killer)
    }

    /// 返回会话所属 Domain。
    pub fn domain(&self) -> Arc<Domain> {
        Arc::clone(&self.domain)
    }
}

/// Drop 时关闭工作线程。
impl Drop for AnalyzeSessionDatabase {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// 将会话通道适配为 [`Database`]：query 只返回第一个结果集。
impl Database for AnalyzeSessionDatabase {
    fn connection_id_for_test(&self) -> Option<u64> {
        Some(self.connection_id)
    }

    fn execute(&self, sql: &str, arguments: &[DbValue]) -> TestResult<ExecutionResult> {
        // Go MustExec closes any returned ResultSet; discard record sets here.
        Ok(self.request(sql, arguments, false)?.execution)
    }

    fn execute_internal(&self, sql: &str, arguments: &[DbValue]) -> TestResult<ExecutionResult> {
        Ok(self.request(sql, arguments, true)?.execution)
    }

    fn query(&self, sql: &str, arguments: &[DbValue]) -> TestResult<QueryRows> {
        let mut rows = self.request(sql, arguments, false)?.record_sets;
        if rows.is_empty() {
            return Err(TestError::new("query returned no record sets"));
        }
        // Go TestKit returns only the first result set for multi-statements.
        Ok(rows.remove(0))
    }

    fn query_internal(&self, sql: &str, arguments: &[DbValue]) -> TestResult<QueryRows> {
        let mut rows = self.request(sql, arguments, true)?.record_sets;
        if rows.is_empty() {
            return Err(TestError::new("internal query returned no record sets"));
        }
        Ok(rows.remove(0))
    }

    fn prepare_statement(&self, sql: &str) -> TestResult<(u64, Vec<PreparedResultField>)> {
        self.prepare_statement_metadata(sql)
    }

    fn execute_prepared_statement(
        &self,
        statement_id: u64,
        arguments: &[DbValue],
    ) -> TestResult<ExecutionResult> {
        let arguments = arguments.to_vec();
        self.send_test_request(|response| SessionRequest::ExecutePrepared {
            statement_id,
            arguments,
            response,
        })?
    }

    fn drop_prepared_statement(&self, statement_id: u64) -> TestResult {
        self.drop_prepared_statement_metadata(statement_id)
    }

    fn close(&self) -> TestResult {
        self.shutdown()
    }

    fn analyze_stats_context(&self) -> Option<AnalyzeStatsContext> {
        Some(self.domain.stats_context())
    }

    fn mem_tracker_children_for_test(&self) -> usize {
        self.mem_tracker_children()
            .unwrap_or_else(|error| panic!("read session MemTracker children: {error}"))
    }

    fn mem_tracker_bytes_consumed_for_test(&self) -> i64 {
        self.mem_tracker_bytes_consumed()
            .unwrap_or_else(|error| panic!("read session MemTracker bytes: {error}"))
    }

    fn mem_tracker_max_consumed_for_test(&self) -> i64 {
        self.mem_tracker_max_consumed()
            .unwrap_or_else(|error| panic!("read session MemTracker max bytes: {error}"))
    }

    fn disk_tracker_max_consumed_for_test(&self) -> i64 {
        self.disk_tracker_max_consumed()
            .unwrap_or_else(|error| panic!("read session DiskTracker max bytes: {error}"))
    }

    fn cte_storage_map_is_empty_for_test(&self) -> bool {
        self.cte_storage_map_is_empty()
            .unwrap_or_else(|error| panic!("read session CTE storage map: {error}"))
    }

    fn optimize_root_plan_id_for_test(&self, sql: &str) -> TestResult<i32> {
        self.optimize_root_plan_id(sql)
    }

    fn index_merge_path_digest_for_test(&self, sql: &str) -> TestResult<String> {
        self.index_merge_path_digest(sql)
    }

    fn last_statement_hints_for_test(&self) -> (i64, u64) {
        self.last_statement_hints()
            .unwrap_or_else(|error| panic!("read latest statement hints: {error}"))
    }

    fn statement_oom_action_priority_for_test(&self) -> Option<i64> {
        self.statement_oom_action_priority()
            .unwrap_or_else(|error| panic!("read statement OOM action priority: {error}"))
    }

    fn alternative_logical_plan_signals_for_test(&self) -> Option<(bool, bool)> {
        Some(
            self.alternative_logical_plan_signals()
                .unwrap_or_else(|error| panic!("read alternative logical plan signals: {error}")),
        )
    }

    fn stats_load_statuses_for_test(&self) -> Vec<(i64, i64, bool, String)> {
        self.stats_load_statuses()
            .unwrap_or_else(|error| panic!("read session stats load statuses: {error}"))
    }

    fn prev_trace_id_for_test(&self) -> Vec<u8> {
        self.prev_trace_id()
            .unwrap_or_else(|error| panic!("read previous trace ID: {error}"))
    }

    fn reset_prev_trace_id_for_test(&self) -> TestResult {
        self.reset_prev_trace_id()
    }

    fn transaction_isolation_for_test(&self) -> Option<String> {
        Some(
            self.transaction_isolation_for_test()
                .unwrap_or_else(|error| panic!("read transaction isolation: {error}")),
        )
    }

    fn transaction_debug_string_for_test(&self) -> Option<String> {
        Some(
            self.transaction_debug_string_for_test()
                .unwrap_or_else(|error| panic!("read transaction debug string: {error}")),
        )
    }

    fn snapshot_cache_size_for_test(&self) -> usize {
        self.snapshot_cache_size_for_test()
            .unwrap_or_else(|error| panic!("read transaction snapshot cache size: {error}"))
    }

    fn set_row_encoder_enabled_for_test(&self, enabled: bool) -> TestResult {
        self.set_row_encoder_enabled_for_test(enabled)
    }

    fn last_message_for_test(&self) -> String {
        self.last_message()
            .unwrap_or_else(|error| panic!("read session last message: {error}"))
    }

    fn query_string_for_test(&self) -> String {
        self.query_string()
            .unwrap_or_else(|error| panic!("read session query string: {error}"))
    }

    fn set_client_capability_for_test(&self, capability: u32) -> TestResult {
        self.set_client_capability(capability)
    }

    fn set_connection_collation_for_test(&self, collation: u8) -> TestResult {
        self.set_connection_collation(collation)
    }

    fn stale_read_state_for_test(&self) -> Option<RuntimeStaleReadState> {
        Some(
            self.stale_read_state_for_test()
                .unwrap_or_else(|error| panic!("read stale-read state: {error}")),
        )
    }

    fn set_inspection_table_cache_enabled_for_test(&self, enabled: bool) -> TestResult {
        self.set_inspection_table_cache_enabled_for_test(enabled)
    }

    fn inspection_table_cache_row_count_for_test(&self, table: &str) -> TestResult<Option<usize>> {
        self.inspection_table_cache_row_count_for_test(table)
    }

    fn set_inspection_table_cache_value_for_test(
        &self,
        table: &str,
        row: usize,
        column: &str,
        value: String,
    ) -> TestResult {
        self.set_inspection_table_cache_value_for_test(table, row, column, value)
    }

    fn report_usage_stats_for_test(&self) -> TestResult {
        self.report_usage_stats_for_test()
    }

    fn authenticate_user_for_test(&self, username: &str, hostname: &str) -> TestResult {
        self.authenticate_user_for_test(username, hostname)
    }

    fn inject_next_dml_commit_error_for_test(&self, message: String) -> TestResult {
        self.inject_next_dml_commit_error(message)
    }

    fn set_retry_auto_increment_ids_for_test(&self, ids: Vec<u64>) -> TestResult {
        self.set_retry_auto_increment_ids_for_test(ids)
    }
}

impl AnalyzeStatsStore {
    fn latest_session(&self) -> TestResult<Arc<AnalyzeSessionDatabase>> {
        self.state
            .lock()
            .expect("analyze statistics store lock poisoned")
            .latest_session
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or_else(|| TestError::new("analyze statistics store has no active session"))
    }

    /// 引导后关闭引导会话，留下共享 Domain 供后续会话复用。
    fn new() -> Self {
        let active_sessions = Arc::new(AtomicUsize::new(0));
        let bootstrap = AnalyzeSessionDatabase::bootstrap(Arc::clone(&active_sessions));
        bootstrap.ensure_go_stats_system_tables();
        Self::from_bootstrap(active_sessions, bootstrap)
    }

    /// Build the canonical mock store using the requested schema lease.
    fn new_with_schema_lease(schema_lease: Duration) -> Self {
        let active_sessions = Arc::new(AtomicUsize::new(0));
        let bootstrap = AnalyzeSessionDatabase::bootstrap_with_schema_lease(
            Arc::clone(&active_sessions),
            schema_lease,
        );
        bootstrap.ensure_go_stats_system_tables();
        Self::from_bootstrap(active_sessions, bootstrap)
    }

    /// Build the canonical store with a shared InfoSchema-v2 loader.
    fn new_v2(cache_capacity: u64) -> Self {
        let active_sessions = Arc::new(AtomicUsize::new(0));
        let bootstrap =
            AnalyzeSessionDatabase::bootstrap_v2(Arc::clone(&active_sessions), cache_capacity);
        bootstrap.ensure_go_stats_system_tables();
        Self::from_bootstrap(active_sessions, bootstrap)
    }

    fn from_bootstrap(
        active_sessions: Arc<AtomicUsize>,
        bootstrap: AnalyzeSessionDatabase,
    ) -> Self {
        let domain = bootstrap.domain();
        let runtime_topology = vec![
            (1, "tikv-1:20160".to_owned()),
            (2, "tikv-2:20160".to_owned()),
            (3, "tikv-3:20160".to_owned()),
        ];
        RegisterRuntimeTopology(&domain, runtime_topology.clone());
        bootstrap
            .shutdown()
            .unwrap_or_else(|error| panic!("close bootstrap analyze session: {error}"));
        Self {
            domain,
            runtime_topology,
            active_sessions,
            next_connection_id: AtomicU64::new(1),
            state: Mutex::new(AnalyzeStoreState {
                closed: false,
                sessions: Vec::new(),
                latest_session: None,
                auto_analyze_session: None,
            }),
        }
    }

    /// Go's auto-analyze worker keeps one internal session for the lifetime of
    /// the Domain; it is created lazily so idle stores never spawn one.
    ///
    /// Go 自动分析 worker 在 Domain 生命周期内保留一个内部会话；惰性创建以免空闲 Store 白占线程。
    fn auto_analyze_session(&self) -> TestResult<Arc<AnalyzeSessionDatabase>> {
        let mut state = self
            .state
            .lock()
            .expect("analyze statistics store lock poisoned");
        if state.closed {
            return Err(TestError::new("analyze statistics store is closed"));
        }
        if let Some(session) = state.auto_analyze_session.as_ref() {
            return Ok(Arc::clone(session));
        }
        let session = Arc::new(AnalyzeSessionDatabase::from_domain_with_scope(
            Arc::clone(&self.domain),
            Arc::new(AtomicUsize::new(0)),
            0,
            true,
        ));
        state.auto_analyze_session = Some(Arc::clone(&session));
        Ok(session)
    }

    /// 打开新用户会话并登记弱引用。
    fn open_session(&self) -> TestResult<Arc<AnalyzeSessionDatabase>> {
        let mut state = self
            .state
            .lock()
            .expect("analyze statistics store lock poisoned");
        if state.closed {
            return Err(TestError::new("analyze statistics store is closed"));
        }
        // 清理已释放的弱引用后再登记新会话。
        state.sessions.retain(|session| session.strong_count() > 0);
        let connection_id = self.next_connection_id.fetch_add(1, Ordering::Relaxed);
        let session = Arc::new(AnalyzeSessionDatabase::from_domain(
            Arc::clone(&self.domain),
            Arc::clone(&self.active_sessions),
            connection_id,
        ));
        let weak = Arc::downgrade(&session);
        state.latest_session = Some(weak.clone());
        state.sessions.push(weak);
        Ok(session)
    }

    /// 关闭全部用户会话与自动分析会话。
    fn shutdown(&self) -> TestResult {
        let (sessions, auto_analyze_session) = {
            let mut state = self
                .state
                .lock()
                .expect("analyze statistics store lock poisoned");
            if state.closed {
                return Ok(());
            }
            state.closed = true;
            state.latest_session = None;
            let auto_analyze_session = state.auto_analyze_session.take();
            (
                state
                    .sessions
                    .drain(..)
                    .filter_map(|session| session.upgrade())
                    .collect::<Vec<_>>(),
                auto_analyze_session,
            )
        };
        // 依次关闭仍存活的用户会话与自动分析会话。
        for session in sessions {
            session.shutdown()?;
        }
        if let Some(session) = auto_analyze_session {
            session.shutdown()?;
        }
        Ok(())
    }

    /// 返回共享 Domain。
    pub fn domain(&self) -> Arc<Domain> {
        Arc::clone(&self.domain)
    }

    /// Return the TiKV fixture nodes backing runtime region observations.
    pub fn runtime_topology(&self) -> &[(u64, String)] {
        &self.runtime_topology
    }

    /// 返回最近活跃会话的 SQLKiller。
    pub fn sql_killer(&self) -> Arc<SQLKiller> {
        self.latest_session()
            .unwrap_or_else(|_| panic!("analyze statistics store has no active session"))
            .sql_killer()
    }

    /// Expire the most recently opened user session's pessimistic locks.
    pub fn expire_latest_pessimistic_locks_for_test(&self) -> TestResult {
        self.latest_session()?.expire_pessimistic_locks_for_test()
    }

    /// Override the most recently opened user session's lock TTL.
    pub fn set_latest_pessimistic_lock_ttl_for_test(&self, ttl: std::time::Duration) -> TestResult {
        self.latest_session()?
            .set_pessimistic_lock_ttl_for_test(ttl)
    }

    /// Inject a commit failure into the most recently opened user session.
    pub fn inject_latest_dml_commit_error(&self, message: impl Into<String>) -> TestResult {
        self.latest_session()?.inject_next_dml_commit_error(message)
    }

    /// Return the process-local deadlock-history edge count.
    pub fn runtime_deadlock_history_count(&self) -> TestResult<usize> {
        self.latest_session()?.runtime_deadlock_history_count()
    }

    /// Clear process-local deadlock history through the latest session.
    pub fn clear_runtime_deadlock_history(&self) -> TestResult {
        self.latest_session()?.clear_runtime_deadlock_history()
    }

    /// Read the latest session's SQL-to-KV replica decision.
    pub fn last_replica_read_request_for_test(
        &self,
    ) -> TestResult<Option<RuntimeReplicaReadRequest>> {
        self.latest_session()?.last_replica_read_request_for_test()
    }

    /// Clear the latest session's replica observation.
    pub fn clear_replica_read_request_for_test(&self) -> TestResult {
        self.latest_session()?.clear_replica_read_request_for_test()
    }

    /// Read the latest session's relational SELECT request branches.
    pub fn last_select_request_for_test(&self) -> TestResult<Option<RuntimeSelectRequest>> {
        self.latest_session()?.last_select_request_for_test()
    }

    /// Clear the latest session's relational SELECT request observation.
    pub fn clear_select_request_for_test(&self) -> TestResult {
        self.latest_session()?.clear_select_request_for_test()
    }

    pub fn stale_read_state_for_test(&self) -> TestResult<RuntimeStaleReadState> {
        self.latest_session()?.stale_read_state_for_test()
    }

    /// Read the canonical mock oracle's successful timestamp request count.
    pub fn tso_request_count_for_test(&self) -> u64 {
        self.domain
            .storage()
            .with_storage(|storage| storage.TSORequestCountForTest())
    }

    /// 当前活跃用户会话数。
    pub fn active_session_count(&self) -> usize {
        self.active_sessions.load(Ordering::Acquire)
    }
}

/// Store 本身不直接执行 SQL，须先 `create_session`。
impl Database for AnalyzeStatsStore {
    fn execute(&self, _sql: &str, _arguments: &[DbValue]) -> TestResult<ExecutionResult> {
        Err(TestError::new(
            "analyze statistics store must create a session before executing SQL",
        ))
    }

    fn query(&self, _sql: &str, _arguments: &[DbValue]) -> TestResult<QueryRows> {
        Err(TestError::new(
            "analyze statistics store must create a session before querying SQL",
        ))
    }

    fn create_session(&self) -> TestResult<Option<Arc<dyn Database>>> {
        Ok(Some(self.open_session()?))
    }

    fn close(&self) -> TestResult {
        self.shutdown()
    }

    fn analyze_stats_context(&self) -> Option<AnalyzeStatsContext> {
        Some(self.domain.stats_context())
    }
}

/// 创建已注册自动分析执行器的 AnalyzeStatsStore。
pub fn CreateAnalyzeStatsStore() -> Arc<AnalyzeStatsStore> {
    CreateMockStoreAndDomain().0
}

/// Canonical multi-store test cluster sharing Domain cross-keyspace state.
pub struct CrossKeyspaceTestCluster {
    coordinator: Arc<CrossKeyspaceCoordinator>,
    stores: BTreeMap<String, Arc<AnalyzeStatsStore>>,
}

impl CrossKeyspaceTestCluster {
    /// Return a named keyspace store.
    pub fn store(&self, name: &str) -> Arc<AnalyzeStatsStore> {
        self.stores
            .get(name)
            .unwrap_or_else(|| panic!("unknown cross-keyspace test store {name}"))
            .clone()
    }

    /// Return the coordinator shared by all stores in this cluster.
    pub fn coordinator(&self) -> Arc<CrossKeyspaceCoordinator> {
        Arc::clone(&self.coordinator)
    }

    /// Return configured keyspace names in deterministic order.
    pub fn keyspaces(&self) -> Vec<String> {
        self.stores.keys().cloned().collect()
    }
}

/// Go's auto-analyze worker runs `analyze table ...` on a system session; the
/// Domain has no SQL engine of its own, so the store provides that session.
///
/// Go 自动分析 worker 在系统会话上执行 `analyze table`；Domain 自身无 SQL 引擎，由 Store 提供该会话。
impl AutoAnalyzeExecutor for AnalyzeStatsStore {
    fn execute_auto_analyze(&self, sql: &str) -> Result<(), String> {
        self.auto_analyze_session()
            .and_then(|session| session.execute(sql, &[]))
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// 创建 AnalyzeStatsStore 与 Domain，并注册自动分析执行器弱引用。
pub fn CreateMockStoreAndDomain() -> (Arc<AnalyzeStatsStore>, Arc<Domain>) {
    let store = Arc::new(AnalyzeStatsStore::new());
    let domain = store.domain();
    domain.register_auto_analyze_executor(Arc::downgrade(&store) as Weak<dyn AutoAnalyzeExecutor>);
    (store, domain)
}

/// Create the canonical mock store and Domain with an explicit schema lease.
pub fn CreateMockStoreAndDomainWithSchemaLease(
    schema_lease: Duration,
) -> (Arc<AnalyzeStatsStore>, Arc<Domain>) {
    let store = Arc::new(AnalyzeStatsStore::new_with_schema_lease(schema_lease));
    let domain = store.domain();
    domain.register_auto_analyze_executor(Arc::downgrade(&store) as Weak<dyn AutoAnalyzeExecutor>);
    (store, domain)
}

/// Use the production TiKV driver and canonical SQL sessions for integration
/// tests. Connection/bootstrap failures are reported; this never falls back to
/// an in-memory store or the RealTiKV lifecycle stubs.
pub fn CreateTiKVStoreAndDomain(path: &str) -> TestResult<(Arc<AnalyzeStatsStore>, Arc<Domain>)> {
    let store = astersql_store_driver::TiKVDriver::default()
        .Open(path)
        .map_err(|error| TestError::new(format!("open integration TiKV store: {error}")))?;
    let active_sessions = Arc::new(AtomicUsize::new(0));
    let bootstrap = AnalyzeSessionDatabase::spawn(Arc::clone(&active_sessions), 0, move || {
        let factory = astersql_session::runtime::CanonicalSessionFactory::from_tikv_store(store)
            .map_err(|error| error.to_string())?;
        Ok((Arc::clone(factory.domain()), factory.create_session()))
    });
    bootstrap.ensure_go_stats_system_tables();
    let store = Arc::new(AnalyzeStatsStore::from_bootstrap(
        active_sessions,
        bootstrap,
    ));
    let domain = store.domain();
    domain.register_auto_analyze_executor(Arc::downgrade(&store) as Weak<dyn AutoAnalyzeExecutor>);
    Ok((store, domain))
}

/// Create the canonical SQL store with a production InfoSchema-v2 loader.
pub fn CreateMockStoreAndDomainV2(cache_capacity: u64) -> (Arc<AnalyzeStatsStore>, Arc<Domain>) {
    let store = Arc::new(AnalyzeStatsStore::new_v2(cache_capacity));
    let domain = store.domain();
    domain.register_auto_analyze_executor(Arc::downgrade(&store) as Weak<dyn AutoAnalyzeExecutor>);
    (store, domain)
}

/// Create canonical AnalyzeStatsStore runtimes bound to one shared
/// cross-keyspace coordinator.
///
/// Each tuple is `(keyspace_name, new_collation_enabled)`. SYSTEM is supplied
/// with new collation enabled when omitted, matching the NextGen test fixture.
pub fn CreateCrossKeyspaceTestCluster(keyspaces: &[(&str, bool)]) -> CrossKeyspaceTestCluster {
    let coordinator = Arc::new(CrossKeyspaceCoordinator::new());
    let mut configurations = BTreeMap::new();
    configurations.insert("SYSTEM".to_owned(), true);
    for (keyspace, new_collation) in keyspaces {
        configurations.insert((*keyspace).to_owned(), *new_collation);
    }

    let stores = configurations
        .into_iter()
        .map(|(keyspace, new_collation)| {
            let (store, domain) = CreateMockStoreAndDomain();
            domain.bind_cross_keyspace(Arc::clone(&coordinator), keyspace.clone(), new_collation);
            (keyspace, store)
        })
        .collect();

    CrossKeyspaceTestCluster {
        coordinator,
        stores,
    }
}
