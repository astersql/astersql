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

//! Go `multi_file_test.go` 的远程多文件 LOAD DATA 集成契约。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use astersql_testkit::Rows;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::testkit::TestKit;

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
                serve_request(&mut stream, &objects);
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

fn serve_request(stream: &mut TcpStream, objects: &HashMap<String, Vec<u8>>) {
    let mut request = [0; 8192];
    let size = stream.read(&mut request).unwrap_or_default();
    let request = String::from_utf8_lossy(&request[..size]);
    let first = request.lines().next().unwrap_or_default();
    let method = first.split_whitespace().next().unwrap_or_default();
    let target = first.split_whitespace().nth(1).unwrap_or_default();
    let path = percent_decode(target.split('?').next().unwrap_or_default());
    let query = target.split_once('?').map_or("", |(_, query)| query);
    if query.contains("list-type=2") {
        let bucket = path.trim_start_matches('/');
        let prefix = query_value(query, "prefix")
            .map(percent_decode)
            .unwrap_or_default();
        let mut keys = objects
            .keys()
            .filter_map(|object_path| {
                let key = object_path.strip_prefix(&format!("/{bucket}/"))?;
                key.starts_with(&prefix).then_some(key)
            })
            .collect::<Vec<_>>();
        keys.sort_unstable();
        let contents = keys.into_iter().map(|key| {
            let size = objects[&format!("/{bucket}/{key}")].len();
            format!("<Contents><Key>{key}</Key><LastModified>2015-10-21T07:28:00Z</LastModified><ETag>&quot;test&quot;</ETag><Size>{size}</Size></Contents>")
        }).collect::<String>();
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult>{contents}</ListBucketResult>"
        );
        write_response(stream, method, "200 OK", "application/xml", body.as_bytes());
    } else if let Some(content) = objects.get(&path) {
        write_response(
            stream,
            method,
            "200 OK",
            "application/octet-stream",
            content,
        );
    } else {
        write_response(stream, method, "404 Not Found", "text/plain", b"");
    }
}

fn query_value<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        (key == name).then_some(value)
    })
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&value[index + 1..index + 3], 16) {
                decoded.push(byte);
                index += 3;
                continue;
            }
        }
        decoded.push(if bytes[index] == b'+' {
            b' '
        } else {
            bytes[index]
        });
        index += 1;
    }
    String::from_utf8(decoded).unwrap()
}

fn write_response(
    stream: &mut TcpStream,
    method: &str,
    status: &str,
    content_type: &str,
    body: &[u8],
) {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nETag: \"test\"\r\nLast-Modified: Wed, 21 Oct 2015 07:28:00 GMT\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).unwrap();
    if method != "HEAD" {
        stream.write_all(body).unwrap();
    }
}

fn new_testkit() -> TestKit {
    let (store, _domain) = CreateMockStoreAndDomain();
    TestKit::new(store)
}

