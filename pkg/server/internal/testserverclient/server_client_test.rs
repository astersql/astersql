// Copyright 2026 AsterSQL.

//! `TestServerClient` 的 Rust 对等测试。
//!
//! 本文件以 Go 版 `server_client.go` 为行为基线：脚本化执行器用于精确核对 SQL、
//! MySQL 协议事件和错误传播，临时 TCP 监听器用于模拟状态与指标接口；末尾少量场景
//! 再通过规范 mock store 执行真实 SQL，避免测试仅验证预置响应。

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use super::server_client::{
    ExecuteResult, GO_SCENARIO_NAMES, MysqlConfig, QueryResult, SchemaStateController,
    ServerConnection, ServerConnector, SqlExecutor, SqlValue, TestDatabase, TestServerClient,
    get_database_statement_count, get_statement_count,
};
use astersql_testkit::{Database, DbValue};

#[derive(Default)]
/// 记录 SQL，并按先进先出顺序返回预置结果或注入错误的执行器。
struct RecordingExecutor {
    statements: Vec<String>,
    execute_results: VecDeque<ExecuteResult>,
    query_results: VecDeque<QueryResult>,
    fail_on: Option<String>,
    fail_on_contains: Option<(String, String)>,
    errors_by_statement: HashMap<String, String>,
    parameterized_statements: Vec<(String, Vec<SqlValue>)>,
}

impl SqlExecutor for RecordingExecutor {
    fn execute(&mut self, sql: &str, parameters: &[SqlValue]) -> Result<ExecuteResult, String> {
        self.statements.push(sql.to_owned());
        if !parameters.is_empty() {
            self.parameterized_statements
                .push((sql.to_owned(), parameters.to_vec()));
        }
        if self.fail_on.as_deref() == Some(sql) {
            return Err(format!("forced failure: {sql}"));
        }
        if let Some((needle, error)) = &self.fail_on_contains
            && sql.contains(needle)
        {
            return Err(error.clone());
        }
        if let Some(error) = self.errors_by_statement.get(sql) {
            return Err(error.clone());
        }
        Ok(self.execute_results.pop_front().unwrap_or_default())
    }

    fn query(&mut self, sql: &str, parameters: &[SqlValue]) -> Result<QueryResult, String> {
        self.statements.push(sql.to_owned());
        if !parameters.is_empty() {
            self.parameterized_statements
                .push((sql.to_owned(), parameters.to_vec()));
        }
        self.query_results
            .pop_front()
            .ok_or_else(|| format!("missing query result for {sql}"))
    }
}

#[derive(Default)]
/// 同时模拟普通 SQL 与 MySQL 预处理协议事件的单连接脚本。
struct ProtocolScript {
    statements: Arc<Mutex<Vec<String>>>,
    query_results: VecDeque<QueryResult>,
    protocol_events: Vec<String>,
}

impl SqlExecutor for ProtocolScript {
    fn execute(&mut self, sql: &str, _parameters: &[SqlValue]) -> Result<ExecuteResult, String> {
        self.statements
            .lock()
            .expect("statement log")
            .push(sql.into());
        Ok(ExecuteResult::default())
    }

    fn query(&mut self, sql: &str, _parameters: &[SqlValue]) -> Result<QueryResult, String> {
        self.statements
            .lock()
            .expect("statement log")
            .push(sql.into());
        self.query_results
            .pop_front()
            .ok_or_else(|| format!("missing protocol query result for {sql}"))
    }
}

impl ServerConnection for ProtocolScript {
    fn ping(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn prepare(&mut self, _sql: &str) -> Result<u32, String> {
        self.protocol_events.push(format!("prepare:{_sql}"));
        Ok(1)
    }

    fn send_long_data(
        &mut self,
        statement_id: u32,
        parameter_index: u16,
        data: &[u8],
    ) -> Result<(), String> {
        self.protocol_events.push(format!(
            "long-data:{statement_id}:{parameter_index}:{}",
            data.len()
        ));
        Ok(())
    }

    fn execute_prepared(
        &mut self,
        _statement_id: u32,
        _parameters: &[SqlValue],
    ) -> Result<ExecuteResult, String> {
        self.protocol_events.push(format!(
            "execute-prepared:{_statement_id}:{}",
            _parameters.len()
        ));
        Ok(ExecuteResult::default())
    }

    fn close_prepared(&mut self, _statement_id: u32) -> Result<(), String> {
        self.protocol_events
            .push(format!("close-prepared:{_statement_id}"));
        Ok(())
    }
}

#[derive(Default)]
/// 在 DDL 进入 write reorganization 状态时立即执行事务动作的测试控制器。
struct RecordingSchemaState {
    ddls: Vec<String>,
}

impl SchemaStateController for RecordingSchemaState {
    fn at_write_reorganization(
        &mut self,
        ddl: &str,
        action: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<(), String> {
        self.ddls.push(ddl.into());
        action()
    }
}

#[derive(Default)]
/// 记录每次连接配置，并依次交付预置连接脚本。
struct RecordingConnector {
    configs: Vec<MysqlConfig>,
    scripts: VecDeque<Result<ProtocolScript, String>>,
}

impl ServerConnector for RecordingConnector {
    fn connect(&mut self, config: &MysqlConfig) -> Result<Box<dyn ServerConnection>, String> {
        self.configs.push(config.clone());
        self.scripts
            .pop_front()
            .unwrap_or_else(|| Ok(ProtocolScript::default()))
            .map(|script| Box::new(script) as Box<dyn ServerConnection>)
    }
}

/// 连接规范 mock store 的执行器，用于验证场景 SQL 本身能够真实执行。
struct RealSqlExecutor {
    store: Arc<dyn Database>,
    database: Arc<dyn Database>,
    owns_store: bool,
}

impl RealSqlExecutor {
    fn new() -> Self {
        let (store, _domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
        let database = store
            .create_session()
            .expect("create canonical SQL session")
            .expect("store exposes a SQL session");
        database
            .execute("create database if not exists test", &[])
            .expect("create default test database");
        database
            .execute("use test", &[])
            .expect("select default test database");
        Self {
            store,
            database,
            owns_store: true,
        }
    }
}

impl Drop for RealSqlExecutor {
    fn drop(&mut self) {
        let _ = self.database.close();
        if self.owns_store {
            let _ = self.store.close();
        }
    }
}

impl SqlExecutor for RealSqlExecutor {
    fn execute(&mut self, sql: &str, parameters: &[SqlValue]) -> Result<ExecuteResult, String> {
        let result = self
            .database
            .execute(sql, &parameters.iter().map(to_db_value).collect::<Vec<_>>())
            .map_err(|error| error.to_string())?;
        Ok(ExecuteResult {
            rows_affected: result.affected_rows,
            last_insert_id: result.last_insert_id,
        })
    }

