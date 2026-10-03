// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// Server 单元测试：连接上限注册、关闭模式与 capability/正常关闭缓存。

use super::server::*;
use crate::conn::{ConnectionDomain, SessionDriver};
use crate::runtime::{BootstrapAuthMode, CanonicalConnectionDomain, ConcreteSessionDriver};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// 测试用 ServerDriver，名称固定为 tidb。
struct Driver;
impl ServerDriver for Driver {
    fn name(&self) -> &str {
        "tidb"
    }
}
/// 可记录 close 的测试 ManagedConnection。
struct Connection {
    id: u64,
    internal: bool,
    closed: AtomicBool,
    query_killed: AtomicBool,
}
impl ManagedConnection for Connection {
    fn id(&self) -> u64 {
        self.id
    }
    fn capability(&self) -> u32 {
        7
    }
    fn process_info(&self) -> Option<ProcessInfo> {
        Some(ProcessInfo {
            connection_id: self.id,
            user: "alice".into(),
            host: "127.0.0.1".into(),
            internal: self.internal,
            ..Default::default()
        })
    }
    fn transaction_info(&self) -> Option<TransactionInfo> {
        None
    }
    fn connection_attributes(&self) -> HashMap<String, String> {
        HashMap::from([("program".into(), "mysql".into())])
    }
    fn status_variables(&self) -> HashMap<String, String> {
        HashMap::new()
    }
    fn update_cpu_time(&self, _: u64, _: Duration) {}
    fn kill_query(&self, _: bool, _: bool) {
        self.query_killed.store(true, Ordering::Release);
    }
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }
}

struct TestDomain;
impl Domain for TestDomain {
    fn server_id(&self) -> u64 {
        7
    }

    fn start_timestamp(&self) -> i64 {
        1
    }
}

