// Copyright 2026 AsterSQL.

// DDL 测试工具单元测试。
//
// 验证真实 `CallWithSCtx(FlagWrapTxn)` 事务路径、DDL 内部来源、错误回滚，
// 以及事件查找的 Go 边界行为。

use super::*;
use aster_sql_ddl_notifier::{NewDropSchemaEvent, NewFlashbackClusterEvent, SchemaChangeEvent};
use aster_sql_kv::{Context, GetInternalSourceType, InternalDDLNotifier};
use aster_sql_meta_model::{ACTION_DROP_SCHEMA, DBInfo};
use aster_sql_statistics_handle_util::{
    ExecOption, ExecutionContext, GlobalVariableAccessor, INNODB_LOCK_WAIT_TIMEOUT,
    RestrictedSqlExecutor, ResultField, Row, SessionContext, SessionPool, SessionVariables,
    SqlExecutor, SqlValue, StatsError, TIDB_ANALYZE_PARTITION_CONCURRENCY,
    TIDB_ANALYZE_SKIP_COLUMN_TYPES, TIDB_ANALYZE_VERSION, TIDB_ENABLE_ANALYZE_SNAPSHOT,
    TIDB_ENABLE_ASYNC_MERGE_GLOBAL_STATS, TIDB_ENABLE_HISTORICAL_STATS,
    TIDB_MERGE_PARTITION_STATS_CONCURRENCY, TIDB_PARTITION_PRUNE_MODE,
    TIDB_SKIP_MISSING_PARTITION_STATS, TIME_ZONE, Transaction,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Instant;

struct TestGlobalVariables;

impl GlobalVariableAccessor for TestGlobalVariables {
    fn get_global_sys_var(&self, name: &str) -> std::result::Result<String, StatsError> {
        let values = HashMap::from([
            (TIDB_ENABLE_ASYNC_MERGE_GLOBAL_STATS, "0"),
            (TIDB_ANALYZE_PARTITION_CONCURRENCY, "1"),
            (TIDB_ANALYZE_VERSION, "2"),
            (TIDB_ENABLE_HISTORICAL_STATS, "0"),
            (TIDB_PARTITION_PRUNE_MODE, "dynamic"),
            (TIDB_ENABLE_ANALYZE_SNAPSHOT, "0"),
            (TIDB_ANALYZE_SKIP_COLUMN_TYPES, ""),
            (TIDB_SKIP_MISSING_PARTITION_STATS, "0"),
            (TIDB_MERGE_PARTITION_STATS_CONCURRENCY, "1"),
            (INNODB_LOCK_WAIT_TIMEOUT, "50"),
            (TIME_ZONE, "UTC"),
        ]);
        values
            .get(name)
            .map(|value| (*value).to_owned())
            .ok_or_else(|| StatsError::GlobalVariable {
                name: name.to_owned(),
                message: "unknown test variable".to_owned(),
            })
    }
}

struct TestRecordSet;
impl aster_sql_statistics_handle_util::RecordSet for TestRecordSet {}

struct TestSqlExecutor;

impl SqlExecutor for TestSqlExecutor {
    fn execute_internal(
        &self,
        _context: &ExecutionContext,
        _sql: &str,
        _arguments: &[SqlValue],
    ) -> std::result::Result<Box<dyn aster_sql_statistics_handle_util::RecordSet>, StatsError> {
        Ok(Box::new(TestRecordSet))
    }
}

struct TestRestrictedSqlExecutor {
    statements: Arc<Mutex<Vec<String>>>,
}

impl RestrictedSqlExecutor for TestRestrictedSqlExecutor {
    fn exec_restricted_sql(
        &self,
        _context: &ExecutionContext,
        _options: &[ExecOption],
        sql: &str,
        _arguments: &[SqlValue],
    ) -> std::result::Result<(Vec<Row>, Vec<ResultField>), StatsError> {
        self.statements.lock().unwrap().push(sql.to_owned());
        Ok((Vec::new(), Vec::new()))
    }
}

struct TestSession {
    variables: Arc<SessionVariables>,
    restricted: Arc<TestRestrictedSqlExecutor>,
}

impl SessionContext for TestSession {
    fn session_variables(&self) -> Arc<SessionVariables> {
        Arc::clone(&self.variables)
    }

    fn transaction(&self, _active: bool) -> std::result::Result<Arc<dyn Transaction>, StatsError> {
        unreachable!("DDL helper transaction wrapping executes SQL directly")
    }

    fn sql_executor(&self) -> Arc<dyn SqlExecutor> {
        Arc::new(TestSqlExecutor)
    }

    fn restricted_sql_executor(&self) -> Arc<dyn RestrictedSqlExecutor> {
        self.restricted.clone()
    }

    fn set_system_variable(
        &self,
        _name: &str,
        _value: &str,
    ) -> std::result::Result<(), StatsError> {
        Ok(())
    }

    fn location(&self) -> String {
        "UTC".to_owned()
    }
}

struct TestSessionPool {
    session: TestSession,
    calls: AtomicUsize,
}

impl SessionPool for TestSessionPool {
    fn with_session(
        &self,
        callback: &mut dyn FnMut(&dyn SessionContext) -> std::result::Result<(), StatsError>,
    ) -> std::result::Result<(), StatsError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        callback(&self.session)
    }
}