    fn query(&mut self, sql: &str, parameters: &[SqlValue]) -> Result<QueryResult, String> {
        let result = self
            .database
            .query(sql, &parameters.iter().map(to_db_value).collect::<Vec<_>>())
            .map_err(|error| error.to_string())?;
        let preserve_byte_parameter = sql.trim().eq_ignore_ascii_case("select ?")
            && matches!(parameters, [SqlValue::Bytes(_)]);
        let mut query_result = QueryResult {
            columns: result.columns,
            rows: result
                .rows
                .into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|value| {
                            if preserve_byte_parameter {
                                match value {
                                    DbValue::String(value) if value == "0x" => {
                                        SqlValue::Bytes(Vec::new())
                                    }
                                    DbValue::String(value) => SqlValue::Bytes(value.into_bytes()),
                                    value => from_db_value(value),
                                }
                            } else {
                                from_db_value(value)
                            }
                        })
                        .collect()
                })
                .collect(),
        };
        normalize_go_scan_types(sql, &mut query_result)?;
        Ok(query_result)
    }

    fn fork(&self) -> Result<Box<dyn SqlExecutor>, String> {
        let database = self
            .store
            .create_session()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "store does not expose an independent SQL session".to_string())?;
        Ok(Box::new(Self {
            store: Arc::clone(&self.store),
            database,
            owns_store: false,
        }))
    }
}

/// 将 mock store 的结果调整为 Go `database/sql` 在对应场景中的扫描类型。
///
/// DECIMAL、BIT 与聚合值在两套驱动中的默认表示不同；这里仅对已知查询做转换，
/// 使后续断言比较的是服务端语义，而不是驱动表示细节。
fn normalize_go_scan_types(sql: &str, result: &mut QueryResult) -> Result<(), String> {
    let normalized = sql.trim().to_ascii_lowercase();
    if normalized == "select * from test where a > ?" {
        for row in &mut result.rows {
            let decimal = row
                .first()
                .map(SqlValue::display)
                .ok_or_else(|| "special-type query returned no DECIMAL column".to_owned())?;
            row[0] = SqlValue::Float(
                decimal
                    .parse()
                    .map_err(|error| format!("scan DECIMAL into float64: {error}"))?,
            );
            if let Some(SqlValue::Text(bit)) = row.get(3) {
                let byte = bit
                    .strip_prefix("0x")
                    .and_then(|value| u8::from_str_radix(value, 16).ok())
                    .or_else(|| {
                        let bytes = bit.as_bytes();
                        (bytes.len() == 1).then_some(bytes[0])
                    })
                    .ok_or_else(|| format!("scan BIT into []byte: {bit}"))?;
                row[3] = SqlValue::Bytes(vec![byte]);
            }
        }
    } else if normalized.starts_with("select sum(") || normalized.starts_with("select avg(") {
        for row in &mut result.rows {
            for value in row {
                let number = value.display();
                *value = SqlValue::Float(
                    number
                        .parse()
                        .map_err(|error| format!("scan aggregate into float64: {error}"))?,
                );
            }
        }
    }
    Ok(())
}

/// 将测试客户端参数转换为规范数据库接口使用的值类型。
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

/// 将规范数据库结果转换回测试客户端的值类型。
fn from_db_value(value: DbValue) -> SqlValue {
    match value {
        DbValue::Null => SqlValue::Null,
        DbValue::Bool(value) => SqlValue::Bool(value),
        DbValue::I64(value) => SqlValue::Signed(value),
        DbValue::U64(value) => SqlValue::Unsigned(value),
        DbValue::F64(value) => SqlValue::Float(value),
        DbValue::Bytes(value) => SqlValue::Bytes(value),
        DbValue::String(value) => SqlValue::Text(value),
    }
}

/// 快速构造仅含文本单元格的查询结果，供脚本化场景复用。
fn query_rows(rows: &[&[&str]]) -> QueryResult {
    QueryResult {
        columns: Vec::new(),
        rows: rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|value| SqlValue::Text((*value).to_owned()))
                    .collect()
            })
            .collect(),
    }
}

// 状态端口测试使用临时监听器，直接校验 Go HTTP 客户端兼容的请求与响应语义。
#[test]
fn status_url_matches_go_literal_path_concatenation() {
    let client = TestServerClient::new();
    assert_eq!(client.status_url("/status"), "http://localhost:0/status");
    assert_eq!(client.status_url("status"), "http://localhost:0status");
    assert!(client.fetch_status("status").is_err());
}

#[test]
fn fetch_status_decodes_http_chunked_body_like_go_http_client() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind status listener");
    let port = listener.local_addr().expect("listener address").port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept status request");
        let mut request = [0_u8; 1024];
        let read = stream.read(&mut request).expect("read status request");
        assert!(String::from_utf8_lossy(&request[..read]).starts_with("GET /status HTTP/1.1\r\n"));
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
                  5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n",
            )
            .expect("write status response");
    });

    let mut client = TestServerClient::new();
    client.host = "127.0.0.1".into();
    client.status_port = port;
    let response = client.fetch_status("/status").expect("fetch status");
    assert_eq!(response.body, b"hello world");
    server.join().expect("join status server");
}

