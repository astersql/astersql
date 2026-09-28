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
