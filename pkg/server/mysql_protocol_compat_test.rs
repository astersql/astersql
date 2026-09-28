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

// MySQL 8.0 连接与命令协议兼容性测试。
//
// 通过原始协议客户端校验能力协商、连接级命令、会话重置及多语句结果的线协议表现，
// 并分别覆盖启用 CLIENT_DEPRECATE_EOF 与传统 EOF 包的客户端路径。

use crate::mysql_compat_test_support::{
    CLIENT_CONNECT_ATTRS, CLIENT_DEPRECATE_EOF, CLIENT_MULTI_RESULTS, CLIENT_MULTI_STATEMENTS,
    CLIENT_PLUGIN_AUTH, CLIENT_PROTOCOL_41, MysqlCompatServer, MysqlTestClient, TextResultSet,
    TextValue, WireResponse,
};

// 本文件直接发送的 MySQL 命令字。
const COM_QUIT: u8 = 0x01;
const COM_INIT_DB: u8 = 0x02;
const COM_SET_OPTION: u8 = 0x1b;
const COM_RESET_CONNECTION: u8 = 0x1f;

// COM_SET_OPTION 的载荷是小端 u16，而非文本布尔值。
const MYSQL_OPTION_MULTI_STATEMENTS_ON: [u8; 2] = 0_u16.to_le_bytes();
const MYSQL_OPTION_MULTI_STATEMENTS_OFF: [u8; 2] = 1_u16.to_le_bytes();

// OK/EOF 包中与本组断言相关的服务端状态位。
const SERVER_STATUS_IN_TRANS: u16 = 0x0001;
const SERVER_STATUS_AUTOCOMMIT: u16 = 0x0002;
const SERVER_MORE_RESULTS_EXISTS: u16 = 0x0008;

/// 要求响应为 OK 包，并返回状态位供调用方继续验证会话状态。
fn expect_ok(response: WireResponse, context: &str) -> u16 {
    let WireResponse::Ok(packet) = response else {
        panic!("{context} did not return OK: {response:?}");
    };
    assert_eq!(packet.header, 0x00, "{context} OK header");
    packet.status
}

/// 要求响应为文本结果集。
fn expect_result(response: WireResponse, context: &str) -> TextResultSet {
    let WireResponse::ResultSet(result) = response else {
        panic!("{context} did not return a result set: {response:?}");
    };
    result
}

/// 验证单行单列结果，并保留结果结束包中的状态位。
fn expect_single_value(response: WireResponse, expected: TextValue, context: &str) -> u16 {
    let result = expect_result(response, context);
    assert_eq!(result.rows, vec![vec![expected]], "{context} row");
    result.status
}

/// 通过 COM_SET_OPTION 动态切换多语句能力，并校验兼容 MySQL 的 EOF 形响应。
fn set_multi_statements(client: &mut MysqlTestClient, enabled: bool) {
    let payload = if enabled {
        MYSQL_OPTION_MULTI_STATEMENTS_ON
    } else {
        MYSQL_OPTION_MULTI_STATEMENTS_OFF
    };
    client
        .write_command(COM_SET_OPTION, &payload)
        .expect("write COM_SET_OPTION");
    let eof = client.read_packet().expect("read COM_SET_OPTION EOF");
    assert_eq!(eof.sequence, 1);
    if eof.payload.first() == Some(&0xff) {
        let code = u16::from_le_bytes([eof.payload[1], eof.payload[2]]);
        // 4.1 协议错误包带 SQLSTATE 标记时，错误消息从第 9 字节开始。
        let message_offset = if eof.payload.get(3) == Some(&b'#') {
            9
        } else {
            3
        };
        panic!(
            "COM_SET_OPTION returned ERR {code}: {}",
            String::from_utf8_lossy(&eof.payload[message_offset..])
        );
    }
    assert_eq!(
        eof.payload.first(),
        Some(&0xfe),
        "unexpected COM_SET_OPTION packet: {:?}",
        eof.payload
    );
    assert!(
        matches!(eof.payload.len(), 5 | 7),
        "COM_SET_OPTION must return an EOF-shaped packet, got {:?}",
        eof.payload
    );
}

/// 验证多语句开关、结果数量及 MORE_RESULTS 状态位的首尾语义。
fn verify_multi_results(client: &mut MysqlTestClient) {
    set_multi_statements(client, true);
    let results = client
        .query_all("SELECT 1; SELECT 2")
        .expect("execute multi-statement query");
    assert_eq!(results.len(), 2);
    let first_status = expect_single_value(
        results[0].clone(),
        TextValue::Bytes(b"1".to_vec()),
        "first multi result",
    );
    let second_status = expect_single_value(
        results[1].clone(),
        TextValue::Bytes(b"2".to_vec()),
        "second multi result",
    );
    assert_ne!(first_status & SERVER_MORE_RESULTS_EXISTS, 0);
    assert_eq!(second_status & SERVER_MORE_RESULTS_EXISTS, 0);

    set_multi_statements(client, false);
    let WireResponse::Err(disabled) = client
        .query("SELECT 1; SELECT 2")
        .expect("parse disabled multi-statement error")
    else {
        panic!("disabled multi statements did not return ERR");
    };
    assert!(
        disabled
            .message
            .contains("multi-statement execution is disabled"),
        "unexpected disabled multi-statement error: {disabled:?}"
    );
}