#[test]
fn form_status_matches_go_sorted_query_encoding() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind form listener");
    let port = listener.local_addr().expect("listener address").port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept form request");
        let mut request = Vec::new();
        stream.read_to_end(&mut request).expect("read form request");
        let request = String::from_utf8(request).expect("UTF-8 request");
        assert!(request.starts_with("POST /settings HTTP/1.1\r\n"));
        assert!(request.contains("Content-Type: application/x-www-form-urlencoded\r\n"));
        assert!(request.ends_with("\r\n\r\na=first+value&z=last%2Fvalue"));
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
            .expect("write form response");
    });

    let mut client = TestServerClient::new();
    client.host = "127.0.0.1".into();
    client.status_port = port;
    let response = client
        .form_status(
            "/settings",
            &HashMap::from([
                ("z".into(), "last/value".into()),
                ("a".into(), "first value".into()),
            ]),
        )
        .expect("post form");
    assert_eq!(response.status, 204);
    server.join().expect("join form server");
}

#[test]
fn status_api_requires_exact_version_and_git_hash() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind status listener");
    let port = listener.local_addr().expect("listener address").port();
    let version = astersql_parser_mysql::r#const::ServerVersion();
    let git_hash = *astersql_util_versioninfo::TiDBGitHash
        .read()
        .expect("read git hash");
    let body = format!(r#"{{"version":"{version}","git_hash":"{git_hash}"}}"#);
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept status request");
        let mut request = [0_u8; 1024];
        stream.read(&mut request).expect("read status request");
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("write status response");
    });

    let mut client = TestServerClient::new();
    client.host = "127.0.0.1".into();
    client.status_port = port;
    client.run_test_status_api().expect("status API matches");
    server.join().expect("join status server");
}

// 在线判定必须先连通 MySQL 端口，再探测状态端口，顺序与 Go 实现一致。
#[test]
fn wait_until_online_requires_mysql_then_status_like_go() {
    let mysql_listener = TcpListener::bind("127.0.0.1:0").expect("bind mysql listener");
    let mysql_port = mysql_listener.local_addr().expect("mysql address").port();
    let status_listener = TcpListener::bind("127.0.0.1:0").expect("bind status listener");
    let status_port = status_listener.local_addr().expect("status address").port();

    let mysql = thread::spawn(move || {
        let (_stream, _) = mysql_listener.accept().expect("accept mysql probe");
    });
    let status = thread::spawn(move || {
        let (mut stream, _) = status_listener.accept().expect("accept status probe");
        let mut request = [0_u8; 1024];
        stream.read(&mut request).expect("read status probe");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
            .expect("write status response");
    });

    let mut client = TestServerClient::new();
    client.host = "127.0.0.1".into();
    client.port = mysql_port;
    client.status_port = status_port;
    client
        .wait_until_server_online(Duration::from_secs(1))
        .expect("both listeners become available");
    mysql.join().expect("join mysql listener");
    status.join().expect("join status listener");
}

#[test]
fn wait_until_online_does_not_accept_status_without_mysql() {
    let status_listener = TcpListener::bind("127.0.0.1:0").expect("bind status listener");
    let status_port = status_listener.local_addr().expect("status address").port();
    let missing_mysql = TcpListener::bind("127.0.0.1:0").expect("reserve missing mysql port");
    let mysql_port = missing_mysql.local_addr().expect("mysql address").port();
    drop(missing_mysql);

    let mut client = TestServerClient::new();
    client.host = "127.0.0.1".into();
    client.port = mysql_port;
    client.status_port = status_port;
    client.connect_timeout = Duration::from_millis(10);
    client.retry_interval = Duration::from_millis(1);
    let error = client
        .wait_until_server_online(Duration::from_millis(25))
        .expect_err("status alone is not online");
    assert!(error.contains("server did not accept connections"));
    drop(status_listener);
}

// DSN 测试固定参数转义、覆盖顺序和 nil 覆盖器等 Go 驱动兼容约束。
#[test]
fn mysql_dsn_uses_go_query_escape_for_parameters() {
    let config = MysqlConfig {
        user: "root".into(),
        password: String::new(),
        network: "tcp".into(),
        address: "127.0.0.1:4000".into(),
        database: "test".into(),
        parameters: HashMap::from([("sql_mode".into(), "STRICT ALL".into())]),
        allow_all_files: false,
        collation: "utf8mb4_general_ci".into(),
        max_allowed_packet: 64 << 20,
        multi_statements: false,
        tls_config: String::new(),
        fetch_size: None,
    };
    assert_eq!(
        config.format_dsn(),
        "root@tcp(127.0.0.1:4000)/test?sql_mode=STRICT+ALL"
    );
}

#[test]
fn mysql_dsn_exposes_go_scenario_overrides_in_driver_order() {
    let mut client = TestServerClient::new();
    client.port = 4000;
    let override_config = |config: &mut MysqlConfig| {
        config.database = "db/name".into();
        config.allow_all_files = true;
        config.collation = "utf8mb4_bin".into();
        config.multi_statements = true;
        config.tls_config = "skip-verify".into();
        config.max_allowed_packet = 1024;
        config.parameters.insert("sql_mode".into(), "''".into());
    };
    assert_eq!(
        client.get_dsn(&[&override_config]),
        "root@tcp(127.0.0.1:4000)/db%2Fname?allowAllFiles=true&collation=utf8mb4_bin&\
         multiStatements=true&tls=skip-verify&maxAllowedPacket=1024&sql_mode=%27%27"
    );
}

#[test]
fn mysql_dsn_ignores_nil_overrider_like_go() {
    let mut client = TestServerClient::new();
    client.port = 4000;
    let set_user = |config: &mut MysqlConfig| config.user = "alice".into();
    assert_eq!(
        client.get_dsn_optional(&[None, Some(&set_user), None]),
        "alice@tcp(127.0.0.1:4000)/test"
    );
}

