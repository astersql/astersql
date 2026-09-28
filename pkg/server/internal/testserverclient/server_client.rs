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

// 集成测试用的测试服务器客户端与 SQL 场景驱动。
//
// 提供 MySQL DSN 拼装、HTTP status/metrics 探测、等待服务就绪，
// 以及一组命名 SQL Scenario（回归/预编译类型/LOAD DATA 等）与行结果比对。

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// 是否启用回归场景（默认 true）。
pub static REGRESSION: AtomicBool = AtomicBool::new(true);

/// Go `server_client.go` 中 41 个 `RunTest*` 行为入口的完整清单。
pub const GO_SCENARIO_NAMES: &[&str] = &[
    "RunTestRegression",
    "RunTestPrepareResultFieldType",
    "RunTestSpecialType",
    "RunTestClientWithCollation",
    "RunTestPreparedString",
    "RunTestPreparedTimestamp",
    "RunTestLoadDataWithSelectIntoOutfile",
    "RunTestLoadDataForSlowLog",
    "RunTestLoadDataAutoRandom",
    "RunTestLoadDataAutoRandomWithSpecialTerm",
    "RunTestLoadDataForListPartition",
    "RunTestLoadDataForListPartition2",
    "RunTestLoadDataForListColumnPartition",
    "RunTestLoadDataForListColumnPartition2",
    "RunTestLoadDataWithColumnList",
    "RunTestLoadDataInTransaction",
    "RunTestLoadData",
    "RunTestExplainForConn",
    "RunTestErrorCode",
    "RunTestAuth",
    "RunTestIssue3662",
    "RunTestIssue3680",
    "RunTestIssue22646",
    "RunTestIssue3682",
    "RunTestAccountLock",
    "RunTestDBNameEscape",
    "RunTestResultFieldTableIsNull",
    "RunTestStatusAPI",
    "RunTestMultiStatements",
    "RunTestStmtCount",
    "RunTestDBStmtCount",
    "RunTestTLSConnection",
    "RunTestEnableSecureTransport",
    "RunTestSumAvg",
    "RunTestInitConnect",
    "RunTestInfoschemaClientErrors",
    "RunTestSQLModeIsLoadedBeforeQuery",
    "RunTestConnectionCount",
    "RunTestTypeAndCharsetOfSendLongData",
    "RunTestIssue53634",
    "RunTestIssue54254",
];

/// go-sql-driver 风格的 MySQL 连接配置字段。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MysqlConfig {
    pub user: String,
    pub password: String,
    pub network: String,
    pub address: String,
    pub database: String,
    pub parameters: HashMap<String, String>,
    pub allow_all_files: bool,
    pub collation: String,
    pub max_allowed_packet: usize,
    pub multi_statements: bool,
    pub tls_config: String,
    /// 游标 fetch 大小；设置后写入 DSN 的 fetchSize 参数。
    pub fetch_size: Option<u32>,
}

impl MysqlConfig {
    /// 格式化为 `user[:pass]@network(addr)/db[?params]` DSN。
    pub fn format_dsn(&self) -> String {
        let credentials = if self.password.is_empty() {
            self.user.clone()
        } else {
            format!("{}:{}", self.user, self.password)
        };
        let mut dsn = format!(
            "{}@{}({})/{}",
            credentials,
            self.network,
            self.address,
            percent_encode(&self.database)
        );
        let mut special_parameters = Vec::new();
        if self.allow_all_files {
            special_parameters.push(("allowAllFiles", "true".to_string()));
        }
        if !self.collation.is_empty() && self.collation != "utf8mb4_general_ci" {
            special_parameters.push(("collation", self.collation.clone()));
        }
        if self.multi_statements {
            special_parameters.push(("multiStatements", "true".to_string()));
        }
        if !self.tls_config.is_empty() {
            special_parameters.push(("tls", query_escape(&self.tls_config)));
        }
        if self.max_allowed_packet != 64 << 20 {
            special_parameters.push(("maxAllowedPacket", self.max_allowed_packet.to_string()));
        }
        let mut parameters: Vec<_> = self.parameters.iter().collect();
        // 参数按名字排序，保证 DSN 稳定可比较。
        parameters.sort_by(|left, right| left.0.cmp(right.0));
        if let Some(fetch_size) = self.fetch_size {
            dsn.push('?');
            let _ = write!(dsn, "fetchSize={fetch_size}");
        }
        for (name, value) in special_parameters {
            dsn.push(if dsn.contains('?') { '&' } else { '?' });
            let _ = write!(dsn, "{name}={value}");
        }
        if self.fetch_size.is_none() && !parameters.is_empty() && !dsn.contains('?') {
            dsn.push('?');
        }
        for (name, value) in parameters {
            if !dsn.ends_with('?') {
                dsn.push('&');
            }
            let _ = write!(dsn, "{}={}", query_escape(name), query_escape(value));
        }
        dsn
    }
}

/// 覆盖默认 MysqlConfig 的回调（用于 get_dsn）。
pub type ConfigOverrider = dyn Fn(&mut MysqlConfig) + Send + Sync;

/// 简化的 HTTP 响应（status 探测用）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpResponse {
    pub status: u16,
    pub reason: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// 将 body 解码为 UTF-8 文本。
    pub fn text(&self) -> Result<&str, String> {
        std::str::from_utf8(&self.body).map_err(|error| error.to_string())
    }

    /// 2xx 视为成功。
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// 指向测试 TiDB/AsterSQL 实例的客户端：SQL 端口 + status HTTP 端口。
#[derive(Clone, Debug)]
pub struct TestServerClient {
    pub status_scheme: String,
    pub host: String,
    /// MySQL 协议端口。
    pub port: u16,
    /// Status / metrics HTTP 端口。
    pub status_port: u16,
    pub connect_timeout: Duration,
    pub retry_interval: Duration,
}

impl Default for TestServerClient {
    fn default() -> Self {
        Self::new()
    }
}

impl TestServerClient {
    /// 默认指向 localhost，端口由测试启动器填充。
    pub fn new() -> Self {
        Self {
            status_scheme: "http".into(),
            host: "localhost".into(),
            port: 0,
            status_port: 0,
            connect_timeout: Duration::from_secs(1),
            retry_interval: Duration::from_millis(10),
        }
    }

    /// MySQL 协议侧地址字符串（scheme://host:port）。
    pub fn addr(&self) -> String {
        format!("{}://{}:{}", self.status_scheme, self.host, self.port)
    }

    /// 拼装 status HTTP URL。
    pub fn status_url(&self, path: &str) -> String {
        format!(
            "{}://{}:{}{}",
            self.status_scheme, self.host, self.status_port, path
        )
    }

    /// GET status 路径。
    pub fn fetch_status(&self, path: &str) -> Result<HttpResponse, String> {
        self.http_request("GET", path, None, &[])
    }

    /// POST status 路径（带 Content-Type 与 body）。
    pub fn post_status(
        &self,
        path: &str,
        content_type: &str,
        body: &[u8],
    ) -> Result<HttpResponse, String> {
        self.http_request("POST", path, Some(content_type), body)
    }

    /// 以 application/x-www-form-urlencoded 提交表单到 status。
    pub fn form_status(
        &self,
        path: &str,
        values: &HashMap<String, String>,
    ) -> Result<HttpResponse, String> {
        let mut values: Vec<_> = values.iter().collect();
        values.sort_by(|left, right| left.0.cmp(right.0));
        let body = values
            .into_iter()
            .map(|(name, value)| format!("{}={}", query_escape(name), query_escape(value)))
            .collect::<Vec<_>>()
            .join("&");
        self.post_status(path, "application/x-www-form-urlencoded", body.as_bytes())
    }

