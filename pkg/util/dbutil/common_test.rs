// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// `dbutil::common` 标识符转义、占位符替换与时区偏移格式化的回归测试。
//
// 文件前半 `GO_REFERENCE` 保留 Go 原测试草稿供对照；可执行用例在文件末尾，
// 验证 `TableName` / `ColumnName` / `ReplacePlaceholder` / `FormatTimeZoneOffset` 与 Go 行为一致。

/// 迁移对照用的 Go 测试草稿原文（未编译执行）。
const GO_REFERENCE: &str = r################"

// replacePlaceholderCase 对应 Go 匿名结构体：记录占位符 SQL、替换参数与期望 SQL。
struct ReplacePlaceholderCase {
    origin_str: &'static str,
    args: &'static [&'static str],
    expect_str: &'static str,
}

// TestReplacePlaceholder 对应 Go 测试：逐个把 ? 替换成带单引号的参数。
#[test]
fn test_replace_placeholder() {
    let test_cases = vec![
        ReplacePlaceholderCase {
            origin_str: "a > ? AND a < ?",
            args: &["1", "2"],
            expect_str: "a > '1' AND a < '2'",
        },
        ReplacePlaceholderCase {
            origin_str: "a = ? AND b = ?",
            args: &["1", "2"],
            expect_str: "a = '1' AND b = '2'",
        },
    ];

    for test_case in test_cases {
        // Go 原测试调用 ReplacePlaceholder 并用 require.Equal 比较字符串；这里保留同样的断言形状。
        let str_value = ReplacePlaceholder(test_case.origin_str, test_case.args);
        assert_eq!(test_case.expect_str, str_value);
    }
}

// tableNameCase 对应 Go 匿名结构体：验证 schema/table 反引号转义。
struct TableNameCase {
    schema: &'static str,
    table: &'static str,
    expect_table_name: &'static str,
}

// TestTableName 对应 Go 测试：表名需要按 `schema`.`table` 输出并转义内部反引号。
#[test]
fn test_table_name() {
    let test_cases = vec![
        TableNameCase {
            schema: "test",
            table: "testa",
            expect_table_name: "`test`.`testa`",
        },
        TableNameCase {
            schema: "test-1",
            table: "test-a",
            expect_table_name: "`test-1`.`test-a`",
        },
        TableNameCase {
            schema: "test",
            table: "t`esta",
            expect_table_name: "`test`.`t``esta`",
        },
    ];

    for test_case in test_cases {
        let table_name = TableName(test_case.schema, test_case.table);
        assert_eq!(test_case.expect_table_name, table_name);
    }
}

// columnNameCase 对应 Go 匿名结构体：验证列名反引号转义。
struct ColumnNameCase {
    column: &'static str,
    expect_col_name: &'static str,
}

// TestColumnName 对应 Go 测试：单列名需要包反引号并把内部 ` 翻倍。
#[test]
fn test_column_name() {
    let test_cases = vec![
        ColumnNameCase {
            column: "test",
            expect_col_name: "`test`",
        },
        ColumnNameCase {
            column: "test-1",
            expect_col_name: "`test-1`",
        },
        ColumnNameCase {
            column: "t`esta",
            expect_col_name: "`t``esta`",
        },
    ];

    for test_case in test_cases {
        let col_name = ColumnName(test_case.column);
        assert_eq!(test_case.expect_col_name, col_name);
    }
}

// MysqlErr 是 Go *mysql.MySQLError 的替身，只保留错误码和消息两个字段。
struct MysqlErr {
    number: u16,
    message: &'static str,
}

// newMysqlErr 对应 Go 辅助函数：构造 mysql.MySQLError 供错误分类测试复用。
fn new_mysql_err(number: u16, message: &'static str) -> MysqlErr {
    MysqlErr { number, message }
}

// ignoreErrorCase 对应 Go 匿名结构体：记录输入错误和是否可忽略。
struct IgnoreErrorCase {
    err: TestErr,
    can_ignore: bool,
}