// 场景清单从 Go 源码提取，防止 Rust 迁移时遗漏或打乱 RunTest 入口。
#[test]
fn go_run_test_inventory_is_complete_and_ordered() {
    let go = include_str!("server_client.go");
    let discovered = go
        .lines()
        .filter_map(|line| {
            let marker = "func (cli *TestServerClient) RunTest";
            let suffix = line.strip_prefix(marker)?;
            let name_suffix = suffix.split_once('(')?.0;
            if !name_suffix.chars().next().is_some_and(char::is_uppercase) {
                return None;
            }
            Some(format!("RunTest{name_suffix}"))
        })
        .collect::<Vec<_>>();
    assert_eq!(discovered.len(), 41);
    assert_eq!(
        discovered,
        GO_SCENARIO_NAMES
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>()
    );
}

// 新数据库场景重点验证建库、逐场景清表和无条件删库的生命周期顺序。
#[test]
fn run_tests_on_new_database_matches_go_statement_order_and_cleanup() {
    let client = TestServerClient::new();
    let mut executor = RecordingExecutor::default();
    let mut first = |db: &mut TestDatabase<'_>| {
        db.must_execute("SELECT 1", &[])?;
        Ok(())
    };
    let mut second = |db: &mut TestDatabase<'_>| {
        db.must_execute("SELECT 2", &[])?;
        Ok(())
    };
    client
        .run_tests_on_new_database(&mut executor, "db-name", &mut [&mut first, &mut second])
        .expect("scenario succeeds");
    assert_eq!(
        executor.statements,
        [
            "DROP DATABASE IF EXISTS `db-name`;",
            "CREATE DATABASE `db-name`;",
            "USE `db-name`;",
            "SELECT 1",
            "DROP TABLE IF EXISTS test",
            "SELECT 2",
            "DROP TABLE IF EXISTS test",
            "DROP DATABASE IF EXISTS `db-name`;",
        ]
    );
}

#[test]
fn run_tests_on_new_database_always_drops_database_after_body_error() {
    let client = TestServerClient::new();
    let mut executor = RecordingExecutor::default();
    let mut failing = |_db: &mut TestDatabase<'_>| Err("body failed".to_owned());
    let error = client
        .run_tests_on_new_database(&mut executor, "failing-db", &mut [&mut failing])
        .expect_err("body failure propagates");
    assert_eq!(error, "body failed");
    assert_eq!(
        executor.statements,
        [
            "DROP DATABASE IF EXISTS `failing-db`;",
            "CREATE DATABASE `failing-db`;",
            "USE `failing-db`;",
            "DROP DATABASE IF EXISTS `failing-db`;",
        ]
    );
}

#[test]
fn run_tests_stops_at_first_error() {
    let client = TestServerClient::new();
    let mut executor = RecordingExecutor::default();
    let mut first = |_db: &mut TestDatabase<'_>| Err("stop".to_owned());
    let mut unreachable = |db: &mut TestDatabase<'_>| {
        db.must_execute("SHOULD NOT RUN", &[])?;
        Ok(())
    };
    assert_eq!(
        client
            .run_tests(&mut executor, &mut [&mut first, &mut unreachable])
            .expect_err("first error stops execution"),
        "stop"
    );
    assert!(executor.statements.is_empty());
}

#[test]
fn row_and_column_helpers_preserve_go_null_and_order_semantics() {
    let result = QueryResult {
        columns: vec!["a".into(), "b".into(), "c".into()],
        rows: vec![vec![
            SqlValue::Null,
            SqlValue::Bytes(b"bytes".to_vec()),
            SqlValue::Bool(true),
        ]],
    };
    assert_eq!(
        super::server_client::rows(&result),
        vec![vec!["<nil>", "bytes", "1"]]
    );
    super::server_client::check_rows(&result, &[&["<nil>", "bytes", "1"]]).expect("rows match");
    assert!(super::server_client::columns_as_expected(
        &result.columns,
        &["a", "b", "c"]
    ));
    assert!(!super::server_client::columns_as_expected(
        &result.columns,
        &["b", "a", "c"]
    ));
}

#[test]
fn prepare_load_data_rows_truncates_and_tab_separates_like_go() {
    let client = TestServerClient::new();
    let path = std::env::temp_dir().join(format!(
        "astersql-task-385-{}-{}.csv",
        std::process::id(),
        std::thread::current().name().unwrap_or("unnamed")
    ));
    std::fs::write(&path, b"stale trailing contents").expect("seed file");
    client
        .prepare_load_data_rows(&path, &["1 a", "2 b c"])
        .expect("rewrite load-data rows");
    assert_eq!(
        std::fs::read(&path).expect("read rewritten file"),
        b"1\ta\n2\tb\tc\n"
    );
    std::fs::remove_file(path).expect("remove task file");
}

#[test]
fn tls_and_reload_entrypoints_execute_exact_go_statements() {
    let client = TestServerClient::new();
    let mut executor = RecordingExecutor::default();
    client
        .run_test_tls_connection(&mut executor)
        .expect("TLS connection SQL");
    client
        .run_test_enable_secure_transport(&mut executor)
        .expect("secure transport SQL");
    client
        .run_reload_tls(&mut executor, false)
        .expect("reload TLS");
    client
        .run_reload_tls(&mut executor, true)
        .expect("reload TLS without rollback");
    assert_eq!(
        executor.statements,
        [
            "USE test",
            "SET GLOBAL require_secure_transport = 1",
            "alter instance reload tls",
            "alter instance reload tls no rollback on error",
        ]
    );
}

#[test]
fn multi_statement_scenario_preserves_go_order_and_result_assertions() {
    let client = TestServerClient::new();
    let mut executor = RecordingExecutor {
        execute_results: VecDeque::from([
            ExecuteResult::default(),
            ExecuteResult {
                rows_affected: 1,
                last_insert_id: 0,
            },
            ExecuteResult {
                rows_affected: 1,
                last_insert_id: 0,
            },
        ]),
        query_results: VecDeque::from([
            QueryResult {
                columns: vec!["value".into()],
                rows: vec![vec![SqlValue::Signed(5)]],
            },
            QueryResult {
                columns: vec!["database".into()],
                rows: vec![vec![SqlValue::Text("success".into())]],
            },
            QueryResult::default(),
            QueryResult::default(),
        ]),
        ..RecordingExecutor::default()
    };
    client
        .run_test_multi_statements(&mut executor)
        .expect("multi-statement Go scenario");
    assert_eq!(
        executor.statements.last().map(String::as_str),
        Some("commit;")
    );
    assert!(executor.statements.iter().any(|sql| {
        sql == "use multistmtuse; create table if not exists t1 (id int); drop table t1;"
    }));
}