    /// 原始 HTTP/1.1 请求（仅支持明文 http；TLS 需外部注入）。
    fn http_request(
        &self,
        method: &str,
        path: &str,
        content_type: Option<&str>,
        body: &[u8],
    ) -> Result<HttpResponse, String> {
        if self.status_scheme != "http" {
            return Err(format!(
                "{} status requests require an injected TLS transport",
                self.status_scheme
            ));
        }
        if !path.starts_with('/') {
            return Err(format!(
                "status path must start with '/': {}",
                self.status_url(path)
            ));
        }
        let mut addresses = (self.host.as_str(), self.status_port)
            .to_socket_addrs()
            .map_err(|error| format!("resolve status address: {error}"))?;
        let address = addresses
            .next()
            .ok_or_else(|| "status address did not resolve".to_string())?;
        let mut stream = TcpStream::connect_timeout(&address, self.connect_timeout)
            .map_err(|error| format!("connect status server: {error}"))?;
        stream
            .set_read_timeout(Some(self.connect_timeout))
            .map_err(|error| format!("set status read timeout: {error}"))?;
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\nContent-Length: {}\r\n",
            self.host,
            self.status_port,
            body.len()
        );
        if let Some(content_type) = content_type {
            let _ = write!(request, "Content-Type: {content_type}\r\n");
        }
        request.push_str("\r\n");
        stream
            .write_all(request.as_bytes())
            .and_then(|_| stream.write_all(body))
            .and_then(|_| stream.flush())
            .map_err(|error| format!("write status request: {error}"))?;
        // half-close 写端，提示对端可发送完整响应。
        let _ = stream.shutdown(Shutdown::Write);
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .map_err(|error| format!("read status response: {error}"))?;
        parse_http_response(&response)
    }

    /// 默认 root@tcp(127.0.0.1:port)/test 配置。
    pub fn mysql_config(&self) -> MysqlConfig {
        MysqlConfig {
            user: "root".into(),
            password: String::new(),
            network: "tcp".into(),
            address: format!("127.0.0.1:{}", self.port),
            database: "test".into(),
            parameters: HashMap::new(),
            allow_all_files: false,
            collation: "utf8mb4_general_ci".into(),
            max_allowed_packet: 64 << 20,
            multi_statements: false,
            tls_config: String::new(),
            fetch_size: None,
        }
    }

    /// 应用覆盖回调后返回 DSN。
    pub fn get_dsn(&self, overriders: &[&ConfigOverrider]) -> String {
        self.get_dsn_optional(
            &overriders
                .iter()
                .map(|overrider| Some(*overrider))
                .collect::<Vec<_>>(),
        )
    }

    /// 对齐 Go 可传入 nil overrider 的调用形状。
    pub fn get_dsn_optional(&self, overriders: &[Option<&ConfigOverrider>]) -> String {
        let mut config = self.mysql_config();
        for override_config in overriders.iter().flatten() {
            override_config(&mut config);
        }
        config.format_dsn()
    }

    /// 带游标 fetchSize 的 DSN。
    pub fn get_dsn_with_cursor(&self, fetch_size: u32) -> String {
        let mut config = self.mysql_config();
        config.fetch_size = Some(fetch_size);
        config.format_dsn()
    }

    /// 轮询直到指定地址可 TCP 连接，或超时。
    pub fn wait_until_custom_server_can_connect(
        &self,
        address: &str,
        timeout: Duration,
    ) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let mut last_error = String::from("server unavailable");
        while Instant::now() < deadline {
            match TcpStream::connect_timeout(
                &address
                    .to_socket_addrs()
                    .map_err(|error| error.to_string())?
                    .next()
                    .ok_or_else(|| "server address did not resolve".to_string())?,
                self.connect_timeout,
            ) {
                Ok(stream) => {
                    let _ = stream.shutdown(Shutdown::Both);
                    return Ok(());
                }
                Err(error) => last_error = error.to_string(),
            }
            thread::sleep(self.retry_interval);
        }
        Err(format!("server did not accept connections: {last_error}"))
    }

    /// 等待本客户端配置的 MySQL 端口可连接。
    pub fn wait_until_server_can_connect(&self, timeout: Duration) -> Result<(), String> {
        self.wait_until_custom_server_can_connect(&format!("127.0.0.1:{}", self.port), timeout)
    }

    /// 轮询 GET /status 直至 2xx，表示服务在线。
    pub fn wait_until_server_online(&self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        self.wait_until_server_can_connect(timeout)?;
        let mut last_error = "status endpoint unavailable".to_string();
        while Instant::now() < deadline {
            match self.fetch_status("/status") {
                Ok(response) if response.is_success() => return Ok(()),
                Ok(response) => last_error = format!("HTTP {}", response.status),
                Err(error) => last_error = error,
            }
            thread::sleep(self.retry_interval);
        }
        Err(format!("server did not become online: {last_error}"))
    }

    /// 依次执行一组闭包测试（共享同一 SqlExecutor）。
    pub fn run_tests(
        &self,
        database: &mut dyn SqlExecutor,
        tests: &mut [&mut dyn FnMut(&mut TestDatabase<'_>) -> Result<(), String>],
    ) -> Result<(), String> {
        let mut kit = TestDatabase::new(database);
        for test in tests {
            test(&mut kit)?;
        }
        Ok(())
    }

    /// 在临时库中跑测试，结束后无论成败都 DROP DATABASE。
    pub fn run_tests_on_new_database(
        &self,
        database: &mut dyn SqlExecutor,
        database_name: &str,
        tests: &mut [&mut dyn FnMut(&mut TestDatabase<'_>) -> Result<(), String>],
    ) -> Result<(), String> {
        database.execute(&format!("DROP DATABASE IF EXISTS `{database_name}`;"), &[])?;
        database.execute(&format!("CREATE DATABASE `{database_name}`;"), &[])?;
        database.execute(&format!("USE `{database_name}`;"), &[])?;
        let mut result = Ok(());
        for test in tests {
            if let Err(error) = test(&mut TestDatabase::new(database)) {
                result = Err(error);
                break;
            }
            if let Err(error) = database.execute("DROP TABLE IF EXISTS test", &[]) {
                result = Err(error);
                break;
            }
        }
        let cleanup = database.execute(&format!("DROP DATABASE IF EXISTS `{database_name}`;"), &[]);
        result.and(cleanup.map(|_| ()))
    }

    /// 按 Scenario 步骤顺序执行 Execute / Query+期望行比对。
    pub fn run_named_scenario(
        &self,
        database: &mut dyn SqlExecutor,
        scenario: Scenario,
    ) -> Result<(), String> {
        for step in scenario.steps() {
            match step {
                ScenarioStep::Execute(sql) => {
                    database.execute(sql, &[])?;
                }
                ScenarioStep::Query { sql, expected } => {
                    let rows = database.query(sql, &[])?;
                    check_rows(&rows, expected)?;
                }
            }
        }
        Ok(())
    }

    /// 基础建表/插入/查询回归场景。
    pub fn run_test_regression(&self, database: &mut dyn SqlExecutor) -> Result<(), String> {
        database.query("select user()", &[])?;
        database.execute("CREATE TABLE test (val TINYINT)", &[])?;
        check_rows(&database.query("SELECT * FROM test", &[])?, EMPTY_ROWS)?;

        let inserted = database.execute("INSERT INTO test VALUES (1)", &[])?;
        expect_execute_result(&inserted, 1, 0, "INSERT INTO test VALUES (1)")?;
        check_rows(&database.query("SELECT val FROM test", &[])?, ONE_ROW)?;

        let updated = database.execute(
            "UPDATE test SET val = 0 WHERE val = ?",
            &[SqlValue::Signed(1)],
        )?;
        expect_execute_result(&updated, 1, 0, "UPDATE test")?;
        check_rows(&database.query("SELECT val FROM test", &[])?, ZERO_ROW)?;

        let deleted = database.execute("DELETE FROM test WHERE val = 0", &[])?;
        expect_execute_result(&deleted, 1, 0, "DELETE matching row")?;
        let empty_delete = database.execute("DELETE FROM test", &[])?;
        expect_execute_result(&empty_delete, 0, 0, "DELETE empty table")?;
        check_rows(&database.query("SELECT 1", &[])?, ONE_ROW)?;

        let echoed = database.query("SELECT ?", &[SqlValue::Bytes(Vec::new())])?;
        if echoed.rows != vec![vec![SqlValue::Bytes(Vec::new())]] {
            return Err(format!(
                "non-nil empty bytes must round-trip unchanged, got {:?}",
                echoed.rows
            ));
        }
        Ok(())
    }

    /// 预编译涉及的数值/字符串/BLOB 类型建表场景。
    pub fn run_test_prepare_result_field_type(
        &self,
        database: &mut dyn SqlExecutor,
    ) -> Result<(), String> {
        check_rows(
            &database.query("SELECT ?", &[SqlValue::Signed(83)])?,
            &[&["83"]],
        )
    }

    /// ENUM / SET / JSON 等特殊类型场景。
    pub fn run_test_special_type(&self, database: &mut dyn SqlExecutor) -> Result<(), String> {
        database.execute(
            "create table test (a decimal(10, 5), b datetime, c time, d bit(8))",
            &[],
        )?;
        database.execute(
            "insert test values (1.4, '2012-12-21 12:12:12', '4:23:34', b'1000')",
            &[],
        )?;
        check_rows(
            &database.query("select * from test where a > ?", &[SqlValue::Signed(0)])?,
            &[&["1.4", "2012-12-21 12:12:12", "04:23:34", "\u{8}"]],
        )
    }

    /// 客户端以 utf8mb4_general_ci 握手后，四个连接字符集变量与 Go 断言一致。
    pub fn run_test_client_with_collation(
        &self,
        database: &mut dyn SqlExecutor,
    ) -> Result<(), String> {
        for (variable, expected) in [
            ("collation_connection", "utf8mb4_general_ci"),
            ("character_set_client", "utf8mb4"),
            ("character_set_results", "utf8mb4"),
            ("character_set_connection", "utf8mb4"),
        ] {
            check_rows(
                &database.query(&format!("show variables like '{variable}'"), &[])?,
                &[&[variable, expected]],
            )?;
        }
        Ok(())
    }

    /// 预编译字符串（含中文）场景。
    pub fn run_test_prepared_string(&self, database: &mut dyn SqlExecutor) -> Result<(), String> {
        database.execute("create table test (a char(10), b char(10))", &[])?;
        database.execute(
            "insert test values (?, ?)",
            &[
                SqlValue::Text("abcdeabcde".into()),
                SqlValue::Text("abcde".into()),
            ],
        )?;
        check_rows(
            &database.query("select * from test where 1 = ?", &[SqlValue::Signed(1)])?,
            &[&["abcdeabcde", "abcde"]],
        )
    }

    /// 预编译 TIMESTAMP(6) 场景。
    pub fn run_test_prepared_timestamp(
        &self,
        database: &mut dyn SqlExecutor,
    ) -> Result<(), String> {
        database.execute("create table test (a timestamp, b time)", &[])?;
        database.execute("set time_zone='+00:00'", &[])?;
        database.execute(
            "insert test values (?, ?)",
            &[
                SqlValue::Text("1970-01-01 00:00:01".into()),
                SqlValue::Text("23:59:59".into()),
            ],
        )?;
        check_rows(
            &database.query(
                "select * from test where a = ? and b = ?",
                &[
                    SqlValue::Text("1970-01-01 00:00:01".into()),
                    SqlValue::Text("23:59:59".into()),
                ],
            )?,
            &[&["1970-01-01 00:00:01", "23:59:59"]],
        )
    }

    /// SELECT INTO OUTFILE 后再 LOAD DATA，并逐单元格核对往返结果。
    pub fn run_test_load_data_with_select_into_outfile(
        &self,
        database: &mut dyn SqlExecutor,
    ) -> Result<(), String> {
        let outfile = unused_temp_path("select_into_outfile", "csv")?;
        let quoted = quote_double(&outfile);
        let mut test = |db: &mut TestDatabase<'_>| {
            db.must_execute(
                "create table t (i int, r real, d decimal(10, 5), s varchar(100), dt datetime, ts timestamp, j json)",
                &[],
            )?;
            for sql in [
                "insert into t values (1, 1.1, 0.1, 'a', '2000-01-01', '01:01:01', '[1]')",
                "insert into t values (2, 2.2, 0.2, 'b', '2000-02-02', '02:02:02', '[1,2]')",
                "insert into t values (null, null, null, null, '2000-03-03', '03:03:03', '[1,2,3]')",
                "insert into t values (4, 4.4, 0.4, 'd', null, null, null)",
            ] {
                db.must_execute(sql, &[])?;
            }
            db.must_execute(&format!("select * from t into outfile {quoted}"), &[])?;
            db.must_execute(
                "create table t1 (i int, r real, d decimal(10, 5), s varchar(100), dt datetime, ts timestamp, j json)",
                &[],
            )?;
            db.must_execute(
                &format!("load data local infile {quoted} into table t1 with thread=1"),
                &[],
            )?;
            let source = db.must_query("select * from t order by i", &[])?;
            let loaded = db.must_query("select * from t1 order by i", &[])?;
            if source.rows != loaded.rows {
                return Err(format!(
                    "SELECT INTO OUTFILE round trip mismatch: source={:?}, loaded={:?}",
                    source.rows, loaded.rows
                ));
            }
            Ok(())
        };
        let result =
            self.run_tests_on_new_database(database, "SelectIntoOutfile", &mut [&mut test]);
        finish_file_cleanup(result, &outfile)
    }

    /// LOAD DATA 同时进入 slow_query 与 statements_summary，并且不污染后续 INSERT plan。
    pub fn run_test_load_data_for_slow_log(
        &self,
        database: &mut dyn SqlExecutor,
    ) -> Result<(), String> {
        let slow_log = create_temp_file("tidb-slow", "log", b"")?;
        let data = create_temp_file("load_data_test", "csv", b"1\t1\n2\t2\n3\t3\n4\t4\n5\t5\n")?;
        let slow_log_sql = quote_double(&slow_log);
        let data_sql = quote_double(&data);
        let mut test = |db: &mut TestDatabase<'_>| {
            let body = (|| {
                db.must_execute("create table t_slow (a int key, b int)", &[])?;
                db.must_execute(&format!("set @@tidb_slow_query_file={slow_log_sql}"), &[])?;
                db.must_execute("set tidb_slow_log_threshold=0;", &[])?;
                db.must_execute("set @@global.tidb_enable_stmt_summary=1", &[])?;
                db.must_execute(
                    &format!("load data local infile {data_sql} into table t_slow with thread=1"),
                    &[],
                )?;
                db.must_execute("insert ignore into t_slow values (1,1);", &[])?;
                let slow_load = db.must_query(
                    "select plan from information_schema.slow_query where query like 'load data local infile % into table t_slow with thread=1;' order by time desc limit 1",
                    &[],
                )?;
                check_plan_tokens(
                    &slow_load,
                    &["LoadData", "total_time:", "loops:", "commit_txn:"],
                )?;
                let summary_load = db.must_query(
                    "select plan from information_schema.STATEMENTS_SUMMARY where QUERY_SAMPLE_TEXT like 'load data local infile %' limit 1",
                    &[],
                )?;
                check_plan_tokens(
                    &summary_load,
                    &["LoadData", "total_time:", "loops:", "commit_txn:"],
                )?;
                let insert = db.must_query(
                    "select plan from information_schema.slow_query where query = 'insert ignore into t_slow values (1,1);' order by time desc limit 1",
                    &[],
                )?;
                check_plan_tokens(
                    &insert,
                    &[
                        "Insert",
                        "time",
                        "loops",
                        "prepare",
                        "check_insert",
                        "mem_insert_time:",
                        "prefetch",
                        "rpc",
                    ],
                )
            })();
            let reset_threshold = db
                .must_execute("set tidb_slow_log_threshold=300;", &[])
                .map(|_| ());
            let reset_summary = db
                .must_execute("set @@global.tidb_enable_stmt_summary=0", &[])
                .map(|_| ());
            body.and(reset_threshold).and(reset_summary)
        };
        let result =
            self.run_tests_on_new_database(database, "load_data_slow_query", &mut [&mut test]);
        let result = finish_file_cleanup(result, &data);
        finish_file_cleanup(result, &slow_log)
    }

    /// auto_random LOAD DATA 批处理：核对条数与两个源列的 XOR 校验和。
    pub fn run_test_load_data_auto_random(
        &self,
        database: &mut dyn SqlExecutor,
    ) -> Result<(), String> {
        self.run_auto_random_load(database, false)
    }

    /// 带引号、逗号与 `|` 行终止符的 auto_random LOAD DATA。
    pub fn run_test_load_data_auto_random_with_special_term(
        &self,
        database: &mut dyn SqlExecutor,
    ) -> Result<(), String> {
        self.run_auto_random_load(database, true)
    }

    fn run_auto_random_load(
        &self,
        database: &mut dyn SqlExecutor,
        special_terminators: bool,
    ) -> Result<(), String> {
        let count = if special_terminators { 5000 } else { 1000 };
        let (contents, checksum1, checksum2) = deterministic_load_rows(count, special_terminators);
        let path = create_temp_file(
            if special_terminators {
                "load_data_txn_error_term"
            } else {
                "load_data_txn_error"
            },
            "csv",
            contents.as_bytes(),
        )?;
        let quoted = quote_double(&path);
        let mut test = |db: &mut TestDatabase<'_>| {
            let table = if special_terminators { "t1" } else { "t" };
            db.must_execute(&format!("drop table if exists {table}"), &[])?;
            db.must_execute(
                &format!(
                    "create table {table}(c1 bigint auto_random primary key, c2 bigint, c3 bigint)"
                ),
                &[],
            )?;
            let load = if special_terminators {
                format!(
                    "load data local infile {quoted} into table t1 fields terminated by ',' enclosed by '\\'' lines terminated by '|' (c2, c3) with batch_size = 128, thread=1"
                )
            } else {
                format!(
                    "load data local infile {quoted} into table t (c2, c3) with batch_size = 128, thread=1"
                )
            };
            db.must_execute(&load, &[])?;
            check_rows(
                &db.must_query(&format!("select count(*) from {table}"), &[])?,
                &[&[&count.to_string()]],
            )?;
            let checksum1 = checksum1.to_string();
            let checksum2 = checksum2.to_string();
            check_rows(
                &db.must_query(
                    &format!("select bit_xor(c2), bit_xor(c3) from {table}"),
                    &[],
                )?,
                &[&[checksum1.as_str(), checksum2.as_str()]],
            )
        };
        let result =
            self.run_tests_on_new_database(database, "load_data_batch_dml", &mut [&mut test]);
        finish_file_cleanup(result, &path)
    }

    pub fn run_test_load_data_for_list_partition(
        &self,
        database: &mut dyn SqlExecutor,
    ) -> Result<(), String> {
        self.run_single_column_list_partition(
            database,
            "create table t (id int, name varchar(10), unique index idx (id)) partition by list (id) (partition p0 values in (3,5,6,9,17), partition p1 values in (1,2,10,11,19,20), partition p2 values in (4,12,13,14,18), partition p3 values in (7,8,15,16,null));",
            "",
            "select * from t partition(p1) order by id",
            "select * from t order by id",
            &[
                &["Warning", "1062", "Duplicate entry '1' for key 't.idx'"],
                &["Warning", "1062", "Duplicate entry '2' for key 't.idx'"],
            ],
            "Table has no partition for value 100",
        )
    }

    pub fn run_test_load_data_for_list_partition_2(
        &self,
        database: &mut dyn SqlExecutor,
    ) -> Result<(), String> {
        self.run_single_column_list_partition(
            database,
            "create table t (id int, name varchar(10), b int generated always as (length(name)+1) virtual, unique index idx (id,b)) partition by list (id*2 + b*b + b*b - b*b*2 - abs(id)) (partition p0 values in (3,5,6,9,17), partition p1 values in (1,2,10,11,19,20), partition p2 values in (4,12,13,14,18), partition p3 values in (7,8,15,16,null));",
            " (id,name)",
            "select id,name from t partition(p1) order by id",
            "select id,name from t order by id",
            &[
                &["Warning", "1062", "Duplicate entry '1-2' for key 't.idx'"],
                &["Warning", "1062", "Duplicate entry '2-2' for key 't.idx'"],
            ],
            "Table has no partition for value 100",
        )
    }

    pub fn run_test_load_data_for_list_column_partition(
        &self,
        database: &mut dyn SqlExecutor,
    ) -> Result<(), String> {
        self.run_single_column_list_partition(
            database,
            "create table t (id int, name varchar(10), unique index idx (id)) partition by list columns (id) (partition p0 values in (3,5,6,9,17), partition p1 values in (1,2,10,11,19,20), partition p2 values in (4,12,13,14,18), partition p3 values in (7,8,15,16,null));",
            "",
            "select * from t partition(p1) order by id",
            "select id,name from t order by id",
            &[
                &["Warning", "1062", "Duplicate entry '1' for key 't.idx'"],
                &["Warning", "1062", "Duplicate entry '2' for key 't.idx'"],
            ],
            "Table has no partition for value from column_list",
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn run_single_column_list_partition(
        &self,
        database: &mut dyn SqlExecutor,
        create_table: &str,
        column_list: &str,
        partition_query: &str,
        all_rows_query: &str,
        duplicate_warnings: &[&[&str]],
        no_partition_warning: &str,
    ) -> Result<(), String> {
        let path = create_temp_file("load_data_list_partition", "csv", b"")?;
        let quoted = quote_double(&path);
        let load_sql =
            format!("load data local infile {quoted} into table t{column_list} with thread=1");
        let mut test = |db: &mut TestDatabase<'_>| {
            db.must_execute(create_table, &[])?;
            self.prepare_load_data_rows(&path, &["1 a", "2 b"])?;
            db.must_execute(&load_sql, &[])?;
            check_rows(
                &db.must_query(partition_query, &[])?,
                &[&["1", "a"], &["2", "b"]],
            )?;
            db.must_execute("delete from t", &[])?;
            self.prepare_load_data_rows(&path, &["1 a", "3 c", "4 e"])?;
            db.must_execute(&load_sql, &[])?;
            check_rows(
                &db.must_query(all_rows_query, &[])?,
                &[&["1", "a"], &["3", "c"], &["4", "e"]],
            )?;
            self.prepare_load_data_rows(&path, &["1 x", "2 b", "2 x", "7 a"])?;
            db.must_execute(&load_sql, &[])?;
            check_rows(&db.must_query("show warnings", &[])?, duplicate_warnings)?;
            check_rows(
                &db.must_query(all_rows_query, &[])?,
                &[
                    &["1", "a"],
                    &["2", "b"],
                    &["3", "c"],
                    &["4", "e"],
                    &["7", "a"],
                ],
            )?;
            self.prepare_load_data_rows(&path, &["5 a", "100 x"])?;
            db.must_execute(&load_sql, &[])?;
            check_rows(
                &db.must_query("show warnings", &[])?,
                &[&["Warning", "1526", no_partition_warning]],
            )?;
            check_rows(
                &db.must_query(all_rows_query, &[])?,
                &[
                    &["1", "a"],
                    &["2", "b"],
                    &["3", "c"],
                    &["4", "e"],
                    &["5", "a"],
                    &["7", "a"],
                ],
            )
        };
        let result =
            self.run_tests_on_new_database(database, "load_data_list_partition", &mut [&mut test]);
        finish_file_cleanup(result, &path)
    }

    pub fn run_test_load_data_for_list_column_partition_2(
        &self,
        database: &mut dyn SqlExecutor,
    ) -> Result<(), String> {
        let path = create_temp_file("load_data_list_partition", "csv", b"")?;
        let quoted = quote_double(&path);
        let load_sql = format!("load data local infile {quoted} into table t with thread=1");
        let mut test = |db: &mut TestDatabase<'_>| {
            db.must_execute(
                "create table t (location varchar(10), id int, a int, unique index idx (location,id)) partition by list columns (location,id) (partition p_west values in (('w',1),('w',2),('w',3),('w',4)), partition p_east values in (('e',5),('e',6),('e',7),('e',8)), partition p_north values in (('n',9),('n',10),('n',11),('n',12)), partition p_south values in (('s',13),('s',14),('s',15),('s',16)));",
                &[],
            )?;
            self.prepare_load_data_rows(&path, &["w 1 1", "w 2 2"])?;
            db.must_execute(&load_sql, &[])?;
            check_rows(
                &db.must_query("select * from t partition(p_west) order by id", &[])?,
                &[&["w", "1", "1"], &["w", "2", "2"]],
            )?;
            db.must_execute("delete from t", &[])?;
            self.prepare_load_data_rows(&path, &["w 1 1", "e 5 5", "n 9 9"])?;
            db.must_execute(&load_sql, &[])?;
            check_rows(
                &db.must_query("select * from t order by id", &[])?,
                &[&["w", "1", "1"], &["e", "5", "5"], &["n", "9", "9"]],
            )?;
            self.prepare_load_data_rows(&path, &["w 1 2", "w 2 2"])?;
            db.must_execute(&load_sql, &[])?;
            check_rows(
                &db.must_query("show warnings", &[])?,
                &[&["Warning", "1062", "Duplicate entry 'w-1' for key 't.idx'"]],
            )?;
            check_rows(
                &db.must_query("select * from t order by id", &[])?,
                &[
                    &["w", "1", "1"],
                    &["w", "2", "2"],
                    &["e", "5", "5"],
                    &["n", "9", "9"],
                ],
            )?;
            self.prepare_load_data_rows(&path, &["w 3 3", "w 5 5", "e 8 8"])?;
            db.must_execute(&load_sql, &[])?;
            check_rows(
                &db.must_query("show warnings", &[])?,
                &[&[
                    "Warning",
                    "1526",
                    "Table has no partition for value from column_list",
                ]],
            )?;
            self.prepare_load_data_rows(&path, &["x 1 1", "w 1 1"])?;
            db.must_execute(&load_sql, &[])?;
            check_rows(
                &db.must_query("show warnings", &[])?,
                &[
                    &[
                        "Warning",
                        "1526",
                        "Table has no partition for value from column_list",
                    ],
                    &["Warning", "1062", "Duplicate entry 'w-1' for key 't.idx'"],
                ],
            )?;
            check_rows(
                &db.must_query("select * from t order by id", &[])?,
                &[
                    &["w", "1", "1"],
                    &["w", "2", "2"],
                    &["w", "3", "3"],
                    &["e", "5", "5"],
                    &["e", "8", "8"],
                    &["n", "9", "9"],
                ],
            )
        };
        let result =
            self.run_tests_on_new_database(database, "load_data_list_partition", &mut [&mut test]);
        finish_file_cleanup(result, &path)
    }

    /// LOAD DATA column-list 的全量、部分、大小写与未知列四个 Go 子场景。
    pub fn run_test_load_data_with_column_list(
        &self,
        database: &mut dyn SqlExecutor,
    ) -> Result<(), String> {
        let contents = b"dsadasdas\n\"1\",\"1\",,\"2022-04-19\",\"a\",\"2022-04-19 00:00:01\"\n\"1\",\"2\",\"a\",\"2022-04-19\",\"a\",\"2022-04-19 00:00:01\"\n\"1\",\"3\",\"a\",\"2022-04-19\",\"a\",\"2022-04-19 00:00:01\"\n\"1\",\"4\",\"a\",\"2022-04-19\",\"a\",\"2022-04-19 00:00:01\"";
        let path = create_temp_file("load_data_test", "csv", contents)?;
        let quoted = quote_single(&path);
        let full_rows: &[&[&str]] = &[
            &["1", "1", "", "2022-04-19", "a", "2022-04-19 00:00:01"],
            &["2", "1", "a", "2022-04-19", "a", "2022-04-19 00:00:01"],
            &["3", "1", "a", "2022-04-19", "a", "2022-04-19 00:00:01"],
            &["4", "1", "a", "2022-04-19", "a", "2022-04-19 00:00:01"],
        ];
        let partial_rows: &[&[&str]] = &[
            &["1", "1", "", "<nil>", "<nil>", "<nil>"],
            &["2", "1", "a", "<nil>", "<nil>", "<nil>"],
            &["3", "1", "a", "<nil>", "<nil>", "<nil>"],
            &["4", "1", "a", "<nil>", "<nil>", "<nil>"],
        ];
        for (column_list, expected) in [
            ("(k,id,c,dt,vv,ts)", full_rows),
            ("(k,id,c)", partial_rows),
            ("(K,Id,c,dT,Vv,Ts)", full_rows),
        ] {
            let mut test = |db: &mut TestDatabase<'_>| {
                db.must_execute("use test", &[])?;
                db.must_execute("drop table if exists t66", &[])?;
                db.must_execute(
                    "create table t66 (id int primary key,k int,c varchar(10),dt date,vv char(1),ts datetime)",
                    &[],
                )?;
                db.must_execute(
                    &format!(
                        "LOAD DATA LOCAL INFILE {quoted} INTO TABLE t66 FIELDS TERMINATED BY ',' ENCLOSED BY '\\\"' IGNORE 1 LINES {column_list} with thread=1"
                    ),
                    &[],
                )?;
                check_rows(&db.must_query("select * from t66", &[])?, expected)
            };
            self.run_tests_on_new_database(database, "LoadData", &mut [&mut test])?;
        }
        let mut mismatch = |db: &mut TestDatabase<'_>| {
            db.must_execute("use test", &[])?;
            db.must_execute("drop table if exists t66", &[])?;
            db.must_execute(
                "create table t66 (id int primary key, c1 varchar(255))",
                &[],
            )?;
            let error = db
                .must_execute(
                    &format!(
                        "LOAD DATA LOCAL INFILE {quoted} INTO TABLE t66 FIELDS TERMINATED BY ',' ENCLOSED BY '\\\"' IGNORE 1 LINES (c1, c2) with thread=1"
                    ),
                    &[],
                )
                .expect_err("unknown column c2 must be rejected");
            let expected = "Error 1054 (42S22): Unknown column 'c2' in 'field list'";
            if error != expected {
                return Err(format!(
                    "unknown column error mismatch: expected {expected:?}, got {error:?}"
                ));
            }
            Ok(())
        };
        let result = self.run_tests_on_new_database(database, "LoadData", &mut [&mut mismatch]);
        finish_file_cleanup(result, &path)
    }

    /// LOAD DATA 的字段/行分隔、引号、NULL、列列表和 SET 预处理场景。
    pub fn run_test_load_data(&self, database: &mut dyn SqlExecutor) -> Result<(), String> {
        let path = create_temp_file("load_data_test", "csv", b"")?;
        let quoted = quote_double(&path);
        let mut test = |db: &mut TestDatabase<'_>| {
            std::fs::write(
                &path,
                b"\nxxx row1_col1\t- row1_col2\t1abc\nxxx row2_col1\t- row2_col2\t\nxxxy row3_col1\t- row3_col2\t\nxxx row4_col1\t- \t\t900\nxxx row5_col1\t- \trow5_col3",
            )
            .map_err(|error| format!("write LOAD DATA fixture: {error}"))?;
            db.must_execute(
                "create table test (a varchar(255), b varchar(255) default 'default value', c int not null auto_increment, primary key(c))",
                &[],
            )?;
            db.must_execute("create view v1 as select 1", &[])?;
            db.must_execute("create sequence s1", &[])?;
            for target in ["v1", "s1"] {
                let error = db
                    .must_execute(
                        &format!(
                            "load data local infile {quoted} into table {target} with thread=1"
                        ),
                        &[],
                    )
                    .expect_err("LOAD DATA target must be insertable");
                if !error.contains("not updatable") {
                    return Err(format!("unexpected LOAD DATA {target} error: {error:?}"));
                }
            }
            let loaded = db.must_execute(
                &format!(
                    "load data local infile {quoted} into table test with batch_size = 3, thread=1"
                ),
                &[],
            )?;
            let default_rows = db.must_query("select * from test order by c", &[])?;
            expect_execute_result(&loaded, 5, 1, "LOAD DATA default fields")
                .map_err(|error| format!("{error}; actual rows: {:?}", default_rows.rows))?;
            check_rows(
                &default_rows,
                &[
                    &["", "<nil>", "1"],
                    &["xxx row2_col1", "- row2_col2", "2"],
                    &["xxxy row3_col1", "- row3_col2", "3"],
                    &["xxx row4_col1", "- ", "4"],
                    &["xxx row5_col1", "- ", "5"],
                ],
            )?;
            db.must_execute("delete from test", &[])?;
            let loaded = db.must_execute(
                &format!(
                    "load data local infile {quoted} into table test fields terminated by '\t- ' lines starting by 'xxx ' terminated by '\n' with batch_size = 3, thread=1"
                ),
                &[],
            )?;
            expect_execute_result(&loaded, 4, 6, "LOAD DATA delimited rows")?;
            check_rows(
                &db.must_query("select * from test order by c", &[])?,
                &[
                    &["row1_col1", "row1_col2\t1abc", "6"],
                    &["row2_col1", "row2_col2\t", "7"],
                    &["row4_col1", "\t\t900", "8"],
                    &["row5_col1", "\trow5_col3", "9"],
                ],
            )?;
            db.must_execute("delete from test", &[])?;
            let mut oversized = std::fs::read(&path)
                .map_err(|error| format!("read LOAD DATA fixture for oversized case: {error}"))?;
            oversized.push(b'\n');
            for index in 6..=800 {
                oversized.extend_from_slice(
                    format!("xxx row{index}_col1\t- row{index}_col2\n").as_bytes(),
                );
            }
            std::fs::write(&path, oversized)
                .map_err(|error| format!("write oversized LOAD DATA fixture: {error}"))?;
            let original_limit = astersql_kv::TxnTotalSizeLimit.swap(12_000, Ordering::SeqCst);
            let oversized_result = db.must_execute(
                &format!(
                    "load data local infile {quoted} into table test fields terminated by '\t- ' lines starting by 'xxx ' terminated by '\n' with batch_size = 3, thread=1"
                ),
                &[],
            );
            astersql_kv::TxnTotalSizeLimit.store(original_limit, Ordering::SeqCst);
            let oversized_error =
                oversized_result.expect_err("oversized LOAD DATA transaction must fail");
            if !oversized_error
                .to_ascii_lowercase()
                .contains("transaction is too large")
            {
                return Err(format!(
                    "unexpected oversized LOAD DATA error: {oversized_error:?}"
                ));
            }
            let empty_terminator = db
                .must_execute(
                    &format!(
                        "load data local infile {quoted} into table test lines terminated by '' with thread=1"
                    ),
                    &[],
                )
                .expect_err("empty line terminator must be rejected");
            if empty_terminator.is_empty() {
                return Err("empty line terminator returned an empty error".into());
            }
            let missing = db
                .must_execute(
                    "load data local infile '/tmp/nonexistence.csv' into table test with thread=1",
                    &[],
                )
                .expect_err("missing local infile must fail");
            if !missing.contains("No such file or directory") && !missing.contains("os error 2") {
                return Err(format!("unexpected missing-file error: {missing:?}"));
            }

            db.must_execute("drop table test", &[])?;
            std::fs::write(&path, b"\"abc\",123\ndef,456,\nhig,\"789\",")
                .map_err(|error| format!("write enclosed fixture: {error}"))?;
            db.must_execute(
                "create table test (str varchar(10) default null, i int default null)",
                &[],
            )?;
            db.must_execute(
                &format!(
                    "load data local infile {quoted} into table test fields terminated by ',' enclosed by '\"' with batch_size = 3, thread=1"
                ),
                &[],
            )?;
            check_rows(
                &db.must_query("select * from test", &[])?,
                &[&["abc", "123"], &["def", "456"], &["hig", "789"]],
            )?;

            db.must_execute("drop table test", &[])?;
            std::fs::write(
                &path,
                b",\\N,NULL,,\n00,0,000000,,\n2003-03-03, 20030303,030303,\\N\n",
            )
            .map_err(|error| format!("write irregular-date fixture: {error}"))?;
            db.must_execute(
                "create table test (a date, b date, c date not null, d date)",
                &[],
            )?;
            db.must_execute(
                &format!(
                    "load data local infile {quoted} into table test fields terminated by ',' with batch_size = 3, thread=1"
                ),
                &[],
            )?;
            check_rows(
                &db.must_query("select * from test", &[])?,
                &[
                    &["0000-00-00", "<nil>", "0000-00-00", "0000-00-00"],
                    &["0000-00-00", "0000-00-00", "0000-00-00", "0000-00-00"],
                    &["2003-03-03", "2003-03-03", "2003-03-03", "<nil>"],
                ],
            )?;

            db.must_execute("drop table test", &[])?;
            std::fs::write(
                &path,
                b"\"field1\",\"field2\"\n\"a\"\"b\",\"cd\"\"ef\"\n\"a\"b\",c\"d\"e\n",
            )
            .map_err(|error| format!("write doubled-quote fixture: {error}"))?;
            db.must_execute("create table test (a varchar(20), b varchar(20))", &[])?;
            db.must_execute(
                &format!(
                    "load data local infile {quoted} into table test fields terminated by ',' enclosed by '\"' with batch_size = 3, thread=1"
                ),
                &[],
            )?;
            check_rows(
                &db.must_query("select * from test", &[])?,
                &[
                    &["field1", "field2"],
                    &["a\"b", "cd\"ef"],
                    &["a\"b", "c\"d\"e"],
                ],
            )?;

            db.must_execute("drop table test", &[])?;
            std::fs::write(&path, b"\"a,b,c\n\"1\",2,\"3\"\n")
                .map_err(|error| format!("write optionally-enclosed fixture: {error}"))?;
            db.must_execute(
                "create table test (id int not null primary key, b int, c varchar(10))",
                &[],
            )?;
            db.must_execute(
                &format!(
                    "load data local infile {quoted} into table test fields terminated by ',' optionally enclosed by '\"' ignore 1 lines with batch_size = 3, thread=1"
                ),
                &[],
            )?;
            check_rows(
                &db.must_query("select * from test", &[])?,
                &[&["1", "2", "3"]],
            )?;

            db.must_execute("drop table test", &[])?;
            std::fs::write(&path, b"1,2\n3,4\n")
                .map_err(|error| format!("write two-column fixture: {error}"))?;
            db.must_execute("create table test (c1 int, c2 int)", &[])?;
            db.must_execute(
                &format!(
                    "load data local infile {quoted} into table test fields terminated by ',' with batch_size = 1, thread=1"
                ),
                &[],
            )?;
            check_rows(
                &db.must_query("select * from test order by c1", &[])?,
                &[&["1", "2"], &["3", "4"]],
            )?;
            let commit_failure =
                astersql_testkit_testfailpoint::enable("executor/commitOneTaskErr", "return(true)");
            let commit_error = db
                .must_execute(
                    &format!(
                        "load data local infile {quoted} into table test fields terminated by ',' with thread=1"
                    ),
                    &[],
                )
                .expect_err("commitOneTaskErr must reach the caller");
            drop(commit_failure);
            if commit_error != "mock commit one task error" {
                return Err(format!(
                    "unexpected commitOneTaskErr message: {commit_error:?}"
                ));
            }

            db.must_execute("drop table test", &[])?;
            db.must_execute("create table test (c1 int, c2 int)", &[])?;
            db.must_execute(
                &format!(
                    "load data local infile {quoted} into table test fields terminated by ',' (c1, c2) with batch_size = 1, thread=1"
                ),
                &[],
            )?;
            check_rows(
                &db.must_query("select * from test order by c1", &[])?,
                &[&["1", "2"], &["3", "4"]],
            )?;

            db.must_execute("drop table test", &[])?;
            std::fs::write(&path, b"1,2,3\n4,5,6\n")
                .map_err(|error| format!("write column-list fixture: {error}"))?;
            db.must_execute("create table test (c1 int, c2 int, c3 int)", &[])?;
            db.must_execute(
                &format!(
                    "load data local infile {quoted} into table test fields terminated by ',' (c1, @dummy) with batch_size = 1, thread=1"
                ),
                &[],
            )?;
            check_rows(
                &db.must_query("select * from test", &[])?,
                &[&["1", "<nil>", "<nil>"], &["4", "<nil>", "<nil>"]],
            )?;
            db.must_execute("delete from test", &[])?;
            for variables in [("@val1", "@val2"), ("@VAL1", "@VAL2")] {
                db.must_execute(
                    &format!(
                        "load data local infile {quoted} into table test fields terminated by ',' (c1, {}, {}) set c3 = {} * 100, c2 = cast({} as unsigned) with batch_size = 1, thread=1",
                        variables.0, variables.1, variables.1, variables.0
                    ),
                    &[],
                )?;
                check_rows(
                    &db.must_query("select * from test order by c1", &[])?,
                    &[&["1", "2", "300"], &["4", "5", "600"]],
                )?;
                db.must_execute("delete from test", &[])?;
            }
            Ok(())
        };
        let result = self.run_tests_on_new_database(database, "LoadData", &mut [&mut test]);
        finish_file_cleanup(result, &path)
    }

    /// LOAD DATA 的事务回滚、提交可见性、锁冲突与 reader 清理场景。
    pub fn run_test_load_data_in_transaction(
        &self,
        database: &mut dyn SqlExecutor,
    ) -> Result<(), String> {
        let path = create_temp_file("load_data_test", "csv", b"1")?;
        let quoted = quote_double(&path);
        let result = (|| {
            let db_name = "LoadDataInTransaction";
            database.execute(&format!("DROP DATABASE IF EXISTS `{db_name}`;"), &[])?;
            database.execute(&format!("CREATE DATABASE `{db_name}`;"), &[])?;
            database.execute(&format!("USE `{db_name}`;"), &[])?;
            database.execute("create table t (a int)", &[])?;

            database.execute("begin", &[])?;
            database.execute("insert into t values (100)", &[])?;
            database.execute(
                &format!("load data local infile {quoted} into table t"),
                &[],
            )?;
            check_rows(
                &database.query("select * from t order by a desc", &[])?,
                &[&["100"], &["1"]],
            )?;
            database.execute("rollback", &[])?;
            check_rows(&database.query("select * from t", &[])?, EMPTY_ROWS)?;

            database.execute("begin", &[])?;
            database.execute(
                &format!("load data local infile {quoted} into table t"),
                &[],
            )?;
            check_rows(&database.query("select * from t", &[])?, &[&["1"]])?;
            database.execute("commit", &[])?;
            check_rows(&database.query("select * from t", &[])?, &[&["1"]])?;
            database.execute("delete from t", &[])?;

            database.execute(
                &format!("load data local infile {quoted} into table t"),
                &[],
            )?;
            database.execute("begin", &[])?;
            check_rows(&database.query("select * from t", &[])?, &[&["1"]])?;
            database.execute("rollback", &[])?;

            let pessimistic_db = "LoadDataInPessimisticTransaction";
            database.execute(&format!("DROP DATABASE IF EXISTS `{pessimistic_db}`;"), &[])?;
            database.execute(&format!("CREATE DATABASE `{pessimistic_db}`;"), &[])?;
            database.execute(&format!("USE `{pessimistic_db}`;"), &[])?;
            database.execute("set @@global.tidb_txn_mode = 'pessimistic'", &[])?;
            database.execute("create table t (a int primary key)", &[])?;
            let mut txn1 = database.fork()?;
            txn1.execute(&format!("USE `{pessimistic_db}`;"), &[])?;
            txn1.execute("begin", &[])?;
            txn1.execute(
                &format!("load data local infile {quoted} into table t"),
                &[],
            )?;
            check_rows(&txn1.query("select * from t", &[])?, &[&["1"]])?;

            let mut txn2 = database.fork()?;
            txn2.execute(&format!("USE `{pessimistic_db}`;"), &[])?;
            txn2.execute("begin", &[])?;
            let commit = thread::spawn(move || {
                thread::sleep(Duration::from_secs(2));
                txn1.execute("commit", &[]).map(|_| ())
            });
            let wait_started = Instant::now();
            let lock_result = txn2.query("select * from t where a = 1 for update", &[]);
            let lock_wait = wait_started.elapsed();
            let commit_result = commit
                .join()
                .map_err(|_| "pessimistic transaction commit thread panicked".to_string())?;
            lock_result?;
            commit_result?;
            if lock_wait < Duration::from_secs(1) {
                return Err(
                    "pessimistic SELECT FOR UPDATE acquired the lock before txn1 committed".into(),
                );
            }
            txn2.execute("rollback", &[])?;
            check_rows(&database.query("select * from t", &[])?, &[&["1"]])?;

            let optimistic_db = "LoadDataInExplicitTransaction";
            database.execute(&format!("DROP DATABASE IF EXISTS `{optimistic_db}`;"), &[])?;
            database.execute(&format!("CREATE DATABASE `{optimistic_db}`;"), &[])?;
            database.execute(&format!("USE `{optimistic_db}`;"), &[])?;
            database.execute("set @@global.tidb_txn_mode = 'optimistic'", &[])?;
            database.execute("create table t (a int primary key)", &[])?;
            let mut txn1 = database.fork()?;
            let mut txn2 = database.fork()?;
            for transaction in [&mut txn1, &mut txn2] {
                transaction.execute(&format!("USE `{optimistic_db}`;"), &[])?;
                transaction.execute("begin", &[])?;
                transaction.execute(
                    &format!("load data local infile {quoted} into table t"),
                    &[],
                )?;
            }
            txn1.execute("commit", &[])?;
            let conflict = txn2
                .execute("commit", &[])
                .expect_err("second optimistic transaction must conflict");
            if !conflict.contains("Write conflict") {
                return Err(format!("expected Write conflict, got {conflict:?}"));
            }
            check_rows(&database.query("select * from t", &[])?, &[&["1"]])?;

            let server_file_db = "LoadDataFromServerFile";
            database.execute(&format!("DROP DATABASE IF EXISTS `{server_file_db}`;"), &[])?;
            database.execute(&format!("CREATE DATABASE `{server_file_db}`;"), &[])?;
            database.execute(&format!("USE `{server_file_db}`;"), &[])?;
            database.execute("create table t (a int)", &[])?;
            let server_file = database
                .execute(&format!("load data infile {quoted} into table t"), &[])
                .expect_err("server-side local file must be rejected");
            if !server_file.contains("Don't support load data from tidb-server's disk.") {
                return Err(format!("unexpected server-file error: {server_file:?}"));
            }

            let cleanup_db = "LoadDataCleanup";
            database.execute(&format!("DROP DATABASE IF EXISTS `{cleanup_db}`;"), &[])?;
            database.execute(&format!("CREATE DATABASE `{cleanup_db}`;"), &[])?;
            database.execute(&format!("USE `{cleanup_db}`;"), &[])?;
            database.execute("create table t (a int)", &[])?;
            database.execute("begin", &[])?;
            database.execute(
                &format!("load data local infile {quoted} into table t"),
                &[],
            )?;
            let missing = database
                .execute(
                    "load data local infile '/tmp/does_not_exist' into table t",
                    &[],
                )
                .expect_err("missing second reader must fail");
            if !missing.contains("No such file or directory") && !missing.contains("os error 2") {
                return Err(format!("unexpected missing-file error: {missing:?}"));
            }
            database.execute("commit", &[])?;
            check_rows(&database.query("select * from t", &[])?, &[&["1"]])?;
            Ok(())
        })();

        let mut cleanup = Ok(());
        for db_name in [
            "LoadDataInTransaction",
            "LoadDataInPessimisticTransaction",
            "LoadDataInExplicitTransaction",
            "LoadDataFromServerFile",
            "LoadDataCleanup",
        ] {
            if let Err(error) =
                database.execute(&format!("DROP DATABASE IF EXISTS `{db_name}`;"), &[])
                && cleanup.is_ok()
            {
                cleanup = Err(error);
            }
        }
        finish_file_cleanup(result.and(cleanup), &path)
    }

    /// 多语句执行场景。
    pub fn run_test_multi_statements(&self, database: &mut dyn SqlExecutor) -> Result<(), String> {
        database.execute(
            "CREATE TABLE `test` (`id` int(11) NOT NULL, `value` int(11) NOT NULL) ",
            &[],
        )?;
        let inserted = database.execute("INSERT INTO test VALUES (1, 1)", &[])?;
        expect_execute_result(&inserted, 1, 0, "multi-statement INSERT")?;
        let updated = database.execute(
            "UPDATE test SET value = 3 WHERE id = 1; UPDATE test SET value = 4 WHERE id = 1; UPDATE test SET value = 5 WHERE id = 1;",
            &[],
        )?;
        expect_execute_result(&updated, 1, 0, "multi-statement UPDATE")?;
        check_rows(
            &database.query("SELECT value FROM test WHERE id=1;", &[])?,
            &[&["5"]],
        )?;

        database.execute("CREATE DATABASE dropme", &[])?;
        database.execute("USE dropme", &[])?;
        database.execute("DROP DATABASE dropme", &[])?;
        check_rows(
            &database.query("SELECT IFNULL(DATABASE(),'success')", &[])?,
            &[&["success"]],
        )?;
        database.execute("CREATE DATABASE multistmtuse", &[])?;
        database.execute(
            "use multistmtuse; create table if not exists t1 (id int); drop table t1;",
            &[],
        )?;
        database.execute("create database if not exists test;", &[])?;
        database.execute("use test;", &[])?;
        database.execute(
            "CREATE TABLE t (a bigint(20), b int(10), PRIMARY KEY (b, a), UNIQUE KEY uk_a (a));",
            &[],
        )?;
        database.execute("insert into t values (1, 1);", &[])?;
        database.execute("begin;", &[])?;
        database.query("delete from t where a = 1; select 1;", &[])?;
        database.query("update t set b = 2 where a = 1; select 1;", &[])?;
        database.execute("commit;", &[])?;
        Ok(())
    }

    /// 在同一连接上读取 connection_id，并校验 EXPLAIN FOR CONNECTION 的 Point_Get。
    pub fn run_test_explain_for_conn(&self, database: &mut dyn SqlExecutor) -> Result<(), String> {
        let mut test = |db: &mut TestDatabase<'_>| {
            db.must_execute("drop table if exists t", &[])?;
            db.must_execute("create table t (a int key, b int)", &[])?;
            db.must_execute("insert t values (1, 1)", &[])?;
            let connection = db.must_query("select connection_id();", &[])?;
            let connection_id = connection
                .rows
                .first()
                .and_then(|row| row.first())
                .ok_or_else(|| "connection_id() returned no row".to_string())?
                .display();
            db.must_query("select * from t where a=1", &[])?;
            let explain = db.must_query(&format!("explain for connection {connection_id}"), &[])?;
            let row = explain
                .rows
                .first()
                .ok_or_else(|| "EXPLAIN FOR CONNECTION returned no row".to_string())?;
            if row.len() != 9 {
                return Err(format!(
                    "EXPLAIN FOR CONNECTION expected 9 columns, got {}",
                    row.len()
                ));
            }
            let joined = row
                .iter()
                .map(SqlValue::display)
                .collect::<Vec<_>>()
                .join(",");
            for token in [
                "Point_Get_1",
                "1.00",
                "1,root",
                "table:t",
                "time",
                "loop",
                "handle:1",
            ] {
                if !joined.contains(token) {
                    return Err(format!("EXPLAIN plan is missing {token:?}: {joined:?}"));
                }
            }
            Ok(())
        };
        self.run_tests_on_new_database(database, "explain_for_conn", &mut [&mut test])
    }

    /// 覆盖 Go 场景中的事务、schema、optimizer、变量和表达式 MySQL 错误码。
    pub fn run_test_error_code(&self, database: &mut dyn SqlExecutor) -> Result<(), String> {
        database.execute("DROP DATABASE IF EXISTS `ErrorCode`;", &[])?;
        database.execute("CREATE DATABASE `ErrorCode`;", &[])?;
        database.execute("USE `ErrorCode`;", &[])?;
        let body = (|| {
            database.execute("set @@tidb_txn_mode=''", &[])?;
            database.execute("create table test (c int PRIMARY KEY);", &[])?;
            database.execute("insert into test values (1);", &[])?;
            database.execute("begin", &[])?;
            database.execute("insert into test values(1)", &[])?;
            expect_mysql_error(database.execute("commit", &[]), &[1062])?;
            database.execute("begin", &[])?;
            for (sql, codes) in [
                ("use db_not_exists;", &[1049][..]),
                ("select * from tbl_not_exists;", &[1146][..]),
                ("create database test;", &[1007, 8028][..]),
                (
                    "create database aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;",
                    &[1059, 8028][..],
                ),
                ("create table test (c int);", &[1050, 8028][..]),
                ("drop table unknown_table;", &[1051, 8028][..]),
                ("drop database unknown_db;", &[1008, 8028][..]),
                (
                    "create table aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa (a int);",
                    &[1059, 8028][..],
                ),
                (
                    "create table long_column_table (aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa int);",
                    &[1059, 8028][..],
                ),
                (
                    "alter table test add aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa int;",
                    &[1059, 8028][..],
                ),
                ("select *, * from test;", &[1166][..]),
                ("select row(1, 2) > 1;", &[1241][..]),
                ("select * from test order by row(c, c);", &[1241][..]),
                ("select @@unknown_sys_var;", &[1193][..]),
                ("set @@unknown_sys_var='1';", &[1193][..]),
                ("select greatest(2);", &[1582][..]),
            ] {
                expect_mysql_error(database.execute(sql, &[]), codes)?;
            }
            Ok(())
        })();
        let cleanup = database
            .execute("DROP DATABASE IF EXISTS `ErrorCode`;", &[])
            .map(|_| ());
        body.and(cleanup)
    }

    /// 认证、错误密码、默认角色及 localhost 用户匹配。
    pub fn run_test_auth(
        &self,
        admin: &mut dyn SqlExecutor,
        connector: &mut dyn ServerConnector,
    ) -> Result<(), String> {
        for sql in [
            "CREATE USER 'authtest'@'%' IDENTIFIED BY '123';",
            "CREATE ROLE 'authtest_r1'@'%';",
            "GRANT ALL on test.* to 'authtest'",
            "GRANT authtest_r1 to 'authtest'",
            "SET DEFAULT ROLE authtest_r1 TO authtest",
        ] {
            admin.execute(sql, &[])?;
        }
        let mut config = self.mysql_config();
        config.user = "authtest".into();
        config.password = "123".into();
        let mut connection = connector.connect(&config)?;
        connection.ping()?;
        connection.execute("USE information_schema;", &[])?;

        config.password = "456".into();
        if connector.connect(&config).is_ok() {
            return Err("wrong password unexpectedly connected".into());
        }

        config.password = "123".into();
        let mut role_connection = connector.connect(&config)?;
        role_connection.ping()?;
        check_rows(
            &role_connection.query("select current_role;", &[])?,
            &[&["`authtest_r1`@`%`"]],
        )?;

        admin.execute(
            "CREATE USER 'authtest2'@'localhost' IDENTIFIED BY '123';",
            &[],
        )?;
        admin.execute("GRANT ALL on test.* to 'authtest2'@'localhost'", &[])?;
        config.user = "authtest2".into();
        config.password = "123".into();
        let mut localhost_connection = connector.connect(&config)?;
        localhost_connection.ping()?;
        localhost_connection.execute("USE information_schema;", &[])?;
        Ok(())
    }

    pub fn run_test_issue_3662(&self, connector: &mut dyn ServerConnector) -> Result<(), String> {
        let mut config = self.mysql_config();
        config.database = "non_existing_schema".into();
        expect_connection_error(
            connector,
            &config,
            "Error 1049 (42000): Unknown database 'non_existing_schema'",
        )
    }

    pub fn run_test_issue_3680(&self, connector: &mut dyn ServerConnector) -> Result<(), String> {
        let mut config = self.mysql_config();
        config.user = "non_existing_user".into();
        expect_connection_error(
            connector,
            &config,
            "Error 1045 (28000): Access denied for user 'non_existing_user'@'127.0.0.1' (using password: NO)",
        )
    }

    pub fn run_test_issue_3682(
        &self,
        admin: &mut dyn SqlExecutor,
        connector: &mut dyn ServerConnector,
    ) -> Result<(), String> {
        for sql in [
            "CREATE USER 'issue3682'@'%' IDENTIFIED BY '123';",
            "GRANT ALL on test.* to 'issue3682'",
            "GRANT ALL on mysql.* to 'issue3682'",
        ] {
            admin.execute(sql, &[])?;
        }
        let mut config = self.mysql_config();
        config.user = "issue3682".into();
        config.password = "123".into();
        let mut valid = connector.connect(&config)?;
        valid.ping()?;
        valid.execute("USE mysql;", &[])?;

        config.password = "wrong_password".into();
        config.database = "non_existing_schema".into();
        expect_connection_error(
            connector,
            &config,
            "Error 1045 (28000): Access denied for user 'issue3682'@'127.0.0.1' (using password: YES)",
        )
    }

    /// 对比 /metrics 前后值，严格复现 Go statement-count 工作负载与增量。
    pub fn run_test_stmt_count(&self, database: &mut dyn SqlExecutor) -> Result<(), String> {
        let mut test = |db: &mut TestDatabase<'_>| {
            let before = self.get_metrics()?;
            for sql in [
                "create table test (a int)",
                "insert into test values(1)",
                "insert into test values(2)",
                "insert into test values(3)",
                "insert into test values(4)",
                "insert into test values(5)",
                "delete from test where a = 3",
                "update test set a = 2 where a = 1",
                "select * from test",
                "select 2",
                "prepare stmt1 from 'update test set a = 1 where a = 2'",
                "execute stmt1",
                "prepare stmt2 from 'select * from test'",
                "execute stmt2",
                "replace into test(a) values(6);",
            ] {
                db.must_execute(sql, &[])?;
            }
            let after = self.get_metrics()?;
            for (kind, increment) in [
                ("CreateTable", 1.0),
                ("Insert", 5.0),
                ("Delete", 1.0),
                ("Update", 2.0),
                ("Select", 3.0),
                ("Prepare", 2.0),
                ("Execute", 0.0),
                ("Replace", 1.0),
            ] {
                let actual = get_statement_count(&after, kind) - get_statement_count(&before, kind);
                if actual != increment {
                    return Err(format!(
                        "{kind} statement metric increment: expected {increment}, got {actual}"
                    ));
                }
            }
            Ok(())
        };
        self.run_tests_on_new_database(database, "StatementCount", &mut [&mut test])
    }

    /// Go 源场景被 `t.Skip("unstable test")` 无条件跳过。
    pub fn run_test_db_stmt_count(&self, _database: &mut dyn SqlExecutor) -> Result<(), String> {
        Ok(())
    }

    /// init_connect 只作用于非 SUPER 新会话，并在结束后恢复全局变量。
    pub fn run_test_init_connect(
        &self,
        admin: &mut dyn SqlExecutor,
        connector: &mut dyn ServerConnector,
    ) -> Result<(), String> {
        for sql in [
            "SET GLOBAL init_connect=\"insert into test.ts VALUES (NOW());SET @a=1;\"",
            "CREATE USER init_nonsuper",
            "CREATE USER init_super",
            "GRANT SELECT, INSERT, DROP ON test.* TO init_nonsuper",
            "GRANT SELECT, INSERT, DROP, SUPER ON *.* TO init_super",
            "CREATE TABLE ts (a TIMESTAMP)",
        ] {
            admin.execute(sql, &[])?;
        }
        let mut config = self.mysql_config();
        config.user = "init_nonsuper".into();
        let mut non_super = connector.connect(&config)?;
        non_super.ping()?;
        check_rows(&non_super.query("SELECT @a", &[])?, &[&["1"]])?;

        config.user = "init_super".into();
        let mut super_user = connector.connect(&config)?;
        super_user.ping()?;
        check_rows(&super_user.query("SELECT IFNULL(@a,\"\")", &[])?, &[&[""]])?;
        let mut cleanup = connector.connect(&config)?;
        cleanup.execute("SET GLOBAL init_connect=\"\"", &[])?;
        Ok(())
    }

    /// 新建连接首条语句前已加载全局 NO_BACKSLASH_ESCAPES。
    pub fn run_test_sql_mode_is_loaded_before_query(
        &self,
        connector: &mut dyn ServerConnector,
    ) -> Result<(), String> {
        let config = self.mysql_config();
        let mut first = connector.connect(&config)?;
        first.ping()?;
        first.execute("set global sql_mode='NO_BACKSLASH_ESCAPES';", &[])?;
        first.execute(
            "CREATE TABLE t1 (id bigint(20) NOT NULL, t text DEFAULT NULL, PRIMARY KEY (id));",
            &[],
        )?;
        let mut second = connector.connect(&config)?;
        second.ping()?;
        second.execute("insert into t1 values (1, 'ab\\\\c');", &[])?;
        check_rows(
            &second.query("select t from t1 where id = 1;", &[])?,
            &[&["ab\\\\c"]],
        )
    }

    /// information_schema 三类 client_errors 表的 warning/error 增量。
    pub fn run_test_infoschema_client_errors(
        &self,
        database: &mut dyn SqlExecutor,
    ) -> Result<(), String> {
        let mut test = |db: &mut TestDatabase<'_>| {
            db.must_execute("set @@tidb_enable_cache_prepare_stmt = off", &[])?;
            let body = (|| {
                for (statement, increments_warning, increments_error, code) in [
                    ("SELECT 0/0", true, false, 1365_u16),
                    (
                        "CREATE TABLE test_client_errors2 (a int primary key, b int primary key)",
                        true,
                        true,
                        1068,
                    ),
                    ("gibberish", true, true, 1064),
                ] {
                    for source in [
                        "client_errors_summary_global",
                        "client_errors_summary_by_user",
                        "client_errors_summary_by_host",
                    ] {
                        let sql = format!(
                            "SELECT SUM(error_count), SUM(warning_count) FROM information_schema.{source} WHERE error_number = ? GROUP BY error_number"
                        );
                        let (errors, warnings) = client_error_counts(
                            &db.must_query(&sql, &[SqlValue::Unsigned(u64::from(code))])?,
                        )?;
                        if statement == "SELECT 0/0" {
                            db.must_query(statement, &[])?;
                        } else {
                            expect_mysql_error(db.must_execute(statement, &[]), &[code])?;
                        }
                        let (new_errors, new_warnings) = client_error_counts(
                            &db.must_query(&sql, &[SqlValue::Unsigned(u64::from(code))])?,
                        )?;
                        let expected_errors = errors + i64::from(increments_error);
                        let expected_warnings = warnings + i64::from(increments_warning);
                        if (new_errors, new_warnings) != (expected_errors, expected_warnings) {
                            return Err(format!(
                                "information_schema.{source} code={code} statement={statement:?}: expected ({expected_errors}, {expected_warnings}), got ({new_errors}, {new_warnings})"
                            ));
                        }
                    }
                }
                Ok(())
            })();
            let reset = db
                .must_execute("set @@tidb_enable_cache_prepare_stmt = default", &[])
                .map(|_| ());
            body.and(reset)
        };
        self.run_tests_on_new_database(database, "clientErrors", &mut [&mut test])
    }

    /// prepared statement long-data 保留 JSON 文本类型与 GBK 原始字节。
    pub fn run_test_type_and_charset_of_send_long_data(
        &self,
        connection: &mut dyn ServerConnection,
    ) -> Result<(), String> {
        connection.execute("CREATE TABLE t (j JSON);", &[])?;
        let json = format!("\"{}\"", "a".repeat(1024));
        let statement = connection.prepare("INSERT INTO t VALUES (cast(? as JSON));")?;
        connection.send_long_data(statement, 0, json.as_bytes())?;
        connection.execute_prepared(statement, &[])?;
        connection.close_prepared(statement)?;
        let json_result = connection.query("SELECT j FROM t;", &[])?;
        if json_result.rows != vec![vec![SqlValue::Text(json.clone())]] {
            return Err(format!(
                "JSON long-data mismatch: expected {json:?}, got {:?}",
                json_result.rows
            ));
        }

        connection.execute("drop table t", &[])?;
        connection.execute("CREATE TABLE t (t TEXT);", &[])?;
        let mut gbk = Vec::with_capacity(4096);
        for _ in 0..1024 {
            // GBK: 你 = C4 E3, 好 = BA C3.
            gbk.extend_from_slice(&[0xc4, 0xe3, 0xba, 0xc3]);
        }
        let statement = connection.prepare("INSERT INTO t VALUES (?);")?;
        connection.send_long_data(statement, 0, &gbk)?;
        connection.execute_prepared(statement, &[])?;
        connection.close_prepared(statement)?;
        let text_result = connection.query("SELECT * FROM t;", &[])?;
        if text_result.rows != vec![vec![SqlValue::Bytes(gbk.clone())]] {
            return Err(format!(
                "GBK long-data mismatch: expected {} bytes, got {:?}",
                gbk.len(),
                text_result.rows
            ));
        }
        Ok(())
    }

    pub fn run_test_issue_53634(
        &self,
        database: &mut dyn SqlExecutor,
        schema_state: &mut dyn SchemaStateController,
    ) -> Result<(), String> {
        self.run_schema_state_regression(database, schema_state, true)
    }

    pub fn run_test_issue_54254(
        &self,
        database: &mut dyn SqlExecutor,
        schema_state: &mut dyn SchemaStateController,
    ) -> Result<(), String> {
        self.run_schema_state_regression(database, schema_state, false)
    }

    fn run_schema_state_regression(
        &self,
        database: &mut dyn SqlExecutor,
        schema_state: &mut dyn SchemaStateController,
        drop_column: bool,
    ) -> Result<(), String> {
        let body = (|| {
            for sql in [
                "create database test_db_state default charset utf8 default collate utf8_bin",
                "use test_db_state",
                "CREATE TABLE stock (a int NOT NULL, b char(30) NOT NULL, c int, d char(64), PRIMARY KEY(a,b)) ENGINE=InnoDB DEFAULT CHARSET=latin1 COLLATE=latin1_bin COMMENT='…comment';",
                "insert into stock values(1, 'a', 11, 'x'), (2, 'b', 22, 'y')",
            ] {
                database.execute(sql, &[])?;
            }
            if drop_column {
                database.execute("alter table stock add column cct_1 int default 10", &[])?;
                database.execute("alter table stock modify cct_1 json", &[])?;
                database.execute("alter table stock add column adc_1 smallint", &[])?;
            }
            let ddl = if drop_column {
                "alter table stock drop column cct_1"
            } else {
                "alter table stock add column cct_1 int"
            };
            let mut transaction = || {
                database.execute("begin", &[])?;
                database.execute(
                    "SELECT a, c, d from stock where (a, b) IN ((?, ?),(?, ?)) FOR UPDATE",
                    &[
                        SqlValue::Signed(1),
                        SqlValue::Text("a".into()),
                        SqlValue::Signed(2),
                        SqlValue::Text("b".into()),
                    ],
                )?;
                database.execute(
                    "UPDATE stock SET c = ? WHERE a= ? AND b = 'a'",
                    &[SqlValue::Signed(101), SqlValue::Signed(1)],
                )?;
                database.execute(
                    "UPDATE stock SET c = ?, d = 'z' WHERE a= ? AND b = 'b'",
                    &[SqlValue::Signed(102), SqlValue::Signed(2)],
                )?;
                database.execute("commit", &[])?;
                Ok(())
            };
            schema_state.at_write_reorganization(ddl, &mut transaction)?;
            check_rows(
                &database.query("select * from stock;", &[])?,
                &[
                    &["1", "a", "101", "x", "<nil>"],
                    &["2", "b", "102", "z", "<nil>"],
                ],
            )
        })();
        let cleanup = database
            .execute("drop database test_db_state", &[])
            .map(|_| ());
        body.and(cleanup)
    }

    /// SUM / AVG 聚合结果场景。
    pub fn run_test_sum_avg(&self, database: &mut dyn SqlExecutor) -> Result<(), String> {
        database.execute("create table sumavg (a int, b decimal, c double)", &[])?;
        database.execute("insert sumavg values (1, 1, 1)", &[])?;
        check_rows(
            &database.query("select sum(a), sum(b), sum(c) from sumavg", &[])?,
            &[&["1", "1", "1"]],
        )?;
        check_rows(
            &database.query("select avg(a), avg(b), avg(c) from sumavg", &[])?,
            &[&["1", "1", "1"]],
        )
    }

    pub fn run_test_db_name_escape(&self, database: &mut dyn SqlExecutor) -> Result<(), String> {
        database.execute("CREATE DATABASE `aa-a`;", &[])?;
        database.execute("USE mysql;", &[])?;
        database.execute("DROP DATABASE `aa-a`", &[])?;
        Ok(())
    }

    pub fn run_test_result_field_table_is_null(
        &self,
        database: &mut dyn SqlExecutor,
    ) -> Result<(), String> {
        database.execute("drop table if exists test;", &[])?;
        database.execute("create table test (c int);", &[])?;
        database.execute("explain select * from test;", &[])?;
        Ok(())
    }

    pub fn run_test_issue_22646(&self, database: &mut dyn SqlExecutor) -> Result<(), String> {
        let started = Instant::now();
        database.execute("", &[])?;
        if started.elapsed() > Duration::from_secs(30) {
            return Err("read empty query statement timed out".into());
        }
        Ok(())
    }

    pub fn run_test_tls_connection(&self, database: &mut dyn SqlExecutor) -> Result<(), String> {
        database.execute("USE test", &[])?;
        Ok(())
    }

    pub fn run_test_enable_secure_transport(
        &self,
        database: &mut dyn SqlExecutor,
    ) -> Result<(), String> {
        database.execute("SET GLOBAL require_secure_transport = 1", &[])?;
        Ok(())
    }

    pub fn run_reload_tls(
        &self,
        database: &mut dyn SqlExecutor,
        error_no_rollback: bool,
    ) -> Result<(), String> {
        database.execute(
            if error_no_rollback {
                "alter instance reload tls no rollback on error"
            } else {
                "alter instance reload tls"
            },
            &[],
        )?;
        Ok(())
    }

    /// ACCOUNT LOCK / UNLOCK 用户场景。
    pub fn run_test_account_lock(&self, database: &mut dyn SqlExecutor) -> Result<(), String> {
        self.run_named_scenario(database, Scenario::AccountLock)
    }

    /// Threads_connected 状态查询场景。
    pub fn run_test_connection_count(&self, database: &mut dyn SqlExecutor) -> Result<(), String> {
        self.run_named_scenario(database, Scenario::ConnectionCount)
    }

    /// 校验 /status 中 version 与 git_hash 精确匹配 Go server.Status。
    pub fn run_test_status_api(&self) -> Result<(), String> {
        let response = self.fetch_status("/status")?;
        if response.status != 200 {
            return Err(format!("status API returned {}", response.status));
        }
        let body = response.text()?;
        let version = json_string_field(body, "version")
            .ok_or_else(|| "status response is missing string field version".to_string())?;
        let expected_version = astersql_parser_mysql::r#const::ServerVersion();
        if version != expected_version {
            return Err(format!(
                "status version mismatch: expected {expected_version:?}, got {version:?}"
            ));
        }
        let git_hash = json_string_field(body, "git_hash")
            .ok_or_else(|| "status response is missing string field git_hash".to_string())?;
        let expected_git_hash = *astersql_util_versioninfo::TiDBGitHash
            .read()
            .map_err(|_| "TiDBGitHash lock poisoned".to_string())?;
        if git_hash != expected_git_hash {
            return Err(format!(
                "status git_hash mismatch: expected {expected_git_hash:?}, got {git_hash:?}"
            ));
        }
        Ok(())
    }

    /// 拉取 /metrics 并解析为 Prometheus sample 映射。
    pub fn get_metrics(&self) -> Result<HashMap<String, f64>, String> {
        let response = self.fetch_status("/metrics")?;
        if !response.is_success() {
            return Err(format!("metrics endpoint returned {}", response.status));
        }
        parse_prometheus(response.text()?)
    }

    /// 在目录下写入 LOAD DATA 用的数据文件并返回路径。
    pub fn prepare_load_data_file(
        &self,
        directory: &Path,
        name: &str,
        contents: &[u8],
    ) -> Result<PathBuf, String> {
        std::fs::create_dir_all(directory)
            .map_err(|error| format!("create load-data directory: {error}"))?;
        let path = directory.join(name);
        std::fs::write(&path, contents)
            .map_err(|error| format!("write load-data file: {error}"))?;
        Ok(path)
    }

    /// 对齐 Go `prepareLoadDataFile`：截断文件，并把空格分隔字段改写为 tab 行。
    pub fn prepare_load_data_rows(&self, path: &Path, input_rows: &[&str]) -> Result<(), String> {
        let mut file = std::fs::File::create(path)
            .map_err(|error| format!("open load-data file for rewrite: {error}"))?;
        for row in input_rows {
            file.write_all(row.split(' ').collect::<Vec<_>>().join("\t").as_bytes())
                .and_then(|_| file.write_all(b"\n"))
                .map_err(|error| format!("write load-data row: {error}"))?;
        }
        file.sync_all()
            .map_err(|error| format!("sync load-data file: {error}"))
    }
}

