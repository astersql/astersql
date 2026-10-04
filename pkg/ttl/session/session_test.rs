// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// TTL 会话行为测试：事务提交/回滚、时区复位与 KillStmt。
//
// 用内存 MockStorage / MockSqlExecutor 模拟 Go testkit 对表 `t(id, v)` 的 SQL。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::{
    ExecutionContext, MetaOnlyInfoSchema, Phase, PhaseTracer, Row, Session, SessionContext,
    SessionError, SessionVariables, SqlExecutor, SqlValue, Storage, TxnMode, new_session,
};

/// 空存储占位。
#[derive(Default)]
struct MockStorage;

impl Storage for MockStorage {}

/// 空 InfoSchema 占位。
#[derive(Default)]
struct MockInfoSchema;

impl MetaOnlyInfoSchema for MockInfoSchema {}

/// In-memory table plus transaction state, mirroring Go testkit SQL against `t(id, v)`.
/// 内存表与事务快照，镜像 Go testkit 对 `t(id, v)` 的行为。
struct TableState {
    committed: BTreeMap<i64, i64>,
    in_txn: bool,
    txn_snapshot: BTreeMap<i64, i64>,
    txn_rows: BTreeMap<i64, i64>,
}

impl Default for TableState {
    fn default() -> Self {
        Self {
            committed: BTreeMap::new(),
            in_txn: false,
            txn_snapshot: BTreeMap::new(),
            txn_rows: BTreeMap::new(),
        }
    }
}

impl TableState {
    /// 开启事务：拷贝已提交行到快照与工作集。
    fn begin(&mut self) {
        self.in_txn = true;
        self.txn_snapshot = self.committed.clone();
        self.txn_rows = self.committed.clone();
    }

    /// 提交：工作集写入已提交，清空事务态。
    fn commit(&mut self) {
        if self.in_txn {
            self.committed = self.txn_rows.clone();
            self.in_txn = false;
            self.txn_snapshot.clear();
            self.txn_rows.clear();
        }
    }

    /// 回滚：恢复快照。
    fn rollback(&mut self) {
        if self.in_txn {
            self.committed = self.txn_snapshot.clone();
            self.in_txn = false;
            self.txn_snapshot.clear();
            self.txn_rows.clear();
        }
    }

    /// 插入行；主键冲突返回 Sql 错误。
    fn insert(&mut self, id: i64, v: i64) -> Result<(), SessionError> {
        let rows = if self.in_txn {
            &mut self.txn_rows
        } else {
            &mut self.committed
        };
        if rows.contains_key(&id) {
            return Err(SessionError::Sql(format!("duplicate primary key {id}")));
        }
        rows.insert(id, v);
        Ok(())
    }

    /// 按主键序返回 `"id v"` 字符串列表。
    fn select_ordered(&self) -> Vec<String> {
        let rows = if self.in_txn {
            &self.txn_rows
        } else {
            &self.committed
        };
        rows.iter().map(|(id, v)| format!("{id} {v}")).collect()
    }
}

/// 解析 BEGIN/COMMIT/ROLLBACK/INSERT/SELECT/SLEEP 的测试执行器。
struct MockSqlExecutor {
    variables: Arc<SessionVariables>,
    table: Mutex<TableState>,
    sleep_running: AtomicBool,
}

impl MockSqlExecutor {
    /// 绑定会话变量构造执行器。
    fn new(variables: Arc<SessionVariables>) -> Self {
        Self {
            variables,
            table: Mutex::new(TableState::default()),
            sleep_running: AtomicBool::new(false),
        }
    }

    /// 仅支持 `select * from t` 与 `select @@time_zone` 的辅助查询。
    fn must_query_rows(&self, sql: &str) -> Vec<String> {
        let sql = sql.trim().to_ascii_lowercase();
        if sql.starts_with("select * from t") {
            return self.table.lock().unwrap().select_ordered();
        }
        if sql == "select @@time_zone" {
            return vec![self.variables.session_or_global_time_zone()];
        }
        panic!("unexpected query: {sql}");
    }
}