#[test]
fn metric_helpers_match_go_labeled_samples() {
    let metrics = HashMap::from([
        (
            "tidb_executor_statement_total{db=\"\",resource_group=\"default\",type=\"Select\"}"
                .into(),
            7.0,
        ),
        (
            "tidb_executor_statement_total{db=\"app\",resource_group=\"default\",type=\"Insert\"}"
                .into(),
            3.0,
        ),
    ]);
    assert_eq!(get_statement_count(&metrics, "Select"), 7.0);
    assert_eq!(get_database_statement_count(&metrics, "app", "Insert"), 3.0);
}

#[test]
fn load_data_select_into_outfile_round_trips_all_rows() {
    let expected = query_rows(&[
        &[
            "<nil>",
            "<nil>",
            "<nil>",
            "<nil>",
            "2000-03-03",
            "03:03:03",
            "[1,2,3]",
        ],
        &["1", "1.1", "0.1", "a", "2000-01-01", "01:01:01", "[1]"],
        &["2", "2.2", "0.2", "b", "2000-02-02", "02:02:02", "[1,2]"],
        &["4", "4.4", "0.4", "d", "<nil>", "<nil>", "<nil>"],
    ]);
    let mut executor = RecordingExecutor {
        query_results: VecDeque::from([expected.clone(), expected]),
        ..RecordingExecutor::default()
    };
    TestServerClient::new()
        .run_test_load_data_with_select_into_outfile(&mut executor)
        .expect("SELECT INTO OUTFILE round trip");
    assert!(
        executor
            .statements
            .iter()
            .any(|sql| sql.starts_with("select * from t into outfile "))
    );
    assert!(
        executor
            .statements
            .iter()
            .any(|sql| sql.starts_with("load data local infile "))
    );
}

#[test]
fn load_data_go_scenarios_execute_against_canonical_session() {
    let _guard = REAL_SQL_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut executor = RealSqlExecutor::new();
    TestServerClient::new()
        .run_test_load_data(&mut executor)
        .expect("full LOAD DATA scenarios");
}

#[test]
fn load_data_transaction_scenarios_execute_against_shared_sessions() {
    let _guard = REAL_SQL_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut executor = RealSqlExecutor::new();
    TestServerClient::new()
        .run_test_load_data_in_transaction(&mut executor)
        .expect("LOAD DATA transaction scenarios");
}

#[test]
fn load_data_slow_log_checks_load_and_followup_insert_plans() {
    let mut executor = RecordingExecutor {
        query_results: VecDeque::from([
            query_rows(&[&["LoadData total_time: 1 loops: 1 commit_txn: 1"]]),
            query_rows(&[&["LoadData total_time: 1 loops: 1 commit_txn: 1"]]),
            query_rows(&[&[
                "Insert time: 1 loops: 1 prepare: 1 check_insert: 1 mem_insert_time: 1 prefetch: 1 rpc: 1",
            ]]),
        ]),
        ..RecordingExecutor::default()
    };
    TestServerClient::new()
        .run_test_load_data_for_slow_log(&mut executor)
        .expect("LOAD DATA slow-log plans");
    assert!(executor.statements.ends_with(&[
        "set tidb_slow_log_threshold=300;".to_owned(),
        "set @@global.tidb_enable_stmt_summary=0".to_owned(),
        "DROP TABLE IF EXISTS test".to_owned(),
        "DROP DATABASE IF EXISTS `load_data_slow_query`;".to_owned(),
    ]));
}

#[test]
fn load_data_auto_random_checks_count_and_xor_checksum() {
    let mut executor = RecordingExecutor {
        query_results: VecDeque::from([query_rows(&[&["1000"]]), query_rows(&[&["677", "580"]])]),
        ..RecordingExecutor::default()
    };
    TestServerClient::new()
        .run_test_load_data_auto_random(&mut executor)
        .expect("auto-random LOAD DATA");
    assert!(
        executor
            .statements
            .iter()
            .any(|sql| { sql.contains("into table t (c2, c3) with batch_size = 128, thread=1") })
    );
}

#[test]
fn load_data_auto_random_special_terminators_check_count_and_checksum() {
    let mut executor = RecordingExecutor {
        query_results: VecDeque::from([query_rows(&[&["5000"]]), query_rows(&[&["215", "428"]])]),
        ..RecordingExecutor::default()
    };
    TestServerClient::new()
        .run_test_load_data_auto_random_with_special_term(&mut executor)
        .expect("special-terminator auto-random LOAD DATA");
    assert!(executor.statements.iter().any(|sql| {
        sql.contains("fields terminated by ',' enclosed by '\\'' lines terminated by '|'")
    }));
}

/// 构造 LIST 分区 LOAD DATA 场景共用的逐阶段行集与 warning 队列。
fn list_partition_results(duplicate_keys: &[&str], no_partition: &str) -> VecDeque<QueryResult> {
    let warning_rows = duplicate_keys
        .iter()
        .map(|warning| {
            vec![
                SqlValue::Text("Warning".into()),
                SqlValue::Text("1062".into()),
                SqlValue::Text((*warning).into()),
            ]
        })
        .collect();
    VecDeque::from([
        query_rows(&[&["1", "a"], &["2", "b"]]),
        query_rows(&[&["1", "a"], &["3", "c"], &["4", "e"]]),
        QueryResult {
            columns: Vec::new(),
            rows: warning_rows,
        },
        query_rows(&[
            &["1", "a"],
            &["2", "b"],
            &["3", "c"],
            &["4", "e"],
            &["7", "a"],
        ]),
        query_rows(&[&["Warning", "1526", no_partition]]),
        query_rows(&[
            &["1", "a"],
            &["2", "b"],
            &["3", "c"],
            &["4", "e"],
            &["5", "a"],
            &["7", "a"],
        ]),
    ])
}