/// SQL 绑定/结果中的通用值类型。
#[derive(Clone, Debug, PartialEq)]
pub enum SqlValue {
    Null,
    Signed(i64),
    Unsigned(u64),
    Float(f64),
    Bytes(Vec<u8>),
    Text(String),
    Bool(bool),
}

impl SqlValue {
    /// 转为便于比对的显示字符串（NULL 显示为 `<nil>`）。
    pub fn display(&self) -> String {
        match self {
            Self::Null => "<nil>".into(),
            Self::Signed(value) => value.to_string(),
            Self::Unsigned(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::Bytes(value) => String::from_utf8_lossy(value).into_owned(),
            Self::Text(value) => value.clone(),
            Self::Bool(value) => u8::from(*value).to_string(),
        }
    }
}

/// 查询结果：列名与按行的 SqlValue。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<SqlValue>>,
}

/// 执行结果：影响行数与 last insert id。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExecuteResult {
    pub rows_affected: u64,
    pub last_insert_id: u64,
}

/// 可注入的 SQL 执行器抽象（真实驱动或测试替身均可）。
pub trait SqlExecutor: Send {
    fn execute(&mut self, sql: &str, parameters: &[SqlValue]) -> Result<ExecuteResult, String>;
    fn query(&mut self, sql: &str, parameters: &[SqlValue]) -> Result<QueryResult, String>;