struct TestHandle {
    receiver: Mutex<Receiver<SchemaChangeEvent>>,
    pool: TestSessionPool,
    events: AtomicUsize,
    sources: Mutex<Vec<String>>,
    event_error: Mutex<Option<StatsError>>,
}

impl TestHandle {
    fn new(receiver: Receiver<SchemaChangeEvent>) -> (Self, Arc<Mutex<Vec<String>>>) {
        let statements = Arc::new(Mutex::new(Vec::new()));
        let restricted = Arc::new(TestRestrictedSqlExecutor {
            statements: Arc::clone(&statements),
        });
        (
            Self {
                receiver: Mutex::new(receiver),
                pool: TestSessionPool {
                    session: TestSession {
                        variables: Arc::new(SessionVariables::new(Arc::new(TestGlobalVariables))),
                        restricted,
                    },
                    calls: AtomicUsize::new(0),
                },
                events: AtomicUsize::new(0),
                sources: Mutex::new(Vec::new()),
                event_error: Mutex::new(None),
            },
            statements,
        )
    }
}

impl TransactionalDDLHandle for TestHandle {
    fn SPool(&self) -> &dyn SessionPool {
        &self.pool
    }

    fn HandleDDLEvent(
        &self,
        context: &Context,
        _session: &dyn SessionContext,
        _event: &SchemaChangeEvent,
    ) -> std::result::Result<(), StatsError> {
        self.sources
            .lock()
            .unwrap()
            .push(GetInternalSourceType(context));
        self.events.fetch_add(1, Ordering::SeqCst);
        self.event_error.lock().unwrap().clone().map_or(Ok(()), Err)
    }

    fn DDLEventCh(&self) -> &Mutex<Receiver<SchemaChangeEvent>> {
        &self.receiver
    }
}

/// 处理下一条事件时应使用真实事务包装、DDL 来源并提交。
#[test]
fn handle_next_event_uses_real_wrapped_transaction() {
    let (sender, receiver) = mpsc::channel();
    sender.send(NewFlashbackClusterEvent()).unwrap();
    let (handle, statements) = TestHandle::new(receiver);

    HandleNextDDLEventWithTxn(&handle).unwrap();

    assert_eq!(handle.pool.calls.load(Ordering::SeqCst), 1);
    assert_eq!(handle.events.load(Ordering::SeqCst), 1);
    assert_eq!(
        handle.sources.lock().unwrap().as_slice(),
        [InternalDDLNotifier]
    );
    assert_eq!(
        statements.lock().unwrap().as_slice(),
        ["BEGIN PESSIMISTIC", "COMMIT"]
    );
}

