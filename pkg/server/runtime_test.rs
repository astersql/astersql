// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// server 生产运行时适配层的集成边界测试。
//
// 这里通过真实的回环 TCP 连接和 canonical session Domain，验证 MySQL 包编解码、
// PROXY 协议探测、鉴权与 SQL 执行，以及连接和会话生命周期没有在适配过程中丢失语义。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::conn::{
    AuthIdentity, AuthRequest, CancellationToken, Command, ConnError, ConnectionDomain, PacketIo,
    ResponseLifecycle, SessionDriver, TiDBContext, Value,
};
use crate::runtime::{
    BootstrapAuthMode, CanonicalConnectionDomain, ConcreteSessionDriver, TcpPacketIo,
};

/// 创建一对已经建立连接的回环 TCP 流，分别模拟客户端和服务端。
fn tcp_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
    let address = listener.local_addr().expect("loopback listener address");
    let client = TcpStream::connect(address).expect("connect loopback client");
    let (server, _) = listener.accept().expect("accept loopback client");
    (client, server)
}

#[test]
fn write_sql_resp_accumulates_protocol_sections_before_finish() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let callback_observed = Arc::clone(&observed);
    let lifecycle = ResponseLifecycle::new(move |duration| {
        callback_observed.lock().unwrap().push(duration);
    });

    lifecycle.add_write_duration(Duration::from_millis(3));
    lifecycle.add_write_duration(Duration::from_millis(5));
    lifecycle.finish();
    lifecycle.finish();

    assert_eq!(*observed.lock().unwrap(), [Duration::from_millis(8)]);
}

#[test]
fn write_sql_resp_excludes_fetch_returned_callback_time() {
    let observed = Arc::new(Mutex::new(Duration::ZERO));
    let callback_observed = Arc::clone(&observed);
    let lifecycle = ResponseLifecycle::new(move |duration| {
        *callback_observed.lock().unwrap() = duration;
    });

    lifecycle.add_write_duration(Duration::from_millis(2));
    thread::sleep(Duration::from_millis(20));
    lifecycle.add_write_duration(Duration::from_millis(4));
    lifecycle.finish();

    assert_eq!(*observed.lock().unwrap(), Duration::from_millis(6));
}

/// 基于真实 canonical Domain 构造测试驱动，并仅允许 root 空密码登录。
fn session_driver() -> ConcreteSessionDriver {
    let (domain, _) =
        astersql_session::runtime::CreateAnalyzeSession().expect("initialize canonical domain");
    ConcreteSessionDriver::new_for_test(domain, BootstrapAuthMode::InsecureRootOnly)
}

/// 构造固定来源和插件的握手鉴权请求，便于只改变用户名与响应数据。
fn auth(user: &str, auth_data: &[u8]) -> AuthRequest {
    AuthRequest {
        identity: AuthIdentity {
            username: user.to_owned(),
            hostname: "127.0.0.1".to_owned(),
            plugin: "mysql_native_password".to_owned(),
        },
        auth_data: auth_data.to_vec(),
        salt: vec![7; 20],
        tls_state: None,
        attributes: Default::default(),
    }
}

/// 验证生产 PacketIO 的读写、序列号、超时、地址和关闭状态均透传到 TCP 套接字。
#[test]
fn tcp_packet_io_flushes_existing_packet_codec_and_exposes_socket_lifecycle() {
    let (mut client, server) = tcp_pair();
    client
        .write_all(&[3, 0, 0, 0, b'o', b'n', b'e'])
        .expect("write client packet");

    let mut packet = TcpPacketIo::new(server, 32 * 1024 * 1024).expect("production packet IO");
    assert_eq!(packet.read_packet().expect("read packet"), b"one");
    let peer = packet.peer_addr().expect("peer address");
    let local = packet.local_addr().expect("local address");
    assert_eq!(peer.0, "127.0.0.1");
    assert_eq!(local.0, "127.0.0.1");
    assert!(!peer.1.is_empty());
    assert!(!local.1.is_empty());

    packet.write_packet(b"two").expect("encode server packet");
    packet.flush().expect("flush encoded bytes to socket");
    let mut response = [0_u8; 7];
    client
        .read_exact(&mut response)
        .expect("read server packet");
    assert_eq!(response, [3, 0, 0, 1, b't', b'w', b'o']);

    packet.reset_sequence();
    packet
        .set_read_timeout(Duration::from_millis(30))
        .expect("install socket read timeout");
    let started = Instant::now();
    assert!(
        packet.read_packet().is_err(),
        "idle socket read must time out"
    );
    assert!(started.elapsed() < Duration::from_secs(2));

    packet.close().expect("close socket");
    assert!(!packet.connection_alive());
}

/// PROXY 协议必须延迟到首次读取时探测，以允许服务端先写出握手包再等待客户端数据。
#[test]
fn proxy_fallback_is_lazy_until_after_server_handshake_write() {
    let (mut client, server) = tcp_pair();
    let client = thread::spawn(move || {
        let mut handshake = [0_u8; 9];
        client
            .read_exact(&mut handshake)
            .expect("receive server handshake before sending client bytes");
        assert_eq!(&handshake[4..], b"hello");
        client
            .write_all(&[3, 0, 0, 1, b'o', b'n', b'e'])
            .expect("write plain MySQL packet without PROXY header");
    });
    let mut packet = TcpPacketIo::new_with_options(
        server,
        32 * 1024 * 1024,
        None,
        None,
        Some(("127.0.0.0/8".into(), true, Duration::from_secs(1))),
    )
    .expect("proxy-aware packet IO");
    packet.write_packet(b"hello").unwrap();
    packet.flush().unwrap();
    assert_eq!(packet.read_packet().unwrap(), b"one");
    assert_eq!(packet.peer_addr().unwrap().0, "127.0.0.1");
    client.join().unwrap();
}

