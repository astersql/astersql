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

// 远程 LOAD 单文件 CSV 字段语义测试。
//
// 对应 Go `pkg/executor/test/loadremotetest` 中单 CSV 导入的核心解析：
// 双引号包围字段内的逗号不作为分隔符；字面 `\N` 映射为 SQL NULL（Datum::Null）。

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

/// 冒烟：引号内逗号保留为字段内容，第三列 `\N` 解析为 `Datum::Null`。
#[test]
fn csv_parser_preserves_quoted_commas_and_mysql_null() {
    use astersql_lightning_mydump::{CsvConfig, Datum, NewCSVParser, NewStringReader, Parser};
    let mut parser = NewCSVParser(
        &CsvConfig::default(),
        Box::new(NewStringReader("1,\"hello,world\",\\N\n")),
        false,
        None,
    )
    .unwrap();
    parser.ReadRow().unwrap();
    assert_eq!(
        parser.LastRow().row,
        vec![
            Datum::Bytes(b"1".to_vec()),
            Datum::Bytes(b"hello,world".to_vec()),
            Datum::Null,
        ]
    );
}

/// Go `TestLoadCSV` 的无尾换行和带尾换行两种文件形态必须得到相同记录。
#[test]
fn load_csv_accepts_files_with_or_without_final_newline() {
    use astersql_lightning_mydump::{CsvConfig, Datum, NewCSVParser, NewStringReader, Parser};

    for input in [
        "i,s\n100,\"test100\"\n101,\"\\\"\"\n102,\"😄😄😄😄😄\"\n104,\"\"",
        "i,s\n100,\"test100\"\n101,\"\\\"\"\n102,\"😄😄😄😄😄\"\n104,\"\"\n",
    ] {
        let mut parser = NewCSVParser(
            &CsvConfig::default(),
            Box::new(NewStringReader(input)),
            true,
            None,
        )
        .unwrap();
        let expected = [
            vec![
                Datum::Bytes(b"100".to_vec()),
                Datum::Bytes(b"test100".to_vec()),
            ],
            vec![Datum::Bytes(b"101".to_vec()), Datum::Bytes(b"\"".to_vec())],
            vec![
                Datum::Bytes(b"102".to_vec()),
                Datum::Bytes("😄😄😄😄😄".as_bytes().to_vec()),
            ],
            vec![Datum::Bytes(b"104".to_vec()), Datum::Bytes(Vec::new())],
        ];
        for row in expected {
            parser.ReadRow().unwrap();
            assert_eq!(parser.LastRow().row, row);
        }
        assert!(matches!(
            parser.ReadRow(),
            Err(astersql_lightning_mydump::Error::Eof)
        ));
    }
}

/// Go `TestIgnoreNLines`：忽略行按物理 terminator 计数，即使 terminator 在引号内。
#[test]
fn load_csv_ignore_lines_counts_physical_terminators() {
    use astersql_lightning_mydump::{CsvConfig, Datum, NewCSVParser, NewStringReader, Parser};
    let mut parser = NewCSVParser(
        &CsvConfig {
            fields_terminated_by: ",".into(),
            fields_enclosed_by: "\"".into(),
            ..CsvConfig::default()
        },
        Box::new(NewStringReader("\"a\n\",1\n\"b\n\",2\n\"c\",3\n")),
        false,
        None,
    )
    .unwrap();
    parser.ReadUntilTerminator().unwrap();
    parser.ReadUntilTerminator().unwrap();
    parser.ReadRow().unwrap();
    assert_eq!(
        parser.LastRow().row,
        vec![Datum::Bytes(b"b\n".to_vec()), Datum::Bytes(b"2".to_vec())]
    );
}

