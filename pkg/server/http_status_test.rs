// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// Status HTTP 监听器的端到端测试。
//
// 通过临时端口启动完整测试服务器，并直接发送 HTTP 请求，验证健康状态、
// Prometheus 指标响应以及服务器关闭时两个监听器的生命周期。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use crate::http_status::{Ballast, Method, Request, serve_error};
use crate::server::{Domain, Server, ServerConfig, ServerDriver, StatusConfig};
use astersql_planner_extstore::{Context, NewExtStorage, SetGlobalExtStorageForTest};

/// 为测试服务器提供稳定名称的最小驱动实现。
struct Driver;
impl ServerDriver for Driver {
    fn name(&self) -> &str {
        "status-test"
    }
}

/// 提供 status 响应所需固定元数据的测试 Domain。
struct TestDomain;
impl Domain for TestDomain {
    fn server_id(&self) -> u64 {
        11
    }

    fn start_timestamp(&self) -> i64 {
        1
    }
}

fn ballast_request(method: Method, body: &[u8]) -> Request {
    Request {
        method,
        path: "/debug/ballast-object-sz".into(),
        query: HashMap::new(),
        raw_query: String::new(),
        headers: HashMap::new(),
        body: body.to_vec(),
    }
}

#[test]
fn ballast_handler_matches_go_method_body_and_error_contract() {
    let ballast = Arc::new(Ballast::new(16));

    let response = ballast.handler(&ballast_request(Method::Post, b"8"));
    assert_eq!(response.status, 200);
    assert!(response.body.is_empty());
    assert_eq!(ballast.size(), 8);

    let response = ballast.handler(&ballast_request(Method::Post, b" 4 "));
    assert_eq!(response.status, 400);
    assert_eq!(ballast.size(), 8);

    let response = ballast.handler(&ballast_request(Method::Post, b"-1"));
    assert_eq!(response.status, 400);
    assert!(response.headers.is_empty());
    assert_eq!(
        String::from_utf8(response.body).expect("UTF-8 error response"),
        "newSz cannot be negative: -1"
    );

    let response = ballast.handler(&ballast_request(Method::Post, b"17"));
    assert_eq!(response.status, 400);
    assert_eq!(
        String::from_utf8(response.body).expect("UTF-8 error response"),
        "newSz cannot be bigger than 16 but it has value 17"
    );

    let response = ballast.handler(&ballast_request(Method::Put, b""));
    assert_eq!(response.status, 200);
    assert!(response.body.is_empty());
}

#[test]
fn serve_error_matches_go_headers_and_line_terminated_body() {
    let response = serve_error(500, "broken");
    assert_eq!(response.status, 500);
    assert_eq!(
        response.headers["Content-Type"],
        "text/plain; charset=utf-8"
    );
    assert_eq!(response.headers["X-Go-Pprof"], "1");
    assert_eq!(response.body, b"broken\n");
}

#[test]
/// 验证 `/status` 可用，并且关闭服务器会同步停止 status 与 MySQL 监听器。
fn status_listener_reports_health_and_exits_during_shutdown() {
    let server = Server::new_test(
        ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
            status: StatusConfig {
                report_status: true,
                host: "127.0.0.1".into(),
                port: 0,
                ..StatusConfig::default()
            },
            ..ServerConfig::default()
        },
        Arc::new(Driver),
    );
    server
        .run(Arc::new(TestDomain))
        .expect("start status listener");
    let status_addr = server
        .status_listener_addr()
        .expect("status listener address");
    let mysql_addr = server.listener_addr().expect("MySQL listener address");

    let mut stream = TcpStream::connect(status_addr).expect("connect status listener");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set status timeout");
    stream
        .write_all(b"GET /status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .expect("write status request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read status response");
    assert!(response.starts_with("HTTP/1.1 200 OK"));
    assert!(response.contains("\"connections\":0"));

    server.close();
    assert!(!server.health());
    assert!(TcpStream::connect(status_addr).is_err());
    assert!(TcpStream::connect(mysql_addr).is_err());
}

