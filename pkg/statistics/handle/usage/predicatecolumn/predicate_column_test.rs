// Copyright 2026 AsterSQL.

// 谓词列 SQL 路径单元测试。
//
// 用 Mock 执行器/InfoSchema/会话校验：加载跳过空 ID、按表绑定、
// 清理已删列、缺失表跳过清理、Go "NULL" 时间字符串，以及写失败不上报成功。

use super::*;
use chrono::{NaiveDate, NaiveDateTime};
use meta_model::TableItemID;
use stats_types::ColStatsTimeInfo;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

/// 记录 Mock 执行器收到的调用，便于断言 SQL 与参数。
#[derive(Clone, Debug, PartialEq, Eq)]
enum Call {
    QueryUsage(String, Vec<SqlArg>),
    QueryPredicate(String, Vec<SqlArg>),
    Execute(String, Vec<SqlArg>),
}

/// 可预置查询/执行结果队列的 Mock 执行器。
#[derive(Default)]
struct MockExecutor {
    usage_results: Mutex<VecDeque<Result<Vec<ColumnStatsUsageRecord>, PredicateColumnError>>>,
    predicate_results: Mutex<VecDeque<Result<Vec<PredicateColumnRecord>, PredicateColumnError>>>,
    execute_results: Mutex<VecDeque<Result<(), PredicateColumnError>>>,
    calls: Mutex<Vec<Call>>,
}

impl MockExecutor {
    /// 压入一次成功的 usage 查询结果。
    fn push_usage(&self, value: Vec<ColumnStatsUsageRecord>) {
        self.usage_results.lock().unwrap().push_back(Ok(value));
    }

    /// 压入一次成功的谓词列查询结果。
    fn push_predicates(&self, value: Vec<PredicateColumnRecord>) {
        self.predicate_results.lock().unwrap().push_back(Ok(value));
    }

    /// 返回已记录的全部调用副本。
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}

impl PredicateColumnExecutor for MockExecutor {
    fn query_usage(
        &self,
        sql: &str,
        args: &[SqlArg],
    ) -> Result<Vec<ColumnStatsUsageRecord>, PredicateColumnError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::QueryUsage(sql.to_owned(), args.to_vec()));
        self.usage_results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(Vec::new()))
    }

    fn query_predicate_columns(
        &self,
        sql: &str,
        args: &[SqlArg],
    ) -> Result<Vec<PredicateColumnRecord>, PredicateColumnError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::QueryPredicate(sql.to_owned(), args.to_vec()));
        self.predicate_results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(Vec::new()))
    }

    fn execute(&self, sql: &str, args: &[SqlArg]) -> Result<(), PredicateColumnError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::Execute(sql.to_owned(), args.to_vec()));
        self.execute_results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(()))
    }
}

/// Mock InfoSchema：表 ID → 当前列 ID 列表。
#[derive(Default)]
struct MockInfoSchema {
    columns: HashMap<i64, Vec<i64>>,
}

impl PredicateColumnInfoSchema for MockInfoSchema {
    fn table_column_ids(&self, table_id: i64) -> Option<Vec<i64>> {
        self.columns.get(&table_id).cloned()
    }
}

/// 组合 Mock 执行器与 InfoSchema 的测试会话。
#[derive(Default)]
struct MockSession {
    executor: MockExecutor,
    info_schema: MockInfoSchema,
}

impl PredicateColumnSession for MockSession {
    fn executor(&self) -> &dyn PredicateColumnExecutor {
        &self.executor
    }

    fn latest_info_schema(&self) -> &dyn PredicateColumnInfoSchema {
        &self.info_schema
    }
}

struct NoopGlobalVariables;

impl stats_types::GlobalVariableAccessor for NoopGlobalVariables {
    fn get_global_sys_var(&self, name: &str) -> Result<String, stats_types::StatsExecError> {
        Err(stats_types::StatsExecError::GlobalVariable {
            name: name.to_owned(),
            message: "unused".to_owned(),
        })
    }
}

struct NoopTransaction;

impl stats_types::StatsTransaction for NoopTransaction {
    fn start_ts(&self) -> u64 {
        0
    }
}

struct NoopSqlExecutor;