// TestErr 用来描述 Go 测试中混合出现的 MySQL 错误与普通 errors.New。
enum TestErr {
    Mysql(MysqlErr),
    Other(&'static str),
}

// TestIsIgnoreError 对应 Go 测试：只允许指定 infoschema 错误码被 ignoreError 忽略。
#[test]
fn test_is_ignore_error() {
    let cases = vec![
        IgnoreErrorCase {
            err: TestErr::Mysql(new_mysql_err(infoschema::ErrDatabaseExists.Code() as u16, "Can't create database, database exists")),
            can_ignore: true,
        },
        IgnoreErrorCase {
            err: TestErr::Mysql(new_mysql_err(infoschema::ErrDatabaseDropExists.Code() as u16, "Can't drop database, database doesn't exists")),
            can_ignore: true,
        },
        IgnoreErrorCase {
            err: TestErr::Mysql(new_mysql_err(infoschema::ErrTableExists.Code() as u16, "Can't create table, table exists")),
            can_ignore: true,
        },
        IgnoreErrorCase {
            err: TestErr::Mysql(new_mysql_err(infoschema::ErrTableDropExists.Code() as u16, "Can't drop table, table dosen't exists")),
            can_ignore: true,
        },
        IgnoreErrorCase {
            err: TestErr::Mysql(new_mysql_err(infoschema::ErrColumnExists.Code() as u16, "Duplicate column name")),
            can_ignore: true,
        },
        IgnoreErrorCase {
            err: TestErr::Mysql(new_mysql_err(infoschema::ErrIndexExists.Code() as u16, "Duplicate Index")),
            can_ignore: true,
        },
        IgnoreErrorCase {
            err: TestErr::Mysql(new_mysql_err(999, "fake error")),
            can_ignore: false,
        },
        IgnoreErrorCase {
            err: TestErr::Other("unknown error"),
            can_ignore: false,
        },
    ];

    for case in cases {
        // Go 里 t.Logf 记录错误和期望值；只保留对 ignoreError 的布尔断言。
        assert_eq!(case.can_ignore, ignoreError(case.err));
    }
}

// TestDeleteRows 对应 Go 的 sqlmock 场景：DELETE 返回满批次后再返回不足批次，函数应停止且无错误。
#[test]
fn test_delete_rows() {
    let (db, mock) = sqlmock::New().expect("sqlmock.New should succeed");

    // Go 注释为 delete twice：第一次影响 DefaultDeleteRowsNum 行，第二次少一行，模拟分批删除收尾。
    mock.ExpectExec("DELETE FROM")
        .WillReturnResult(sqlmock::NewResult(0, DefaultDeleteRowsNum));
    mock.ExpectExec("DELETE FROM")
        .WillReturnResult(sqlmock::NewResult(0, DefaultDeleteRowsNum - 1));

    let err = DeleteRows(context::Background(), db, "test", "t", "", None);
    assert!(err.is_ok());

    // Go 原测试检查 ExpectationsWereMet；这里保留 mock 期望必须全部消费的收尾语义。
    assert!(mock.ExpectationsWereMet().is_ok());
}

// parserCase 对应 Go 匿名结构体：记录 SQL mode 字符串和是否应解析失败。
struct ParserCase {
    sql_mode_str: &'static str,
    has_err: bool,
}

// TestGetParser 对应 Go 测试：合法 SQL mode 创建 parser，非法后缀返回错误。
#[test]
fn test_get_parser() {
    let test_cases = vec![
        ParserCase { sql_mode_str: "", has_err: false },
        ParserCase { sql_mode_str: "ANSI_QUOTES", has_err: false },
        ParserCase { sql_mode_str: "ANSI_QUOTES,IGNORE_SPACE", has_err: false },
        ParserCase { sql_mode_str: "ANSI_QUOTES123", has_err: true },
        ParserCase { sql_mode_str: "ANSI_QUOTES,IGNORE_SPACE123", has_err: true },
    ];

    for test_case in test_cases {
        let (parser, err) = getParser(test_case.sql_mode_str);
        if test_case.has_err {
            assert!(err.is_some());
        } else {
            assert!(err.is_none());
            assert!(parser.is_some());
        }
    }
}

// bucketCase 对应 Go 匿名结构体：记录 histogram bucket 字符串、列类型和期望格式化结果。
struct BucketCase {
    value: &'static str,
    col: model::ColumnInfo,
    expect: &'static str,
}

// TestAnalyzeValuesFromBuckets 对应 Go 测试：时间类型 bucket 值既可能是可读字符串，也可能是 encoded datum。
#[test]
fn test_analyze_values_from_buckets() {
    let cases = vec![
        BucketCase {
            value: "2021-03-05 21:31:03",
            col: model::ColumnInfo { FieldType: types::NewFieldTypeBuilder().SetType(pmysql::TypeDatetime).Build() },
            expect: "2021-03-05 21:31:03",
        },
        BucketCase {
            value: "2021-03-05 21:31:03",
            col: model::ColumnInfo { FieldType: types::NewFieldTypeBuilder().SetType(pmysql::TypeTimestamp).Build() },
            expect: "2021-03-05 21:31:03",
        },
        BucketCase {
            value: "2021-03-05",
            col: model::ColumnInfo { FieldType: types::NewFieldTypeBuilder().SetType(pmysql::TypeDate).Build() },
            expect: "2021-03-05",
        },
        BucketCase {
            value: "1847956477067657216",
            col: model::ColumnInfo { FieldType: types::NewFieldTypeBuilder().SetType(pmysql::TypeDatetime).Build() },
            expect: "2020-01-01 10:00:00",
        },
        BucketCase {
            value: "1847955927311843328",
            col: model::ColumnInfo { FieldType: types::NewFieldTypeBuilder().SetType(pmysql::TypeTimestamp).Build() },
            expect: "2020-01-01 02:00:00",
        },
        BucketCase {
            value: "1847955789872889856",
            col: model::ColumnInfo { FieldType: types::NewFieldTypeBuilder().SetType(pmysql::TypeDate).Build() },
            expect: "2020-01-01 00:00:00",
        },
    ];

    for case in cases {
        let (val, err) = AnalyzeValuesFromBuckets(case.value, vec![case.col]);
        assert!(err.is_none());
        assert_eq!(1, val.len());
        assert_eq!(case.expect, val[0]);
    }
}

// TestFormatTimeZoneOffset 对应 Go 测试：正负 Duration 均格式化成 +HH:MM 或 -HH:MM。
#[test]
fn test_format_time_zone_offset() {
    let cases = vec![
        ("+00:00", time::Duration::from_secs(0)),
        ("+01:00", time::Hour),
        ("-08:03", -1 * (8 * time::Hour + 3 * time::Minute)),
        ("-12:59", -1 * (12 * time::Hour + 59 * time::Minute)),
        ("+12:59", 12 * time::Hour + 59 * time::Minute),
    ];

    for (expected, duration) in cases {
        let offset = FormatTimeZoneOffset(duration);
        assert_eq!(offset, expected);
    }
}

// TestGetTimeZoneOffset 对应 Go 的 sqlmock 查询：TIMEDIFF 返回 01:00:00 时应转成 1h0m0s。
#[test]
fn test_get_time_zone_offset() {
    let (db, mock) = sqlmock::New().expect("sqlmock.New should succeed");

    mock.ExpectQuery(r"SELECT cast\(TIMEDIFF\(NOW\(6\), UTC_TIMESTAMP\(6\)\) as time\);")
        .WillReturnRows(mock.NewRows(vec![""]).AddRow("01:00:00"));
    let (duration, err) = GetTimeZoneOffset(context::Background(), db);
    assert!(err.is_none());
    assert_eq!("1h0m0s", duration.String());
}
"################;

use crate::common::{
    BuildDSN, DBConfig, DefaultDeleteRowsNum, DefaultRetryTime, DeleteRows, ExecSQLWithRetry,
    ExecuteSQLs, GetBucketsInfo, GetCRC32Checksum, GetCreateTableSQL, GetDBVersion, GetMinMaxValue,
    GetRandomValues, GetRowCount, GetSQLMode, GetSchemas, GetSessionVariable, GetTables,
    GetTidbLatestTSO, GetTimeZoneOffset, GetViews, IsTiDB, ParseTimeZoneOffset,
};
use crate::interface::{DBExecutor, DbError, QueryExecutor, QueryResult, Transaction, Value};
use crate::query::{ScanRow, ScanRowsToInterfaces};
use crate::types::{IsFloatType, IsNumberType, IsTimeTypeAndNeedDecode};
use astersql_infoschema::infoschema::{CiString, ColumnInfo, IndexInfo, TableInfo};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

fn result(columns: &[&str], rows: Vec<Vec<Value>>) -> QueryResult {
    QueryResult {
        columns: columns.iter().map(|column| (*column).to_owned()).collect(),
        rows,
    }
}

#[derive(Default)]
struct ScriptedDb {
    queries: Mutex<VecDeque<Result<QueryResult, DbError>>>,
    execs: Mutex<VecDeque<Result<u64, DbError>>>,
}

impl ScriptedDb {
    fn with_queries(queries: Vec<Result<QueryResult, DbError>>) -> Self {
        Self {
            queries: Mutex::new(queries.into()),
            execs: Mutex::new(VecDeque::new()),
        }
    }

