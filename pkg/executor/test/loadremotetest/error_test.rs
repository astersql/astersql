// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! 远程 `LOAD DATA` 的客户端错误契约，对应 Go `error_test.go`。
//!
//! 覆盖远程访问前校验、列数/赋值错误、约束模式及截断警告；通过本地 GCS 协议
//! 边界驱动 canonical session，避免用 parser 冒烟替代 executor 集成行为。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::testkit::TestKit;
use astersql_testkit::{Rows, RowsWithSep};

const STRICT_MODE: &str = "ONLY_FULL_GROUP_BY,STRICT_TRANS_TABLES,NO_ZERO_IN_DATE,NO_ZERO_DATE,ERROR_FOR_DIVISION_BY_ZERO,NO_AUTO_CREATE_USER,NO_ENGINE_SUBSTITUTION";

struct GcsServer {
    endpoint: String,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl GcsServer {
    fn start(objects: &[(&str, &[u8])]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let objects = objects
            .iter()
            .map(|(path, content)| ((*path).to_owned(), (*content).to_vec()))
            .collect::<HashMap<_, _>>();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            for incoming in listener.incoming() {
                let Ok(mut stream) = incoming else { break };
                if thread_stop.load(Ordering::Acquire) {
                    break;
                }
                let mut request = [0; 8192];
                let size = stream.read(&mut request).unwrap_or_default();
                let request = String::from_utf8_lossy(&request[..size]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or_default()
                    .split('?')
                    .next()
                    .unwrap_or_default()
                    .replace("%2E", ".")
                    .replace("%2e", ".")
                    .replace("%2D", "-")
                    .replace("%2d", "-")
                    .replace("%2F", "/")
                    .replace("%2f", "/");
                if let Some(content) = objects.get(&path) {
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"test\"\r\nLast-Modified: Wed, 21 Oct 2015 07:28:00 GMT\r\nConnection: close\r\n\r\n",
                        content.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(content);
                } else {
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            }
        });
        Self {
            endpoint,
            stop,
            thread: Some(thread),
        }
    }

    fn uri(&self, bucket: &str, object: &str) -> String {
        format!("gs://{bucket}/{object}?endpoint={}", self.endpoint)
    }
}

impl Drop for GcsServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.endpoint.trim_start_matches("http://"));
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn assert_client_error_contains(testkit: &mut TestKit, sql: &str, expected: &str) {
    let error = testkit.ExecToErr(sql);
    assert!(
        error.message().contains(expected),
        "sql={sql:?}, error={error}, expected fragment={expected:?}"
    );
}

