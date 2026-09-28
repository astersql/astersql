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

//! Shared real-listener MySQL protocol test support.
//!
//! Compatibility tests use the production `Server`, `ConcreteSessionDriver`,
//! TCP transport, and in-memory session domain. Packet framing and decoding
//! live here so individual tests can assert protocol semantics instead of
//! duplicating a partial client.
//!
//! MySQL 协议兼容性测试的共享支撑：在随机回环端口启动生产 `Server` 与会话运行时，
//! 再通过一个最小同步客户端完成真实 TCP 握手和命令收发。这里集中处理包序号、能力协商、
//! 文本/二进制结果集及预处理语句响应，使各兼容性用例只需断言协议语义。

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use crate::conn::{ConnectionDomain, SessionDriver};
use crate::runtime::{BootstrapAuthMode, CanonicalConnectionDomain, ConcreteSessionDriver};
use crate::server::{Domain, Server, ServerConfig, ServerDriver, StatusConfig};

/// 握手响应携带初始数据库名。
pub(crate) const CLIENT_CONNECT_WITH_DB: u32 = 1 << 3;
/// 使用 MySQL 4.1 及以上版本的握手与响应布局。
pub(crate) const CLIENT_PROTOCOL_41: u32 = 1 << 9;
/// 使用长度前缀认证数据字段。
pub(crate) const CLIENT_SECURE_CONNECTION: u32 = 1 << 15;
/// 单次查询允许包含多条语句。
pub(crate) const CLIENT_MULTI_STATEMENTS: u32 = 1 << 16;
/// 客户端能够连续接收多个结果。
pub(crate) const CLIENT_MULTI_RESULTS: u32 = 1 << 17;
/// 握手响应携带认证插件名。
pub(crate) const CLIENT_PLUGIN_AUTH: u32 = 1 << 19;
/// 握手响应携带长度编码的连接属性。
pub(crate) const CLIENT_CONNECT_ATTRS: u32 = 1 << 20;
/// 以 OK 包替代旧式 EOF 包结束结果集。
pub(crate) const CLIENT_DEPRECATE_EOF: u32 = 1 << 24;

const DEFAULT_CLIENT_CAPABILITIES: u32 =
    CLIENT_PROTOCOL_41 | CLIENT_SECURE_CONNECTION | CLIENT_PLUGIN_AUTH;
const COM_QUERY: u8 = 0x03;
const COM_PING: u8 = 0x0e;
const SERVER_MORE_RESULTS_EXISTS: u16 = 0x0008;
const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// 兼容性服务器使用的最小驱动，仅提供对外展示名称。
struct CompatibilityDriver;

impl ServerDriver for CompatibilityDriver {
    fn name(&self) -> &str {
        "tidb"
    }
}

/// 为真实监听器提供稳定 server id 与启动时间戳的测试域。
struct CompatibilityDomain;

impl Domain for CompatibilityDomain {
    fn server_id(&self) -> u64 {
        7
    }

    fn start_timestamp(&self) -> i64 {
        1
    }
}

/// 绑定随机回环端口、走生产协议栈的测试服务器。
pub(crate) struct MysqlCompatServer {
    server: Arc<Server>,
    mysql_addr: SocketAddr,
}

impl MysqlCompatServer {
    /// 创建内存会话域，安装生产连接运行时，并启动 MySQL 与状态监听器。
    pub(crate) fn start() -> Result<Self, String> {
        let (domain, _) = astersql_session::runtime::CreateAnalyzeSession()
            .map_err(|error| format!("initialize canonical domain: {error}"))?;
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
            Arc::new(CompatibilityDriver),
        );
        server
            .set_connection_runtime(session_driver, connection_domain)
            .map_err(|error| format!("install canonical connection runtime: {error}"))?;
        server
            .run(Arc::new(CompatibilityDomain))
            .map_err(|error| format!("start real listeners: {error}"))?;
        let mysql_addr = server
            .listener_addr()
            .ok_or_else(|| "MySQL listener did not expose its address".to_owned())?;
        Ok(Self { server, mysql_addr })
    }

    /// 返回操作系统实际分配的 MySQL 监听地址。
    pub(crate) fn mysql_addr(&self) -> SocketAddr {
        self.mysql_addr
    }

    /// 以无密码 root 用户建立连接，可按用例追加能力位和初始数据库。
    pub(crate) fn connect_root(
        &self,
        extra_capabilities: u32,
        database: Option<&str>,
    ) -> Result<MysqlTestClient, String> {
        self.connect_root_with_attrs(extra_capabilities, database, &[])
    }

    /// 与 [`Self::connect_root`] 相同，但额外编码客户端连接属性。
    pub(crate) fn connect_root_with_attrs(
        &self,
        extra_capabilities: u32,
        database: Option<&str>,
        attributes: &[(&str, &str)],
    ) -> Result<MysqlTestClient, String> {
        MysqlTestClient::connect(self.mysql_addr, extra_capabilities, database, attributes)
    }

    /// 以指定用户名连接，用于验证认证失败等非 root 路径。
    pub(crate) fn connect_user(
        &self,
        username: &str,
        extra_capabilities: u32,
    ) -> Result<MysqlTestClient, String> {
        MysqlTestClient::connect_as(self.mysql_addr, username, extra_capabilities, None, &[])
    }
}