    fn with_execs(execs: Vec<Result<u64, DbError>>) -> Self {
        Self {
            queries: Mutex::new(VecDeque::new()),
            execs: Mutex::new(execs.into()),
        }
    }
}

impl QueryExecutor for ScriptedDb {
    fn QueryContext(&self, _query: &str, _args: &[Value]) -> Result<QueryResult, DbError> {
        self.queries
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(QueryResult::default()))
    }
}

#[derive(Default)]
struct TestTransaction;

impl QueryExecutor for TestTransaction {
    fn QueryContext(&self, _query: &str, _args: &[Value]) -> Result<QueryResult, DbError> {
        Ok(QueryResult::default())
    }
}

impl Transaction for TestTransaction {
    fn ExecContext(&mut self, _query: &str, _args: &[Value]) -> Result<u64, DbError> {
        Ok(1)
    }

    fn Commit(self: Box<Self>) -> Result<(), DbError> {
        Ok(())
    }

    fn Rollback(self: Box<Self>) -> Result<(), DbError> {
        Ok(())
    }
}

impl DBExecutor for ScriptedDb {
    fn BeginTx(&self) -> Result<Box<dyn Transaction>, DbError> {
        Ok(Box::new(TestTransaction))
    }

    fn ExecContext(&self, _query: &str, _args: &[Value]) -> Result<u64, DbError> {
        self.execs.lock().unwrap().pop_front().unwrap_or(Ok(0))
    }
}

