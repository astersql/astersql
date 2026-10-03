// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// 客户端连接（conn）模块单元测试。
//
// 覆盖 MySQL 线协议命令字节、文本协议取值编码、取消令牌单调性，
// 以及认证插件名称与 TiDB 约定的一致性。

use super::conn::*;
use crate::server::ManagedConnection;

/// 校验 Command 与 MySQL 协议命令字节的映射；未知字节返回 UnsupportedCommand。
#[test]
fn command_bytes_follow_the_mysql_wire_protocol() {
    assert_eq!(Command::try_from(0x03).unwrap(), Command::Query);
    assert_eq!(Command::try_from(0x16).unwrap(), Command::StmtPrepare);
    assert_eq!(Command::try_from(0x1f).unwrap(), Command::ResetConnection);
    assert!(matches!(
        Command::try_from(0xff),
        Err(ConnError::UnsupportedCommand(0xff))
    ));
}

/// 校验 Value 按 MySQL 文本协议编码：NULL 为 None，其余为字节串。
#[test]
fn values_have_mysql_text_protocol_encodings() {
    assert_eq!(Value::Null.encode_text(), None);
    assert_eq!(
        Value::Text("tidb".into()).encode_text(),
        Some(b"tidb".to_vec())
    );
    assert_eq!(Value::Signed(-42).encode_text(), Some(b"-42".to_vec()));
    assert_eq!(Value::Unsigned(42).encode_text(), Some(b"42".to_vec()));
    assert_eq!(Value::Float(1.25).encode_text(), Some(b"1.25".to_vec()));
}

/// 取消令牌一旦 cancel 后保持已取消状态（单调、不可逆）。
#[test]
fn cancellation_is_monotonic() {
    let token = CancellationToken::new();
    assert!(!token.is_cancelled());
    token.cancel();
    assert!(token.is_cancelled());
}

/// 认证插件常量字符串须与 TiDB / MySQL 握手约定一致。
#[test]
fn authentication_plugin_names_match_tidb_contract() {
    assert_eq!(AUTH_NATIVE_PASSWORD, "mysql_native_password");
    assert_eq!(AUTH_CACHING_SHA2_PASSWORD, "caching_sha2_password");
    assert_eq!(AUTH_SOCKET, "auth_socket");
    assert_eq!(AUTH_CLEAR_PASSWORD, "mysql_clear_password");
}

#[test]
fn client_conn_implements_the_canonical_managed_connection_contract() {
    fn assert_managed<T: ManagedConnection>() {}
    assert_managed::<ClientConn>();
}

fn handshake_response_packet(capability: u32, attrs: &[u8], declared_len: u64) -> Vec<u8> {
    let mut packet = Vec::with_capacity(40 + attrs.len());
    packet.extend_from_slice(&capability.to_le_bytes());
    packet.extend_from_slice(&0_u32.to_le_bytes());
    packet.push(0);
    packet.extend_from_slice(&[0; 23]);
    packet.extend_from_slice(b"root\0\0");
    if capability & CLIENT_CONNECT_ATTRS != 0 {
        put_lenenc_int(&mut packet, declared_len);
        packet.extend_from_slice(attrs);
    }
    packet
}

#[test]
fn handshake_response_preserves_legacy_clients_and_rejects_truncated_attributes() {
    let legacy = handshake_response_packet(CLIENT_PROTOCOL_41, &[], 0);
    let response = parse_handshake_response(&legacy).unwrap();
    assert_eq!(response.user, "root");
    assert!(response.attrs.is_empty());

    let truncated = handshake_response_packet(
        CLIENT_PROTOCOL_41 | CLIENT_CONNECT_ATTRS,
        &[2, b'a', b'b', 2, b'c', b'd'],
        9,
    );
    assert!(matches!(
        parse_handshake_response(&truncated),
        Err(ConnError::MalformedPacket("connection attributes overflow"))
    ));
}

#[test]
fn handshake_response_rejects_attributes_over_the_one_mib_hard_limit() {
    let attrs = vec![0_u8; (1 << 20) + 1];
    let packet = handshake_response_packet(
        CLIENT_PROTOCOL_41 | CLIENT_CONNECT_ATTRS,
        &attrs,
        attrs.len() as u64,
    );

    assert!(matches!(
        parse_handshake_response(&packet),
        Err(ConnError::Session(message))
            if message == "connection refused: session connection attributes exceed the 1 MiB hard limit"
    ));
}