/// 跨越单包上限一个字节时应复用标准分包逻辑，并为后续分片递增序列号。
#[test]
fn tcp_packet_io_flushes_mysql_multi_packet_payload_without_reimplementing_framing() {
    let (mut client, server) = tcp_pair();
    let mut packet = TcpPacketIo::new(server, 64 * 1024 * 1024).expect("production packet IO");
    let payload = vec![b'x'; astersql_server_internal::MAX_PAYLOAD_LEN + 1];
    let expected_len = payload.len() + 8;
    // 大包写入可能填满套接字缓冲区，因此由并发读端持续排空全部编码结果。
    let reader = thread::spawn(move || {
        let mut encoded = vec![0; expected_len];
        client
            .read_exact(&mut encoded)
            .expect("read all split packet bytes");
        encoded
    });

    packet.write_packet(&payload).expect("encode split payload");
    packet.flush().expect("flush split payload");
    let encoded = reader.join().expect("join loopback reader");
    assert_eq!(&encoded[..4], &[0xff, 0xff, 0xff, 0]);
    let second = 4 + astersql_server_internal::MAX_PAYLOAD_LEN;
    assert_eq!(&encoded[second..second + 4], &[1, 0, 0, 1]);
    assert_eq!(encoded[second + 4], b'x');
}

/// 验证具体会话驱动贯通真实鉴权、SQL 结果元数据、字段查询、取消和关闭语义。
#[test]
fn concrete_session_driver_authenticates_and_returns_real_sql_results() {
    let driver = session_driver();
    let context = driver
        .open_ctx(81, 1 << 16, 46, "", None)
        .expect("open concrete context");

    context
        .authenticate(&auth("root", &[]))
        .expect("insecure root empty password");
    assert!(matches!(
        context.authenticate(&auth("root", b"password-response")),
        Err(ConnError::AccessDenied { .. })
    ));
    assert!(matches!(
        context.authenticate(&auth("alice", &[])),
        Err(ConnError::AccessDenied { .. })
    ));

    let results = context
        .execute_query(
            "create database runtime_db; use runtime_db; \
             create table items (id bigint primary key, name varchar(32)); \
             insert into items values (1, 'from-tikv'); \
             select * from items",
            true,
            &CancellationToken::new(),
        )
        .expect("execute real multi-statement SQL");
    assert_eq!(results.len(), 5);
    assert_eq!(results[3].state.affected_rows, 1);
    assert_eq!(
        results[4]
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        vec!["id", "name"]
    );
    assert_eq!(results[4].columns[0].schema, "runtime_db");
    assert_eq!(results[4].columns[0].table, "items");
    assert_eq!(results[4].columns[0].org_table, "items");
    assert_eq!(results[4].columns[0].org_name, "id");
    assert_eq!(
        results[4].columns[0].column_type,
        astersql_parser_mysql::r#type::TypeLonglong
    );
    assert!(astersql_parser_mysql::r#type::HasPriKeyFlag(
        results[4].columns[0].flags as usize
    ));
    assert_eq!(
        results[4].columns[1].column_type,
        astersql_parser_mysql::r#type::TypeVarString
    );
    assert_eq!(
        results[4].rows,
        vec![vec![
            Value::Text("1".into()),
            Value::Text("from-tikv".into())
        ]]
    );
    assert_eq!(context.state().status & 0x0002, 0x0002);
    assert_eq!(context.last_statement(), "select * from items");

    let fields = context
        .field_list("items", "%")
        .expect("COM_FIELD_LIST uses canonical table metadata");
    assert_eq!(
        fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>(),
        vec!["id", "name"]
    );
    assert_eq!(fields[0].schema, "runtime_db");
    assert_eq!(fields[0].table, "items");
    assert_eq!(
        fields[0].column_type,
        astersql_parser_mysql::r#type::TypeLonglong
    );
    assert_eq!(
        fields[1].column_type,
        astersql_parser_mysql::r#type::TypeVarString
    );

    // 取消只作用于下一条命令；命中后应被消费，后续查询仍能正常执行。
    context.cancel();
    assert!(
        context
            .execute_query("select 1", false, &CancellationToken::new())
            .expect_err("cancel must reach the concrete session boundary")
            .to_string()
            .contains("interrupted")
    );
    assert_eq!(
        context
            .execute_query("select 2", false, &CancellationToken::new())
            .expect("cancellation applies to one command")[0]
            .rows,
        vec![vec![Value::Text("2".into())]]
    );

    assert!(matches!(
        context.execute_command(Command::StmtPrepare, b"select 1", &CancellationToken::new()),
        Err(ConnError::UnsupportedCommand(0x16))
    ));
    context.close().expect("close concrete context");
    assert!(
        context
            .execute_query("select 1", false, &CancellationToken::new())
            .is_err()
    );
}

/// canonical 连接域应直接委托 Domain 分配唯一连接 ID，并允许成对释放。
#[test]
fn canonical_connection_domain_delegates_id_allocation_and_release() {
    let driver = session_driver();
    let domain = CanonicalConnectionDomain::new(driver.domain().clone());
    let first = domain.next_connection_id();
    let second = domain.next_connection_id();
    assert_ne!(first, second);
    domain.release_connection_id(first);
    domain.release_connection_id(second);
}
