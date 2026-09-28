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

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use astersql_server::server::ServerConfig;
use astersql_server_internal_testserverclient::{
    ExecuteResult, QueryResult, SqlExecutor, SqlValue,
};
use astersql_server_tests_servertestkit::{
    create_tidb_test_suite, create_tidb_test_suite_with_cfg,
};
use astersql_testkit::{DbValue, Rows, TestKit};

struct TestKitExecutor {
    testkit: TestKit,
}

fn serial_guard() -> MutexGuard<'static, ()> {
    static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();
    SERIAL
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl TestKitExecutor {
    fn new() -> Self {
        Self {
            testkit: new_testkit(),
        }
    }
}

impl SqlExecutor for TestKitExecutor {
    fn execute(&mut self, sql: &str, parameters: &[SqlValue]) -> Result<ExecuteResult, String> {
        let result = self
            .testkit
            .Exec(sql, parameters.iter().map(to_db_value).collect())
            .map_err(|error| client_error(error.to_string()))?;
        Ok(ExecuteResult {
            rows_affected: result.affected_rows,
            last_insert_id: result.last_insert_id,
        })
    }

    fn query(&mut self, sql: &str, parameters: &[SqlValue]) -> Result<QueryResult, String> {
        let result = self
            .testkit
            .Query(sql, parameters.iter().map(to_db_value).collect())
            .map_err(|error| client_error(error.to_string()))?;
        Ok(QueryResult {
            columns: result.columns,
            rows: result
                .rows
                .into_iter()
                .map(|row| row.into_iter().map(from_db_value).collect())
                .collect(),
        })
    }

    fn fork(&self) -> Result<Box<dyn SqlExecutor>, String> {
        Ok(Box::new(Self {
            testkit: TestKit::new(self.testkit.Store()),
        }))
    }
}

fn client_error(error: String) -> String {
    error
        .strip_prefix("ERROR ")
        .map_or(error.clone(), |message| format!("Error {message}"))
}

fn to_db_value(value: &SqlValue) -> DbValue {
    match value {
        SqlValue::Null => DbValue::Null,
        SqlValue::Signed(value) => DbValue::I64(*value),
        SqlValue::Unsigned(value) => DbValue::U64(*value),
        SqlValue::Float(value) => DbValue::F64(*value),
        SqlValue::Bytes(value) => DbValue::Bytes(value.clone()),
        SqlValue::Text(value) => DbValue::String(value.clone()),
        SqlValue::Bool(value) => DbValue::Bool(*value),
    }
}

fn from_db_value(value: DbValue) -> SqlValue {
    match value {
        DbValue::Null => SqlValue::Null,
        DbValue::I64(value) => SqlValue::Signed(value),
        DbValue::U64(value) => SqlValue::Unsigned(value),
        DbValue::F64(value) => SqlValue::Float(value),
        DbValue::Bytes(value) => SqlValue::Bytes(value),
        DbValue::String(value) => SqlValue::Text(value),
        DbValue::Bool(value) => SqlValue::Bool(value),
    }
}