impl stats_types::StatsSqlExecutor for NoopSqlExecutor {
    fn execute_internal(
        &self,
        _context: &stats_types::StatsExecutionContext,
        _sql: &str,
        _arguments: &[stats_types::StatsSqlValue],
    ) -> Result<Box<dyn stats_types::StatsRecordSet>, stats_types::StatsExecError> {
        Err(stats_types::StatsExecError::Sql("unused".to_owned()))
    }
}

#[derive(Default)]
struct CanonicalRestrictedExecutor {
    results: Mutex<
        VecDeque<
            Result<
                (
                    Vec<stats_types::StatsRow>,
                    Vec<stats_types::StatsResultField>,
                ),
                stats_types::StatsExecError,
            >,
        >,
    >,
    calls: Mutex<Vec<(String, Vec<stats_types::StatsSqlValue>)>>,
}

impl stats_types::StatsRestrictedSqlExecutor for CanonicalRestrictedExecutor {
    fn exec_restricted_sql(
        &self,
        _context: &stats_types::StatsExecutionContext,
        _options: &[stats_types::ExecOption],
        sql: &str,
        arguments: &[stats_types::StatsSqlValue],
    ) -> Result<
        (
            Vec<stats_types::StatsRow>,
            Vec<stats_types::StatsResultField>,
        ),
        stats_types::StatsExecError,
    > {
        self.calls
            .lock()
            .unwrap()
            .push((sql.to_owned(), arguments.to_vec()));
        self.results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok((Vec::new(), Vec::new())))
    }
}

struct CanonicalSession {
    variables: Arc<stats_types::StatsSessionVariables>,
    restricted: Arc<CanonicalRestrictedExecutor>,
}

impl CanonicalSession {
    fn new(restricted: Arc<CanonicalRestrictedExecutor>) -> Self {
        Self {
            variables: Arc::new(stats_types::StatsSessionVariables::new(Arc::new(
                NoopGlobalVariables,
            ))),
            restricted,
        }
    }
}

impl stats_types::SessionContext for CanonicalSession {
    fn session_variables(&self) -> Arc<stats_types::StatsSessionVariables> {
        Arc::clone(&self.variables)
    }

    fn transaction(
        &self,
        _active: bool,
    ) -> Result<Arc<dyn stats_types::StatsTransaction>, stats_types::StatsExecError> {
        Ok(Arc::new(NoopTransaction))
    }

    fn sql_executor(&self) -> Arc<dyn stats_types::StatsSqlExecutor> {
        Arc::new(NoopSqlExecutor)
    }

    fn restricted_sql_executor(&self) -> Arc<dyn stats_types::StatsRestrictedSqlExecutor> {
        self.restricted.clone()
    }

    fn set_system_variable(
        &self,
        _name: &str,
        _value: &str,
    ) -> Result<(), stats_types::StatsExecError> {
        Ok(())
    }

    fn location(&self) -> String {
        "UTC".to_owned()
    }
}

/// 构造固定日期、指定小时的 UTC 朴素时间。
fn timestamp(hour: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 7, 19)
        .unwrap()
        .and_hms_opt(hour, 30, 15)
        .unwrap()
}

/// 按 Go `types.NewTime(types.FromGoTime(...))` 构造 TIMESTAMP。
fn mysql_time(value: NaiveDateTime, location: chrono_tz::Tz) -> stats_types::Time {
    let utc = chrono::TimeZone::from_utc_datetime(&chrono::Utc, &value);
    stats_types::NewTime(
        stats_types::FromGoTime(utc.with_timezone(&location)),
        stats_types::mysql::TypeTimestamp,
        stats_types::DefaultFsp,
    )
}