#[test]
fn schema_route_uses_tikv_domain_runtime() {
    let (domain, _) =
        astersql_session::runtime::CreateAnalyzeSession().expect("initialize canonical domain");
    domain
        .ddl_create_database("schema_route_test", false)
        .expect("create schema");
    domain
        .ddl_create_table(
            "schema_route_test",
            astersql_meta_model::TableInfo {
                Name: astersql_parser_ast::NewCIStr("schema_route_table"),
                ..Default::default()
            },
            false,
        )
        .expect("create table");
    let server = Server::new_test(
        ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
            status: StatusConfig {
                report_status: true,
                host: "127.0.0.1".into(),
                port: 0,
                ..StatusConfig::default()
            },
            ..ServerConfig::default()
        },
        Arc::new(Driver),
    );
    server
        .run(Arc::new(crate::runtime::CanonicalServerDomain::new(domain)))
        .expect("start canonical status listener");
    let address = server.status_listener_addr().expect("status address");
    let mut stream = TcpStream::connect(address).expect("connect status listener");
    stream
        .write_all(b"GET /schema HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .expect("write schema request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read schema response");
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("schema_route_test"), "{response}");
    assert!(response.contains("schema_route_table"), "{response}");

    let mut stream = TcpStream::connect(address).expect("connect status listener");
    stream
        .write_all(b"GET /schema/schema_route_test HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .expect("write database schema request");
    let mut database_response = String::new();
    stream
        .read_to_string(&mut database_response)
        .expect("read database schema response");
    assert!(
        database_response.starts_with("HTTP/1.1 200 OK"),
        "{database_response}"
    );
    assert!(
        database_response.contains("schema_route_table"),
        "{database_response}"
    );
    server.close();
}