fn error(code: u16, message: &str) -> DbError {
    DbError {
        code,
        sql_state: None,
        message: message.to_owned(),
    }
}

#[test]
fn common_query_helpers_match_go_result_shapes() {
    let table_info = TableInfo {
        columns: vec![ColumnInfo {
            name: CiString::new("id"),
            ..Default::default()
        }],
        ..Default::default()
    };
    let db = ScriptedDb::with_queries(vec![
        Ok(result(
            &["Table", "Create Table"],
            vec![vec!["t".into(), "CREATE TABLE `t` (id INT)".into()]],
        )),
        Ok(result(&["cnt"], vec![vec![42_i64.into()]])),
        Ok(result(
            &["id"],
            vec![vec![Value::Null], vec!["1".into()], vec!["2".into()]],
        )),
        Ok(result(&["MIN", "MAX"], vec![vec!["1".into(), "9".into()]])),
        Ok(result(
            &["Tables", "Table_type"],
            vec![
                vec!["base".into(), "BASE TABLE".into()],
                vec![Value::Null, "VIEW".into()],
            ],
        )),
        Ok(result(
            &["Tables", "Table_type"],
            vec![vec!["view".into(), "VIEW".into()]],
        )),
        Ok(result(&["Database"], vec![vec!["test".into()]])),
        Ok(result(&["checksum"], vec![[7_i64.into()].to_vec()])),
        Ok(result(
            &["Column_name", "Count", "Lower_Bound", "Upper_Bound"],
            vec![vec!["id".into(), 3_i64.into(), "1".into(), "3".into()]],
        )),
        Ok(result(&["Position"], vec![vec![123_i64.into()]])),
        Ok(result(&["version()"], vec![vec!["TiDB-v8".into()]])),
        Ok(result(
            &["Variable_name", "Value"],
            vec![vec!["x".into(), "ON".into()]],
        )),
        Ok(result(
            &["Variable_name", "Value"],
            vec![vec!["x".into(), "STRICT_TRANS_TABLES".into()]],
        )),
        Ok(result(&["version()"], vec![vec!["TiDB-v8".into()]])),
    ]);

    assert_eq!(
        GetCreateTableSQL(&db, "test", "t").unwrap(),
        "CREATE TABLE `t` (id INT)"
    );
    assert_eq!(GetRowCount(&db, "test", "t", "", &[]).unwrap(), 42);
    assert_eq!(
        GetRandomValues(&db, "test", "t", "id", 3, "", &[], "").unwrap(),
        ["1", "2"]
    );
    assert_eq!(
        GetMinMaxValue(&db, "test", "t", "id", "", &[], "").unwrap(),
        ("1".into(), "9".into())
    );
    assert_eq!(GetTables(&db, "test").unwrap(), ["base"]);
    assert_eq!(GetViews(&db, "test").unwrap(), ["view"]);
    assert_eq!(GetSchemas(&db).unwrap(), ["test"]);
    assert_eq!(
        GetCRC32Checksum(&db, "test", "t", &table_info, "TRUE", &[]).unwrap(),
        7
    );
    let bucket_table_info = TableInfo {
        columns: vec![ColumnInfo {
            name: CiString::new("id"),
            ..Default::default()
        }],
        indices: vec![IndexInfo {
            name: CiString::new("PRIMARY"),
            ..Default::default()
        }],
        ..Default::default()
    };
    let buckets = GetBucketsInfo(&db, "test", "t", &bucket_table_info).unwrap();
    assert_eq!(buckets["PRIMARY"][0].Count, 3);
    assert!(!buckets.contains_key("id"));
    assert_eq!(GetTidbLatestTSO(&db).unwrap(), 123);
    assert_eq!(GetDBVersion(&db).unwrap(), "TiDB-v8");
    assert_eq!(GetSessionVariable(&db, "x").unwrap(), "ON");
    assert_eq!(GetSQLMode(&db).unwrap(), "STRICT_TRANS_TABLES");
    assert!(IsTiDB(&db).unwrap());
}