/// Go `TestErrorMessage` 中所有发生在远程存储访问前的错误分支。
#[test]
fn test_error_message_before_remote_storage_access() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);

    testkit.MustExec("DROP DATABASE IF EXISTS load_csv", Vec::new());
    // Canonical Rust test sessions bootstrap with `test` selected; Go TestKit
    // starts this suite without a current database.
    testkit.MustExec("DROP DATABASE IF EXISTS test", Vec::new());
    assert_client_error_contains(
        &mut testkit,
        "LOAD DATA INFILE 'gs://1' INTO TABLE t",
        "ERROR 1046 (3D000): No database selected",
    );
    assert_client_error_contains(
        &mut testkit,
        "LOAD DATA INFILE 'gs://1' INTO TABLE wrongdb.t",
        "ERROR 1146 (42S02): Table 'wrongdb.t' doesn't exist",
    );

    testkit.MustExec("CREATE DATABASE load_csv", Vec::new());
    testkit.MustExec("USE load_csv", Vec::new());
    testkit.MustExec(
        "CREATE TABLE t (i INT PRIMARY KEY, s varchar(32))",
        Vec::new(),
    );

    assert_client_error_contains(
        &mut testkit,
        "LOAD DATA INFILE 'gs://1' INTO TABLE t (wrong)",
        "ERROR 1054 (42S22): Unknown column 'wrong' in 'field list'",
    );
    assert_client_error_contains(
        &mut testkit,
        "LOAD DATA INFILE 'gs://1' INTO TABLE t (i,i)",
        "ERROR 1110 (42000): Column 'i' specified twice",
    );
    assert_client_error_contains(
        &mut testkit,
        "LOAD DATA INFILE 'gs://1' INTO TABLE t (@v) SET wrong=@v",
        "ERROR 1054 (42S22): Unknown column 'wrong' in 'field list'",
    );
    assert_client_error_contains(
        &mut testkit,
        "LOAD DATA INFILE 'abc://1' INTO TABLE t",
        "ERROR 8158 (HY000): The URI of data source is invalid.",
    );
    assert_client_error_contains(
        &mut testkit,
        "LOAD DATA INFILE 's3://no-network' INTO TABLE t",
        "ERROR 8159 (HY000): Access to the data source has been denied. Reason: failed to get region of bucket no-network",
    );

    let server = GcsServer::start(&[("/test-tsv/t.tsv", b"1\t2\n1\t4\n")]);
    assert_client_error_contains(
        &mut testkit,
        &format!(
            "LOAD DATA INFILE '{}' INTO TABLE t",
            server.uri("wrong-bucket", "p")
        ),
        "ERROR 8160 (HY000): Failed to read source files. Reason: the object doesn't exist, file info: input.bucket='wrong-bucket', input.key='p'",
    );
    assert_client_error_contains(
        &mut testkit,
        &format!(
            "LOAD DATA INFILE '{}' INTO TABLE t LINES STARTING BY '\\n'",
            server.uri("test-tsv", "t.tsv")
        ),
        "ERROR 8162 (HY000): STARTING BY '\n' cannot contain LINES TERMINATED BY '\n'",
    );
}

#[test]
fn test_column_number_mismatch() {
    let server = GcsServer::start(&[("/test-tsv/t2.tsv", b"1\t2\n1\t4\n")]);
    let uri = server.uri("test-tsv", "t2.tsv");
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("CREATE DATABASE load_csv", Vec::new());
    testkit.MustExec("USE load_csv", Vec::new());
    testkit.MustExec("CREATE TABLE t (c INT)", Vec::new());
    testkit.MustExec("SET SESSION sql_mode = ''", Vec::new());

    testkit.MustExec(
        &format!("LOAD DATA INFILE '{uri}' INTO TABLE t"),
        Vec::new(),
    );
    assert_eq!(
        testkit.Session().LastMessage(),
        "Records: 2  Deleted: 0  Skipped: 0  Warnings: 2"
    );
    testkit
        .MustQuery("SHOW WARNINGS", Vec::new())
        .Check(RowsWithSep(
            "|",
            &[
                "Warning|1262|Row 1 was truncated; it contained more data than there were input columns",
                "Warning|1262|Row 2 was truncated; it contained more data than there were input columns",
            ],
        ));

    testkit.MustExec(
        &format!("SET SESSION sql_mode = '{STRICT_MODE}'"),
        Vec::new(),
    );
    for modifier in ["", "REPLACE "] {
        assert_client_error_contains(
            &mut testkit,
            &format!("LOAD DATA INFILE '{uri}' {modifier}INTO TABLE t"),
            "ERROR 1262 (01000): Row 1 was truncated; it contained more data than there were input columns",
        );
    }
    testkit.MustExec(
        &format!("LOAD DATA INFILE '{uri}' IGNORE INTO TABLE t"),
        Vec::new(),
    );
    assert_eq!(
        testkit.Session().LastMessage(),
        "Records: 2  Deleted: 0  Skipped: 0  Warnings: 2"
    );
    testkit
        .MustQuery("SHOW WARNINGS", Vec::new())
        .Check(RowsWithSep(
            "|",
            &[
                "Warning|1262|Row 1 was truncated; it contained more data than there were input columns",
                "Warning|1262|Row 2 was truncated; it contained more data than there were input columns",
            ],
        ));

    testkit.MustExec("CREATE TABLE t2 (c1 INT, c2 INT, c3 INT)", Vec::new());
    assert_client_error_contains(
        &mut testkit,
        &format!("LOAD DATA INFILE '{uri}' INTO TABLE t2"),
        "ERROR 1261 (01000): Row 1 doesn't contain data for all columns",
    );
    testkit.MustExec(
        "CREATE TABLE t3 (c1 INT NOT NULL, c2 INT NOT NULL, c3 INT NOT NULL DEFAULT 1)",
        Vec::new(),
    );
    testkit.MustExec(
        &format!("LOAD DATA INFILE '{uri}' INTO TABLE t3 (c1, c2)"),
        Vec::new(),
    );
    testkit
        .MustQuery("SELECT * FROM t3", Vec::new())
        .Check(Rows(&["1 2 1", "1 4 1"]));
}