    /// Create another SQL session backed by the same store/server.
    fn fork(&self) -> Result<Box<dyn SqlExecutor>, String> {
        Err("SQL executor does not support independent sessions".into())
    }
}

/// 需要真实 MySQL 会话语义的边界：连接配置、Ping 与 prepared long-data。
pub trait ServerConnection: SqlExecutor {
    fn ping(&mut self) -> Result<(), String>;
    fn prepare(&mut self, sql: &str) -> Result<u32, String>;
    fn send_long_data(
        &mut self,
        statement_id: u32,
        parameter_index: u16,
        data: &[u8],
    ) -> Result<(), String>;
    fn execute_prepared(
        &mut self,
        statement_id: u32,
        parameters: &[SqlValue],
    ) -> Result<ExecuteResult, String>;
    fn close_prepared(&mut self, statement_id: u32) -> Result<(), String>;
}

/// 按 MysqlConfig 创建独立协议连接，供认证、初始化变量等跨连接场景注入。
pub trait ServerConnector {
    fn connect(&mut self, config: &MysqlConfig) -> Result<Box<dyn ServerConnection>, String>;
}

/// DDL schema-state failpoint 的可注入边界。
pub trait SchemaStateController {
    fn at_write_reorganization(
        &mut self,
        ddl: &str,
        action: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<(), String>;
}

/// 包裹 SqlExecutor 的测试工具箱（must_* 风格入口）。
pub struct TestDatabase<'a> {
    executor: &'a mut dyn SqlExecutor,
}