#[test]
fn common_configuration_retry_transaction_and_parser_match_go_boundaries() {
    let config = DBConfig {
        Host: "db\\host".into(),
        User: "u\"ser".into(),
        Password: "secret".into(),
        Schema: "s".into(),
        Snapshot: "123".into(),
        Port: 4000,
    };
    assert_eq!(
        config.to_string(),
        r#"{"host":"db\\host","user":"u\"ser","schema":"s","snapshot":"123","port":4000}"#
    );
    let mut variables = HashMap::new();
    variables.insert("sql_mode".into(), "ANSI_QUOTES".into());
    assert_eq!(
        BuildDSN(&config, &variables),
        "u\"ser:secret@tcp(db\\host:4000)/s/?charset=utf8mb4&tidb_snapshot=123&sql_mode='ANSI_QUOTES'"
    );
    assert_eq!(ParseTimeZoneOffset("-08:03:00").unwrap().0, -28_980);
    assert!(ParseTimeZoneOffset("08:03").is_err());
    assert!(crate::common::getParser("ANSI_QUOTES,IGNORE_SPACE").is_ok());
    assert!(crate::common::getParser("ANSI_QUOTES123").is_err());

    let delete_db =
        ScriptedDb::with_execs(vec![Ok(DefaultDeleteRowsNum), Ok(DefaultDeleteRowsNum - 1)]);
    assert!(DeleteRows(&delete_db, "test", "t", "id > 0", &[]).is_ok());
    assert_eq!(delete_db.execs.lock().unwrap().len(), 0);

    let retry_db = ScriptedDb::with_execs(vec![Err(error(1213, "deadlock")), Ok(1)]);
    assert!(ExecSQLWithRetry(&retry_db, "UPDATE t SET a = 1", &[]).is_ok());
    assert!(ExecuteSQLs(&ScriptedDb::default(), &["SELECT 1".into()], &[vec![]]).is_ok());
    assert_eq!(DefaultRetryTime, 10);
}