#[test]
fn go_merge_139_cursor_consumer_only_synchronizes_response_bytes() {
    use astersql_server_internal_resultset::{
        AttachCursorRUV2Tracker, New, NewCursorRUV2Tracker, ReportCursorRUV2Delta,
    };
    use astersql_util_execdetails::ruv2_metrics::{RUV2Metrics, kvrpcpb, tikvutil};
    use std::sync::Arc;
    let metrics = Arc::new(RUV2Metrics::default());
    let details = Arc::new(tikvutil::RUDetails::default());
    assert!(NewCursorRUV2Tracker(None, Some(details.clone())).is_none());
    assert!(NewCursorRUV2Tracker(Some(metrics.clone()), None).is_none());
    let mut raw = kvrpcpb::Ruv2::new();
    raw.set_coprocessor_response_bytes(3);
    details.AddRUV2(&raw);
    let tracker = NewCursorRUV2Tracker(Some(metrics.clone()), Some(details.clone())).unwrap();
    assert_eq!(metrics.TiKVCoprocessorResponseBytes(), 3);
    let source = astersql_util_sqlexec::SimpleRecordSet::new(Vec::new(), Vec::new(), 32);
    let mut result = New(Box::new(source), None);
    AttachCursorRUV2Tracker(result.as_mut(), Some(tracker));
    for (delta, expected) in [(6, 9), (4, 13), (0, 13)] {
        raw.set_coprocessor_response_bytes(delta);
        details.AddRUV2(&raw);
        ReportCursorRUV2Delta(result.as_mut());
        ReportCursorRUV2Delta(result.as_mut());
        assert_eq!(metrics.TiKVCoprocessorResponseBytes(), expected);
    }
    metrics.SetBypass(true);
    assert!(NewCursorRUV2Tracker(Some(metrics.clone()), Some(details.clone())).is_none());
    raw.set_coprocessor_response_bytes(7);
    details.AddRUV2(&raw);
    ReportCursorRUV2Delta(result.as_mut());
    assert_eq!(metrics.TiKVCoprocessorResponseBytes(), 13);
    metrics.SetBypass(false);
    ReportCursorRUV2Delta(result.as_mut());
    assert_eq!(metrics.TiKVCoprocessorResponseBytes(), 20);
    result.Close();
}

fn change_user_connection() -> (std::sync::Arc<ClientConn>, std::net::TcpStream) {
    change_user_connection_with_packet_limit(64 << 20)
}

fn change_user_connection_with_packet_limit(
    max_packet: u64,
) -> (std::sync::Arc<ClientConn>, std::net::TcpStream) {
    use std::io::{Read, Write};
    use std::sync::Arc;
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    domain.set_global_system_variable("max_allowed_packet", &max_packet.to_string());
    let driver = Arc::new(crate::runtime::ConcreteSessionDriver::new_for_test(
        domain.clone(),
        crate::runtime::BootstrapAuthMode::InsecureRootOnly,
    ));
    let server = crate::server::Server::new_test(
        crate::server::ServerConfig::default(),
        Arc::new(crate::runtime::CanonicalServerDriver),
    );
    server
        .set_connection_runtime(
            driver,
            Arc::new(crate::runtime::CanonicalConnectionDomain::new(domain)),
        )
        .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut peer = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (socket, _) = listener.accept().unwrap();
    let connection = newClientConn(
        server,
        Box::new(crate::runtime::TcpPacketIo::new(socket, 32 << 20).unwrap()),
        vec![7; 20],
        false,
    );
    let handshake = std::thread::spawn(move || {
        let mut header = [0; 4];
        peer.read_exact(&mut header).unwrap();
        let size =
            usize::from(header[0]) | (usize::from(header[1]) << 8) | (usize::from(header[2]) << 16);
        peer.read_exact(&mut vec![0; size]).unwrap();
        let capability = (1_u32 << 9) | (1 << 15) | (1 << 19);
        let mut response = capability.to_le_bytes().to_vec();
        response.extend_from_slice(&(64_u32 << 20).to_le_bytes());
        response.push(45);
        response.extend_from_slice(&[0; 23]);
        response.extend_from_slice(b"root\0\0mysql_native_password\0");
        peer.write_all(&[response.len() as u8, 0, 0, 1]).unwrap();
        peer.write_all(&response).unwrap();
        peer
    });
    connection.handshake().unwrap();
    let peer = handshake.join().unwrap();
    let old = connection.getCtx().unwrap().unwrap();
    let cancel = CancellationToken::new();
    old.execute_query("CREATE DATABASE old_db", false, &cancel)
        .unwrap();
    connection.useDB("old_db", &cancel).unwrap();
    old.execute_query("CREATE TABLE preserved (id INT)", false, &cancel)
        .unwrap();
    old.execute_query("INSERT INTO preserved VALUES (37)", false, &cancel)
        .unwrap();
    (connection, peer)
}

#[test]
fn change_user_auth_failure_restores_old_session() {
    let (connection, _peer) = change_user_connection();
    let old = connection.getCtx().unwrap().unwrap();
    let result = connection.dispatch(b"\x11missing_user\0\0new_db\0\0\0");
    assert!(
        matches!(result, Err(ConnError::AccessDenied { user, .. }) if user == "missing_user"),
        "authentication must run before selecting the new database"
    );
    assert!(std::sync::Arc::ptr_eq(
        &old,
        &connection.getCtx().unwrap().unwrap()
    ));
    assert_eq!(old.user_identity().unwrap(), "root@%");
    let (user, database, _) = connection.identity_snapshot();
    assert_eq!(user, "root");
    assert_eq!(database, "old_db");
    let results = old
        .execute_query("SELECT id FROM preserved", false, &CancellationToken::new())
        .unwrap();
    assert_eq!(results[0].rows[0][0].encode_text(), Some(b"37".to_vec()));
    connection.Close().unwrap();
}