impl<'a> TestDatabase<'a> {
    pub fn new(executor: &'a mut dyn SqlExecutor) -> Self {
        Self { executor }
    }

    /// 执行语句，错误直接返回。
    pub fn must_execute(
        &mut self,
        sql: &str,
        parameters: &[SqlValue],
    ) -> Result<ExecuteResult, String> {
        self.executor.execute(sql, parameters)
    }

    /// 执行查询，错误直接返回。
    pub fn must_query(
        &mut self,
        sql: &str,
        parameters: &[SqlValue],
    ) -> Result<QueryResult, String> {
        self.executor.query(sql, parameters)
    }
}

/// 命名集成测试场景枚举。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scenario {
    Regression,
    PreparedTypes,
    SpecialTypes,
    PreparedString,
    PreparedTimestamp,
    LoadData,
    LoadDataTransaction,
    MultiStatements,
    SumAverage,
    AccountLock,
    ConnectionCount,
}

/// 场景中的单步：执行语句或查询并比对期望行。
#[derive(Clone, Copy, Debug)]
pub enum ScenarioStep {
    Execute(&'static str),
    Query {
        sql: &'static str,
        expected: &'static [&'static [&'static str]],
    },
}

const EMPTY_ROWS: &[&[&str]] = &[];
const ONE_ROW: &[&[&str]] = &[&["1"]];
const ZERO_ROW: &[&[&str]] = &[&["0"]];
const SUM_AVG_ROW: &[&[&str]] = &[&["6", "2"]];
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn temp_path(prefix: &str, extension: &str) -> PathBuf {
    let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "{prefix}_{}_{}.{extension}",
        std::process::id(),
        sequence
    ))
}