/// 事件处理失败时应执行 rollback，并原样返回处理错误。
#[test]
fn handle_event_error_rolls_back_and_propagates() {
    let (_sender, receiver) = mpsc::channel();
    let (handle, statements) = TestHandle::new(receiver);
    *handle.event_error.lock().unwrap() = Some(StatsError::Sql("event failed".to_owned()));
    let event = NewFlashbackClusterEvent();

    let error = HandleDDLEventWithTxn(&handle, &event).unwrap_err();

    assert_eq!(error, StatsError::Sql("event failed".to_owned()));
    assert_eq!(
        statements.lock().unwrap().as_slice(),
        ["BEGIN PESSIMISTIC", "rollback"]
    );
}

/// Go 从已关闭事件通道读取到 nil，后续处理会 panic；Rust 保持可见失败。
#[test]
#[should_panic(expected = "DDL event channel is closed")]
fn handle_next_event_panics_when_channel_is_closed() {
    let (sender, receiver) = mpsc::channel();
    drop(sender);
    let (handle, _statements) = TestHandle::new(receiver);
    let _ = HandleNextDDLEventWithTxn(&handle);
}

/// `FindEvent` 应跳过无关事件直到匹配目标类型。
#[test]
fn find_event_discards_non_matching_events() {
    let (sender, receiver) = mpsc::channel();
    sender.send(NewFlashbackClusterEvent()).unwrap();
    sender
        .send(NewDropSchemaEvent(&DBInfo::default(), Vec::new()))
        .unwrap();
    let event = FindEvent(&receiver, ACTION_DROP_SCHEMA);
    assert_eq!(event.GetType(), ACTION_DROP_SCHEMA);
}

/// 超时前没有目标事件时返回 `None`，且使用总截止时间。
#[test]
fn find_event_timeout_returns_none_at_deadline() {
    let (sender, receiver) = mpsc::channel();
    sender.send(NewFlashbackClusterEvent()).unwrap();
    let started = Instant::now();

    let event = FindEventWithTimeout(&receiver, ACTION_DROP_SCHEMA, 1);

    assert!(event.is_none());
    assert!(started.elapsed().as_secs_f32() >= 0.9);
}

/// `FindEventWithTimeout` 应跳过无关事件并在截止时间前返回目标事件。
#[test]
fn find_event_timeout_returns_matching_event() {
    let (sender, receiver) = mpsc::channel();
    sender.send(NewFlashbackClusterEvent()).unwrap();
    sender
        .send(NewDropSchemaEvent(&DBInfo::default(), Vec::new()))
        .unwrap();

    let event = FindEventWithTimeout(&receiver, ACTION_DROP_SCHEMA, 1).unwrap();

    assert_eq!(event.GetType(), ACTION_DROP_SCHEMA);
}

/// Go `time.NewTicker(0)` 会 panic。
#[test]
#[should_panic(expected = "non-positive interval for NewTicker")]
fn find_event_timeout_rejects_non_positive_interval() {
    let (sender, receiver) = mpsc::channel();
    sender.send(NewFlashbackClusterEvent()).unwrap();
    let _ = FindEventWithTimeout(&receiver, ACTION_DROP_SCHEMA, 0);
}

/// Go 从关闭通道收到 nil 后调用 `GetType` 会 panic。
#[test]
#[should_panic(expected = "DDL event channel is closed")]
fn find_event_panics_when_channel_is_closed() {
    let (sender, receiver) = mpsc::channel();
    drop(sender);
    let _ = FindEvent(&receiver, ACTION_DROP_SCHEMA);
}

/// Go 的超时版同样会在关闭通道返回 nil 后调用 `GetType` 而 panic。
#[test]
#[should_panic(expected = "DDL event channel is closed")]
fn find_event_with_timeout_panics_when_channel_is_closed() {
    let (sender, receiver) = mpsc::channel();
    drop(sender);
    let _ = FindEventWithTimeout(&receiver, ACTION_DROP_SCHEMA, 1);
}