#[test]
fn canonical_exec_rows_adapter_decodes_rows_and_preserves_bound_values() {
    let restricted = Arc::new(CanonicalRestrictedExecutor::default());
    restricted.results.lock().unwrap().push_back(Ok((
        vec![stats_types::StatsRow {
            values: vec![
                stats_types::StatsSqlValue::Integer(10),
                stats_types::StatsSqlValue::Integer(2),
                stats_types::StatsSqlValue::String("2026-07-19 01:30:15".to_owned()),
                stats_types::StatsSqlValue::Null,
            ],
        }],
        Vec::new(),
    )));
    restricted
        .results
        .lock()
        .unwrap()
        .push_back(Ok((Vec::new(), Vec::new())));
    let context = CanonicalSession::new(Arc::clone(&restricted));
    let executor = ExecRowsPredicateColumnExecutor::new(&context);

    assert_eq!(
        executor
            .query_usage(LOAD_TABLE_SQL, &[SqlArg::I64(10)])
            .unwrap(),
        vec![ColumnStatsUsageRecord {
            table_id: Some(10),
            column_id: Some(2),
            last_used_at_utc: Some(timestamp(1)),
            last_analyzed_at_utc: None,
        }]
    );
    executor
        .execute(
            CLEANUP_DROPPED_COLUMNS_SQL,
            &[
                SqlArg::I64(10),
                SqlArg::StringList(vec!["2".to_owned(), "4".to_owned()]),
            ],
        )
        .unwrap();

    assert_eq!(
        restricted.calls.lock().unwrap().as_slice(),
        [
            (
                LOAD_TABLE_SQL.to_owned(),
                vec![stats_types::StatsSqlValue::Integer(10)]
            ),
            (
                CLEANUP_DROPPED_COLUMNS_SQL.to_owned(),
                vec![
                    stats_types::StatsSqlValue::Integer(10),
                    stats_types::StatsSqlValue::StringList(vec!["2".to_owned(), "4".to_owned()]),
                ]
            ),
        ]
    );
}

/// Go skips rows with a NULL table/column ID before decoding either timestamp.
#[test]
fn canonical_adapter_does_not_decode_times_for_rows_with_null_ids() {
    let restricted = Arc::new(CanonicalRestrictedExecutor::default());
    restricted.results.lock().unwrap().push_back(Ok((
        vec![stats_types::StatsRow {
            values: vec![
                stats_types::StatsSqlValue::Null,
                stats_types::StatsSqlValue::Integer(2),
                stats_types::StatsSqlValue::String("not a timestamp".to_owned()),
                stats_types::StatsSqlValue::String("also invalid".to_owned()),
            ],
        }],
        Vec::new(),
    )));
    let context = CanonicalSession::new(restricted);
    let executor = ExecRowsPredicateColumnExecutor::new(&context);

    assert_eq!(
        executor.query_usage(LOAD_ALL_SQL, &[]).unwrap(),
        vec![ColumnStatsUsageRecord {
            table_id: None,
            column_id: Some(2),
            last_used_at_utc: None,
            last_analyzed_at_utc: None,
        }]
    );
}

/// Go only checks whether the converted timestamp is NULL when listing predicates.
#[test]
fn canonical_predicate_adapter_only_checks_timestamp_nullness() {
    let restricted = Arc::new(CanonicalRestrictedExecutor::default());
    restricted.results.lock().unwrap().push_back(Ok((
        vec![stats_types::StatsRow {
            values: vec![
                stats_types::StatsSqlValue::Integer(8),
                stats_types::StatsSqlValue::String("non-null marker".to_owned()),
            ],
        }],
        Vec::new(),
    )));
    let context = CanonicalSession::new(restricted);
    let executor = ExecRowsPredicateColumnExecutor::new(&context);

    assert_eq!(
        executor
            .query_predicate_columns(PREDICATE_COLUMNS_SQL, &[SqlArg::I64(9)])
            .unwrap(),
        vec![PredicateColumnRecord {
            column_id: 8,
            converted_last_used_at_is_null: false,
        }]
    );
}

/// 空 ID 行被跳过；有效行经时区转换后写入结果。
#[test]
fn load_skips_null_ids_and_converts_utc_through_location() {
    let session = MockSession::default();
    session.executor.push_usage(vec![
        ColumnStatsUsageRecord {
            table_id: None,
            column_id: Some(2),
            last_used_at_utc: Some(timestamp(1)),
            last_analyzed_at_utc: None,
        },
        ColumnStatsUsageRecord {
            table_id: Some(10),
            column_id: Some(2),
            last_used_at_utc: Some(timestamp(1)),
            last_analyzed_at_utc: Some(timestamp(3)),
        },
    ]);

    let loaded = LoadColumnStatsUsage(&session, chrono_tz::Asia::Shanghai).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(
        loaded[&TableItemID {
            TableID: 10,
            ID: 2,
            IsIndex: false,
            IsSyncLoadFailed: false
        }],
        ColStatsTimeInfo {
            LastUsedAt: Some(mysql_time(timestamp(1), chrono_tz::Asia::Shanghai)),
            LastAnalyzedAt: Some(mysql_time(timestamp(3), chrono_tz::Asia::Shanghai)),
        }
    );
    assert_eq!(
        session.executor.calls(),
        vec![Call::QueryUsage(LOAD_ALL_SQL.to_owned(), vec![])]
    );
}