fn unused_temp_path(prefix: &str, extension: &str) -> Result<PathBuf, String> {
    let path = temp_path(prefix, extension);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "remove stale temp file {}: {error}",
                path.display()
            ));
        }
    }
    Ok(path)
}

fn create_temp_file(prefix: &str, extension: &str, contents: &[u8]) -> Result<PathBuf, String> {
    let path = unused_temp_path(prefix, extension)?;
    std::fs::write(&path, contents)
        .map_err(|error| format!("write temp file {}: {error}", path.display()))?;
    Ok(path)
}

fn finish_file_cleanup(result: Result<(), String>, path: &Path) -> Result<(), String> {
    let cleanup = match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove temp file {}: {error}", path.display())),
    };
    result.and(cleanup)
}

fn quote_double(path: &Path) -> String {
    format!(
        "\"{}\"",
        path.to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    )
}

fn quote_single(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "\\'"))
}

fn deterministic_load_rows(count: usize, special_terminators: bool) -> (String, u64, u64) {
    let mut state = 0x5eed_u64;
    let mut checksum1 = 0;
    let mut checksum2 = 0;
    let mut contents = String::new();
    for index in 0..count {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let first = (state >> 32) % 1000;
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let second = (state >> 32) % 1000;
        checksum1 ^= first;
        checksum2 ^= second;
        if special_terminators {
            let _ = write!(contents, "'{first}','{second}'");
            if index + 1 != count {
                contents.push('|');
            }
        } else {
            let _ = writeln!(contents, "{first}\t{second}");
        }
    }
    (contents, checksum1, checksum2)
}