impl Drop for MysqlCompatServer {
    fn drop(&mut self) {
        // 测试夹具离开作用域时关闭真实监听线程，避免端口和后台任务泄漏。
        self.server.close();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 初始握手中服务器信息及客户端最终协商出的能力集合。
pub(crate) struct Handshake {
    pub(crate) protocol_version: u8,
    pub(crate) server_version: String,
    pub(crate) connection_id: u32,
    pub(crate) server_capabilities: u32,
    pub(crate) negotiated_capabilities: u32,
    pub(crate) character_set: u8,
    pub(crate) status: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 一个已拆帧的 MySQL 包；序号用于校验命令内的包顺序。
pub(crate) struct Packet {
    pub(crate) sequence: u8,
    pub(crate) payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// OK 包中兼容性测试关注的字段。
pub(crate) struct OkPacket {
    pub(crate) header: u8,
    pub(crate) affected_rows: u64,
    pub(crate) last_insert_id: u64,
    pub(crate) status: u16,
    pub(crate) warnings: u16,
    pub(crate) info: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// ERR 包；仅在 4.1 协议能力开启时包含 SQLSTATE。
pub(crate) struct ErrPacket {
    pub(crate) code: u16,
    pub(crate) sql_state: Option<[u8; 5]>,
    pub(crate) message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 文本或二进制结果集共用的列定义。
pub(crate) struct ColumnDefinition {
    pub(crate) catalog: String,
    pub(crate) schema: String,
    pub(crate) table: String,
    pub(crate) org_table: String,
    pub(crate) name: String,
    pub(crate) org_name: String,
    pub(crate) character_set: u16,
    pub(crate) column_length: u32,
    pub(crate) column_type: u8,
    pub(crate) flags: u16,
    pub(crate) decimals: u8,
    pub(crate) default_value: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 为便于统一断言，文本值与解码后的二进制值都保留为字节串。
pub(crate) enum TextValue {
    Null,
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 完整结果集及其结束包携带的状态和告警计数。
pub(crate) struct TextResultSet {
    pub(crate) columns: Vec<ColumnDefinition>,
    pub(crate) rows: Vec<Vec<TextValue>>,
    pub(crate) status: u16,
    pub(crate) warnings: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// COM_STMT_PREPARE 返回的语句标识、参数数量与结果列元数据。
pub(crate) struct PreparedResponse {
    pub(crate) statement_id: u32,
    pub(crate) parameter_count: usize,
    pub(crate) columns: Vec<ColumnDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 一条命令可能返回的三类顶层协议响应。
pub(crate) enum WireResponse {
    Ok(OkPacket),
    Err(ErrPacket),
    ResultSet(TextResultSet),
}

/// 协议兼容性测试使用的最小同步 MySQL 客户端。
///
/// 客户端只实现测试所需命令，并严格校验每个响应包的序号与结构，避免服务端协议错误
/// 被宽松的第三方客户端掩盖。
pub(crate) struct MysqlTestClient {
    stream: TcpStream,
    pub(crate) handshake: Handshake,
}

impl MysqlTestClient {
    fn connect(
        address: SocketAddr,
        extra_capabilities: u32,
        database: Option<&str>,
        attributes: &[(&str, &str)],
    ) -> Result<Self, String> {
        Self::connect_as(address, "root", extra_capabilities, database, attributes)
    }

    fn connect_as(
        address: SocketAddr,
        username: &str,
        extra_capabilities: u32,
        database: Option<&str>,
        attributes: &[(&str, &str)],
    ) -> Result<Self, String> {
        let mut stream =
            TcpStream::connect(address).map_err(|error| format!("connect: {error}"))?;
        stream
            .set_read_timeout(Some(IO_TIMEOUT))
            .map_err(|error| format!("set read timeout: {error}"))?;
        stream
            .set_write_timeout(Some(IO_TIMEOUT))
            .map_err(|error| format!("set write timeout: {error}"))?;

        let initial = read_packet_from(&mut stream)?;
        if initial.sequence != 0 {
            return Err(format!(
                "initial handshake sequence {}, expected 0",
                initial.sequence
            ));
        }
        let mut handshake = parse_handshake(&initial.payload)?;
        // 初始数据库会隐式要求 CLIENT_CONNECT_WITH_DB；所有请求能力必须先由服务端声明。
        let requested = DEFAULT_CLIENT_CAPABILITIES
            | extra_capabilities
            | u32::from(database.is_some()) * CLIENT_CONNECT_WITH_DB;
        let missing = requested & !handshake.server_capabilities;
        if missing != 0 {
            return Err(format!(
                "server does not advertise requested capabilities 0x{missing:08x}"
            ));
        }
        handshake.negotiated_capabilities = requested;

        // 按 Protocol::HandshakeResponse41 布局构造无密码认证响应。
        let mut response = Vec::new();
        response.extend_from_slice(&requested.to_le_bytes());
        response.extend_from_slice(&(64_u32 << 20).to_le_bytes());
        response.push(45);
        response.extend_from_slice(&[0; 23]);
        response.extend_from_slice(username.as_bytes());
        response.push(0);
        response.push(0);
        if let Some(database) = database {
            response.extend_from_slice(database.as_bytes());
            response.push(0);
        }
        response.extend_from_slice(b"mysql_native_password\0");
        if requested & CLIENT_CONNECT_ATTRS != 0 {
            // 属性区自身也是一个长度编码字节串，内部按 key/value 成对编码。
            let mut encoded_attributes = Vec::new();
            for (key, value) in attributes {
                put_lenenc_bytes(&mut encoded_attributes, key.as_bytes());
                put_lenenc_bytes(&mut encoded_attributes, value.as_bytes());
            }
            put_lenenc_bytes(&mut response, &encoded_attributes);
        }
        write_packet_to(&mut stream, 1, &response)?;

        let auth = read_packet_from(&mut stream)?;
        if auth.sequence != 2 {
            return Err(format!(
                "authentication response sequence {}, expected 2",
                auth.sequence
            ));
        }
        match parse_simple_response(&auth.payload, requested)? {
            WireResponse::Ok(_) => Ok(Self { stream, handshake }),
            WireResponse::Err(error) => Err(format!(
                "root authentication failed: {} {}",
                error.code, error.message
            )),
            WireResponse::ResultSet(_) => Err("authentication returned a result set".to_owned()),
        }
    }

    pub(crate) fn ping(&mut self) -> Result<WireResponse, String> {
        self.command(COM_PING, &[])
    }

    pub(crate) fn query(&mut self, sql: &str) -> Result<WireResponse, String> {
        self.command(COM_QUERY, sql.as_bytes())
    }

    /// 发送 COM_STMT_PREPARE，并依次读取参数定义、列定义及各自结束包。
    pub(crate) fn prepare(&mut self, sql: &str) -> Result<PreparedResponse, String> {
        self.write_command(0x16, sql.as_bytes())?;
        let first = self.read_expected_packet(1)?;
        if first.payload.first() == Some(&0xff) {
            let WireResponse::Err(error) =
                parse_simple_response(&first.payload, self.handshake.negotiated_capabilities)?
            else {
                unreachable!()
            };
            return Err(format!(
                "COM_STMT_PREPARE failed with {}: {}",
                error.code, error.message
            ));
        }
        let mut cursor = PacketCursor::new(&first.payload);
        if cursor.read_u8()? != 0 {
            return Err("COM_STMT_PREPARE response is not PREPARE_OK".to_owned());
        }
        let statement_id = cursor.read_u32()?;
        let column_count = usize::from(cursor.read_u16()?);
        let parameter_count = usize::from(cursor.read_u16()?);
        cursor.read_u8()?;
        cursor.read_u16()?;
        cursor.finish("COM_STMT_PREPARE response")?;
        let mut sequence = 2;
        for _ in 0..parameter_count {
            let packet = self.read_expected_packet(sequence)?;
            parse_column_definition(&packet.payload)?;
            sequence = sequence.wrapping_add(1);
        }
        if parameter_count > 0 {
            let packet = self.read_expected_packet(sequence)?;
            parse_terminator(
                &packet.payload,
                self.handshake.negotiated_capabilities & CLIENT_DEPRECATE_EOF != 0,
            )?;
            sequence = sequence.wrapping_add(1);
        }
        let mut columns = Vec::with_capacity(column_count);
        for _ in 0..column_count {
            let packet = self.read_expected_packet(sequence)?;
            columns.push(parse_column_definition(&packet.payload)?);
            sequence = sequence.wrapping_add(1);
        }
        if column_count > 0 {
            let packet = self.read_expected_packet(sequence)?;
            parse_terminator(
                &packet.payload,
                self.handshake.negotiated_capabilities & CLIENT_DEPRECATE_EOF != 0,
            )?;
        }
        Ok(PreparedResponse {
            statement_id,
            parameter_count,
            columns,
        })
    }

    /// 发送调用方已编码的 COM_STMT_EXECUTE 负载并解码二进制结果集。
    pub(crate) fn execute_prepared(&mut self, payload: &[u8]) -> Result<WireResponse, String> {
        self.write_command(0x17, payload)?;
        let first = self.read_expected_packet(1)?;
        if matches!(first.payload.first(), Some(0x00 | 0xff)) {
            return parse_simple_response(&first.payload, self.handshake.negotiated_capabilities);
        }
        self.read_binary_result_set(first)
            .map(|(result, _)| WireResponse::ResultSet(result))
    }

    pub(crate) fn send_long_data(
        &mut self,
        statement_id: u32,
        parameter: u16,
        value: &[u8],
    ) -> Result<(), String> {
        let mut payload = statement_id.to_le_bytes().to_vec();
        payload.extend_from_slice(&parameter.to_le_bytes());
        payload.extend_from_slice(value);
        self.write_command(0x18, &payload)
    }

    pub(crate) fn reset_prepared(&mut self, statement_id: u32) -> Result<WireResponse, String> {
        self.command(0x1a, &statement_id.to_le_bytes())
    }

    pub(crate) fn close_prepared(&mut self, statement_id: u32) -> Result<(), String> {
        self.write_command(0x19, &statement_id.to_le_bytes())
    }

    pub(crate) fn field_list(
        &mut self,
        table: &str,
        wildcard: &str,
    ) -> Result<Vec<ColumnDefinition>, String> {
        let mut payload = Vec::with_capacity(table.len() + wildcard.len() + 1);
        payload.extend_from_slice(table.as_bytes());
        payload.push(0);
        payload.extend_from_slice(wildcard.as_bytes());
        self.write_command(0x04, &payload)?;

        let mut sequence = 1;
        let mut columns = Vec::new();
        loop {
            let packet = self.read_expected_packet(sequence)?;
            if is_result_set_terminator(&packet.payload) {
                parse_terminator(
                    &packet.payload,
                    self.handshake.negotiated_capabilities & CLIENT_DEPRECATE_EOF != 0,
                )?;
                return Ok(columns);
            }
            if packet.payload.first() == Some(&0xff) {
                let WireResponse::Err(error) =
                    parse_simple_response(&packet.payload, self.handshake.negotiated_capabilities)?
                else {
                    unreachable!("0xff packet parsed as a non-error response")
                };
                return Err(format!(
                    "COM_FIELD_LIST failed with {}: {}",
                    error.code, error.message
                ));
            }
            columns.push(parse_column_definition(&packet.payload)?);
            sequence = sequence.wrapping_add(1);
        }
    }

    /// 执行可能包含多条语句的查询，直到状态位表明没有后续结果。
    pub(crate) fn query_all(&mut self, sql: &str) -> Result<Vec<WireResponse>, String> {
        self.write_command(COM_QUERY, sql.as_bytes())?;
        self.read_all_responses()
    }

    pub(crate) fn command(&mut self, command: u8, payload: &[u8]) -> Result<WireResponse, String> {
        self.write_command(command, payload)?;
        self.read_response()
    }

    pub(crate) fn write_command(&mut self, command: u8, payload: &[u8]) -> Result<(), String> {
        let mut packet = Vec::with_capacity(payload.len() + 1);
        packet.push(command);
        packet.extend_from_slice(payload);
        write_packet_to(&mut self.stream, 0, &packet)
    }

    pub(crate) fn read_packet(&mut self) -> Result<Packet, String> {
        read_packet_from(&mut self.stream)
    }

    pub(crate) fn read_response(&mut self) -> Result<WireResponse, String> {
        self.read_response_at(1).map(|(response, _)| response)
    }

    pub(crate) fn read_all_responses(&mut self) -> Result<Vec<WireResponse>, String> {
        let mut expected_sequence = 1;
        let mut responses = Vec::new();
        loop {
            let (response, next_sequence) = self.read_response_at(expected_sequence)?;
            // 多结果属于同一条命令，下一响应继续沿用前一响应之后的包序号。
            let has_more = response_status(&response) & SERVER_MORE_RESULTS_EXISTS != 0;
            responses.push(response);
            if !has_more {
                return Ok(responses);
            }
            expected_sequence = next_sequence;
        }
    }

    fn read_response_at(&mut self, expected_sequence: u8) -> Result<(WireResponse, u8), String> {
        let first = self.read_packet()?;
        if first.sequence != expected_sequence {
            return Err(format!(
                "first command response sequence {}, expected {expected_sequence}",
                first.sequence,
            ));
        }
        if matches!(first.payload.first(), Some(0x00 | 0xff)) {
            let next_sequence = first.sequence.wrapping_add(1);
            return parse_simple_response(&first.payload, self.handshake.negotiated_capabilities)
                .map(|response| (response, next_sequence));
        }
        self.read_result_set(first)
            .map(|(result, next_sequence)| (WireResponse::ResultSet(result), next_sequence))
    }

    fn read_result_set(&mut self, first: Packet) -> Result<(TextResultSet, u8), String> {
        let mut count_cursor = PacketCursor::new(&first.payload);
        let column_count = count_cursor.read_lenenc_int()? as usize;
        count_cursor.finish("column count")?;

        let mut sequence = first.sequence.wrapping_add(1);
        let mut columns = Vec::with_capacity(column_count);
        for _ in 0..column_count {
            let packet = self.read_expected_packet(sequence)?;
            columns.push(parse_column_definition(&packet.payload)?);
            sequence = sequence.wrapping_add(1);
        }

        let deprecate_eof = self.handshake.negotiated_capabilities & CLIENT_DEPRECATE_EOF != 0;
        if !deprecate_eof {
            // 旧协议在列元数据后单独发送 EOF；新协议省略这一包，只保留行结束包。
            let metadata_end = self.read_expected_packet(sequence)?;
            if !is_result_set_terminator(&metadata_end.payload) {
                return Err("result-set metadata is missing its EOF packet".to_owned());
            }
            parse_terminator(&metadata_end.payload, false)?;
            sequence = sequence.wrapping_add(1);
        }

        let mut rows = Vec::new();
        loop {
            let packet = self.read_expected_packet(sequence)?;
            if is_result_set_terminator(&packet.payload) {
                let (status, warnings) = parse_terminator(&packet.payload, deprecate_eof)?;
                return Ok((
                    TextResultSet {
                        columns,
                        rows,
                        status,
                        warnings,
                    },
                    sequence.wrapping_add(1),
                ));
            }
            rows.push(parse_text_row(&packet.payload, column_count)?);
            sequence = sequence.wrapping_add(1);
        }
    }

    fn read_binary_result_set(&mut self, first: Packet) -> Result<(TextResultSet, u8), String> {
        let mut count_cursor = PacketCursor::new(&first.payload);
        let column_count = count_cursor.read_lenenc_int()? as usize;
        count_cursor.finish("binary column count")?;
        let mut sequence = first.sequence.wrapping_add(1);
        let mut columns = Vec::with_capacity(column_count);
        for _ in 0..column_count {
            let packet = self.read_expected_packet(sequence)?;
            columns.push(parse_column_definition(&packet.payload)?);
            sequence = sequence.wrapping_add(1);
        }
        let deprecate_eof = self.handshake.negotiated_capabilities & CLIENT_DEPRECATE_EOF != 0;
        if !deprecate_eof {
            let packet = self.read_expected_packet(sequence)?;
            parse_terminator(&packet.payload, false)?;
            sequence = sequence.wrapping_add(1);
        }
        let mut rows = Vec::new();
        loop {
            let packet = self.read_expected_packet(sequence)?;
            if is_result_set_terminator(&packet.payload) {
                let (status, warnings) = parse_terminator(&packet.payload, deprecate_eof)?;
                return Ok((
                    TextResultSet {
                        columns,
                        rows,
                        status,
                        warnings,
                    },
                    sequence.wrapping_add(1),
                ));
            }
            rows.push(parse_binary_row(&packet.payload, &columns)?);
            sequence = sequence.wrapping_add(1);
        }
    }

    fn read_expected_packet(&mut self, expected: u8) -> Result<Packet, String> {
        let packet = self.read_packet()?;
        if packet.sequence != expected {
            return Err(format!(
                "packet sequence {}, expected {expected}",
                packet.sequence
            ));
        }
        Ok(packet)
    }
}

fn response_status(response: &WireResponse) -> u16 {
    match response {
        WireResponse::Ok(packet) => packet.status,
        WireResponse::Err(_) => 0,
        WireResponse::ResultSet(result) => result.status,
    }
}

/// 按 MySQL 长度编码整数格式追加数值；三字节分支使用小端低 24 位。
pub(crate) fn put_lenenc_int(output: &mut Vec<u8>, value: u64) {
    match value {
        0..=250 => output.push(value as u8),
        251..=0xffff => {
            output.push(0xfc);
            output.extend_from_slice(&(value as u16).to_le_bytes());
        }
        0x1_0000..=0xff_ffff => {
            output.push(0xfd);
            output.extend_from_slice(&[value as u8, (value >> 8) as u8, (value >> 16) as u8]);
        }
        _ => {
            output.push(0xfe);
            output.extend_from_slice(&value.to_le_bytes());
        }
    }
}

/// 追加“长度编码整数 + 原始内容”形式的字节串。
pub(crate) fn put_lenenc_bytes(output: &mut Vec<u8>, value: &[u8]) {
    put_lenenc_int(output, value.len() as u64);
    output.extend_from_slice(value);
}

fn read_packet_from(stream: &mut TcpStream) -> Result<Packet, String> {
    let mut packet = read_frame_from(stream)?;
    if packet.payload.len() < 0x00ff_ffff {
        return Ok(packet);
    }

    let first_sequence = packet.sequence;
    let mut expected_sequence = first_sequence.wrapping_add(1);
    loop {
        let continuation = read_frame_from(stream)?;
        if continuation.sequence != expected_sequence {
            return Err(format!(
                "continuation packet sequence {}, expected {expected_sequence}",
                continuation.sequence
            ));
        }
        expected_sequence = expected_sequence.wrapping_add(1);
        let final_frame = continuation.payload.len() < 0x00ff_ffff;
        packet.payload.extend_from_slice(&continuation.payload);
        if final_frame {
            packet.sequence = first_sequence;
            return Ok(packet);
        }
    }
}

fn read_frame_from(stream: &mut TcpStream) -> Result<Packet, String> {
    let mut header = [0_u8; 4];
    stream
        .read_exact(&mut header)
        .map_err(|error| format!("read packet header: {error}"))?;
    let length =
        usize::from(header[0]) | (usize::from(header[1]) << 8) | (usize::from(header[2]) << 16);
    let mut payload = vec![0_u8; length];
    stream
        .read_exact(&mut payload)
        .map_err(|error| format!("read packet payload: {error}"))?;
    Ok(Packet {
        sequence: header[3],
        payload,
    })
}

fn write_packet_to(stream: &mut TcpStream, sequence: u8, payload: &[u8]) -> Result<(), String> {
    // 测试客户端不实现 16 MiB 负载的多帧拆分，超限时显式报错。
    if payload.len() > 0x00ff_ffff {
        return Err(format!(
            "test client only writes single-frame packets, got {} bytes",
            payload.len()
        ));
    }
    let length = payload.len();
    let header = [
        length as u8,
        (length >> 8) as u8,
        (length >> 16) as u8,
        sequence,
    ];
    stream
        .write_all(&header)
        .and_then(|_| stream.write_all(payload))
        .and_then(|_| stream.flush())
        .map_err(|error| format!("write packet: {error}"))
}

fn parse_handshake(payload: &[u8]) -> Result<Handshake, String> {
    let mut cursor = PacketCursor::new(payload);
    let protocol_version = cursor.read_u8()?;
    let server_version = String::from_utf8_lossy(cursor.read_nul_terminated()?).into_owned();
    let connection_id = cursor.read_u32()?;
    cursor.read_exact(8)?;
    cursor.read_u8()?;
    let lower_capabilities = u32::from(cursor.read_u16()?);
    let character_set = cursor.read_u8()?;
    let status = cursor.read_u16()?;
    let upper_capabilities = u32::from(cursor.read_u16()?);
    Ok(Handshake {
        protocol_version,
        server_version,
        connection_id,
        server_capabilities: lower_capabilities | (upper_capabilities << 16),
        negotiated_capabilities: 0,
        character_set,
        status,
    })
}

fn parse_simple_response(payload: &[u8], capabilities: u32) -> Result<WireResponse, String> {
    match payload.first() {
        Some(0x00) => parse_ok_packet(payload, capabilities).map(WireResponse::Ok),
        Some(0xff) => parse_err_packet(payload, capabilities).map(WireResponse::Err),
        Some(header) => Err(format!(
            "expected OK or ERR response, got header 0x{header:02x}"
        )),
        None => Err("empty MySQL response packet".to_owned()),
    }
}

fn parse_ok_packet(payload: &[u8], capabilities: u32) -> Result<OkPacket, String> {
    let mut cursor = PacketCursor::new(payload);
    let header = cursor.read_u8()?;
    let affected_rows = cursor.read_lenenc_int()?;
    let last_insert_id = cursor.read_lenenc_int()?;
    let (status, warnings) = if capabilities & CLIENT_PROTOCOL_41 != 0 {
        (cursor.read_u16()?, cursor.read_u16()?)
    } else {
        (0, 0)
    };
    // TiDB's Go writeOkWith encodes a non-empty message as a length-encoded
    // string, despite the protocol manual historically describing string<EOF>.
    let info = if cursor.remaining().is_empty() {
        Vec::new()
    } else {
        cursor
            .read_lenenc_bytes()?
            .ok_or_else(|| "OK packet info cannot be NULL".to_owned())?
            .to_vec()
    };
    cursor.finish("OK packet")?;
    Ok(OkPacket {
        header,
        affected_rows,
        last_insert_id,
        status,
        warnings,
        info,
    })
}

fn parse_err_packet(payload: &[u8], capabilities: u32) -> Result<ErrPacket, String> {
    let mut cursor = PacketCursor::new(payload);
    if cursor.read_u8()? != 0xff {
        return Err("ERR packet has an invalid header".to_owned());
    }
    let code = cursor.read_u16()?;
    let sql_state = if capabilities & CLIENT_PROTOCOL_41 != 0 {
        if cursor.read_u8()? != b'#' {
            return Err("ERR packet is missing the SQLSTATE marker".to_owned());
        }
        let bytes = cursor.read_exact(5)?;
        Some(bytes.try_into().expect("five-byte SQLSTATE"))
    } else {
        None
    };
    let message = String::from_utf8_lossy(cursor.remaining()).into_owned();
    Ok(ErrPacket {
        code,
        sql_state,
        message,
    })
}

fn parse_column_definition(payload: &[u8]) -> Result<ColumnDefinition, String> {
    let mut cursor = PacketCursor::new(payload);
    let catalog = cursor.read_lenenc_string()?;
    let schema = cursor.read_lenenc_string()?;
    let table = cursor.read_lenenc_string()?;
    let org_table = cursor.read_lenenc_string()?;
    let name = cursor.read_lenenc_string()?;
    let org_name = cursor.read_lenenc_string()?;
    let fixed_length = cursor.read_lenenc_int()?;
    if fixed_length != 0x0c {
        return Err(format!(
            "column definition fixed field length is {fixed_length}, expected 12"
        ));
    }
    let character_set = cursor.read_u16()?;
    let column_length = cursor.read_u32()?;
    let column_type = cursor.read_u8()?;
    let flags = cursor.read_u16()?;
    let decimals = cursor.read_u8()?;
    cursor.read_exact(2)?;
    let default_value = if cursor.remaining().is_empty() {
        None
    } else {
        cursor.read_lenenc_bytes()?.map(ToOwned::to_owned)
    };
    cursor.finish("column definition")?;
    Ok(ColumnDefinition {
        catalog,
        schema,
        table,
        org_table,
        name,
        org_name,
        character_set,
        column_length,
        column_type,
        flags,
        decimals,
        default_value,
    })
}

fn parse_text_row(payload: &[u8], column_count: usize) -> Result<Vec<TextValue>, String> {
    let mut cursor = PacketCursor::new(payload);
    let mut row = Vec::with_capacity(column_count);
    for _ in 0..column_count {
        row.push(match cursor.read_lenenc_bytes()? {
            Some(value) => TextValue::Bytes(value.to_vec()),
            None => TextValue::Null,
        });
    }
    cursor.finish("text row")?;
    Ok(row)
}

fn parse_binary_row(
    payload: &[u8],
    columns: &[ColumnDefinition],
) -> Result<Vec<TextValue>, String> {
    let mut cursor = PacketCursor::new(payload);
    if cursor.read_u8()? != 0 {
        return Err("binary row is missing its 0x00 header".to_owned());
    }
    let bitmap = cursor.read_exact((columns.len() + 9) / 8)?.to_vec();
    let mut row = Vec::with_capacity(columns.len());
    for (index, column) in columns.iter().enumerate() {
        // 二进制行的 NULL 位图为两个保留位预留偏移，因此列索引需加 2。
        if bitmap[(index + 2) >> 3] & (1 << ((index + 2) & 7)) != 0 {
            row.push(TextValue::Null);
            continue;
        }
        let unsigned = column.flags & 0x0020 != 0;
        let value = match column.column_type {
            0x01 => {
                let value = cursor.read_u8()?;
                if unsigned {
                    value.to_string()
                } else {
                    (value as i8).to_string()
                }
                .into_bytes()
            }
            0x02 | 0x0d => {
                let value = cursor.read_u16()?;
                if unsigned {
                    value.to_string()
                } else {
                    (value as i16).to_string()
                }
                .into_bytes()
            }
            0x03 | 0x09 => {
                let value = cursor.read_u32()?;
                if unsigned {
                    value.to_string()
                } else {
                    (value as i32).to_string()
                }
                .into_bytes()
            }
            0x08 => {
                let value = u64::from_le_bytes(
                    cursor
                        .read_exact(8)?
                        .try_into()
                        .expect("eight-byte integer"),
                );
                if unsigned {
                    value.to_string()
                } else {
                    (value as i64).to_string()
                }
                .into_bytes()
            }
            0x04 => f32::from_le_bytes(cursor.read_exact(4)?.try_into().expect("four-byte float"))
                .to_string()
                .into_bytes(),
            0x05 => f64::from_le_bytes(cursor.read_exact(8)?.try_into().expect("eight-byte float"))
                .to_string()
                .into_bytes(),
            0x0a | 0x07 | 0x0b | 0x0c => parse_binary_temporal(&mut cursor, column.column_type)?,
            _ => cursor
                .read_lenenc_bytes()?
                .ok_or_else(|| "binary row encoded NULL outside the bitmap".to_owned())?
                .to_vec(),
        };
        row.push(TextValue::Bytes(value));
    }
    cursor.finish("binary row")?;
    Ok(row)
}

fn parse_binary_temporal(cursor: &mut PacketCursor<'_>, tp: u8) -> Result<Vec<u8>, String> {
    let length = usize::from(cursor.read_u8()?);
    if length == 0 {
        return Ok(if tp == 0x0b {
            b"00:00:00".to_vec()
        } else {
            b"0000-00-00".to_vec()
        });
    }
    let value = cursor.read_exact(length)?;
    if tp == 0x0b {
        // TIME 的天数需并入小时，且负号与微秒由长度字段决定。
        if !matches!(length, 8 | 12) {
            return Err(format!("invalid binary TIME length {length}"));
        }
        let days = u32::from_le_bytes(value[1..5].try_into().expect("TIME days"));
        let hours = days * 24 + u32::from(value[5]);
        let mut rendered = format!(
            "{}{hours:02}:{:02}:{:02}",
            if value[0] == 0 { "" } else { "-" },
            value[6],
            value[7]
        );
        if length == 12 {
            let micros = u32::from_le_bytes(value[8..12].try_into().expect("TIME micros"));
            rendered.push_str(&format!(".{micros:06}"));
        }
        return Ok(rendered.into_bytes());
    }
    if !matches!(length, 4 | 7 | 11) {
        return Err(format!("invalid binary date/time length {length}"));
    }
    let year = u16::from_le_bytes(value[0..2].try_into().expect("date year"));
    let mut rendered = format!("{year:04}-{:02}-{:02}", value[2], value[3]);
    if length >= 7 {
        rendered.push_str(&format!(" {:02}:{:02}:{:02}", value[4], value[5], value[6]));
    }
    if length == 11 {
        let micros = u32::from_le_bytes(value[7..11].try_into().expect("datetime micros"));
        rendered.push_str(&format!(".{micros:06}"));
    }
    Ok(rendered.into_bytes())
}

fn is_result_set_terminator(payload: &[u8]) -> bool {
    // 0xfe 只有在包长小于 9 时才是 EOF/替代 EOF 的 OK，避免与长度编码值混淆。
    payload.first() == Some(&0xfe) && payload.len() < 9
}

fn parse_terminator(payload: &[u8], deprecate_eof: bool) -> Result<(u16, u16), String> {
    if !is_result_set_terminator(payload) {
        return Err("packet is not a result-set terminator".to_owned());
    }
    if deprecate_eof {
        let packet = parse_ok_packet(payload, CLIENT_PROTOCOL_41 | CLIENT_DEPRECATE_EOF)?;
        return Ok((packet.status, packet.warnings));
    }
    let mut cursor = PacketCursor::new(payload);
    cursor.read_u8()?;
    let warnings = cursor.read_u16()?;
    let status = cursor.read_u16()?;
    cursor.finish("EOF packet")?;
    Ok((status, warnings))
}

/// 在单个包负载上执行带边界检查的小端与长度编码读取。
struct PacketCursor<'a> {
    payload: &'a [u8],
    offset: usize,
}

impl<'a> PacketCursor<'a> {
    fn new(payload: &'a [u8]) -> Self {
        Self { payload, offset: 0 }
    }

    fn remaining(&self) -> &'a [u8] {
        &self.payload[self.offset..]
    }

    fn read_u8(&mut self) -> Result<u8, String> {
        Ok(self.read_exact(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16, String> {
        let bytes: [u8; 2] = self.read_exact(2)?.try_into().expect("two-byte slice");
        Ok(u16::from_le_bytes(bytes))
    }

    fn read_u32(&mut self) -> Result<u32, String> {
        let bytes: [u8; 4] = self.read_exact(4)?.try_into().expect("four-byte slice");
        Ok(u32::from_le_bytes(bytes))
    }

    fn read_exact(&mut self, length: usize) -> Result<&'a [u8], String> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| "packet cursor overflow".to_owned())?;
        if end > self.payload.len() {
            return Err(format!(
                "packet ended at {}, need {length} bytes at {}",
                self.payload.len(),
                self.offset
            ));
        }
        let value = &self.payload[self.offset..end];
        self.offset = end;
        Ok(value)
    }

    fn read_nul_terminated(&mut self) -> Result<&'a [u8], String> {
        let remaining = self.remaining();
        let length = remaining
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| "packet is missing a NUL terminator".to_owned())?;
        let value = self.read_exact(length)?;
        self.read_u8()?;
        Ok(value)
    }

    fn read_lenenc_int(&mut self) -> Result<u64, String> {
        match self.read_u8()? {
            value @ 0x00..=0xfa => Ok(u64::from(value)),
            0xfb => Err("NULL cannot be decoded as a length-encoded integer".to_owned()),
            0xfc => Ok(u64::from(self.read_u16()?)),
            0xfd => {
                let bytes = self.read_exact(3)?;
                Ok(u64::from(bytes[0]) | (u64::from(bytes[1]) << 8) | (u64::from(bytes[2]) << 16))
            }
            0xfe => {
                let bytes: [u8; 8] = self.read_exact(8)?.try_into().expect("eight-byte slice");
                Ok(u64::from_le_bytes(bytes))
            }
            0xff => Err("0xff is not a length-encoded integer prefix".to_owned()),
        }
    }

    fn read_lenenc_bytes(&mut self) -> Result<Option<&'a [u8]>, String> {
        if self.remaining().first() == Some(&0xfb) {
            self.offset += 1;
            return Ok(None);
        }
        let length = self.read_lenenc_int()?;
        let length = usize::try_from(length)
            .map_err(|_| "length-encoded value does not fit usize".to_owned())?;
        self.read_exact(length).map(Some)
    }

    fn read_lenenc_string(&mut self) -> Result<String, String> {
        let bytes = self
            .read_lenenc_bytes()?
            .ok_or_else(|| "column definition string is NULL".to_owned())?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }

    fn finish(&self, context: &str) -> Result<(), String> {
        if self.offset == self.payload.len() {
            Ok(())
        } else {
            Err(format!(
                "{context} has {} trailing bytes",
                self.payload.len() - self.offset
            ))
        }
    }
}

#[cfg(test)]
#[path = "mysql_compat_test_support_test.rs"]
mod tests;