impl SqlExecutor for MockSqlExecutor {
    fn execute_internal(
        &self,
        _context: &ExecutionContext,
        sql: &str,
        _arguments: &[SqlValue],
    ) -> Result<Option<Box<dyn crate::RecordSet>>, SessionError> {
        let trimmed = sql.trim();
        let upper = trimmed.to_ascii_uppercase();
        match upper.as_str() {
            "BEGIN OPTIMISTIC" | "BEGIN PESSIMISTIC" => {
                self.table.lock().unwrap().begin();
                return Ok(None);
            }
            "COMMIT" => {
                self.table.lock().unwrap().commit();
                return Ok(None);
            }
            "ROLLBACK" => {
                self.table.lock().unwrap().rollback();
                return Ok(None);
            }
            "SET @@TIME_ZONE=@@GLOBAL.TIME_ZONE" => {
                let global = self.variables.global_system_variable("time_zone")?;
                self.variables.set_time_zone(Some(global));
                return Ok(None);
            }
            _ => {}
        }

        let lower = trimmed.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("insert into t values (") {
            let body = rest.trim_end_matches(')');
            let mut parts = body.split(',');
            let id: i64 = parts
                .next()
                .ok_or_else(|| SessionError::Sql("missing id".into()))?
                .trim()
                .parse()
                .map_err(|_| SessionError::Sql("bad id".into()))?;
            let v: i64 = parts
                .next()
                .ok_or_else(|| SessionError::Sql("missing v".into()))?
                .trim()
                .parse()
                .map_err(|_| SessionError::Sql("bad v".into()))?;
            self.table.lock().unwrap().insert(id, v)?;
            return Ok(None);
        }

        // 模拟长时间 sleep，轮询 killed 标志；被打断时返回 "1"。
        if lower == "select sleep(123)" {
            self.sleep_running.store(true, Ordering::Release);
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(10) {
                if self.variables.is_query_interrupted() {
                    self.sleep_running.store(false, Ordering::Release);
                    // Go: killed sleep returns "1"
                    return Ok(Some(Box::new(VecRecordSet {
                        rows: vec![Row {
                            values: vec![SqlValue::String("1".into())],
                        }],
                    })));
                }
                thread::sleep(Duration::from_millis(5));
            }
            self.sleep_running.store(false, Ordering::Release);
            return Err(SessionError::Sql("sleep timeout".into()));
        }

        Err(SessionError::Sql(format!("unsupported sql: {trimmed}")))
    }
}

/// 内存结果集：一次 drain 取走全部行。
struct VecRecordSet {
    rows: Vec<Row>,
}

impl crate::RecordSet for VecRecordSet {
    fn drain(&mut self, _batch_size: usize) -> Result<Vec<Row>, SessionError> {
        Ok(std::mem::take(&mut self.rows))
    }

    fn close(&mut self) -> Result<(), SessionError> {
        Ok(())
    }
}

/// 组装 Store / Variables / InfoSchema / Executor 的测试上下文。
struct MockSessionContext {
    store: Arc<dyn Storage>,
    variables: Arc<SessionVariables>,
    info_schema: Arc<dyn MetaOnlyInfoSchema>,
    sql_executor: Arc<dyn SqlExecutor>,
}

impl SessionContext for MockSessionContext {
    fn store(&self) -> Arc<dyn Storage> {
        Arc::clone(&self.store)
    }

    fn session_variables(&self) -> Arc<SessionVariables> {
        Arc::clone(&self.variables)
    }

    fn latest_info_schema(&self) -> Arc<dyn MetaOnlyInfoSchema> {
        Arc::clone(&self.info_schema)
    }

    fn transaction_info_schema(&self) -> Arc<dyn MetaOnlyInfoSchema> {
        Arc::clone(&self.info_schema)
    }

    fn sql_executor(&self) -> Arc<dyn SqlExecutor> {
        Arc::clone(&self.sql_executor)
    }
}

/// 进程列表条目（仅保留当前 SQL 文本）。
struct ProcessInfo {
    info: String,
}

/// 简易 SessionManager：用于 Kill 测试发现 sleep 语句。
struct MockSessionManager {
    current_sql: Mutex<Option<String>>,
}

impl MockSessionManager {
    fn show_process_list(&self) -> Vec<ProcessInfo> {
        match self.current_sql.lock().unwrap().clone() {
            Some(info) => vec![ProcessInfo { info }],
            None => Vec::new(),
        }
    }

    fn set_current_sql(&self, sql: Option<String>) {
        *self.current_sql.lock().unwrap() = sql;
    }
}