fn start_real_server() -> Arc<Server> {
    let (domain, _) =
        astersql_session::runtime::CreateAnalyzeSession().expect("initialize canonical domain");
    let session_driver: Arc<dyn SessionDriver> = Arc::new(ConcreteSessionDriver::new_for_test(
        Arc::clone(&domain),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let connection_domain: Arc<dyn ConnectionDomain> =
        Arc::new(CanonicalConnectionDomain::new(domain));
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
        .set_connection_runtime(session_driver, connection_domain)
        .expect("install canonical connection runtime");
    server
        .run(Arc::new(TestDomain))
        .expect("start real listeners");
    server
}

fn read_packet(stream: &mut TcpStream) -> Vec<u8> {
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).expect("read packet header");
    let length =
        usize::from(header[0]) | (usize::from(header[1]) << 8) | (usize::from(header[2]) << 16);
    let mut payload = vec![0_u8; length];
    stream
        .read_exact(&mut payload)
        .expect("read packet payload");
    payload
}

fn write_packet(stream: &mut TcpStream, sequence: u8, payload: &[u8]) {
    let length = payload.len();
    let header = [
        length as u8,
        (length >> 8) as u8,
        (length >> 16) as u8,
        sequence,
    ];
    stream.write_all(&header).expect("write packet header");
    stream.write_all(payload).expect("write packet payload");
    stream.flush().expect("flush packet");
}

fn root_handshake(stream: &mut TcpStream) {
    root_handshake_with_capability(stream, 0);
}

fn root_handshake_with_capability(stream: &mut TcpStream, extra_capability: u32) {
    let initial = read_packet(stream);
    assert_eq!(initial[0], 10, "server must emit protocol v10");
    assert!(
        initial
            .windows("8.0.11-TiDB-AsterSQL".len())
            .any(|value| value == b"8.0.11-TiDB-AsterSQL")
    );

    const CLIENT_PROTOCOL_41: u32 = 1 << 9;
    const CLIENT_SECURE_CONNECTION: u32 = 1 << 15;
    const CLIENT_PLUGIN_AUTH: u32 = 1 << 19;
    let capability =
        CLIENT_PROTOCOL_41 | CLIENT_SECURE_CONNECTION | CLIENT_PLUGIN_AUTH | extra_capability;
    let mut response = Vec::new();
    response.extend_from_slice(&capability.to_le_bytes());
    response.extend_from_slice(&(64_u32 << 20).to_le_bytes());
    response.push(45);
    response.extend_from_slice(&[0; 23]);
    response.extend_from_slice(b"root\0");
    response.push(0);
    response.extend_from_slice(b"mysql_native_password\0");
    write_packet(stream, 1, &response);
    assert_eq!(read_packet(stream).first(), Some(&0));
}

#[test]
fn deprecate_eof_result_set_starts_rows_immediately_after_columns() {
    const CLIENT_DEPRECATE_EOF: u32 = 1 << 24;

    let server = start_real_server();
    let mysql_addr = server.listener_addr().expect("MySQL listener address");
    let mut stream = TcpStream::connect(mysql_addr).expect("connect real MySQL listener");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set client timeout");
    root_handshake_with_capability(&mut stream, CLIENT_DEPRECATE_EOF);

    let mut query = vec![0x03];
    query.extend_from_slice(b"SELECT 1");
    write_packet(&mut stream, 0, &query);
    assert_eq!(read_packet(&mut stream), [1]);
    let _column = read_packet(&mut stream);
    assert_eq!(read_packet(&mut stream), [1, b'1']);
    assert_eq!(read_packet(&mut stream).first(), Some(&0xfe));

    server.close();
}

#[test]
fn write_sql_resp_cursor_execute_and_fetch_use_production_lifecycle() {
    let server = start_real_server();
    let mysql_addr = server.listener_addr().expect("MySQL listener address");
    let mut stream = TcpStream::connect(mysql_addr).expect("connect real MySQL listener");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set client timeout");
    root_handshake(&mut stream);

    let mut prepare = vec![0x16];
    prepare.extend_from_slice(b"SELECT 1 UNION ALL SELECT 2");
    write_packet(&mut stream, 0, &prepare);
    let prepared = read_packet(&mut stream);
    assert_eq!(prepared.first(), Some(&0));
    let statement_id = u32::from_le_bytes(prepared[1..5].try_into().unwrap());
    assert_eq!(u16::from_le_bytes(prepared[5..7].try_into().unwrap()), 1);
    let _prepare_column = read_packet(&mut stream);
    assert_eq!(read_packet(&mut stream).first(), Some(&0xfe));

    let mut execute = vec![0x17];
    execute.extend_from_slice(&statement_id.to_le_bytes());
    execute.push(1);
    execute.extend_from_slice(&1_u32.to_le_bytes());
    write_packet(&mut stream, 0, &execute);
    assert_eq!(read_packet(&mut stream), [1]);
    let _execute_column = read_packet(&mut stream);
    assert_eq!(read_packet(&mut stream).first(), Some(&0xfe));

    for expected in [b'1', b'2'] {
        let mut fetch = vec![0x1c];
        fetch.extend_from_slice(&statement_id.to_le_bytes());
        fetch.extend_from_slice(&1_u32.to_le_bytes());
        write_packet(&mut stream, 0, &fetch);
        let row = read_packet(&mut stream);
        assert_eq!(row[0], 0);
        assert_eq!(row.last(), Some(&expected));
        assert_eq!(read_packet(&mut stream).first(), Some(&0xfe));
    }

    server.close();
}

#[test]
fn show_processlist_uses_live_server_session_manager() {
    let server = start_real_server();
    let mysql_addr = server.listener_addr().expect("MySQL listener address");
    let mut stream = TcpStream::connect(mysql_addr).expect("connect real MySQL listener");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set client timeout");
    root_handshake(&mut stream);

    let mut query = vec![0x03];
    query.extend_from_slice(b"SHOW PROCESSLIST");
    write_packet(&mut stream, 0, &query);

    assert_eq!(
        read_packet(&mut stream),
        [8],
        "SHOW must expose eight columns"
    );
    for _ in 0..8 {
        let column = read_packet(&mut stream);
        assert_ne!(
            column.first(),
            Some(&0xff),
            "column metadata must not be ERR"
        );
    }
    assert_eq!(read_packet(&mut stream).first(), Some(&0xfe));
    let row = read_packet(&mut stream);
    assert!(
        row.windows(b"root".len()).any(|value| value == b"root"),
        "process row must contain the authenticated user"
    );
    assert!(
        row.windows(b"Query".len()).any(|value| value == b"Query"),
        "current SHOW statement must be reported as a query"
    );
    assert!(
        row.windows(b"SHOW PROCESSLIST".len())
            .any(|value| value == b"SHOW PROCESSLIST"),
        "current SHOW SQL must be visible in its own process row"
    );
    assert!(
        row.contains(&0xfb),
        "empty process database must be encoded as MySQL NULL"
    );
    assert_eq!(read_packet(&mut stream).first(), Some(&0xfe));

    server.close();
}

fn wait_until(timeout: Duration, predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + timeout;
    while !predicate() {
        assert!(Instant::now() < deadline, "condition did not become true");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
/// 验证 max_connections=1 时二次注册失败，注销后关机模式拒绝新连接。
fn connection_limit_registration_and_shutdown_are_consistent() {
    let server = Server::new_test(
        ServerConfig {
            max_connections: 1,
            ..Default::default()
        },
        Arc::new(Driver),
    );
    let first = Arc::new(Connection {
        id: 1,
        internal: false,
        closed: AtomicBool::new(false),
        query_killed: AtomicBool::new(false),
    });
    server.register_connection(first.clone()).unwrap();
    assert_eq!(server.connection_count(), 1);
    assert!(
        server
            .register_connection(Arc::new(Connection {
                id: 2,
                internal: false,
                closed: AtomicBool::new(false),
                query_killed: AtomicBool::new(false),
            }))
            .is_err()
    );
    assert_eq!(server.show_process_list()[&1].user, "alice");
    assert_eq!(server.client_capability_list()[&1], 7);
    assert!(server.unregister_connection(1));
    server.enter_shutdown_mode();
    assert!(server.register_connection(first).is_err());
}

#[test]
fn performance_schema_connection_totals_exclude_internal_sessions() {
    let server = Server::new_test(ServerConfig::default(), Arc::new(Driver));
    server
        .register_connection(Arc::new(Connection {
            id: 1,
            internal: false,
            closed: AtomicBool::new(false),
            query_killed: AtomicBool::new(false),
        }))
        .unwrap();
    server
        .register_connection(Arc::new(Connection {
            id: 2,
            internal: true,
            closed: AtomicBool::new(false),
            query_killed: AtomicBool::new(false),
        }))
        .unwrap();

    let summaries =
        astersql_session_sessmgr::Manager::GetPerformanceSchemaAccountSummaries(server.as_ref());
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].user.as_deref(), Some("alice"));
    assert_eq!(summaries[0].host.as_deref(), Some("127.0.0.1"));
    assert_eq!(summaries[0].current_connections, 1);
    assert_eq!(summaries[0].total_connections, 1);
}

#[test]
fn kill_connection_also_interrupts_the_running_query() {
    let server = Server::new_test(ServerConfig::default(), Arc::new(Driver));
    let connection = Arc::new(Connection {
        id: 1,
        internal: false,
        closed: AtomicBool::new(false),
        query_killed: AtomicBool::new(false),
    });
    server.register_connection(connection.clone()).unwrap();

    assert!(server.kill(1, false, false, false, None));
    assert!(connection.closed.load(Ordering::Acquire));
    assert!(
        connection.query_killed.load(Ordering::Acquire),
        "KILL CONNECTION must also cancel the running query"
    );
}

#[test]
/// 验证 capability 位运算与 normal_closed_connection 缓存读写。
fn capability_and_normal_close_cache_round_trip() {
    let server = Server::new_test(ServerConfig::default(), Arc::new(Driver));
    let original = server.capability();
    server.xor_capability(0x10);
    assert_eq!(server.capability(), original ^ 0x10);
    server.add_capability(0x10);
    assert_ne!(server.capability() & 0x10, 0);
    server.set_normal_closed_connection("ks", "42", "gateway");
    assert_eq!(
        server.normal_closed_connection("ks", "42").as_deref(),
        Some("gateway")
    );
}

#[test]
fn shutdown_flags_do_not_start_server_shutdown() {
    let server = Server::new_test(ServerConfig::default(), Arc::new(Driver));
    assert!(!server.force_shutdown());
    assert!(!server.need_request_manager_free());
    assert!(!server.is_shutdown());

    server.set_force_shutdown();
    server.set_need_request_manager_free();

    assert!(server.force_shutdown());
    assert!(server.need_request_manager_free());
    assert!(
        !server.is_shutdown(),
        "setting force-shutdown is only a flag and must not enter shutdown mode"
    );
}

#[test]
fn real_listener_serves_handshake_ping_select_and_drains_connection() {
    let server = start_real_server();
    let mysql_addr = server.listener_addr().expect("MySQL listener address");
    let mut stream = TcpStream::connect(mysql_addr).expect("connect real MySQL listener");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set client timeout");

    root_handshake(&mut stream);
    wait_until(Duration::from_secs(2), || server.connection_count() == 1);

    write_packet(&mut stream, 0, &[0x0e]);
    assert_eq!(read_packet(&mut stream).first(), Some(&0));

    let mut query = vec![0x03];
    query.extend_from_slice(b"SELECT 1");
    write_packet(&mut stream, 0, &query);
    assert_eq!(read_packet(&mut stream), [1]);
    let _column = read_packet(&mut stream);
    assert_eq!(read_packet(&mut stream).first(), Some(&0xfe));
    assert_eq!(read_packet(&mut stream), [1, b'1']);
    assert_eq!(read_packet(&mut stream).first(), Some(&0xfe));

    server.drain_clients(Duration::ZERO, Duration::ZERO);
    wait_until(Duration::from_secs(2), || server.connection_count() == 0);
    let mut closed = [0_u8; 1];
    assert_eq!(stream.read(&mut closed).unwrap_or(0), 0);
    wait_until(Duration::from_secs(2), || {
        TcpStream::connect(mysql_addr).is_err()
    });
    server.close();
    assert!(TcpStream::connect(mysql_addr).is_err());
}

#[test]
fn postgres_listener_lifecycle() {
    let config = ServerConfig {
        host: "127.0.0.1".into(),
        port: 0,
        postgres_port: Some(0),
        status: StatusConfig {
            report_status: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let server = Server::new_test(config.clone(), Arc::new(Driver));
    server.run(Arc::new(TestDomain)).unwrap();
    let pg = server.postgres_listener_addr().unwrap();
    let mysql = server.listener_addr().unwrap();
    assert_ne!(pg, mysql);
    drop(TcpStream::connect(pg).unwrap());
    drop(TcpStream::connect(mysql).unwrap());
    server.close();
    assert!(server.postgres_listener_addr().is_none());
    let rebound = std::net::TcpListener::bind(pg).unwrap();
    drop(rebound);

    let disabled = Server::new_test(
        ServerConfig {
            postgres_port: None,
            ..config.clone()
        },
        Arc::new(Driver),
    );
    disabled.run(Arc::new(TestDomain)).unwrap();
    assert!(disabled.postgres_listener_addr().is_none());
    drop(TcpStream::connect(disabled.listener_addr().unwrap()).unwrap());
    disabled.close();

    let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let blocked = Server::new_test(
        ServerConfig {
            postgres_port: Some(occupied.local_addr().unwrap().port()),
            ..config
        },
        Arc::new(Driver),
    );
    assert!(blocked.run(Arc::new(TestDomain)).is_err());
    assert!(blocked.listener_addr().is_none());
    assert!(blocked.postgres_listener_addr().is_none());
}

#[test]
fn connection_events_log_success_and_quit_but_not_rejected_handshakes() {
    use astersql_sessionctx_vardef::EnableConnectionEventLog;
    use astersql_util_logutil::log::{LogField, LogLevel, background_logger};
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            EnableConnectionEventLog.Store(self.0);
        }
    }
    let _restore = Restore(EnableConnectionEventLog.Load());
    EnableConnectionEventLog.Store(true);
    let server = start_real_server();
    let logger = background_logger();
    let mut stream = TcpStream::connect(server.listener_addr().unwrap()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let remote = stream.local_addr().unwrap().to_string();
    root_handshake(&mut stream);
    write_packet(&mut stream, 0, &[1]);
    // EOF proves the worker logged QUIT and closed the connection.
    assert_eq!(stream.read(&mut [0; 1]).unwrap(), 0);
    let entries = logger.entries();
    let events = entries
        .iter()
        .filter(|entry| {
            entry.message == "connection event"
                && entry
                    .fields
                    .contains(&LogField::String("remoteAddr".into(), remote.clone()))
        })
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 2);
    for (entry, event) in events.iter().zip(["login_success", "logout"]) {
        assert_eq!(entry.level, LogLevel::Info);
        assert!(
            entry
                .fields
                .contains(&LogField::String("event".into(), event.into()))
        );
        assert!(
            entry
                .fields
                .contains(&LogField::String("user".into(), "root@%".into()))
        );
    }
    let mut rejected = TcpStream::connect(server.listener_addr().unwrap()).unwrap();
    rejected
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let rejected_remote = rejected.local_addr().unwrap().to_string();
    assert_eq!(read_packet(&mut rejected)[0], 10);
    let capability = (1_u32 << 9) | (1 << 15) | (1 << 19);
    let mut response = capability.to_le_bytes().to_vec();
    response.extend_from_slice(&(64_u32 << 20).to_le_bytes());
    response.push(45);
    response.extend_from_slice(&[0; 23]);
    response.extend_from_slice(b"denied\0\0mysql_native_password\0");
    write_packet(&mut rejected, 1, &response);
    assert_eq!(rejected.read(&mut [0; 1]).unwrap(), 0);
    assert!(
        !logger
            .entries()
            .iter()
            .any(|entry| entry.message == "connection event"
                && entry.fields.contains(&LogField::String(
                    "remoteAddr".into(),
                    rejected_remote.clone()
                )))
    );
    EnableConnectionEventLog.Store(false);
    let mut disabled = TcpStream::connect(server.listener_addr().unwrap()).unwrap();
    disabled
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let disabled_remote = disabled.local_addr().unwrap().to_string();
    root_handshake(&mut disabled);
    write_packet(&mut disabled, 0, &[1]);
    assert_eq!(disabled.read(&mut [0; 1]).unwrap(), 0);
    assert!(
        !logger
            .entries()
            .iter()
            .any(|entry| entry.message == "connection event"
                && entry.fields.contains(&LogField::String(
                    "remoteAddr".into(),
                    disabled_remote.clone()
                )))
    );
    server.close();
}