#[test]
fn change_user_malformed_packet_preserves_identity() {
    let (connection, _peer) = change_user_connection();
    let old = connection.getCtx().unwrap().unwrap();
    for packet in [
        b"\x11new_user\0".as_slice(),
        b"\x11new_user\0\x05x".as_slice(),
    ] {
        assert!(matches!(
            connection.dispatch(packet),
            Err(ConnError::MalformedPacket(_))
        ));
        assert!(std::sync::Arc::ptr_eq(
            &old,
            &connection.getCtx().unwrap().unwrap()
        ));
        assert_eq!(connection.identity_snapshot().0, "root");
        assert_eq!(connection.identity_snapshot().1, "old_db");
    }
    connection.Close().unwrap();
}

#[test]
fn change_user_plugin_exchange_failure_preserves_old_session() {
    let (connection, peer) = change_user_connection();
    let old = connection.getCtx().unwrap().unwrap();
    peer.shutdown(std::net::Shutdown::Both).unwrap();
    assert!(matches!(
        connection.dispatch(b"\x11root\0\0old_db\0\0\0unknown\0"),
        Err(ConnError::Io(_))
    ));
    assert!(std::sync::Arc::ptr_eq(
        &old,
        &connection.getCtx().unwrap().unwrap()
    ));
    assert_eq!(connection.identity_snapshot().0, "root");
    old.execute_query("SELECT id FROM preserved", false, &CancellationToken::new())
        .unwrap();
    connection.Close().unwrap();
}

#[test]
fn change_user_success_replaces_and_closes_old_session() {
    let (connection, _peer) = change_user_connection();
    let old = connection.getCtx().unwrap().unwrap();
    connection.dispatch(b"\x11root\0\0old_db\0\0\0").unwrap();
    let new = connection.getCtx().unwrap().unwrap();
    assert!(!std::sync::Arc::ptr_eq(&old, &new));
    assert!(
        old.execute_query("SELECT 1", false, &CancellationToken::new())
            .is_err()
    );
    let result = new
        .execute_query("SELECT id FROM preserved", false, &CancellationToken::new())
        .unwrap();
    assert_eq!(result[0].rows[0][0].encode_text(), Some(b"37".to_vec()));
    connection.Close().unwrap();
}

#[test]
fn canonical_long_data_limit_is_deferred_to_execute_and_encoded_as_1153() {
    use std::io::Read;
    fn packet(peer: &mut std::net::TcpStream) -> Vec<u8> {
        let mut header = [0; 4];
        peer.read_exact(&mut header).unwrap();
        let len = header[0] as usize | (header[1] as usize) << 8 | (header[2] as usize) << 16;
        let mut data = vec![0; len];
        peer.read_exact(&mut data).unwrap();
        data
    }
    let (connection, mut peer) = change_user_connection_with_packet_limit(1024);
    peer.set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    let context = connection.getCtx().unwrap().unwrap();
    let cancel = CancellationToken::new();
    assert_eq!(context.max_allowed_packet().unwrap(), 1024);
    connection
        .handleStmt(Command::StmtPrepare, b"select ?, ?", &cancel)
        .unwrap();
    let id = loop {
        let data = packet(&mut peer);
        if data.len() == 12 && data[0] == 0 {
            break u32::from_le_bytes(data[1..5].try_into().unwrap());
        }
    };
    for parameter in [0u16, 1] {
        let mut data = id.to_le_bytes().to_vec();
        data.extend_from_slice(&parameter.to_le_bytes());
        data.resize(1030, b'a');
        connection
            .handleStmt(Command::StmtSendLongData, &data, &cancel)
            .unwrap();
    }
    let mut data = id.to_le_bytes().to_vec();
    data.extend_from_slice(&[0, 0, b'c']);
    connection
        .handleStmt(Command::StmtSendLongData, &data, &cancel)
        .unwrap();
    let mut execute = id.to_le_bytes().to_vec();
    execute.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0]);
    let error = connection
        .handleStmt(Command::StmtExecute, &execute, &cancel)
        .unwrap_err();
    assert_eq!(error, ConnError::NetPacketTooLarge);
    connection.writeError(&error).unwrap();
    connection.flush().unwrap();
    let error_packet = loop {
        let data = packet(&mut peer);
        if data[0] == 0xff {
            break data;
        }
    };
    assert_eq!(
        u16::from_le_bytes(error_packet[1..3].try_into().unwrap()),
        1153
    );
    assert_eq!(&error_packet[4..9], b"08S01");
    // Both buffers and the flag were reset: a following EXECUTE returns fresh values.
    execute.truncate(4);
    execute.extend_from_slice(&[0, 1, 0, 0, 0, 0, 1, 253, 0, 253, 0, 1, b'x', 1, b'y']);
    connection
        .handleStmt(Command::StmtExecute, &execute, &cancel)
        .unwrap();
    // Column count, two column definitions, metadata EOF, then a binary row.
    for _ in 0..4 {
        packet(&mut peer);
    }
    assert_eq!(packet(&mut peer), vec![0, 0, 1, b'x', 1, b'y']);
    connection.Close().unwrap();
}