#[test]
fn ddl_ignore_error_codes_match_go() {
    for code in [1007, 1008, 1050, 1051, 1060, 1061] {
        let db = ScriptedDb::with_execs(vec![Err(error(code, "idempotent DDL"))]);
        assert!(ExecSQLWithRetry(&db, "DDL", &[]).is_ok(), "code {code}");
    }

    for code in [999, 1062] {
        let db = ScriptedDb::with_execs(vec![Err(error(code, "not ignored"))]);
        assert_eq!(ExecSQLWithRetry(&db, "DDL", &[]).unwrap_err().code, code);
    }
}

#[test]
fn bucket_time_decoding_matches_go_cases() {
    for (value, column_type, expected) in [
        ("2021-03-05 21:31:03", 12, "2021-03-05 21:31:03"),
        ("2021-03-05 21:31:03", 7, "2021-03-05 21:31:03"),
        ("2021-03-05", 10, "2021-03-05"),
        ("1847956477067657216", 12, "2020-01-01 10:00:00"),
        ("1847955927311843328", 7, "2020-01-01 02:00:00"),
        ("1847955789872889856", 10, "2020-01-01 00:00:00"),
    ] {
        assert_eq!(
            crate::common::AnalyzeValuesFromBuckets(value, &[column_type]).unwrap(),
            [expected]
        );
    }
}

#[test]
fn timezone_query_and_format_cases_match_go() {
    let db = ScriptedDb::with_queries(vec![Ok(result(&[""], vec![vec!["01:00:00".into()]]))]);
    assert_eq!(GetTimeZoneOffset(&db).unwrap().0, 3_600);

    for (seconds, expected) in [
        (0, "+00:00"),
        (3_600, "+01:00"),
        (-(8 * 3_600 + 3 * 60), "-08:03"),
        (-(12 * 3_600 + 59 * 60), "-12:59"),
        (12 * 3_600 + 59 * 60, "+12:59"),
    ] {
        assert_eq!(
            crate::common::FormatTimeZoneOffset(DurationOffset(seconds)),
            expected
        );
    }
}

#[test]
fn query_scanners_and_mysql_type_classification_match_go() {
    let rows = result(
        &["id", "name"],
        vec![
            vec![1_i64.into(), Value::Null],
            vec![2_i64.into(), "two".into()],
        ],
    );
    assert_eq!(ScanRowsToInterfaces(rows.clone()), rows.rows);
    let scanned = ScanRow(&rows.columns, &rows.rows[0]).unwrap();
    assert_eq!(scanned["id"].Data, b"1".to_vec());
    assert!(!scanned["id"].IsNull);
    assert!(scanned["name"].IsNull);
    assert!(ScanRow(&rows.columns, &[Value::Null]).is_err());

    for type_code in [1, 2, 3, 8, 9, 13] {
        assert!(IsNumberType(type_code));
    }
    for type_code in [4, 5, 246] {
        assert!(IsFloatType(type_code));
    }
    for type_code in [7, 10, 12] {
        assert!(IsTimeTypeAndNeedDecode(type_code));
    }
    assert!(!IsNumberType(4));
    assert!(!IsFloatType(3));
    assert!(!IsTimeTypeAndNeedDecode(8));
}

use crate::common::{
    ColumnName, DurationOffset, FormatTimeZoneOffset, ReplacePlaceholder, TableName,
};

#[test]
/// 核对表名/列名反引号转义、? 占位替换以及时区 Duration 格式化。
fn names_placeholders_and_timezone_format_match_go() {
    assert_eq!(TableName("a`b", "t"), "`a``b`.`t`");
    assert_eq!(ColumnName("c`d"), "`c``d`");
    assert_eq!(
        ReplacePlaceholder("a > ? AND a < ?", &["1".into(), "2".into()]),
        "a > '1' AND a < '2'"
    );
    assert_eq!(FormatTimeZoneOffset(DurationOffset(3_600)), "+01:00");
    assert_eq!(FormatTimeZoneOffset(DurationOffset(-19_800)), "-05:30");
}