#[test]
fn test_assignment_evaluation_errors() {
    let server = GcsServer::start(&[("/test-tsv/t3.tsv", b"1\t2\n1\t4\n")]);
    let uri = server.uri("test-tsv", "t3.tsv");
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("CREATE DATABASE load_csv", Vec::new());
    testkit.MustExec("USE load_csv", Vec::new());
    testkit.MustExec("CREATE TABLE t (c INT, c2 INT UNIQUE)", Vec::new());
    testkit.MustExec(
        &format!("SET SESSION sql_mode = '{STRICT_MODE}'"),
        Vec::new(),
    );

    for modifier in ["", "REPLACE "] {
        assert_client_error_contains(
            &mut testkit,
            &format!("LOAD DATA INFILE '{uri}' {modifier}INTO TABLE t (@v1, c2) SET c=@v1+'asd'"),
            "ERROR 1292 (22007): Truncated incorrect DOUBLE value: 'asd'",
        );
    }
    testkit.MustExec(
        &format!("LOAD DATA INFILE '{uri}' IGNORE INTO TABLE t (@v1, c2) SET c=@v1+'asd'"),
        Vec::new(),
    );
    assert_eq!(
        testkit.Session().LastMessage(),
        "Records: 2  Deleted: 0  Skipped: 0  Warnings: 2"
    );
    testkit
        .MustQuery("SHOW WARNINGS", Vec::new())
        .Check(RowsWithSep(
            "|",
            &[
                "Warning|1292|Truncated incorrect DOUBLE value: 'asd'",
                "Warning|1292|Truncated incorrect DOUBLE value: 'asd'",
            ],
        ));
    testkit
        .MustQuery("SELECT * FROM t", Vec::new())
        .Check(Rows(&["1 2", "1 4"]));
}