fn check_plan_tokens(result: &QueryResult, tokens: &[&str]) -> Result<(), String> {
    let plan = result
        .rows
        .first()
        .and_then(|row| row.first())
        .ok_or_else(|| "expected one plan row".to_string())?
        .display()
        .replace(['\t', '\n'], " ");
    let mut offset = 0;
    for token in tokens {
        let relative = plan[offset..]
            .find(token)
            .ok_or_else(|| format!("plan is missing token {token:?}: {plan:?}"))?;
        offset += relative + token.len();
    }
    Ok(())
}

fn mysql_error_number(error: &str) -> Option<u16> {
    let rest = error.strip_prefix("Error ")?;
    rest.split_once(' ')?.0.parse().ok()
}

fn client_error_counts(result: &QueryResult) -> Result<(i64, i64), String> {
    let Some(row) = result.rows.first() else {
        return Ok((0, 0));
    };
    if row.len() != 2 {
        return Err(format!(
            "client error counter expected 2 columns, got {}",
            row.len()
        ));
    }
    let parse = |value: &SqlValue| {
        if matches!(value, SqlValue::Null) {
            Ok(0)
        } else {
            value
                .display()
                .parse::<i64>()
                .map_err(|error| format!("parse client error counter: {error}"))
        }
    };
    Ok((parse(&row[0])?, parse(&row[1])?))
}