fn new_testkit() -> TestKit {
    let (store, _domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    TestKit::new(store as Arc<dyn astersql_testkit::Database>)
}

fn assert_live_suite(suite: &astersql_server_tests_servertestkit::TidbTestSuite) {
    assert!(suite.server.health());
    assert_ne!(suite.test_server_client.port, 0);
    assert_ne!(suite.test_server_client.status_port, 0);
    assert!(
        suite
            .test_server_client
            .fetch_status("/status")
            .unwrap()
            .is_success()
    );
}

/// Go 文件包含 11 个顶层串行测试；Rust 保持一一对应的测试入口。
#[test]
fn rust_serial_test_inventory_matches_go() {
    let rust_source = include_str!("tidb_serial_test.rs");
    let test_attributes = rust_source
        .lines()
        .filter(|line| line.trim() == "#[test]")
        .count();
    assert_eq!(test_attributes - 1, 11);
}

#[test]
fn test_load_data_1() {
    let _serial = serial_guard();
    let suite = create_tidb_test_suite();
    assert_live_suite(&suite);
    let mut executor = TestKitExecutor::new();
    suite
        .test_server_client
        .run_test_load_data_with_column_list(&mut executor)
        .expect("LOAD DATA column-list scenarios");
    suite
        .test_server_client
        .run_test_load_data(&mut executor)
        .expect("LOAD DATA scenarios");
    suite
        .test_server_client
        .run_test_load_data_with_select_into_outfile(&mut executor)
        .expect("SELECT INTO OUTFILE round trip");
    suite
        .test_server_client
        .run_test_load_data_for_slow_log(&mut executor)
        .expect("LOAD DATA slow-log scenarios");
}

#[test]
fn test_load_data_in_transaction() {
    let _serial = serial_guard();
    let suite = create_tidb_test_suite();
    assert_live_suite(&suite);
    let mut executor = TestKitExecutor::new();
    suite
        .test_server_client
        .run_test_load_data_in_transaction(&mut executor)
        .expect("LOAD DATA transaction scenarios");
}

#[test]
fn test_config_default_value() {
    let _serial = serial_guard();
    let suite = create_tidb_test_suite();
    assert_live_suite(&suite);
    let mut tk = new_testkit();
    tk.MustExec("use test", vec![]);
    tk.MustQuery("select @@tidb_slow_log_threshold", vec![])
        .Check(Rows(&["300"]));
}

#[test]
fn test_load_data_auto_random() {
    let _serial = serial_guard();
    let suite = create_tidb_test_suite();
    assert_live_suite(&suite);
    let mut executor = TestKitExecutor::new();
    suite
        .test_server_client
        .run_test_load_data_auto_random(&mut executor)
        .expect("auto-random LOAD DATA scenario");
}

#[test]
fn test_load_data_auto_random_with_special_term() {
    let _serial = serial_guard();
    let suite = create_tidb_test_suite();
    assert_live_suite(&suite);
    let mut executor = TestKitExecutor::new();
    suite
        .test_server_client
        .run_test_load_data_auto_random_with_special_term(&mut executor)
        .expect("special-terminator auto-random LOAD DATA scenario");
}

#[test]
fn test_explain_for() {
    let _serial = serial_guard();
    let suite = create_tidb_test_suite();
    assert_live_suite(&suite);
    let mut executor = TestKitExecutor::new();
    suite
        .test_server_client
        .run_test_explain_for_conn(&mut executor)
        .expect("EXPLAIN FOR CONNECTION scenario");
}

#[test]
fn test_stmt_count() {
    let _serial = serial_guard();
    crate::main_test::ensure_test_main_environment()
        .expect("server test metrics must be initialized");
    let mut cfg = ServerConfig::default();
    cfg.port = 0;
    cfg.status.port = 0;
    cfg.status.report_status = true;
    let suite = create_tidb_test_suite_with_cfg(cfg);
    assert_live_suite(&suite);
    let actual = suite.server.config();
    assert_eq!(actual.port, 0);
    assert_eq!(actual.status.port, 0);
    assert!(actual.status.report_status);
    let mut executor = TestKitExecutor::new();
    suite
        .test_server_client
        .run_test_stmt_count(&mut executor)
        .expect("statement metrics scenario");
}

#[test]
#[ignore = "unstable test"]
fn test_db_stmt_count() {
    let _serial = serial_guard();
    let suite = create_tidb_test_suite();
    assert_live_suite(&suite);
    let mut executor = TestKitExecutor::new();
    suite
        .test_server_client
        .run_test_db_stmt_count(&mut executor)
        .expect("Go marks the unstable DB statement-count scenario skipped");
}

#[test]
fn test_load_data_list_partition() {
    let _serial = serial_guard();
    let suite = create_tidb_test_suite();
    assert_live_suite(&suite);
    let mut executor = TestKitExecutor::new();
    suite
        .test_server_client
        .run_test_load_data_for_list_partition(&mut executor)
        .expect("LIST partition LOAD DATA scenario");
    suite
        .test_server_client
        .run_test_load_data_for_list_partition_2(&mut executor)
        .expect("LIST expression partition LOAD DATA scenario");
    suite
        .test_server_client
        .run_test_load_data_for_list_column_partition(&mut executor)
        .expect("LIST COLUMNS partition LOAD DATA scenario");
    suite
        .test_server_client
        .run_test_load_data_for_list_column_partition_2(&mut executor)
        .expect("multi-column LIST COLUMNS LOAD DATA scenario");
}

#[test]
fn test_prepare_execute() {
    let _serial = serial_guard();
    let suite = create_tidb_test_suite();
    assert_live_suite(&suite);
    let mut tk = new_testkit();
    tk.MustExec("use test", vec![]);
    tk.MustExec("create table t1(id int primary key, v int)", vec![]);
    tk.MustExec("insert into t1 values(1, 100)", vec![]);

    let statement = tk.Prepare("select * from t1 where id=1");
    let first = statement.query(&[]).unwrap();
    assert_eq!(first.columns.len(), 2);
    assert_eq!(
        first.string_rows(),
        vec![vec!["1".to_owned(), "100".to_owned()]]
    );

    tk.MustExec("alter table t1 drop column v", vec![]);
    let second = statement.query(&[]).unwrap();
    assert_eq!(second.columns.len(), 1);
    assert_eq!(second.string_rows(), vec![vec!["1".to_owned()]]);
}

#[test]
fn test_default_character_and_collation() {
    let _serial = serial_guard();
    let suite = create_tidb_test_suite();
    assert_live_suite(&suite);
    let mut tk = new_testkit();
    tk.Session()
        .SetConnectionCollationForTest(255)
        .expect("apply client collation 255");
    tk.MustExec("use test", vec![]);
    assert_eq!(
        astersql_parser_mysql::charset::GetCollationNameByID(255),
        Some("utf8mb4_0900_ai_ci")
    );
    assert_eq!(
        astersql_parser_mysql::charset::GetCollationIDByName("utf8mb4_0900_ai_ci"),
        Some(255)
    );
    tk.MustQuery("select @@collation_connection", vec![])
        .Check(Rows(&["utf8mb4_0900_ai_ci"]));
    tk.MustQuery("select @@character_set_connection", vec![])
        .Check(Rows(&["utf8mb4"]));
    tk.MustQuery("select @@character_set_client", vec![])
        .Check(Rows(&["utf8mb4"]));
}
