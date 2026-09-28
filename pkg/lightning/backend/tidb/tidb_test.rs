// Copyright 2019 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// TiDB 逻辑导入后端（tidbBackend / tidbEncoder）单元测试。
//
// 使用 MockExecutor 验证重复键策略、批量/预编译写入、错误预算降级、
// 远程表模型拉取与 SQL 字面量转义。

use std::any::Any;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use backend::{Backend, LocalWriterConfig};
use encode::{Column, Context, Datum, EncodingConfig, SessionOptions, Table};
use uuid::Uuid;

use crate::*;

#[derive(Default)]
/// 记录 execute/query 调用并按队列返回预设结果的 mock。
struct MockExecutor {
    executions: Mutex<Vec<(String, Vec<SqlValue>)>>,
    queries: Mutex<Vec<(String, Vec<SqlValue>)>>,
    executeResults: Mutex<VecDeque<Result<u64, SqlError>>>,
    queryResults: Mutex<VecDeque<Result<Vec<Vec<SqlValue>>, SqlError>>>,
}

impl SqlExecutor for MockExecutor {
    fn execute(&self, query: &str, values: &[SqlValue]) -> Result<u64, SqlError> {
        self.executions
            .lock()
            .unwrap()
            .push((query.into(), values.to_vec()));
        self.executeResults
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(1))
    }
    fn query(&self, query: &str, values: &[SqlValue]) -> Result<Vec<Vec<SqlValue>>, SqlError> {
        self.queries
            .lock()
            .unwrap()
            .push((query.into(), values.to_vec()));
        self.queryResults
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(Vec::new()))
    }
}

/// 测试套件：持有共享 MockExecutor。
struct mysqlSuite {
    executor: Arc<MockExecutor>,
}

/// 创建默认 mysqlSuite。
fn createMysqlSuite() -> mysqlSuite {
    mysqlSuite {
        executor: Arc::new(MockExecutor::default()),
    }
}

impl mysqlSuite {
    /// 用例收尾占位。
    fn TearDownTest(&self) {}
    /// 按重复键策略构造 tidbBackend。
    fn backend(&self, onDuplicate: DuplicateResolution) -> Arc<tidbBackend> {
        let executor: Arc<dyn SqlExecutor> = self.executor.clone();
        NewTiDBBackend(
            executor,
            TiDBBackendConfig {
                onDuplicate,
                ..Default::default()
            },
        )
    }
}