/// 按表加载应绑定 table_id 参数。
#[test]
fn load_for_table_binds_the_table_id() {
    let session = MockSession::default();
    session.executor.push_usage(Vec::new());
    let loaded = LoadColumnStatsUsageForTable(&session, chrono_tz::UTC, 42).unwrap();
    assert!(loaded.is_empty());
    assert_eq!(
        session.executor.calls(),
        vec![Call::QueryUsage(
            LOAD_TABLE_SQL.to_owned(),
            vec![SqlArg::I64(42)],
        )]
    );
}

/// 先按 schema 清理已删列，再过滤 last_used_at 为空的行。
#[test]
fn predicate_query_cleans_dropped_columns_and_skips_null_time() {
    let mut session = MockSession::default();
    session.info_schema.columns.insert(7, vec![3, 5]);
    session.executor.push_predicates(vec![
        PredicateColumnRecord {
            column_id: 3,
            converted_last_used_at_is_null: false,
        },
        PredicateColumnRecord {
            column_id: 99,
            converted_last_used_at_is_null: true,
        },
    ]);

    assert_eq!(GetPredicateColumns(&session, 7).unwrap(), vec![3]);
    assert_eq!(
        session.executor.calls(),
        vec![
            Call::Execute(
                CLEANUP_DROPPED_COLUMNS_SQL.to_owned(),
                vec![
                    SqlArg::I64(7),
                    SqlArg::StringList(vec!["3".to_owned(), "5".to_owned()]),
                ],
            ),
            Call::QueryPredicate(PREDICATE_COLUMNS_SQL.to_owned(), vec![SqlArg::I64(7)],),
        ]
    );
}

/// 表在 InfoSchema 中不存在时跳过 DELETE，仍执行谓词列查询。
#[test]
fn missing_table_skips_cleanup_but_still_queries_usage() {
    let session = MockSession::default();
    session.executor.push_predicates(Vec::new());
    assert!(GetPredicateColumns(&session, 88).unwrap().is_empty());
    assert_eq!(
        session.executor.calls(),
        vec![Call::QueryPredicate(
            PREDICATE_COLUMNS_SQL.to_owned(),
            vec![SqlArg::I64(88)],
        )]
    );
}

/// Go 将缺失时间绑定为字符串 "NULL"，非 SQL NULL。
#[test]
fn replace_uses_go_null_string_for_missing_times() {
    let session = MockSession::default();
    let mut values = HashMap::new();
    values.insert(
        TableItemID {
            TableID: 12,
            ID: 4,
            IsIndex: false,
            IsSyncLoadFailed: false,
        },
        ColStatsTimeInfo {
            LastUsedAt: None,
            LastAnalyzedAt: Some(mysql_time(timestamp(3), chrono_tz::UTC)),
        },
    );

    SaveColumnStatsUsageForTable(&session, &values).unwrap();
    assert_eq!(
        session.executor.calls(),
        vec![Call::Execute(
            REPLACE_USAGE_SQL.to_owned(),
            vec![
                SqlArg::I64(12),
                SqlArg::I64(4),
                SqlArg::String("NULL".to_owned()),
                SqlArg::String("2026-07-19 03:30:15".to_owned()),
            ],
        )]
    );
}

/// 执行器返回错误时 Save 必须向上传播，不得伪装成功。
#[test]
fn executor_errors_are_not_reported_as_success() {
    let session = MockSession::default();
    session
        .executor
        .execute_results
        .lock()
        .unwrap()
        .push_back(Err(PredicateColumnError::new("write failed")));
    let mut values = HashMap::new();
    values.insert(TableItemID::default(), ColStatsTimeInfo::default());

    let error = SaveColumnStatsUsageForTable(&session, &values).unwrap_err();
    assert_eq!(error.to_string(), "write failed");
    assert_eq!(session.executor.calls().len(), 1);
}