/// 构造会话与共享 MockSqlExecutor 对。
fn new_mock_pair() -> (Arc<dyn Session>, Arc<MockSqlExecutor>) {
    let variables = Arc::new(SessionVariables::default());
    let executor = Arc::new(MockSqlExecutor::new(Arc::clone(&variables)));
    let ctx = Arc::new(MockSessionContext {
        store: Arc::new(MockStorage),
        variables,
        info_schema: Arc::new(MockInfoSchema),
        sql_executor: Arc::clone(&executor) as Arc<dyn SqlExecutor>,
    });
    let se = new_session(ctx, || {});
    (se, executor)
}

/// TestSessionRunInTxn: successful commit, error rollback, then another commit.
/// 成功提交、回调错误触发回滚、再次提交。
#[test]
fn TestSessionRunInTxn() {
    let (se, executor) = new_mock_pair();
    let ctx = ExecutionContext::default();

    se.run_in_transaction(
        &ctx,
        &mut || {
            executor
                .execute_internal(&ctx, "insert into t values (1, 10)", &[])
                .map(|_| ())
        },
        TxnMode::Optimistic,
    )
    .expect("commit should succeed");
    assert_eq!(
        executor.must_query_rows("select * from t order by id asc"),
        vec!["1 10".to_string()]
    );

    let err = se
        .run_in_transaction(
            &ctx,
            &mut || {
                executor
                    .execute_internal(&ctx, "insert into t values (2, 20)", &[])
                    .map(|_| ())?;
                Err(SessionError::Sql("mockErr".into()))
            },
            TxnMode::Optimistic,
        )
        .expect_err("callback error should surface");
    // Go require.EqualError(..., "mockErr") — Sql payload carries the same text.
    assert!(matches!(err, SessionError::Sql(ref s) if s == "mockErr"));
    assert_eq!(
        executor.must_query_rows("select * from t order by id asc"),
        vec!["1 10".to_string()]
    );

    se.run_in_transaction(
        &ctx,
        &mut || {
            executor
                .execute_internal(&ctx, "insert into t values (3, 30)", &[])
                .map(|_| ())
        },
        TxnMode::Optimistic,
    )
    .expect("second commit should succeed");
    assert_eq!(
        executor.must_query_rows("select * from t order by id asc"),
        vec!["1 10".to_string(), "3 30".to_string()]
    );
}

/// Go defers rollback and phase restoration even while a callback panic unwinds.
#[test]
fn run_in_transaction_rolls_back_and_restores_phase_on_panic() {
    let (se, executor) = new_mock_pair();
    let tracer = Arc::new(PhaseTracer::default());
    tracer.enter_phase(Phase::CommitTransaction);
    let ctx = ExecutionContext::default().with_phase_tracer(Arc::clone(&tracer));

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = se.run_in_transaction(
            &ctx,
            &mut || {
                executor
                    .execute_internal(&ctx, "insert into t values (2, 20)", &[])
                    .map(|_| ())?;
                panic!("mock panic");
            },
            TxnMode::Pessimistic,
        );
    }));

    assert!(panic.is_err());
    assert_eq!(
        executor.must_query_rows("select * from t order by id asc"),
        Vec::<String>::new(),
        "panic must not leave the transaction active or its rows visible"
    );
    assert_eq!(tracer.phase(), Phase::CommitTransaction);
}

/// TestSessionKill: background poll finds sleep SQL then KillStmt; sleep returns "1".
/// 后台轮询发现 sleep SQL 后 KillStmt，sleep 返回 "1"。
#[test]
fn TestSessionKill() {
    let (se, executor) = new_mock_pair();
    let mgr = Arc::new(MockSessionManager {
        current_sql: Mutex::new(None),
    });
    let sleep_stmt = "select sleep(123)";
    let se_bg = Arc::clone(&se);
    let mgr_bg = Arc::clone(&mgr);

    let killer = thread::spawn(move || {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            thread::sleep(Duration::from_millis(10));
            for proc in mgr_bg.show_process_list() {
                if proc.info == sleep_stmt {
                    se_bg.kill_statement();
                    return;
                }
            }
        }
        panic!("wait sleep stmt timeout");
    });

    mgr.set_current_sql(Some(sleep_stmt.to_string()));
    let rows = se
        .execute_sql(&ExecutionContext::default(), sleep_stmt, &[])
        .expect("killed sleep should return");
    assert_eq!(
        rows,
        vec![Row {
            values: vec![SqlValue::String("1".into())]
        }]
    );
    mgr.set_current_sql(None);
    killer.join().expect("killer thread");
    assert!(!executor.sleep_running.load(Ordering::Acquire));
}