#[test]
fn load_data_list_partition_preserves_rows_and_warnings() {
    let mut executor = RecordingExecutor {
        query_results: list_partition_results(
            &[
                "Duplicate entry '1' for key 't.idx'",
                "Duplicate entry '2' for key 't.idx'",
            ],
            "Table has no partition for value 100",
        ),
        ..RecordingExecutor::default()
    };
    TestServerClient::new()
        .run_test_load_data_for_list_partition(&mut executor)
        .expect("LIST partition LOAD DATA");
}

#[test]
fn load_data_list_partition_generated_column_preserves_rows_and_warnings() {
    let mut executor = RecordingExecutor {
        query_results: list_partition_results(
            &[
                "Duplicate entry '1-2' for key 't.idx'",
                "Duplicate entry '2-2' for key 't.idx'",
            ],
            "Table has no partition for value 100",
        ),
        ..RecordingExecutor::default()
    };
    TestServerClient::new()
        .run_test_load_data_for_list_partition_2(&mut executor)
        .expect("LIST expression partition LOAD DATA");
}

#[test]
fn load_data_list_columns_partition_preserves_rows_and_warnings() {
    let mut executor = RecordingExecutor {
        query_results: list_partition_results(
            &[
                "Duplicate entry '1' for key 't.idx'",
                "Duplicate entry '2' for key 't.idx'",
            ],
            "Table has no partition for value from column_list",
        ),
        ..RecordingExecutor::default()
    };
    TestServerClient::new()
        .run_test_load_data_for_list_column_partition(&mut executor)
        .expect("LIST COLUMNS partition LOAD DATA");
}

#[test]
fn load_data_multi_column_list_partition_preserves_rows_and_warnings() {
    let mut executor = RecordingExecutor {
        query_results: VecDeque::from([
            query_rows(&[&["w", "1", "1"], &["w", "2", "2"]]),
            query_rows(&[&["w", "1", "1"], &["e", "5", "5"], &["n", "9", "9"]]),
            query_rows(&[&["Warning", "1062", "Duplicate entry 'w-1' for key 't.idx'"]]),
            query_rows(&[
                &["w", "1", "1"],
                &["w", "2", "2"],
                &["e", "5", "5"],
                &["n", "9", "9"],
            ]),
            query_rows(&[&[
                "Warning",
                "1526",
                "Table has no partition for value from column_list",
            ]]),
            query_rows(&[
                &[
                    "Warning",
                    "1526",
                    "Table has no partition for value from column_list",
                ],
                &["Warning", "1062", "Duplicate entry 'w-1' for key 't.idx'"],
            ]),
            query_rows(&[
                &["w", "1", "1"],
                &["w", "2", "2"],
                &["w", "3", "3"],
                &["e", "5", "5"],
                &["e", "8", "8"],
                &["n", "9", "9"],
            ]),
        ]),
        ..RecordingExecutor::default()
    };
    TestServerClient::new()
        .run_test_load_data_for_list_column_partition_2(&mut executor)
        .expect("multi-column LIST COLUMNS partition LOAD DATA");
}

#[test]
fn load_data_column_lists_cover_full_partial_case_insensitive_and_unknown_names() {
    let full = query_rows(&[
        &["1", "1", "", "2022-04-19", "a", "2022-04-19 00:00:01"],
        &["2", "1", "a", "2022-04-19", "a", "2022-04-19 00:00:01"],
        &["3", "1", "a", "2022-04-19", "a", "2022-04-19 00:00:01"],
        &["4", "1", "a", "2022-04-19", "a", "2022-04-19 00:00:01"],
    ]);
    let partial = query_rows(&[
        &["1", "1", "", "<nil>", "<nil>", "<nil>"],
        &["2", "1", "a", "<nil>", "<nil>", "<nil>"],
        &["3", "1", "a", "<nil>", "<nil>", "<nil>"],
        &["4", "1", "a", "<nil>", "<nil>", "<nil>"],
    ]);
    let mut executor = RecordingExecutor {
        query_results: VecDeque::from([full.clone(), partial, full]),
        fail_on_contains: Some((
            "(c1, c2)".to_owned(),
            "Error 1054 (42S22): Unknown column 'c2' in 'field list'".to_owned(),
        )),
        ..RecordingExecutor::default()
    };
    TestServerClient::new()
        .run_test_load_data_with_column_list(&mut executor)
        .expect("LOAD DATA column-list variants");
}

#[test]
fn explain_for_connection_checks_point_get_plan_shape() {
    let mut executor = RecordingExecutor {
        query_results: VecDeque::from([
            query_rows(&[&["42"]]),
            QueryResult::default(),
            query_rows(&[&[
                "Point_Get_1",
                "1.00",
                "1",
                "root",
                "table:t",
                "time:1, loops:1",
                "",
                "handle:1",
                "",
            ]]),
        ]),
        ..RecordingExecutor::default()
    };
    TestServerClient::new()
        .run_test_explain_for_conn(&mut executor)
        .expect("EXPLAIN FOR CONNECTION");
    assert!(
        executor
            .statements
            .iter()
            .any(|sql| sql == "explain for connection 42")
    );
}

#[test]
fn error_code_scenario_checks_every_mysql_number() {
    let cases = [
        ("commit", 1062),
        ("use db_not_exists;", 1049),
        ("select * from tbl_not_exists;", 1146),
        ("create database test;", 1007),
        (
            "create database aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;",
            1059,
        ),
        ("create table test (c int);", 1050),
        ("drop table unknown_table;", 1051),
        ("drop database unknown_db;", 1008),
        (
            "create table aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa (a int);",
            1059,
        ),
        (
            "create table long_column_table (aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa int);",
            1059,
        ),
        (
            "alter table test add aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa int;",
            1059,
        ),
        ("select *, * from test;", 1166),
        ("select row(1, 2) > 1;", 1241),
        ("select * from test order by row(c, c);", 1241),
        ("select @@unknown_sys_var;", 1193),
        ("set @@unknown_sys_var='1';", 1193),
        ("select greatest(2);", 1582),
    ];
    let mut executor = RecordingExecutor {
        errors_by_statement: cases
            .into_iter()
            .map(|(sql, code)| (sql.into(), format!("Error {code} (HY000): expected")))
            .collect(),
        ..RecordingExecutor::default()
    };
    TestServerClient::new()
        .run_test_error_code(&mut executor)
        .expect("all Go error numbers");
}