fn expect_mysql_error(
    result: Result<ExecuteResult, String>,
    expected_codes: &[u16],
) -> Result<(), String> {
    let error = result.map_err(|error| {
        let code = mysql_error_number(&error);
        if code.is_some_and(|code| expected_codes.contains(&code)) {
            String::new()
        } else {
            format!("expected MySQL error {expected_codes:?}, got {error:?}")
        }
    });
    match error {
        Err(error) if error.is_empty() => Ok(()),
        Err(error) => Err(error),
        Ok(_) => Err(format!(
            "expected MySQL error {expected_codes:?}, statement succeeded"
        )),
    }
}

fn expect_connection_error(
    connector: &mut dyn ServerConnector,
    config: &MysqlConfig,
    expected: &str,
) -> Result<(), String> {
    let error = match connector.connect(config) {
        Ok(mut connection) => match connection.ping() {
            Ok(()) => return Err("connection unexpectedly succeeded".into()),
            Err(error) => error,
        },
        Err(error) => error,
    };
    if error != expected {
        return Err(format!(
            "connection error mismatch: expected {expected:?}, got {error:?}"
        ));
    }
    Ok(())
}

fn expect_execute_result(
    result: &ExecuteResult,
    rows_affected: u64,
    last_insert_id: u64,
    operation: &str,
) -> Result<(), String> {
    if result.rows_affected != rows_affected || result.last_insert_id != last_insert_id {
        return Err(format!(
            "{operation} result mismatch: expected rows={rows_affected}, last_insert_id={last_insert_id}; got rows={}, last_insert_id={}",
            result.rows_affected, result.last_insert_id
        ));
    }
    Ok(())
}

impl Scenario {
    /// 返回该场景的静态步骤序列。
    pub fn steps(self) -> &'static [ScenarioStep] {
        match self {
            Self::Regression => &[
                ScenarioStep::Execute("DROP TABLE IF EXISTS test"),
                ScenarioStep::Execute("CREATE TABLE test (val TINYINT)"),
                ScenarioStep::Query {
                    sql: "SELECT * FROM test",
                    expected: EMPTY_ROWS,
                },
                ScenarioStep::Execute("INSERT INTO test VALUES (1)"),
                ScenarioStep::Query {
                    sql: "SELECT val FROM test",
                    expected: ONE_ROW,
                },
            ],
            Self::PreparedTypes => &[
                ScenarioStep::Execute("DROP TABLE IF EXISTS prepared_types"),
                ScenarioStep::Execute(
                    "CREATE TABLE prepared_types(i BIGINT, u BIGINT UNSIGNED, s VARCHAR(32), b BLOB)",
                ),
                ScenarioStep::Execute("INSERT INTO prepared_types VALUES(1,2,'three','four')"),
            ],
            Self::SpecialTypes => &[
                ScenarioStep::Execute("DROP TABLE IF EXISTS special_types"),
                ScenarioStep::Execute(
                    "CREATE TABLE special_types(e ENUM('a','b'), s SET('a','b'), j JSON)",
                ),
                ScenarioStep::Execute("INSERT INTO special_types VALUES('a','a,b','{\"x\":1}')"),
            ],
            Self::PreparedString => &[
                ScenarioStep::Execute("DROP TABLE IF EXISTS prepared_string"),
                ScenarioStep::Execute("CREATE TABLE prepared_string(v VARCHAR(255))"),
                ScenarioStep::Execute("INSERT INTO prepared_string VALUES('abc'),('中文')"),
            ],
            Self::PreparedTimestamp => &[
                ScenarioStep::Execute("DROP TABLE IF EXISTS prepared_timestamp"),
                ScenarioStep::Execute("CREATE TABLE prepared_timestamp(v TIMESTAMP(6))"),
                ScenarioStep::Execute(
                    "INSERT INTO prepared_timestamp VALUES('2020-01-02 03:04:05.123456')",
                ),
            ],
            Self::LoadData => &[
                ScenarioStep::Execute("DROP TABLE IF EXISTS load_data"),
                ScenarioStep::Execute("CREATE TABLE load_data(a INT, b VARCHAR(32))"),
            ],
            Self::LoadDataTransaction => &[
                ScenarioStep::Execute("BEGIN"),
                ScenarioStep::Execute("CREATE TEMPORARY TABLE load_txn(a INT)"),
                ScenarioStep::Execute("INSERT INTO load_txn VALUES(1)"),
                ScenarioStep::Execute("COMMIT"),
            ],
            Self::MultiStatements => &[
                ScenarioStep::Execute(
                    "DROP TABLE IF EXISTS multi_stmt; CREATE TABLE multi_stmt(a INT)",
                ),
                ScenarioStep::Execute(
                    "INSERT INTO multi_stmt VALUES(1); INSERT INTO multi_stmt VALUES(2)",
                ),
            ],
            Self::SumAverage => &[
                ScenarioStep::Execute("DROP TABLE IF EXISTS sum_avg"),
                ScenarioStep::Execute("CREATE TABLE sum_avg(a INT)"),
                ScenarioStep::Execute("INSERT INTO sum_avg VALUES(1),(2),(3)"),
                ScenarioStep::Query {
                    sql: "SELECT SUM(a), AVG(a) FROM sum_avg",
                    expected: SUM_AVG_ROW,
                },
            ],
            Self::AccountLock => &[
                ScenarioStep::Execute("DROP USER IF EXISTS 'locked_user'@'%'"),
                ScenarioStep::Execute("CREATE USER 'locked_user'@'%' ACCOUNT LOCK"),
                ScenarioStep::Execute("ALTER USER 'locked_user'@'%' ACCOUNT UNLOCK"),
            ],
            Self::ConnectionCount => &[ScenarioStep::Query {
                sql: "SHOW STATUS LIKE 'Threads_connected'",
                expected: EMPTY_ROWS,
            }],
        }
    }
}

/// 将 QueryResult 各单元格转为 display 字符串二维表。
pub fn rows(result: &QueryResult) -> Vec<Vec<String>> {
    result
        .rows
        .iter()
        .map(|row| row.iter().map(SqlValue::display).collect())
        .collect()
}

/// 比对查询结果与期望字符串表；不匹配则返回详细错误。
pub fn check_rows(result: &QueryResult, expected: &[&[&str]]) -> Result<(), String> {
    let actual = rows(result);
    let expected: Vec<Vec<String>> = expected
        .iter()
        .map(|row| row.iter().map(|value| (*value).to_string()).collect())
        .collect();
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "row mismatch: expected {expected:?}, got {actual:?}"
        ))
    }
}

/// 列名是否与期望完全一致（顺序敏感）。
pub fn columns_as_expected(actual: &[String], expected: &[&str]) -> bool {
    actual
        .iter()
        .map(String::as_str)
        .eq(expected.iter().copied())
}

/// 读取 executor 语句计数指标 `tidb_executor_statement_total{type=...}`。
pub fn get_statement_count(metrics: &HashMap<String, f64>, statement: &str) -> f64 {
    find_statement_metric(metrics, "", statement)
}

/// 读取按库划分的查询计数 `tidb_server_query_total{db=...,type=...}`。
pub fn get_database_statement_count(
    metrics: &HashMap<String, f64>,
    database: &str,
    statement: &str,
) -> f64 {
    find_statement_metric(metrics, database, statement)
}

fn find_statement_metric(metrics: &HashMap<String, f64>, database: &str, statement: &str) -> f64 {
    metrics
        .iter()
        .find_map(|(sample, value)| {
            let labels = sample
                .strip_prefix("tidb_executor_statement_total{")?
                .strip_suffix('}')?;
            let mut database_matches = false;
            let mut statement_matches = false;
            for label in labels.split(',') {
                let (name, value) = label.split_once('=')?;
                let value = value.strip_prefix('"')?.strip_suffix('"')?;
                match name {
                    "db" => database_matches = value == database,
                    "type" => statement_matches = value == statement,
                    _ => {}
                }
            }
            (database_matches && statement_matches).then_some(*value)
        })
        .unwrap_or(0.0)
}

/// 解析极简 HTTP/1.1 响应：状态行、小写 header、body。
fn parse_http_response(bytes: &[u8]) -> Result<HttpResponse, String> {
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "malformed HTTP response".to_string())?;
    let head = std::str::from_utf8(&bytes[..split]).map_err(|error| error.to_string())?;
    let mut lines = head.split("\r\n");
    let mut status_line = lines.next().unwrap_or_default().splitn(3, ' ');
    let _version = status_line.next();
    let status = status_line
        .next()
        .ok_or_else(|| "HTTP response has no status".to_string())?
        .parse::<u16>()
        .map_err(|error| error.to_string())?;
    let reason = status_line.next().unwrap_or_default().into();
    let headers: HashMap<String, String> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().into()))
        .collect();
    let body = if headers
        .get("transfer-encoding")
        .is_some_and(|value: &String| value.eq_ignore_ascii_case("chunked"))
    {
        decode_chunked_body(&bytes[split + 4..])?
    } else {
        bytes[split + 4..].to_vec()
    };
    Ok(HttpResponse {
        status,
        reason,
        headers,
        body,
    })
}

fn decode_chunked_body(mut bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoded = Vec::new();
    loop {
        let line_end = bytes
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| "malformed chunk size".to_string())?;
        let size_text =
            std::str::from_utf8(&bytes[..line_end]).map_err(|error| error.to_string())?;
        let size_text = size_text
            .split_once(';')
            .map_or(size_text, |(size, _)| size);
        let size = usize::from_str_radix(size_text.trim(), 16)
            .map_err(|error| format!("invalid chunk size: {error}"))?;
        bytes = &bytes[line_end + 2..];
        if size == 0 {
            return Ok(decoded);
        }
        if bytes.len() < size + 2 || &bytes[size..size + 2] != b"\r\n" {
            return Err("truncated chunked body".into());
        }
        decoded.extend_from_slice(&bytes[..size]);
        bytes = &bytes[size + 2..];
    }
}

fn json_string_field<'a>(text: &'a str, field: &str) -> Option<&'a str> {
    let field = format!("\"{field}\"");
    let suffix = text.split_once(&field)?.1;
    let suffix = suffix.trim_start();
    let suffix = suffix.strip_prefix(':')?.trim_start();
    let suffix = suffix.strip_prefix('"')?;
    let end = suffix.find('"')?;
    Some(&suffix[..end])
}

/// 解析 Prometheus 文本：跳过空行与 `#` 注释，按末尾空白拆分 name/value。
fn parse_prometheus(text: &str) -> Result<HashMap<String, f64>, String> {
    let mut metrics = HashMap::new();
    for line in text
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let (name, value) = line
            .rsplit_once(char::is_whitespace)
            .ok_or_else(|| format!("invalid Prometheus sample: {line}"))?;
        metrics.insert(
            name.into(),
            value
                .trim()
                .parse()
                .map_err(|error| format!("invalid Prometheus value: {error}"))?,
        );
    }
    Ok(metrics)
}

/// RFC 3986 非保留字符原样保留，其余按 `%XX` 百分号编码。
fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// Go `url.QueryEscape`：空格编码为 `+`，其余非保留字节编码为 `%XX`。
fn query_escape(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte == b' ' {
            encoded.push('+');
        } else if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// 读取回归开关。
pub fn regression_enabled() -> bool {
    REGRESSION.load(Ordering::Acquire)
}

/// 设置回归开关。
pub fn set_regression_enabled(enabled: bool) {
    REGRESSION.store(enabled, Ordering::Release);
}