/// Go `TestCustomizeNULL`：NULL 标记、引号字段和自定义 NULL 标记各自保持边界。
#[test]
fn load_csv_honors_null_marker_and_quoted_null_mode() {
    use astersql_lightning_mydump::{CsvConfig, Datum, NewCSVParser, NewStringReader, Parser};

    let mut config = CsvConfig {
        quoted_null_is_text: true,
        ..CsvConfig::default()
    };
    let mut parser = NewCSVParser(
        &config,
        Box::new(NewStringReader("\\N,\"\\N\"\n!N,\"!N\"\nNULL,\"NULL\"\n")),
        false,
        None,
    )
    .unwrap();
    parser.ReadRow().unwrap();
    assert_eq!(
        parser.LastRow().row,
        vec![Datum::Null, Datum::Bytes(b"N".to_vec())]
    );
    parser.ReadRow().unwrap();
    assert_eq!(
        parser.LastRow().row,
        vec![Datum::Bytes(b"!N".to_vec()), Datum::Bytes(b"!N".to_vec())]
    );

    config.null = "NULL".into();
    let mut custom = NewCSVParser(
        &config,
        Box::new(NewStringReader("NULL,\"NULL\"\nvalue,\"value\"\n")),
        false,
        None,
    )
    .unwrap();
    custom.ReadRow().unwrap();
    assert_eq!(
        custom.LastRow().row,
        vec![Datum::Null, Datum::Bytes(b"NULL".to_vec())]
    );
}

/// Go `TestGBK`/`TestOtherCharset`：输入字符集转换发生在字段切分之后的解码阶段。
#[test]
fn load_csv_decodes_gbk_and_latin1_input() {
    use astersql_lightning_mydump::{
        CsvConfig, Datum, NewCSVParser, NewCharsetConvertor, NewStringReader, Parser,
    };

    let gbk = NewCharsetConvertor("gb18030", "�").unwrap();
    let encoded = gbk.Encode("1\t你好\n").unwrap();
    let mut parser = NewCSVParser(
        &CsvConfig {
            fields_terminated_by: "\t".into(),
            ..CsvConfig::default()
        },
        Box::new(astersql_lightning_mydump::StringReader::from_bytes(encoded)),
        false,
        Some(gbk),
    )
    .unwrap();
    parser.ReadRow().unwrap();
    assert_eq!(
        parser.LastRow().row,
        vec![
            Datum::Bytes(b"1".to_vec()),
            Datum::Bytes("你好".as_bytes().to_vec())
        ]
    );

    let latin1 = NewCharsetConvertor("latin1", "�").unwrap();
    let mut parser = NewCSVParser(
        &CsvConfig {
            fields_terminated_by: "\t".into(),
            ..CsvConfig::default()
        },
        Box::new(astersql_lightning_mydump::StringReader::from_bytes(vec![
            b'1', b'\t', 0xa1, 0xa2,
        ])),
        false,
        Some(latin1),
    )
    .unwrap();
    parser.ReadRow().unwrap();
    assert_eq!(
        parser.LastRow().row,
        vec![
            Datum::Bytes(b"1".to_vec()),
            Datum::Bytes("¡¢".as_bytes().to_vec())
        ]
    );
}

/// Go `TestColumnsAndUserVars` 的字段映射基础：解析出的字段数量和顺序不改变。
#[test]
fn load_csv_preserves_column_and_user_variable_field_order() {
    use astersql_lightning_mydump::{CsvConfig, Datum, NewCSVParser, NewStringReader, Parser};
    let mut parser = NewCSVParser(
        &CsvConfig::default(),
        Box::new(NewStringReader("1,first,100\n2,second,200\n")),
        false,
        None,
    )
    .unwrap();
    parser.ReadRow().unwrap();
    assert_eq!(
        parser.LastRow().row,
        vec![
            Datum::Bytes(b"1".to_vec()),
            Datum::Bytes(b"first".to_vec()),
            Datum::Bytes(b"100".to_vec()),
        ]
    );
    parser.ReadRow().unwrap();
    assert_eq!(parser.LastRow().row.len(), 3);
}