#[test]
fn auth_uses_independent_configs_and_loads_default_role() {
    let role_log = Arc::new(Mutex::new(Vec::new()));
    let mut connector = RecordingConnector {
        scripts: VecDeque::from([
            Ok(ProtocolScript::default()),
            Err("Error 1045 (28000): wrong password".into()),
            Ok(ProtocolScript {
                statements: role_log.clone(),
                query_results: VecDeque::from([query_rows(&[&["`authtest_r1`@`%`"]])]),
                ..ProtocolScript::default()
            }),
            Ok(ProtocolScript::default()),
        ]),
        ..RecordingConnector::default()
    };
    let mut admin = RecordingExecutor::default();
    TestServerClient::new()
        .run_test_auth(&mut admin, &mut connector)
        .expect("auth variants");
    assert_eq!(
        connector
            .configs
            .iter()
            .map(|config| (config.user.as_str(), config.password.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("authtest", "123"),
            ("authtest", "456"),
            ("authtest", "123"),
            ("authtest2", "123"),
        ]
    );
    assert!(
        role_log
            .lock()
            .expect("role log")
            .contains(&"select current_role;".to_owned())
    );
}

#[test]
fn issue_3662_and_3680_preserve_exact_ping_errors() {
    let mut missing_db = RecordingConnector {
        scripts: VecDeque::from([Err(
            "Error 1049 (42000): Unknown database 'non_existing_schema'".into(),
        )]),
        ..RecordingConnector::default()
    };
    TestServerClient::new()
        .run_test_issue_3662(&mut missing_db)
        .expect("unknown DB error");
    let mut missing_user = RecordingConnector {
        scripts: VecDeque::from([Err(
            "Error 1045 (28000): Access denied for user 'non_existing_user'@'127.0.0.1' (using password: NO)".into(),
        )]),
        ..RecordingConnector::default()
    };
    TestServerClient::new()
        .run_test_issue_3680(&mut missing_user)
        .expect("unknown user error");
}

#[test]
fn issue_3682_reports_auth_before_unknown_database() {
    let mut connector = RecordingConnector {
        scripts: VecDeque::from([
            Ok(ProtocolScript::default()),
            Err("Error 1045 (28000): Access denied for user 'issue3682'@'127.0.0.1' (using password: YES)".into()),
        ]),
        ..RecordingConnector::default()
    };
    TestServerClient::new()
        .run_test_issue_3682(&mut RecordingExecutor::default(), &mut connector)
        .expect("authentication precedence");
    assert_eq!(connector.configs[1].database, "non_existing_schema");
}

#[test]
fn db_statement_count_remains_explicitly_skipped_like_go() {
    let mut executor = RecordingExecutor::default();
    TestServerClient::new()
        .run_test_db_stmt_count(&mut executor)
        .expect("Go scenario is skipped");
    assert!(executor.statements.is_empty());
}

#[test]
fn init_connect_distinguishes_super_and_non_super_sessions() {
    let mut connector = RecordingConnector {
        scripts: VecDeque::from([
            Ok(ProtocolScript {
                query_results: VecDeque::from([query_rows(&[&["1"]])]),
                ..ProtocolScript::default()
            }),
            Ok(ProtocolScript {
                query_results: VecDeque::from([query_rows(&[&[""]])]),
                ..ProtocolScript::default()
            }),
            Ok(ProtocolScript::default()),
        ]),
        ..RecordingConnector::default()
    };
    let mut admin = RecordingExecutor::default();
    TestServerClient::new()
        .run_test_init_connect(&mut admin, &mut connector)
        .expect("init_connect users");
    assert_eq!(
        connector
            .configs
            .iter()
            .map(|config| config.user.as_str())
            .collect::<Vec<_>>(),
        vec!["init_nonsuper", "init_super", "init_super"]
    );
    assert_eq!(
        admin.statements.first().map(String::as_str),
        Some("SET GLOBAL init_connect=\"insert into test.ts VALUES (NOW());SET @a=1;\"")
    );
}

#[test]
fn sql_mode_is_loaded_before_first_query_on_new_connection() {
    let first_log = Arc::new(Mutex::new(Vec::new()));
    let second_log = Arc::new(Mutex::new(Vec::new()));
    let mut connector = RecordingConnector {
        scripts: VecDeque::from([
            Ok(ProtocolScript {
                statements: first_log,
                ..ProtocolScript::default()
            }),
            Ok(ProtocolScript {
                statements: second_log.clone(),
                query_results: VecDeque::from([query_rows(&[&["ab\\\\c"]])]),
                ..ProtocolScript::default()
            }),
        ]),
        ..RecordingConnector::default()
    };
    TestServerClient::new()
        .run_test_sql_mode_is_loaded_before_query(&mut connector)
        .expect("new connection SQL mode");
    let second = second_log.lock().expect("second log");
    assert_eq!(
        second.as_slice(),
        [
            "insert into t1 values (1, 'ab\\\\c');",
            "select t from t1 where id = 1;"
        ]
    );
}

/// 生成 Prometheus 文本格式的语句计数样本，供请求前后差值测试使用。
fn statement_metrics(values: &[(&str, usize)]) -> String {
    values
        .iter()
        .map(|(kind, value)| {
            format!(
                "tidb_executor_statement_total{{db=\"\",resource_group=\"default\",type=\"{kind}\"}} {value}\n"
            )
        })
        .collect()
}

#[test]
fn statement_count_executes_go_workload_and_checks_metric_deltas() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind metrics listener");
    let port = listener.local_addr().expect("metrics address").port();
    let before = statement_metrics(&[
        ("CreateTable", 10),
        ("Insert", 20),
        ("Delete", 30),
        ("Update", 40),
        ("Select", 50),
        ("Prepare", 60),
        ("Execute", 70),
        ("Replace", 80),
    ]);
    let after = statement_metrics(&[
        ("CreateTable", 11),
        ("Insert", 25),
        ("Delete", 31),
        ("Update", 42),
        ("Select", 53),
        ("Prepare", 62),
        ("Execute", 70),
        ("Replace", 81),
    ]);
    let server = thread::spawn(move || {
        for body in [before, after] {
            let (mut stream, _) = listener.accept().expect("accept metrics request");
            let mut request = [0_u8; 1024];
            stream.read(&mut request).expect("read metrics request");
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write metrics response");
        }
    });
    let mut client = TestServerClient::new();
    client.host = "127.0.0.1".into();
    client.status_port = port;
    let mut executor = RecordingExecutor::default();
    client
        .run_test_stmt_count(&mut executor)
        .expect("statement count deltas");
    assert!(
        executor
            .statements
            .contains(&"prepare stmt2 from 'select * from test'".to_owned())
    );
    server.join().expect("join metrics server");
}