#[test]
fn tiflash_report_updates_canonical_domain_replica() {
    let (domain, _) =
        astersql_session::runtime::CreateAnalyzeSession().expect("initialize canonical domain");
    domain
        .ddl_create_database("replica_report_test", false)
        .expect("create schema");
    let table = domain
        .ddl_create_table(
            "replica_report_test",
            astersql_meta_model::TableInfo {
                Name: astersql_parser_ast::NewCIStr("t"),
                ..Default::default()
            },
            false,
        )
        .expect("create table");
    domain
        .ddl_set_tiflash_replica("replica_report_test", "t", 1, Vec::new())
        .expect("configure replica");
    let server = Server::new_test(
        ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
            status: StatusConfig {
                report_status: true,
                host: "127.0.0.1".into(),
                port: 0,
                ..StatusConfig::default()
            },
            ..ServerConfig::default()
        },
        Arc::new(Driver),
    );
    server
        .run(Arc::new(crate::runtime::CanonicalServerDomain::new(
            Arc::clone(&domain),
        )))
        .expect("start canonical status listener");
    let address = server.status_listener_addr().expect("status address");
    for (flash_regions, available) in [(1, false), (2, true), (1, false)] {
        let body = format!(
            "{{\"id\":{},\"region_count\":2,\"flash_region_count\":{flash_regions}}}",
            table.ID
        );
        let mut stream = TcpStream::connect(address).expect("connect status listener");
        stream
            .write_all(
                format!(
                    "POST /tiflash/replica-deprecated HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .expect("write TiFlash report");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("read TiFlash response");
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        assert_eq!(
            domain
                .table_by_name("replica_report_test", "t")
                .expect("table")
                .TiFlashReplica
                .as_ref()
                .expect("replica")
                .Available,
            available
        );
    }
    server.close();
}

#[test]
/// 验证 `/metrics` 返回 Prometheus 文本格式的成功响应。
fn status_listener_exposes_prometheus_metrics() {
    let server = Server::new_test(
        ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
            status: StatusConfig {
                report_status: true,
                host: "127.0.0.1".into(),
                port: 0,
                ..StatusConfig::default()
            },
            ..ServerConfig::default()
        },
        Arc::new(Driver),
    );
    server
        .run(Arc::new(TestDomain))
        .expect("start status listener");
    let status_addr = server
        .status_listener_addr()
        .expect("status listener address");

    let mut stream = TcpStream::connect(status_addr).expect("connect status listener");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set metrics timeout");
    stream
        .write_all(b"GET /metrics HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .expect("write metrics request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read metrics response");

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(
        response.contains("Content-Type: text/plain; version=0.0.4"),
        "{response}"
    );
    server.close();
}

#[test]
fn status_listener_downloads_plan_replayer_from_ext_storage() {
    let root = std::env::temp_dir().join(format!(
        "astersql-plan-replayer-status-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("create storage root");
    let context = Context::background();
    let storage = NewExtStorage(&context, &format!("file://{}", root.display()), "")
        .expect("create local ext storage");
    storage
        .WriteFile(&context, "replayer/replayer_test.zip", b"zip-body")
        .expect("write plan replayer fixture");
    SetGlobalExtStorageForTest(Some(Arc::clone(&storage)));

    let server = Server::new_test(
        ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
            status: StatusConfig {
                report_status: true,
                host: "127.0.0.1".into(),
                port: 0,
                ..StatusConfig::default()
            },
            ..ServerConfig::default()
        },
        Arc::new(Driver),
    );
    server.run(Arc::new(TestDomain)).expect("start server");
    let status_addr = server.status_listener_addr().expect("status address");

    let mut stream = TcpStream::connect(status_addr).expect("connect status listener");
    stream
        .write_all(
            b"GET /plan_replayer/dump/replayer_test.zip HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .expect("write download request");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).expect("read response");
    let separator = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("HTTP header separator");
    let headers = String::from_utf8_lossy(&response[..separator]);
    assert!(headers.starts_with("HTTP/1.1 200 OK"), "{headers}");
    assert!(
        headers.contains("Content-Type: application/zip"),
        "{headers}"
    );
    assert!(
        headers.contains("filename=\"plan_replayer.zip\""),
        "{headers}"
    );
    assert_eq!(&response[separator + 4..], b"zip-body");

    server.close();
    SetGlobalExtStorageForTest(None);
    storage.Close();
    std::fs::remove_dir_all(&root).expect("remove storage root");
}

#[test]
fn maintenance_routes_reject_user_keyspace_before_runtime_access() {
    struct UserDomain;
    impl Domain for UserDomain {
        fn server_id(&self) -> u64 {
            1
        }
        fn start_timestamp(&self) -> i64 {
            0
        }
        fn dxf_history_available(&self) -> bool {
            false
        }
    }
    let server = Server::new_test(
        ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
            status: StatusConfig {
                report_status: true,
                host: "127.0.0.1".into(),
                port: 0,
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(Driver),
    );
    server.run(Arc::new(UserDomain)).unwrap();
    let router = crate::http_status::build_status_router(server.clone());
    for path in [
        "/dxf/schedule/task_cleanup_batch_size",
        "/dxf/schedule/max_concurrent_task",
    ] {
        for method in [Method::Get, Method::Post] {
            let request = Request {
                method,
                path: path.into(),
                query: HashMap::new(),
                raw_query: String::new(),
                headers: HashMap::new(),
                body: vec![],
            };
            assert_eq!(router.handle(&request).status, 404);
        }
    }
    server.close();
}

#[test]
fn profiling_routes_log_request_fields_over_tcp() {
    use astersql_util_logutil::log::{LogField, background_logger};
    let server = Server::new_test(
        ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
            status: StatusConfig {
                report_status: true,
                host: "127.0.0.1".into(),
                port: 0,
                ..StatusConfig::default()
            },
            ..ServerConfig::default()
        },
        Arc::new(Driver),
    );
    server.run(Arc::new(TestDomain)).unwrap();
    let logger = background_logger();
    for route in [
        "/debug/pprof/",
        "/debug/pprof/heap?debug=1&gc=1",
        "/debug/pprof/goroutine?debug=2",
        "/debug/pprof/allocs?debug=1",
        "/debug/pprof/block?debug=1",
        "/debug/pprof/threadcreate?debug=1",
        "/debug/pprof/cmdline",
        "/debug/pprof/profile?seconds=5",
        "/debug/pprof/mutex?debug=1",
        "/debug/pprof/symbol",
        "/debug/pprof/trace",
        "/debug/zip?seconds=1",
        "/debug/gogc",
        "/debug/ballast-object-sz",
    ] {
        let mut stream = TcpStream::connect(server.status_listener_addr().unwrap()).unwrap();
        let remote = stream.local_addr().unwrap().to_string();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        write!(stream, "GET {route} HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(
            response.starts_with("HTTP/1.1 200 OK"),
            "{route}: {response}"
        );
        let entries: Vec<_> = logger
            .entries()
            .into_iter()
            .filter(|entry| {
                entry.message == "profiling request received"
                    && entry
                        .fields
                        .contains(&LogField::String("remote-addr".into(), remote.clone()))
            })
            .collect();
        let profiling = route.starts_with("/debug/pprof/") || route.starts_with("/debug/zip");
        assert_eq!(entries.len(), usize::from(profiling), "{route}");
        if profiling {
            let (path, query) = route.split_once('?').unwrap_or((route, ""));
            let fields = &entries[0].fields;
            assert!(fields.contains(&LogField::String("method".into(), "GET".into())));
            assert!(fields.contains(&LogField::String("path".into(), path.into())));
            for pair in query.split('&').filter(|pair| !pair.is_empty()) {
                let (key, value) = pair.split_once('=').unwrap();
                assert!(fields.contains(&LogField::String(key.into(), value.into())));
            }
        }
    }
    server.close();
}

#[test]
fn profiling_log_query_values_match_go_first_value_and_decode_rules() {
    use astersql_util_logutil::log::{LogField, background_logger};
    let server = Server::new_test(ServerConfig::default(), Arc::new(Driver));
    let router = crate::http_status::build_status_router(server);
    let logger = background_logger();
    for (raw, expected) in [
        (
            "seconds=%35&seconds=9&debug=2&gc=1&ignored=secret",
            vec![("seconds", "5"), ("debug", "2"), ("gc", "1")],
        ),
        ("seconds=&seconds=9&debug&gc=", vec![]),
        ("seconds=%ZZ&debug=1&gc=2;bad=1", vec![("debug", "1")]),
        (
            "%73econds=5&debug=a+b&gc=%2B",
            vec![("seconds", "5"), ("debug", "a b"), ("gc", "+")],
        ),
    ] {
        let before = logger.entries().len();
        let mut request = ballast_request(Method::Get, b"");
        request.path = "/debug/pprof/log-query-contract".into();
        request.raw_query = raw.into();
        assert_eq!(router.handle(&request).status, 200);
        let entries = logger.entries();
        let entries: Vec<_> = entries[before..]
            .iter()
            .filter(|entry| {
                entry
                    .fields
                    .contains(&LogField::String("path".into(), request.path.clone()))
            })
            .collect();
        assert_eq!(entries.len(), 1);
        let fields = &entries[0].fields;
        for key in ["seconds", "debug", "gc"] {
            let actual: Vec<_> = fields.iter().filter(|field| field.key() == key).collect();
            let wanted = expected.iter().find(|(name, _)| *name == key);
            assert_eq!(
                actual,
                wanted
                    .map(|(_, value)| LogField::String(key.into(), (*value).into()))
                    .as_ref()
                    .into_iter()
                    .collect::<Vec<_>>()
            );
        }
        assert!(!fields.iter().any(|field| field.key() == "ignored"));
    }
    // The wrapper must log even when the downstream handler rejects a method.
    let before = logger.entries().len();
    let mut request = ballast_request(Method::Post, b"");
    request.path = "/debug/zip".into();
    assert_eq!(router.handle(&request).status, 405);
    assert!(logger.entries()[before..].iter().any(|entry| {
        entry.message == "profiling request received"
            && entry
                .fields
                .contains(&LogField::String("method".into(), "POST".into()))
    }));
}