#[derive(Clone)]
/// 测试用最小 Table 实现。
struct TestTable {
    name: String,
    columns: Vec<Column>,
}
impl Table for TestTable {
    fn name(&self) -> &str {
        &self.name
    }
    fn columns(&self) -> &[Column] {
        &self.columns
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 用 tidbEncoder 将 (id, name) 行编码为 tidbRows。
fn encodeRowsTiDB(prepared: bool, values: &[(i64, &str)]) -> tidbRows {
    let table: Arc<dyn Table> = Arc::new(TestTable {
        name: "t".into(),
        columns: vec![
            Column {
                name: "id".into(),
                ..Default::default()
            },
            Column {
                name: "name".into(),
                ..Default::default()
            },
        ],
    });
    let config = EncodingConfig {
        SessionOptions: SessionOptions {
            LogicalImportPrepStmt: prepared,
            ..Default::default()
        },
        Path: "input.csv".into(),
        Table: Some(table),
        ..Default::default()
    };
    let builder = NewEncodingBuilder();
    let mut encoder = builder.NewEncoder(&Context::default(), &config).unwrap();
    // 逐行 Encode 并收集 tidbRow。
    tidbRows(
        values
            .iter()
            .enumerate()
            .map(|(offset, (id, name))| {
                encoder
                    .Encode(
                        &[Datum::Int(*id), Datum::String((*name).into())],
                        *id,
                        &[0, 1],
                        offset as i64,
                    )
                    .unwrap()
                    .as_any()
                    .downcast_ref::<tidbRow>()
                    .unwrap()
                    .clone()
            })
            .collect(),
    )
}

/// REPLACE 策略：语句以 REPLACE INTO 开头。
#[test]
fn TestWriteRowsReplaceOnDup() {
    let suite = createMysqlSuite();
    suite
        .backend(DuplicateResolution::Replace)
        .WriteRows(
            "`db`.`t`",
            &["id".into()],
            &encodeRowsTiDB(false, &[(1, "a")]),
        )
        .unwrap();
    assert!(
        suite.executor.executions.lock().unwrap()[0]
            .0
            .starts_with("REPLACE INTO")
    );
    suite.TearDownTest();
}

/// 覆盖 Go appendSQL 的全部专用 Datum 分支。
#[test]
fn TestWriteRowsReplaceOnDupEncodesAllGoDatumKinds() {
    let table: Arc<dyn Table> = Arc::new(TestTable {
        name: "t".into(),
        columns: (0..13)
            .map(|index| Column {
                name: format!("c{index}"),
                ..Default::default()
            })
            .collect(),
    });
    let config = EncodingConfig {
        Table: Some(table),
        ..Default::default()
    };
    let mut encoder = NewEncodingBuilder()
        .NewEncoder(&Context::default(), &config)
        .unwrap();
    let row = encoder
        .Encode(
            &[
                Datum::MinNotNull,
                Datum::MaxValue,
                Datum::Int(i64::MIN),
                Datum::UInt(u64::MAX),
                Datum::Float(5e-324),
                Datum::Json(r#"{"a":1}"#.into()),
                Datum::BinaryLiteral(vec![0, 0, 0, 0xab, 0xcd, 0xef]),
                Datum::Bit(vec![0x98, 0x76, 0x54, 0x32]),
                Datum::Enum {
                    name: "ENUM_NAME".into(),
                    value: 51,
                },
                Datum::Set {
                    name: "SET_NAME".into(),
                    value: 7,
                },
                Datum::Decimal("12.5".into()),
                Datum::Timestamp("2026-08-02 12:34:56".into()),
                Datum::Duration("12:34:56".into()),
            ],
            0,
            &(0..13).collect::<Vec<_>>(),
            0,
        )
        .unwrap();
    assert_eq!(
        row.as_any().downcast_ref::<tidbRow>().unwrap().String(),
        "(MINVALUE,MAXVALUE,-9223372036854775808,18446744073709551615,5e-324,'{\"a\":1}',x'000000abcdef',2557891634,51,7,'12.5','2026-08-02 12:34:56','12:34:56')"
    );
}

/// IGNORE 策略：语句以 INSERT IGNORE INTO 开头。
#[test]
fn TestWriteRowsIgnoreOnDup() {
    let suite = createMysqlSuite();
    suite
        .backend(DuplicateResolution::Ignore)
        .WriteRows("t", &[], &encodeRowsTiDB(false, &[(1, "a")]))
        .unwrap();
    assert!(
        suite.executor.executions.lock().unwrap()[0]
            .0
            .starts_with("INSERT IGNORE INTO")
    );
}

/// Ignore + max-record-rows 必须先以 INSERT 执行，才能定位并记录冲突行。
#[test]
fn TestWriteRowsIgnoreWithRecordingUsesErrorInsert() {
    let suite = createMysqlSuite();
    let executor: Arc<dyn SqlExecutor> = suite.executor.clone();
    let backend = NewTiDBBackend(
        executor,
        TiDBBackendConfig {
            onDuplicate: DuplicateResolution::Ignore,
            maxRecordRows: 10,
            conflictThreshold: 10,
            ..Default::default()
        },
    );
    backend
        .WriteRows("t", &[], &encodeRowsTiDB(false, &[(1, "a")]))
        .unwrap();
    assert!(
        suite.executor.executions.lock().unwrap()[0]
            .0
            .starts_with("INSERT INTO")
    );
}

/// Error 策略遇 duplicate 错误应失败。
#[test]
fn TestWriteRowsErrorOnDup() {
    let suite = createMysqlSuite();
    suite
        .executor
        .executeResults
        .lock()
        .unwrap()
        .push_back(Err(SqlError::duplicate("Duplicate entry")));
    assert!(
        suite
            .backend(DuplicateResolution::Error)
            .WriteRows("t", &[], &encodeRowsTiDB(false, &[(1, "a")]))
            .is_err()
    );
}

/// 按 SQL mode 校验字符串转义结果。
fn testStrictMode(sqlMode: u64, expected: &str) {
    let table: Arc<dyn Table> = Arc::new(TestTable {
        name: "t".into(),
        columns: vec![Column {
            name: "v".into(),
            ..Default::default()
        }],
    });
    let config = EncodingConfig {
        SessionOptions: SessionOptions {
            SQLMode: sqlMode,
            ..Default::default()
        },
        Table: Some(table),
        ..Default::default()
    };
    let mut encoder = NewEncodingBuilder()
        .NewEncoder(&Context::default(), &config)
        .unwrap();
    let row = encoder
        .Encode(&[Datum::String("a\\b'c".into())], 1, &[0], 0)
        .unwrap();
    assert_eq!(
        row.as_any().downcast_ref::<tidbRow>().unwrap().String(),
        expected
    );
}

/// 拉取远程表模型（列信息查询结果映射）。
#[test]
fn TestFetchRemoteTableModels_4_0() {
    let suite = createMysqlSuite();
    suite.executor.queryResults.lock().unwrap().extend([
        Ok(vec![vec![
            SqlValue::String("T1".into()),
            SqlValue::String("id".into()),
            SqlValue::String("bigint".into()),
            SqlValue::String(String::new()),
            SqlValue::String("auto_increment".into()),
        ]]),
        Ok(vec![vec![
            SqlValue::String("db".into()),
            SqlValue::String("T1".into()),
            SqlValue::String("id".into()),
            SqlValue::UInt(1),
        ]]),
    ]);
    let executor: Arc<dyn SqlExecutor> = suite.executor.clone();
    let getter = NewTargetInfoGetter(executor);
    let models = getter
        .FetchRemoteTableModels(&Context::default(), "db", &["T1".into()])
        .unwrap();
    assert_eq!(models["t1"].name, "T1");
}

/// 远程模型必须保留列顺序、flag、生成表达式及 AUTO_RANDOM 表属性。
#[test]
fn TestFetchRemoteTableModelsPreservesGoMetadata() {
    let suite = createMysqlSuite();
    suite.executor.queryResults.lock().unwrap().extend([
        Ok(vec![
            vec![
                SqlValue::String("T1".into()),
                SqlValue::String("id".into()),
                SqlValue::String("bigint(20) unsigned".into()),
                SqlValue::String("1 + 2".into()),
                SqlValue::String(String::new()),
            ],
            vec![
                SqlValue::String("T1".into()),
                SqlValue::String("value".into()),
                SqlValue::String("varchar(255)".into()),
                SqlValue::String(String::new()),
                SqlValue::String(String::new()),
            ],
        ]),
        Ok(vec![vec![
            SqlValue::String("db".into()),
            SqlValue::String("T1".into()),
            SqlValue::String("id".into()),
            SqlValue::UInt(42),
            SqlValue::String("AUTO_RANDOM".into()),
        ]]),
    ]);
    let executor: Arc<dyn SqlExecutor> = suite.executor.clone();
    let models = NewTargetInfoGetter(executor)
        .FetchRemoteTableModels(&Context::default(), "db", &["T1".into()])
        .unwrap();
    let model = &models["t1"];
    assert!(model.public);
    assert!(model.pk_is_handle);
    assert_eq!(model.auto_random_bits, 1);
    assert_eq!(model.columns.len(), 2);
    assert_eq!(model.columns[0].offset, 0);
    assert!(model.columns[0].unsigned);
    assert!(model.columns[0].primary_key);
    assert_eq!(model.columns[0].generated_expression, "1 + 2");
}

/// NEXT_ROW_ID 四列结果解析为 AUTO_INCREMENT。
#[test]
fn TestFetchRemoteTableModels_4_x_auto_increment() {
    let suite = createMysqlSuite();
    suite
        .executor
        .queryResults
        .lock()
        .unwrap()
        .push_back(Ok(vec![vec![
            SqlValue::String("db".into()),
            SqlValue::String("t".into()),
            SqlValue::String("id".into()),
            SqlValue::UInt(42),
        ]]));
    let result = FetchTableAutoIDInfos(suite.executor.as_ref(), "`db`.`t`").unwrap();
    assert_eq!(result[0].IDType, "AUTO_INCREMENT");
}

/// NEXT_ROW_ID 五列结果解析为 AUTO_RANDOM。
#[test]
fn TestFetchRemoteTableModels_4_x_auto_random() {
    let suite = createMysqlSuite();
    suite
        .executor
        .queryResults
        .lock()
        .unwrap()
        .push_back(Ok(vec![vec![
            SqlValue::String("db".into()),
            SqlValue::String("t".into()),
            SqlValue::String("id".into()),
            SqlValue::UInt(42),
            SqlValue::String("AUTO_RANDOM".into()),
        ]]));
    assert_eq!(
        FetchTableAutoIDInfos(suite.executor.as_ref(), "t").unwrap()[0].IDType,
        "AUTO_RANDOM"
    );
}

/// 查询中途失败应向上返回错误。
#[test]
fn TestFetchRemoteTableModelsDropTableHalfway() {
    let suite = createMysqlSuite();
    suite
        .executor
        .queryResults
        .lock()
        .unwrap()
        .push_back(Err(SqlError::new("table dropped")));
    let executor: Arc<dyn SqlExecutor> = suite.executor.clone();
    let getter = NewTargetInfoGetter(executor);
    assert!(
        getter
            .FetchRemoteTableModels(&Context::default(), "db", &["t".into()])
            .is_err()
    );
}

/// 超过批大小的表名列表应分批查询且可成功。
#[test]
fn TestFetchRemoteTableModelsConcurrency() {
    let suite = createMysqlSuite();
    suite.executor.queryResults.lock().unwrap().extend([
        Ok(Vec::new()),
        Ok(Vec::new()),
        Ok(Vec::new()),
    ]);
    let names = (0..65).map(|id| format!("t{id}")).collect::<Vec<_>>();
    let executor: Arc<dyn SqlExecutor> = suite.executor.clone();
    let getter = NewTargetInfoGetter(executor);
    assert!(
        getter
            .FetchRemoteTableModels(&Context::default(), "db", &names)
            .unwrap()
            .is_empty()
    );
}

/// Go rows.Scan 在 SHOW DATABASES 返回畸形行时会报错，不可静默丢弃。
#[test]
fn TestFetchRemoteDBModelsRejectsMalformedRows() {
    let suite = createMysqlSuite();
    suite
        .executor
        .queryResults
        .lock()
        .unwrap()
        .push_back(Ok(vec![vec![SqlValue::Int(42)]]));
    let executor: Arc<dyn SqlExecutor> = suite.executor.clone();
    let getter = NewTargetInfoGetter(executor);
    assert!(getter.FetchRemoteDBModels(&Context::default()).is_err());
}

/// Go rows.Scan 要求 information_schema.columns 的五列均可解码。
#[test]
fn TestFetchRemoteTableModelsRejectsMalformedRows() {
    let suite = createMysqlSuite();
    suite
        .executor
        .queryResults
        .lock()
        .unwrap()
        .push_back(Ok(vec![vec![
            SqlValue::String("t".into()),
            SqlValue::String("id".into()),
            SqlValue::String("bigint".into()),
            SqlValue::String(String::new()),
            SqlValue::Int(0),
        ]]));
    let executor: Arc<dyn SqlExecutor> = suite.executor.clone();
    let getter = NewTargetInfoGetter(executor);
    assert!(
        getter
            .FetchRemoteTableModels(&Context::default(), "db", &["t".into()])
            .is_err()
    );
}

/// 不可重试错误只执行一次。
#[test]
fn TestWriteRowsErrorNoRetry() {
    let suite = createMysqlSuite();
    suite
        .executor
        .executeResults
        .lock()
        .unwrap()
        .push_back(Err(SqlError::new("syntax")));
    assert!(
        suite
            .backend(DuplicateResolution::Error)
            .WriteRows("t", &[], &encodeRowsTiDB(false, &[(1, "a")]))
            .is_err()
    );
    assert_eq!(suite.executor.executions.lock().unwrap().len(), 1);
}

/// 批失败后在错误预算内降级为逐行写并成功。
#[test]
fn TestWriteRowsErrorDowngradingAll() {
    let suite = createMysqlSuite();
    suite.executor.executeResults.lock().unwrap().extend([
        Err(SqlError::new("batch")),
        Ok(1),
        Ok(1),
    ]);
    let executor: Arc<dyn SqlExecutor> = suite.executor.clone();
    let backend = NewTiDBBackend(
        executor,
        TiDBBackendConfig {
            errorBudget: 2,
            ..Default::default()
        },
    );
    backend
        .WriteRows("t", &[], &encodeRowsTiDB(false, &[(1, "a"), (2, "b")]))
        .unwrap();
    assert_eq!(suite.executor.executions.lock().unwrap().len(), 3);
}

/// 错误预算为 0 时批失败直接报错。
#[test]
fn TestWriteRowsErrorDowngradingExceedThreshold() {
    let suite = createMysqlSuite();
    suite
        .executor
        .executeResults
        .lock()
        .unwrap()
        .extend([Err(SqlError::new("batch")), Err(SqlError::new("row"))]);
    let executor: Arc<dyn SqlExecutor> = suite.executor.clone();
    let backend = NewTiDBBackend(
        executor,
        TiDBBackendConfig {
            errorBudget: 0,
            ..Default::default()
        },
    );
    assert!(
        backend
            .WriteRows("t", &[], &encodeRowsTiDB(false, &[(1, "a")]))
            .is_err()
    );
}

/// 降级逐行时记录坏行错误且其余行成功。
#[test]
fn TestWriteRowsRecordOneError() {
    let suite = createMysqlSuite();
    suite.executor.executeResults.lock().unwrap().extend([
        Err(SqlError::new("batch")),
        Err(SqlError::new("bad row")),
        Ok(1),
    ]);
    let executor: Arc<dyn SqlExecutor> = suite.executor.clone();
    let backend = NewTiDBBackend(
        executor,
        TiDBBackendConfig {
            errorBudget: 2,
            ..Default::default()
        },
    );
    backend
        .WriteRows("t", &[], &encodeRowsTiDB(false, &[(1, "a"), (2, "b")]))
        .unwrap();
    assert_eq!(backend.Errors().len(), 1);
}

/// Error 策略下 duplicate 错误立即失败。
#[test]
fn TestDuplicateThreshold() {
    let suite = createMysqlSuite();
    suite
        .executor
        .executeResults
        .lock()
        .unwrap()
        .push_back(Err(SqlError::duplicate("dup")));
    assert!(
        suite
            .backend(DuplicateResolution::Error)
            .WriteRows("t", &[], &encodeRowsTiDB(false, &[(1, "a")]))
            .is_err()
    );
}

/// INSERT IGNORE 以 RowsAffected 差额消费 conflict.threshold。
#[test]
fn TestDuplicateThresholdCountsAffectedRowDifference() {
    let suite = createMysqlSuite();
    suite
        .executor
        .executeResults
        .lock()
        .unwrap()
        .extend([Ok(1), Ok(0)]);
    let executor: Arc<dyn SqlExecutor> = suite.executor.clone();
    let backend = NewTiDBBackend(
        executor,
        TiDBBackendConfig {
            onDuplicate: DuplicateResolution::Ignore,
            conflictThreshold: 2,
            ..Default::default()
        },
    );
    let rows = encodeRowsTiDB(false, &[(1, "a"), (2, "b")]);
    backend.WriteRows("t", &[], &rows).unwrap();
    let error = backend.WriteRows("t", &[], &rows).unwrap_err();
    assert_eq!(
        error.message,
        "The number of conflict errors exceeds the threshold configured by `conflict.threshold`: '2'"
    );
}

/// 非重复键错误必须消费独立的 max-error.type，而非冲突预算。
#[test]
fn TestTypeErrorThresholdAndRecordClassification() {
    let suite = createMysqlSuite();
    suite.executor.executeResults.lock().unwrap().extend([
        Err(SqlError::new("batch")),
        Err(SqlError::new("bad type")),
        Ok(1),
        Err(SqlError::new("batch")),
        Err(SqlError::new("bad type again")),
    ]);
    let executor: Arc<dyn SqlExecutor> = suite.executor.clone();
    let backend = NewTiDBBackend(
        executor,
        TiDBBackendConfig {
            typeErrorThreshold: 1,
            errorSchema: "tidb_lightning_errors".into(),
            ..Default::default()
        },
    );
    backend
        .WriteRows("t", &[], &encodeRowsTiDB(false, &[(1, "a")]))
        .unwrap();
    let error = backend
        .WriteRows("t", &[], &encodeRowsTiDB(false, &[(2, "b")]))
        .unwrap_err();
    assert!(error.message.contains("max-error.type"));
    assert_eq!(backend.ErrorRecords()[0].kind, ErrorRecordKind::Type);
}

/// EncodeRowForRecord 与 NO_BACKSLASH_ESCAPES 转义。
#[test]
fn TestEncodeRowForRecord() {
    let table: Arc<dyn Table> = Arc::new(TestTable {
        name: "t".into(),
        columns: vec![Column {
            name: "v".into(),
            ..Default::default()
        }],
    });
    assert_eq!(
        EncodeRowForRecord(Some(table), 0, &[Datum::String("a'b".into())], &[0]),
        b"('a''b')"
    );
    testStrictMode(SQL_MODE_NO_BACKSLASH_ESCAPES, "('a\\b''c')");

    let table: Arc<dyn Table> = Arc::new(TestTable {
        name: "t".into(),
        columns: vec![
            Column {
                name: "a".into(),
                ..Default::default()
            },
            Column {
                name: "b".into(),
                ..Default::default()
            },
            Column {
                name: "c".into(),
                ..Default::default()
            },
        ],
    });
    assert_eq!(
        EncodeRowForRecord(
            Some(table),
            0,
            &[
                Datum::Int(5),
                Datum::String("test test".into()),
                Datum::BinaryLiteral(vec![0, 0, 0, 0xab, 0xcd, 0xef]),
            ],
            &[0, 1, 2, 3],
        ),
        b"(5, \"test test\", \x00\x00\x00\xab\xcd\xef)"
    );
}

/// 严格模式按列字符集拒绝非法 UTF-8 与非 ASCII 输入。
#[test]
fn TestStrictModeChecksColumnCharset() {
    let table: Arc<dyn Table> = Arc::new(TestTable {
        name: "t".into(),
        columns: vec![
            Column {
                name: "s0".into(),
                charset: "utf8mb4".into(),
                ..Default::default()
            },
            Column {
                name: "s1".into(),
                charset: "ascii".into(),
                ..Default::default()
            },
        ],
    });
    let config = EncodingConfig {
        SessionOptions: SessionOptions {
            SQLMode: SQL_MODE_STRICT_ALL_TABLES,
            ..Default::default()
        },
        Table: Some(table),
        ..Default::default()
    };
    let mut encoder = NewEncodingBuilder()
        .NewEncoder(&Context::default(), &config)
        .unwrap();
    let error = match encoder.Encode(&[Datum::Bytes(vec![0xff])], 1, &[0, -1], 0) {
        Ok(_) => panic!("invalid UTF-8 must fail in strict mode"),
        Err(error) => error,
    };
    assert!(error.0.ends_with("for column s0"));

    let mut encoder = NewEncodingBuilder()
        .NewEncoder(&Context::default(), &config)
        .unwrap();
    let error = match encoder.Encode(
        &[
            Datum::String(String::new()),
            Datum::String("非 ASCII".into()),
        ],
        1,
        &[0, 1],
        0,
    ) {
        Ok(_) => panic!("non-ASCII input must fail for ASCII columns"),
        Err(error) => error,
    };
    assert!(error.0.ends_with("for column s1"));
}

/// Go appendSQLBytes 将 ASCII 退格转义为 `\b`。
#[test]
fn TestEncodeBackspaceAndExtremeFloatsLikeGo() {
    let table: Arc<dyn Table> = Arc::new(TestTable {
        name: "t".into(),
        columns: vec![Column {
            name: "v".into(),
            ..Default::default()
        }],
    });
    assert_eq!(
        EncodeRowForRecord(
            Some(Arc::clone(&table)),
            0,
            &[Datum::Bytes(vec![b'a', 8, b'b'])],
            &[0],
        ),
        b"('a\\bb')"
    );
    assert_eq!(
        EncodeRowForRecord(Some(Arc::clone(&table)), 0, &[Datum::Float(5e-324)], &[0]),
        b"(5e-324)"
    );
    assert_eq!(
        EncodeRowForRecord(Some(table), 0, &[Datum::Float(f64::MAX)], &[0],),
        b"(1.7976931348623157e+308)"
    );
}

/// Go 分别报告输入列不足与超过映射上限，两条错误文本不可合并。
#[test]
fn TestEncodeColumnCountErrorsMatchGo() {
    let table: Arc<dyn Table> = Arc::new(TestTable {
        name: "t".into(),
        columns: vec![
            Column {
                name: "a".into(),
                ..Default::default()
            },
            Column {
                name: "b".into(),
                ..Default::default()
            },
        ],
    });
    let config = EncodingConfig {
        Table: Some(table),
        ..Default::default()
    };
    let mut encoder = NewEncodingBuilder()
        .NewEncoder(&Context::default(), &config)
        .unwrap();
    let error = match encoder.Encode(&[Datum::Int(1)], 0, &[0, 1], 0) {
        Ok(_) => panic!("short row must fail"),
        Err(error) => error,
    };
    assert_eq!(error.0, "column count mismatch, expected 2, got 1");

    let mut encoder = NewEncodingBuilder()
        .NewEncoder(&Context::default(), &config)
        .unwrap();
    let error = match encoder.Encode(
        &[Datum::Int(1), Datum::Int(2), Datum::Int(3)],
        0,
        &[0, 1],
        0,
    ) {
        Ok(_) => panic!("wide row must fail"),
        Err(error) => error,
    };
    assert_eq!(error.0, "column count mismatch, at most 2 but got 3");
}

/// Go Writer 持有原 backend 指针，行级错误状态必须回写到同一 backend。
#[test]
fn TestLocalWriterSharesBackendErrorState() {
    let suite = createMysqlSuite();
    suite
        .executor
        .executeResults
        .lock()
        .unwrap()
        .extend([Err(SqlError::new("batch")), Err(SqlError::new("row"))]);
    let executor: Arc<dyn SqlExecutor> = suite.executor.clone();
    let backend = NewTiDBBackend(
        executor,
        TiDBBackendConfig {
            errorBudget: 1,
            ..Default::default()
        },
    );
    let mut config = LocalWriterConfig::default();
    config.TiDB.TableName = "t".into();
    let mut writer =
        Backend::LocalWriter(backend.as_ref(), &Context::default(), &config, Uuid::nil()).unwrap();
    writer
        .AppendRows(
            &Context::default(),
            &[],
            &encodeRowsTiDB(false, &[(1, "a")]),
        )
        .unwrap();
    assert_eq!(backend.Errors().len(), 1);
    assert_eq!(writer.Close(&Context::default()).unwrap(), None);
}

/// 批量多值 INSERT 字面量拼接。
#[test]
fn TestLogicalImportBatch() {
    let suite = createMysqlSuite();
    suite
        .backend(DuplicateResolution::Error)
        .WriteRows(
            "t",
            &["id".into(), "name".into()],
            &encodeRowsTiDB(false, &[(1, "a"), (2, "b")]),
        )
        .unwrap();
    let query = &suite.executor.executions.lock().unwrap()[0].0;
    assert!(query.contains("(1,'a'),(2,'b')"));
}

/// 预编译批量：占位符与绑定值数量正确。
#[test]
fn TestLogicalImportBatchPrepStmt() {
    let suite = createMysqlSuite();
    let executor: Arc<dyn SqlExecutor> = suite.executor.clone();
    let backend = NewTiDBBackend(
        executor,
        TiDBBackendConfig {
            preparedStatements: true,
            ..Default::default()
        },
    );
    backend
        .WriteRows(
            "t",
            &["id".into(), "name".into()],
            &encodeRowsTiDB(true, &[(1, "a"), (2, "b")]),
        )
        .unwrap();
    let execution = &suite.executor.executions.lock().unwrap()[0];
    assert!(execution.0.contains("(?,?),(?,?)"));
    assert_eq!(execution.1.len(), 4);
}

/// 预编译路径下降级逐行并记录单个错误。
#[test]
fn TestWriteRowsRecordOneErrorPrepStmt() {
    let suite = createMysqlSuite();
    suite.executor.executeResults.lock().unwrap().extend([
        Err(SqlError::new("batch")),
        Err(SqlError::new("row")),
        Ok(1),
    ]);
    let executor: Arc<dyn SqlExecutor> = suite.executor.clone();
    let backend = NewTiDBBackend(
        executor,
        TiDBBackendConfig {
            preparedStatements: true,
            errorBudget: 2,
            ..Default::default()
        },
    );
    backend
        .WriteRows("t", &[], &encodeRowsTiDB(true, &[(1, "a"), (2, "b")]))
        .unwrap();
    assert_eq!(backend.Errors().len(), 1);
}

/// Go appendSQL 的专用 Datum 分支必须保留字面量形状。
#[test]
fn TestEncodeAllGoDatumKinds() {
    let table: Arc<dyn Table> = Arc::new(TestTable {
        name: "t".into(),
        columns: (0..13)
            .map(|index| Column {
                name: format!("c{index}"),
                ..Default::default()
            })
            .collect(),
    });
    assert_eq!(
        EncodeRowForRecord(
            Some(table),
            0,
            &[
                Datum::MinNotNull,
                Datum::MaxValue,
                Datum::Int(i64::MIN),
                Datum::UInt(u64::MAX),
                Datum::Float(5e-324),
                Datum::Json(r#"{"a":1}"#.into()),
                Datum::BinaryLiteral(vec![0, 0, 0, 0xab, 0xcd, 0xef]),
                Datum::Bit(vec![0x98, 0x76, 0x54, 0x32]),
                Datum::Enum {
                    name: "ENUM_NAME".into(),
                    value: 51,
                },
                Datum::Set {
                    name: "SET_NAME".into(),
                    value: 7,
                },
                Datum::Decimal("12.5".into()),
                Datum::Timestamp("2026-08-02 12:34:56".into()),
                Datum::Duration("12:34:56".into()),
            ],
            &(0..13).collect::<Vec<_>>(),
        ),
        b"(MINVALUE,MAXVALUE,-9223372036854775808,18446744073709551615,5e-324,'{\"a\":1}',x'000000abcdef',2557891634,51,7,'12.5','2026-08-02 12:34:56','12:34:56')"
    );
}

/// TiDB writer 的 Go nil ChunkFlushStatus 映射为 Rust None。
#[test]
fn TestWriterCloseReturnsNilFlushStatus() {
    let suite = createMysqlSuite();
    let backend = suite.backend(DuplicateResolution::Error);
    let mut config = LocalWriterConfig::default();
    config.TiDB.TableName = "t".into();
    let mut writer =
        Backend::LocalWriter(backend.as_ref(), &Context::default(), &config, Uuid::nil()).unwrap();
    assert!(writer.Close(&Context::default()).unwrap().is_none());
}