#[test]
fn test_data_errors_and_duplicate_modes() {
    let server = GcsServer::start(&[
        ("/test-tsv/null.tsv", b"1\t\\N\n1\t4\n"),
        ("/test-tsv/t4.tsv", b"1\t2\n1\t2\n"),
        ("/test-tsv/t5.tsv", b"1\t100\n2\t100\n"),
    ]);
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("CREATE DATABASE load_csv", Vec::new());
    testkit.MustExec("USE load_csv", Vec::new());
    testkit.MustExec(
        "CREATE TABLE t (c INT NOT NULL, c2 INT NOT NULL)",
        Vec::new(),
    );
    testkit.MustExec(
        &format!("SET SESSION sql_mode = '{STRICT_MODE}'"),
        Vec::new(),
    );
    let null_uri = server.uri("test-tsv", "null.tsv");
    assert_client_error_contains(
        &mut testkit,
        &format!("LOAD DATA INFILE '{null_uri}' INTO TABLE t"),
        "ERROR 1263 (22004): Column set to default value; NULL supplied to NOT NULL column 'c2' at row 1",
    );
    testkit.MustExec(
        &format!("LOAD DATA INFILE '{null_uri}' IGNORE INTO TABLE t"),
        Vec::new(),
    );
    assert_eq!(
        testkit.Session().LastMessage(),
        "Records: 2  Deleted: 0  Skipped: 0  Warnings: 1"
    );
    testkit
        .MustQuery("SHOW WARNINGS", Vec::new())
        .Check(RowsWithSep(
            "|",
            &["Warning|1263|Column set to default value; NULL supplied to NOT NULL column 'c2' at row 1"],
        ));

    let duplicate_uri = server.uri("test-tsv", "t4.tsv");
    testkit.MustExec(
        "CREATE TABLE t2 (c INT PRIMARY KEY, c2 INT NOT NULL)",
        Vec::new(),
    );
    assert_client_error_contains(
        &mut testkit,
        &format!("LOAD DATA INFILE '{duplicate_uri}' INTO TABLE t2"),
        "ERROR 1062 (23000): Duplicate entry '1' for key 't2.PRIMARY'",
    );
    testkit.MustExec("CREATE TABLE t3 (c INT, c2 INT UNIQUE)", Vec::new());
    assert_client_error_contains(
        &mut testkit,
        &format!("LOAD DATA INFILE '{duplicate_uri}' INTO TABLE t3"),
        "ERROR 1062 (23000): Duplicate entry '2' for key 't3.c2'",
    );

    let replace_uri = server.uri("test-tsv", "t5.tsv");
    testkit.MustExec(
        &format!("LOAD DATA INFILE '{replace_uri}' REPLACE INTO TABLE t3"),
        Vec::new(),
    );
    testkit
        .MustQuery("SHOW WARNINGS", Vec::new())
        .Check(Rows(&[]));
    testkit
        .MustQuery("SELECT * FROM t3", Vec::new())
        .Check(Rows(&["2 100"]));
    testkit.MustExec("UPDATE t3 SET c = 3", Vec::new());
    testkit.MustExec(
        &format!("LOAD DATA INFILE '{replace_uri}' IGNORE INTO TABLE t3"),
        Vec::new(),
    );
    testkit
        .MustQuery("SHOW WARNINGS", Vec::new())
        .Check(RowsWithSep(
            "|",
            &[
                "Warning|1062|Duplicate entry '100' for key 't3.c2'",
                "Warning|1062|Duplicate entry '100' for key 't3.c2'",
            ],
        ));
    testkit
        .MustQuery("SELECT * FROM t3", Vec::new())
        .Check(Rows(&["3 100"]));
}

#[test]
fn test_issue_43555_truncation_and_missing_columns() {
    let server = GcsServer::start(&[("/test-csv/43555.csv", b"6\n7.1\n")]);
    let uri = server.uri("test-csv", "43555.csv");
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("CREATE DATABASE load_csv", Vec::new());
    testkit.MustExec("USE load_csv", Vec::new());
    testkit.MustExec("CREATE TABLE t (id CHAR(1), id1 INT)", Vec::new());
    testkit.MustExec(
        &format!("SET SESSION sql_mode = '{STRICT_MODE}'"),
        Vec::new(),
    );

    testkit.MustExec(
        &format!("LOAD DATA INFILE '{uri}' IGNORE INTO TABLE t"),
        Vec::new(),
    );
    assert_eq!(
        testkit.Session().LastMessage(),
        "Records: 2  Deleted: 0  Skipped: 0  Warnings: 3"
    );
    testkit
        .MustQuery("SHOW WARNINGS", Vec::new())
        .Check(RowsWithSep(
            "|",
            &[
                "Warning|1261|Row 1 doesn't contain data for all columns",
                "Warning|1261|Row 2 doesn't contain data for all columns",
                "Warning|1265|Data truncated for column 'id' at row 2",
            ],
        ));
    testkit
        .MustQuery("SELECT * FROM t", Vec::new())
        .Check(Rows(&["6 <nil>", "7 <nil>"]));

    assert_client_error_contains(
        &mut testkit,
        &format!("LOAD DATA INFILE '{uri}' INTO TABLE t (id)"),
        "ERROR 1265 (01000): Data truncated for column 'id' at row 2",
    );
    testkit.MustExec(
        &format!("LOAD DATA INFILE '{uri}' IGNORE INTO TABLE t (id1) SET id='7.1'"),
        Vec::new(),
    );
    assert_eq!(
        testkit.Session().LastMessage(),
        "Records: 2  Deleted: 0  Skipped: 0  Warnings: 2"
    );
    testkit
        .MustQuery("SHOW WARNINGS", Vec::new())
        .Check(RowsWithSep(
            "|",
            &[
                "Warning|1265|Data truncated for column 'id' at row 1",
                "Warning|1265|Data truncated for column 'id' at row 2",
            ],
        ));
}