#[test]
fn test_load_csv_and_transaction_contract() {
    let server = GcsServer::start(&[
        ("/test-load-csv/no-newline.csv", b"i,s\n100,\"test100\"\n101,\"\\\"\"\n102,\"\xf0\x9f\x98\x84\xf0\x9f\x98\x84\"\n104,\"\""),
        ("/test-load-csv/newline.csv", b"i,s\n100,\"test100\"\n101,\"\\\"\"\n102,\"\xf0\x9f\x98\x84\xf0\x9f\x98\x84\"\n104,\"\"\n"),
        ("/test-load-csv/data.csv", b"100,test100\n101,hello\n102,world\n104,bye"),
    ]);
    let mut tk = new_testkit();
    tk.MustExec("DROP DATABASE IF EXISTS load_csv", Vec::new());
    tk.MustExec("CREATE DATABASE load_csv", Vec::new());
    tk.MustExec("CREATE TABLE load_csv.t (i INT, s varchar(32))", Vec::new());
    for object in ["no-newline.csv", "newline.csv"] {
        tk.MustExec(&format!("LOAD DATA INFILE '{}' INTO TABLE load_csv.t FIELDS TERMINATED BY ',' OPTIONALLY ENCLOSED BY '\"' LINES TERMINATED BY '\\n' IGNORE 1 LINES", server.uri("test-load-csv", object)), Vec::new());
        tk.MustQuery("SELECT * FROM load_csv.t ORDER BY i", Vec::new())
            .Check(Rows(&["100 test100", "101 \"", "102 😄😄", "104 "]));
        tk.MustExec("TRUNCATE TABLE load_csv.t", Vec::new());
    }
    let error = tk.ExecToErr("LOAD DATA INFILE '/etc/passwd' INTO TABLE load_csv.t");
    assert!(
        error
            .message()
            .contains("Don't support load data from tidb-server's disk")
    );

    let load = format!(
        "LOAD DATA INFILE '{}' INTO TABLE load_csv.t FIELDS TERMINATED BY ','",
        server.uri("test-load-csv", "data.csv")
    );
    tk.MustExec("begin pessimistic", Vec::new());
    tk.MustExec("insert into load_csv.t values (1, 'a')", Vec::new());
    tk.MustExec(&load, Vec::new());
    tk.MustQuery("select i from load_csv.t order by i", Vec::new())
        .Check(Rows(&["1", "100", "101", "102", "104"]));
    tk.MustExec("rollback", Vec::new());
    tk.MustQuery("select * from load_csv.t", Vec::new())
        .Check(Rows(&[]));
    tk.MustExec("begin pessimistic", Vec::new());
    tk.MustExec(&load, Vec::new());
    tk.MustExec("commit", Vec::new());
    tk.MustQuery("select i from load_csv.t order by i", Vec::new())
        .Check(Rows(&["100", "101", "102", "104"]));
}