#[test]
/// 覆盖 MySQL 8.0 客户端连接生命周期中的关键命令与协商分支。
fn mysql_protocol_connection_commands_match_mysql_80() {
    let server = MysqlCompatServer::start().expect("start compatibility server");
    let attributes = [
        ("_client_name", "astersql-compat"),
        ("_client_version", "8.0"),
        ("program_name", "raw-protocol-test"),
    ];
    let base_capabilities = CLIENT_CONNECT_ATTRS | CLIENT_MULTI_RESULTS;

    let mut client = server
        .connect_root_with_attrs(
            base_capabilities | CLIENT_DEPRECATE_EOF,
            Some("test"),
            &attributes,
        )
        .expect("connect with initial database and connection attributes");
    let negotiated = client.handshake.negotiated_capabilities;
    assert_eq!(negotiated & CLIENT_PROTOCOL_41, CLIENT_PROTOCOL_41);
    assert_eq!(negotiated & CLIENT_PLUGIN_AUTH, CLIENT_PLUGIN_AUTH);
    assert_eq!(negotiated & CLIENT_CONNECT_ATTRS, CLIENT_CONNECT_ATTRS);
    assert_eq!(negotiated & CLIENT_MULTI_RESULTS, CLIENT_MULTI_RESULTS);
    assert_eq!(negotiated & CLIENT_MULTI_STATEMENTS, 0);
    assert_eq!(negotiated & CLIENT_DEPRECATE_EOF, CLIENT_DEPRECATE_EOF);

    expect_ok(
        client
            .command(COM_INIT_DB, b"mysql")
            .expect("execute COM_INIT_DB"),
        "COM_INIT_DB",
    );
    expect_ok(client.ping().expect("execute COM_PING"), "COM_PING");
    verify_multi_results(&mut client);

    expect_ok(
        client
            .query("SET @compat_reset = 'leaked'")
            .expect("set user variable"),
        "SET user variable",
    );
    expect_ok(
        client
            .query("PREPARE compat_stmt FROM 'SELECT 9'")
            .expect("prepare named statement"),
        "PREPARE",
    );
    let begin_status = expect_ok(client.query("BEGIN").expect("begin transaction"), "BEGIN");
    assert_ne!(begin_status & SERVER_STATUS_IN_TRANS, 0);

    // 重置必须清理事务、用户变量和预处理语句，同时恢复自动提交且保持连接可用。
    let reset_status = expect_ok(
        client
            .command(COM_RESET_CONNECTION, &[])
            .expect("execute COM_RESET_CONNECTION"),
        "COM_RESET_CONNECTION",
    );
    assert_eq!(reset_status & SERVER_STATUS_IN_TRANS, 0);
    assert_ne!(reset_status & SERVER_STATUS_AUTOCOMMIT, 0);
    expect_single_value(
        client
            .query("SELECT @compat_reset")
            .expect("read reset user variable"),
        TextValue::Null,
        "reset user variable",
    );
    let WireResponse::Err(reset_prepared) = client
        .query("EXECUTE compat_stmt")
        .expect("read reset prepared statement error")
    else {
        panic!("prepared statement survived COM_RESET_CONNECTION");
    };
    assert!(
        reset_prepared
            .message
            .to_ascii_lowercase()
            .contains("unknown prepared statement"),
        "unexpected reset prepared statement error: {reset_prepared:?}"
    );
    expect_single_value(
        client.query("SELECT 4").expect("query after reset error"),
        TextValue::Bytes(b"4".to_vec()),
        "query after reset",
    );

    // 未协商 CLIENT_DEPRECATE_EOF 时，多结果读取仍须走传统 EOF 包并保持同步。
    let mut legacy_eof_client = server
        .connect_root_with_attrs(base_capabilities, Some("test"), &attributes)
        .expect("connect without CLIENT_DEPRECATE_EOF");
    assert_eq!(
        legacy_eof_client.handshake.negotiated_capabilities & CLIENT_DEPRECATE_EOF,
        0
    );
    verify_multi_results(&mut legacy_eof_client);
    expect_single_value(
        legacy_eof_client
            .query("SELECT 5")
            .expect("legacy EOF query remains usable"),
        TextValue::Bytes(b"5".to_vec()),
        "legacy EOF query",
    );

    // COM_QUIT 不返回响应，而是由服务端直接关闭连接。
    client.write_command(COM_QUIT, &[]).expect("write COM_QUIT");
    assert!(
        client.read_packet().is_err(),
        "COM_QUIT must close the TCP connection without a response"
    );
}