#[test]
fn infoschema_client_errors_increment_each_global_user_and_host_counter() {
    let mut results = VecDeque::new();
    for source_index in 0..3 {
        let _ = source_index;
        results.push_back(QueryResult::default());
        results.push_back(QueryResult::default());
        results.push_back(query_rows(&[&["0", "1"]]));
    }
    for _ in 0..3 {
        results.push_back(QueryResult::default());
        results.push_back(query_rows(&[&["1", "1"]]));
    }
    for _ in 0..3 {
        results.push_back(QueryResult::default());
        results.push_back(query_rows(&[&["1", "1"]]));
    }
    let mut executor = RecordingExecutor {
        query_results: results,
        errors_by_statement: HashMap::from([
            (
                "CREATE TABLE test_client_errors2 (a int primary key, b int primary key)".into(),
                "Error 1068 (42000): Multiple primary key defined".into(),
            ),
            (
                "gibberish".into(),
                "Error 1064 (42000): syntax error".into(),
            ),
        ]),
        ..RecordingExecutor::default()
    };
    TestServerClient::new()
        .run_test_infoschema_client_errors(&mut executor)
        .expect("client errors counters");
    assert!(
        executor
            .statements
            .contains(&"set @@tidb_enable_cache_prepare_stmt = default".to_owned())
    );
}

#[test]
fn send_long_data_preserves_json_and_gbk_payload_bytes() {
    let json = format!("\"{}\"", "a".repeat(1024));
    let gbk = [0xc4, 0xe3, 0xba, 0xc3].repeat(1024);
    let mut connection = ProtocolScript {
        query_results: VecDeque::from([
            QueryResult {
                columns: Vec::new(),
                rows: vec![vec![SqlValue::Text(json)]],
            },
            QueryResult {
                columns: Vec::new(),
                rows: vec![vec![SqlValue::Bytes(gbk)]],
            },
        ]),
        ..ProtocolScript::default()
    };
    TestServerClient::new()
        .run_test_type_and_charset_of_send_long_data(&mut connection)
        .expect("long-data type and charset");
    assert!(
        connection
            .protocol_events
            .iter()
            .any(|event| event == "long-data:1:0:1026")
    );
    assert!(
        connection
            .protocol_events
            .iter()
            .any(|event| event == "long-data:1:0:4096")
    );
}

/// DDL 状态切换场景的统一查询结果。
fn schema_state_result() -> QueryResult {
    query_rows(&[
        &["1", "a", "101", "x", "<nil>"],
        &["2", "b", "102", "z", "<nil>"],
    ])
}

#[test]
fn issues_53634_and_54254_execute_transaction_at_write_reorganization() {
    for (drop_or_add, method) in [
        (
            "alter table stock drop column cct_1",
            TestServerClient::run_test_issue_53634
                as fn(
                    &TestServerClient,
                    &mut dyn SqlExecutor,
                    &mut dyn SchemaStateController,
                ) -> Result<(), String>,
        ),
        (
            "alter table stock add column cct_1 int",
            TestServerClient::run_test_issue_54254,
        ),
    ] {
        let mut executor = RecordingExecutor {
            query_results: VecDeque::from([schema_state_result()]),
            ..RecordingExecutor::default()
        };
        let mut state = RecordingSchemaState::default();
        method(&TestServerClient::new(), &mut executor, &mut state)
            .expect("schema-state transaction");
        assert_eq!(state.ddls, [drop_or_add]);
        assert_eq!(executor.parameterized_statements.len(), 3);
        assert!(executor.statements.contains(&"commit".to_owned()));
    }
}

// 这些场景必须经过真实解析、规划和执行链；全局锁避免规范 mock store 的共享状态串扰。
macro_rules! real_sql_scenario_test {
    ($name:ident, $method:ident) => {
        #[test]
        fn $name() {
            let _guard = REAL_SQL_TEST_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let client = TestServerClient::new();
            let mut executor = RealSqlExecutor::new();
            client
                .$method(&mut executor)
                .expect(concat!(stringify!($method), " succeeds"));
        }
    };
}

static REAL_SQL_TEST_LOCK: Mutex<()> = Mutex::new(());

real_sql_scenario_test!(
    regression_scenario_executes_against_canonical_store,
    run_test_regression
);
real_sql_scenario_test!(
    prepared_result_type_executes_against_canonical_store,
    run_test_prepare_result_field_type
);
real_sql_scenario_test!(
    special_types_execute_against_canonical_store,
    run_test_special_type
);
real_sql_scenario_test!(
    prepared_strings_execute_against_canonical_store,
    run_test_prepared_string
);
real_sql_scenario_test!(
    prepared_timestamps_execute_against_canonical_store,
    run_test_prepared_timestamp
);
real_sql_scenario_test!(
    sum_and_average_execute_against_canonical_store,
    run_test_sum_avg
);