#[test]
fn test_ignore_lines_and_generated_columns_contract() {
    let server = GcsServer::start(&[
        (
            "/test-bucket/ignore.csv",
            b"\"bad syntax\"1\n\"b\",2\n\"c\",3\n",
        ),
        (
            "/test-bucket/quoted-lines.csv",
            b"\"a\n\",1\n\"b\n\",2\n\"c\",3\n",
        ),
        ("/test-bucket/generated.tsv", b"1\t2\n2\t3"),
    ]);
    let mut tk = new_testkit();
    tk.MustExec("DROP DATABASE IF EXISTS load_csv", Vec::new());
    tk.MustExec("CREATE DATABASE load_csv", Vec::new());
    tk.MustExec("CREATE TABLE load_csv.t (s varchar(32), i INT)", Vec::new());
    tk.MustExec(&format!("LOAD DATA INFILE '{}' INTO TABLE load_csv.t FIELDS TERMINATED BY ',' OPTIONALLY ENCLOSED BY '\"' IGNORE 1 LINES", server.uri("test-bucket", "ignore.csv")), Vec::new());
    tk.MustQuery("SELECT * FROM load_csv.t ORDER BY i", Vec::new())
        .Check(Rows(&["b 2", "c 3"]));
    tk.MustExec("TRUNCATE TABLE load_csv.t", Vec::new());
    tk.MustExec(&format!("LOAD DATA INFILE '{}' INTO TABLE load_csv.t FIELDS TERMINATED BY ',' OPTIONALLY ENCLOSED BY '\"' IGNORE 100 LINES", server.uri("test-bucket", "ignore.csv")), Vec::new());
    tk.MustQuery("SELECT * FROM load_csv.t", Vec::new())
        .Check(Rows(&[]));
    tk.MustExec(&format!("LOAD DATA INFILE '{}' INTO TABLE load_csv.t FIELDS TERMINATED BY ',' OPTIONALLY ENCLOSED BY '\"' IGNORE 2 LINES", server.uri("test-bucket", "quoted-lines.csv")), Vec::new());
    tk.MustQuery("SELECT * FROM load_csv.t ORDER BY i", Vec::new())
        .Check(Rows(&["b\n 2", "c 3"]));

    tk.MustExec("set @@sql_mode = ''", Vec::new());
    tk.MustExec(
        "CREATE TABLE load_csv.t_gen1 (a int, b int generated ALWAYS AS (a+1))",
        Vec::new(),
    );
    let uri = server.uri("test-bucket", "generated.tsv");
    tk.MustExec(
        &format!("LOAD DATA INFILE '{uri}' INTO TABLE load_csv.t_gen1"),
        Vec::new(),
    );
    tk.MustQuery("select * from load_csv.t_gen1 order by a", Vec::new())
        .Check(Rows(&["1 2", "2 3"]));
    tk.MustExec(
        "CREATE TABLE load_csv.t_gen2 (a int generated ALWAYS AS (b+1), b int)",
        Vec::new(),
    );
    tk.MustExec(
        &format!("LOAD DATA INFILE '{uri}' INTO TABLE load_csv.t_gen2"),
        Vec::new(),
    );
    tk.MustQuery("select * from load_csv.t_gen2 order by b", Vec::new())
        .Check(Rows(&["3 2", "4 3"]));
}

#[test]
fn test_multi_value_index_and_user_variables_contract() {
    let server = GcsServer::start(&[
        ("/test-load/one.csv", b"i,s\n1,\"[1,2,3]\"\n2,\"[2,3,4]\""),
        ("/test-load/cols-1.tsv", b"1,11,111\n2,22,222\n"),
        ("/test-load/cols-2.tsv", b"3,33,333\n4,44,444\n"),
    ]);
    let mut tk = new_testkit();
    tk.MustExec("DROP DATABASE IF EXISTS load_data", Vec::new());
    tk.MustExec("CREATE DATABASE load_data", Vec::new());
    tk.MustExec(
        "CREATE TABLE load_data.mvi (i INT, j JSON, KEY idx ((cast(j as signed array))))",
        Vec::new(),
    );
    tk.MustExec(&format!("LOAD DATA INFILE '{}' INTO TABLE load_data.mvi FIELDS TERMINATED BY ',' OPTIONALLY ENCLOSED BY '\"' IGNORE 1 LINES", server.uri("test-load", "one.csv")), Vec::new());
    tk.MustQuery("SELECT * FROM load_data.mvi ORDER BY i", Vec::new())
        .Check(Rows(&["1 [1, 2, 3]", "2 [2, 3, 4]"]));
    tk.MustExec(
        "CREATE TABLE load_data.cols_and_vars (a INT, b INT, c INT)",
        Vec::new(),
    );
    tk.MustExec(&format!("LOAD DATA INFILE '{}' INTO TABLE load_data.cols_and_vars FIELDS TERMINATED BY ',' (@V1, @v2, @v3) SET a=@V1, b=@V2*10, c=123", server.uri("test-load", "cols-*.tsv")), Vec::new());
    tk.MustQuery(
        "SELECT * FROM load_data.cols_and_vars ORDER BY a",
        Vec::new(),
    )
    .Check(Rows(&["1 110 123", "2 220 123", "3 330 123", "4 440 123"]));
}