#[test]
fn test_filename_asterisk() {
    let server = GcsServer::start(&[
        ("/test-multi-load/db.tbl.001.tsv", b"1\ttest1\n2\ttest2"),
        ("/test-multi-load/db.tbl.002.tsv", b"3\ttest3\n4\ttest4"),
        ("/test-multi-load/db.tbl.003.tsv", b"5\ttest5\n6\ttest6"),
        ("/not-me/db.tbl.001.tsv", b"9\ttest9\n10\ttest10"),
    ]);
    let mut tk = new_testkit();
    tk.MustExec("DROP DATABASE IF EXISTS multi_load", Vec::new());
    tk.MustExec("CREATE DATABASE multi_load", Vec::new());
    tk.MustExec(
        "CREATE TABLE multi_load.t (i INT PRIMARY KEY, s varchar(32))",
        Vec::new(),
    );
    tk.MustExec(
        &format!(
            "LOAD DATA INFILE '{}' INTO TABLE multi_load.t WITH thread=2",
            server.uri("test-multi-load", "db.tbl.*.tsv")
        ),
        Vec::new(),
    );
    tk.MustQuery("SELECT LAST_INSERT_ID()", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustQuery("SELECT * FROM multi_load.t ORDER BY i", Vec::new())
        .Check(Rows(&[
            "1 test1", "2 test2", "3 test3", "4 test4", "5 test5", "6 test6",
        ]));
    tk.MustExec("TRUNCATE TABLE multi_load.t", Vec::new());
    tk.MustExec(
        &format!(
            "LOAD DATA INFILE '{}' INTO TABLE multi_load.t IGNORE 1 LINES WITH thread=20",
            server.uri("test-multi-load", "db.tbl.*.tsv")
        ),
        Vec::new(),
    );
    tk.MustQuery("SELECT LAST_INSERT_ID()", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustQuery("SELECT * FROM multi_load.t ORDER BY i", Vec::new())
        .Check(Rows(&["2 test2", "4 test4", "6 test6"]));
    tk.MustExec("TRUNCATE TABLE multi_load.t", Vec::new());
    tk.MustExec(
        &format!(
            "LOAD DATA INFILE '{}' INTO TABLE multi_load.t WITH thread=1",
            server.uri("test-multi-load", "db.tbl.00[13].tsv")
        ),
        Vec::new(),
    );
    tk.MustQuery("SELECT LAST_INSERT_ID()", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustQuery("SELECT * FROM multi_load.t ORDER BY i", Vec::new())
        .Check(Rows(&["1 test1", "2 test2", "5 test5", "6 test6"]));
}

#[test]
fn test_last_insert_id() {
    let server = GcsServer::start(&[
        ("/last-insert-id/db.tbl.001.tsv", b"1\ttest1\n2\ttest2"),
        ("/last-insert-id/db.tbl.002.tsv", b"3\ttest3\n4\ttest4"),
    ]);
    let mut tk = new_testkit();
    tk.MustExec("DROP DATABASE IF EXISTS multi_load", Vec::new());
    tk.MustExec("CREATE DATABASE multi_load", Vec::new());
    tk.MustExec(
        "CREATE TABLE multi_load.t (i INT auto_increment PRIMARY KEY, s varchar(32))",
        Vec::new(),
    );
    tk.MustExec(
        &format!(
            "LOAD DATA INFILE '{}' INTO TABLE multi_load.t (@1, s) WITH thread=1",
            server.uri("last-insert-id", "db.tbl.00*.tsv")
        ),
        Vec::new(),
    );
    tk.MustQuery("SELECT LAST_INSERT_ID()", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery("SELECT * FROM multi_load.t ORDER BY i", Vec::new())
        .Check(Rows(&["1 test1", "2 test2", "3 test3", "4 test4"]));
}

#[test]
fn test_multi_batch_with_ignore_lines() {
    let first = (1..=10)
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let second = (11..=20)
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let server = GcsServer::start(&[
        ("/test-multi-load/multi-batch.001.tsv", first.as_bytes()),
        ("/test-multi-load/multi-batch.002.tsv", second.as_bytes()),
    ]);
    let mut tk = new_testkit();
    tk.MustExec("DROP DATABASE IF EXISTS multi_load", Vec::new());
    tk.MustExec("CREATE DATABASE multi_load", Vec::new());
    tk.MustExec("CREATE TABLE multi_load.t2 (i INT)", Vec::new());
    tk.MustExec(&format!("LOAD DATA INFILE '{}' INTO TABLE multi_load.t2 IGNORE 2 LINES WITH batch_size=3, thread=1", server.uri("test-multi-load", "multi-batch.*.tsv")), Vec::new());
    tk.MustQuery("SELECT * FROM multi_load.t2 ORDER BY i", Vec::new())
        .Check(Rows(&[
            "3", "4", "5", "6", "7", "8", "9", "10", "13", "14", "15", "16", "17", "18", "19", "20",
        ]));
}

#[test]
fn test_mixed_compression() {
    const GZIP: &[u8] = &[
        0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x33, 0xd4, 0x29, 0x49, 0x2d,
        0x2e, 0x31, 0xe4, 0x32, 0x02, 0xd3, 0x46, 0x5c, 0xc6, 0x60, 0xda, 0x98, 0xcb, 0x04, 0x4c,
        0x9b, 0x00, 0x00, 0x3f, 0xf7, 0x31, 0xb6, 0x1f, 0x00, 0x00, 0x00,
    ];
    let server = GcsServer::start(&[
        ("/test-multi-load/compress.001.tsv.gz", GZIP),
        (
            "/test-multi-load/compress.002.tsv",
            b"5,test5\n6,test6\n7,test7\n8,test8\n9,test9",
        ),
    ]);
    let mut tk = new_testkit();
    tk.MustExec("DROP DATABASE IF EXISTS multi_load", Vec::new());
    tk.MustExec("CREATE DATABASE multi_load", Vec::new());
    tk.MustExec(
        "CREATE TABLE multi_load.t (i INT PRIMARY KEY, s varchar(32))",
        Vec::new(),
    );
    let uri = server.uri("test-multi-load", "compress.*");
    tk.MustExec(
        &format!("LOAD DATA INFILE '{uri}' INTO TABLE multi_load.t FIELDS TERMINATED BY ','"),
        Vec::new(),
    );
    tk.MustQuery("SELECT * FROM multi_load.t ORDER BY i", Vec::new())
        .Check(Rows(&[
            "1 test1", "2 test2", "3 test3", "4 test4", "5 test5", "6 test6", "7 test7", "8 test8",
            "9 test9",
        ]));
    tk.MustExec("TRUNCATE TABLE multi_load.t", Vec::new());
    tk.MustExec(&format!("LOAD DATA INFILE '{uri}' INTO TABLE multi_load.t FIELDS TERMINATED BY ',' IGNORE 3 LINES"), Vec::new());
    tk.MustQuery("SELECT * FROM multi_load.t ORDER BY i", Vec::new())
        .Check(Rows(&["4 test4", "8 test8", "9 test9"]));
}
