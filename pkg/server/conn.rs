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

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals
)]

// MySQL 客户端连接状态机（对应 Go `conn.go`）。
//
// 负责握手、鉴权、命令分发与结果写回；具体 Server/Session/Packet 实现由 trait 注入，
// 本模块保持与 Go 侧调用顺序一致。
//
// MySQL client connection state machine.
//
// The concrete server, session and packet implementations live outside this
// compilation unit.  The traits below make every network, authentication and
// SQL side effect explicit, while keeping the ordering of `conn.go` intact.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 连接正在分发/处理命令。
pub const connStatusDispatching: i32 = 0;
/// 连接正在等待读取客户端报文。
pub const connStatusReading: i32 = 1;
/// 连接已进入关闭流程。
pub const connStatusShutdown: i32 = 2;
/// 连接等待优雅关闭（处理完当前事务后退出）。
pub const connStatusWaitShutdown: i32 = 3;

/// SHOW STATUS 中压缩是否启用的键名。
pub const statusCompression: &str = "Compression";
/// SHOW STATUS 中压缩算法键名。
pub const statusCompressionAlgorithm: &str = "Compression_algorithm";
/// SHOW STATUS 中压缩级别键名。
pub const statusCompressionLevel: &str = "Compression_level";
/// 连接属性中网关连接 ID 的键。
pub const tidbGatewayAttrsConnKey: &str = "TiDB-Gateway-ConnID";

// MySQL 客户端能力标志位（capability flags）。
const CLIENT_CONNECT_WITH_DB: u32 = 1 << 3;
const CLIENT_COMPRESS: u32 = 1 << 5;
pub(crate) const CLIENT_PROTOCOL_41: u32 = 1 << 9;
const CLIENT_SSL: u32 = 1 << 11;
const CLIENT_MULTI_STATEMENTS: u32 = 1 << 16;
const CLIENT_MULTI_RESULTS: u32 = 1 << 17;
const CLIENT_PLUGIN_AUTH: u32 = 1 << 19;
pub(crate) const CLIENT_CONNECT_ATTRS: u32 = 1 << 20;
const CLIENT_PLUGIN_AUTH_LENENC_CLIENT_DATA: u32 = 1 << 21;
const CLIENT_SESSION_TRACK: u32 = 1 << 23;
const CLIENT_DEPRECATE_EOF: u32 = 1 << 24;
const CLIENT_ZSTD_COMPRESSION_ALGORITHM: u32 = 1 << 26;
const MAX_CONNECTION_ATTRIBUTES_SIZE: u64 = 1 << 20;

// 服务器会话状态标志。
const SERVER_STATUS_IN_TRANS: u16 = 0x0001;
const SERVER_STATUS_AUTOCOMMIT: u16 = 0x0002;
const SERVER_MORE_RESULTS_EXISTS: u16 = 0x0008;
const SERVER_STATUS_CURSOR_EXISTS: u16 = 0x0040;
const SERVER_STATUS_LAST_ROW_SENT: u16 = 0x0080;
const SERVER_SESSION_STATE_CHANGED: u16 = 0x4000;

// MySQL 协议包头常量。
const OK_HEADER: u8 = 0x00;
const EOF_HEADER: u8 = 0xfe;
const ERR_HEADER: u8 = 0xff;
const AUTH_SWITCH_REQUEST: u8 = 0xfe;
const LOCAL_IN_FILE_HEADER: u8 = 0xfb;

/// 原生密码插件名。
pub const AUTH_NATIVE_PASSWORD: &str = "mysql_native_password";
/// caching_sha2 密码插件名。
pub const AUTH_CACHING_SHA2_PASSWORD: &str = "caching_sha2_password";
/// TiDB SM3 密码插件名。
pub const AUTH_TIDB_SM3_PASSWORD: &str = "tidb_sm3_password";
/// Unix socket 鉴权插件名。
pub const AUTH_SOCKET: &str = "auth_socket";
/// TiDB 会话令牌鉴权插件。
pub const AUTH_TIDB_SESSION_TOKEN: &str = "tidb_session_token";
/// TiDB 认证令牌插件。
pub const AUTH_TIDB_AUTH_TOKEN: &str = "tidb_auth_token";
/// 明文密码插件。
pub const AUTH_CLEAR_PASSWORD: &str = "mysql_clear_password";
/// LDAP SASL 鉴权插件。
pub const AUTH_LDAP_SASL: &str = "authentication_ldap_sasl";
/// LDAP Simple 鉴权插件。
pub const AUTH_LDAP_SIMPLE: &str = "authentication_ldap_simple";

#[derive(Debug, Clone, PartialEq, Eq)]
/// 连接层错误：IO、协议、鉴权、命令不支持等。
pub enum ConnError {
    Io(String),
    MalformedPacket(&'static str),
    UnsupportedProtocol,
    SecureTransportRequired,
    AccessDenied { user: String, host: String },
    UnknownAuthPlugin(String),
    UnsupportedCommand(u8),
    ServerShutdown,
    ResultUndetermined(String),
    ClientQuit,
    Session(String),
    Poisoned(&'static str),
}

impl fmt::Display for ConnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message) | Self::ResultUndetermined(message) | Self::Session(message) => {
                f.write_str(message)
            }
            Self::MalformedPacket(message) => write!(f, "malformed packet: {message}"),
            Self::UnsupportedProtocol => f.write_str("CLIENT_PROTOCOL_41 is required"),
            Self::SecureTransportRequired => f.write_str("secure transport is required"),
            Self::AccessDenied { user, host } => write!(f, "access denied for {user}@{host}"),
            Self::UnknownAuthPlugin(plugin) => write!(f, "unknown auth plugin {plugin}"),
            Self::UnsupportedCommand(command) => write!(f, "command {command} is not supported"),
            Self::ServerShutdown => f.write_str("server is shutting down"),
            Self::ClientQuit => f.write_str("client quit"),
            Self::Poisoned(name) => write!(f, "{name} lock is poisoned"),
        }
    }
}

impl std::error::Error for ConnError {}

/// 连接操作统一 Result 别名。
pub type ConnResult<T> = Result<T, ConnError>;

fn mysql_error_code_and_state(error: &ConnError) -> (u16, &'static [u8; 5]) {
    match error {
        ConnError::AccessDenied { .. } => (1045, b"28000"),
        ConnError::ServerShutdown => (1053, b"08S01"),
        ConnError::UnsupportedCommand(_) => (1047, b"08S01"),
        ConnError::MalformedPacket(_) => (1835, b"HY000"),
        ConnError::Session(message) => {
            let lower = message.to_ascii_lowercase();
            if lower.contains("[kv:1062]") || lower.contains("duplicate entry") {
                (1062, b"23000")
            } else if lower.contains("[kv:1451]")
                || lower.contains("cannot delete or update a parent row")
            {
                (1451, b"23000")
            } else if lower.contains("[kv:1452]")
                || lower.contains("[ddl:1452]")
                || lower.contains("cannot add or update a child row")
            {
                (1452, b"23000")
            } else if lower.contains("cannot be null") {
                (1048, b"23000")
            } else if lower.contains("unknown column") {
                (1054, b"42S22")
            } else if lower.contains("table") && lower.contains("already exists") {
                (1050, b"42S01")
            } else if lower.contains("database") && lower.contains("already exists") {
                (1007, b"HY000")
            } else if lower.contains("can't drop database") && lower.contains("doesn't exist") {
                (1008, b"HY000")
            } else if lower.contains("unknown database")
                || (lower.contains("database") && lower.contains("doesn't exist"))
            {
                (1049, b"42000")
            } else if lower.contains("no database selected") {
                (1046, b"3D000")
            } else if (lower.contains("table") && lower.contains("not found"))
                || (lower.contains("table") && lower.contains("doesn't exist"))
            {
                (1146, b"42S02")
            } else if lower.contains("[executor:1305]")
                || (lower.contains("savepoint") && lower.contains("does not exist"))
            {
                (1305, b"42000")
            } else if lower.contains("syntax error") || lower.contains("[parser:1064]") {
                (1064, b"42000")
            } else if lower.contains("query execution was interrupted")
                || lower.contains("query interrupted")
                || lower.contains("query cancelled")
                || lower.contains("query canceled")
            {
                (1317, b"70100")
            } else if lower.contains("unbound parameter")
                || lower.contains("incorrect arguments to")
            {
                (1210, b"HY000")
            } else {
                (1105, b"HY000")
            }
        }
        _ => (1105, b"HY000"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// 客户端/服务端协商的压缩算法。
pub enum CompressionAlgorithm {
    None,
    Zlib,
    Zstd,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// TLS 握手后的版本与密码套件摘要。
pub struct TlsState {
    pub version: u16,
    pub cipher_suite: u16,
}

#[derive(Debug, Clone)]
/// 握手与连接限制所需的服务端配置快照。
pub struct ServerConfig {
    pub capability: u32,
    pub default_collation: u8,
    pub server_version: String,
    pub default_auth_plugin: String,
    pub require_secure_transport: bool,
    pub init_connect: String,
    pub max_connections: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 鉴权身份：用户、主机与插件。
pub struct AuthIdentity {
    pub username: String,
    pub hostname: String,
    pub plugin: String,
}

#[derive(Debug, Clone)]
/// 提交给 Session 的完整鉴权请求。
pub struct AuthRequest {
    pub identity: AuthIdentity,
    pub auth_data: Vec<u8>,
    pub salt: Vec<u8>,
    pub tls_state: Option<TlsState>,
    pub attributes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
/// 会话执行后的状态：影响行数、insert id、警告与状态位等。
pub struct SessionState {
    pub affected_rows: u64,
    pub last_insert_id: u64,
    pub warning_count: u16,
    pub status: u16,
    pub last_message: String,
    pub resource_group: String,
}

#[derive(Debug, Clone, PartialEq)]
/// 结果集单元格的文本协议编码值。
pub enum Value {
    Null,
    Bytes(Vec<u8>),
    Text(String),
    Signed(i64),
    Unsigned(u64),
    Float(f64),
}

fn empty_column_info() -> ColumnInfo {
    ColumnInfo {
        schema: String::new(),
        table: String::new(),
        org_table: String::new(),
        name: String::new(),
        org_name: String::new(),
        charset: 0,
        column_length: 0,
        column_type: 0,
        flags: 0,
        decimals: 0,
        default_value: None,
    }
}

fn read_u16_le(data: &[u8], offset: usize) -> ConnResult<u16> {
    let bytes: [u8; 2] = data
        .get(offset..offset + 2)
        .ok_or(ConnError::MalformedPacket("two-byte integer"))?
        .try_into()
        .map_err(|_| ConnError::MalformedPacket("two-byte integer"))?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32_le(data: &[u8], offset: usize) -> ConnResult<u32> {
    let bytes: [u8; 4] = data
        .get(offset..offset + 4)
        .ok_or(ConnError::MalformedPacket("four-byte integer"))?
        .try_into()
        .map_err(|_| ConnError::MalformedPacket("four-byte integer"))?;
    Ok(u32::from_le_bytes(bytes))
}

fn parse_unsigned(bytes: &[u8], context: &str) -> ConnResult<u64> {
    std::str::from_utf8(bytes)
        .map_err(|_| ConnError::Session(format!("{context} is not UTF-8")))?
        .parse::<u64>()
        .map_err(|error| ConnError::Session(format!("parse {context}: {error}")))
}

fn parse_signed(bytes: &[u8], context: &str) -> ConnResult<i64> {
    std::str::from_utf8(bytes)
        .map_err(|_| ConnError::Session(format!("{context} is not UTF-8")))?
        .parse::<i64>()
        .map_err(|error| ConnError::Session(format!("parse {context}: {error}")))
}

fn encode_binary_column(packet: &mut Vec<u8>, column: &ColumnInfo, value: &[u8]) -> ConnResult<()> {
    const UNSIGNED_FLAG: u16 = 1 << 5;
    match column.column_type {
        0x01 => packet.push(if column.flags & UNSIGNED_FLAG != 0 {
            parse_unsigned(value, "TINYINT")? as u8
        } else {
            parse_signed(value, "TINYINT")? as i8 as u8
        }),
        0x02 | 0x0d => {
            let encoded = if column.flags & UNSIGNED_FLAG != 0 {
                parse_unsigned(value, "SMALLINT")? as u16
            } else {
                parse_signed(value, "SMALLINT")? as i16 as u16
            };
            packet.extend_from_slice(&encoded.to_le_bytes());
        }
        0x03 | 0x09 => {
            let encoded = if column.flags & UNSIGNED_FLAG != 0 {
                parse_unsigned(value, "INTEGER")? as u32
            } else {
                parse_signed(value, "INTEGER")? as i32 as u32
            };
            packet.extend_from_slice(&encoded.to_le_bytes());
        }
        0x08 => {
            let encoded = if column.flags & UNSIGNED_FLAG != 0 {
                parse_unsigned(value, "BIGINT")?
            } else {
                parse_signed(value, "BIGINT")? as u64
            };
            packet.extend_from_slice(&encoded.to_le_bytes());
        }
        0x04 => {
            let value = std::str::from_utf8(value)
                .map_err(|_| ConnError::Session("FLOAT is not UTF-8".to_owned()))?
                .parse::<f32>()
                .map_err(|error| ConnError::Session(format!("parse FLOAT: {error}")))?;
            packet.extend_from_slice(&value.to_le_bytes());
        }
        0x05 => {
            let value = std::str::from_utf8(value)
                .map_err(|_| ConnError::Session("DOUBLE is not UTF-8".to_owned()))?
                .parse::<f64>()
                .map_err(|error| ConnError::Session(format!("parse DOUBLE: {error}")))?;
            packet.extend_from_slice(&value.to_le_bytes());
        }
        0x0a => encode_binary_date(packet, value)?,
        0x07 | 0x0c => encode_binary_datetime(packet, value)?,
        0x0b => encode_binary_time(packet, value)?,
        _ => put_lenenc_bytes(packet, value),
    }
    Ok(())
}

fn encode_binary_date(packet: &mut Vec<u8>, value: &[u8]) -> ConnResult<()> {
    let value = std::str::from_utf8(value)
        .map_err(|_| ConnError::Session("DATE is not UTF-8".to_owned()))?;
    let parts = value
        .split('-')
        .map(str::parse::<u16>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ConnError::Session(format!("parse DATE: {error}")))?;
    if parts.len() != 3 {
        return Err(ConnError::Session(format!("invalid DATE {value}")));
    }
    packet.push(4);
    packet.extend_from_slice(&parts[0].to_le_bytes());
    packet.push(parts[1] as u8);
    packet.push(parts[2] as u8);
    Ok(())
}

fn encode_binary_datetime(packet: &mut Vec<u8>, value: &[u8]) -> ConnResult<()> {
    let value = std::str::from_utf8(value)
        .map_err(|_| ConnError::Session("DATETIME is not UTF-8".to_owned()))?;
    let (date, time) = value
        .split_once(' ')
        .ok_or_else(|| ConnError::Session(format!("invalid DATETIME {value}")))?;
    let date = date
        .split('-')
        .map(str::parse::<u16>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ConnError::Session(format!("parse DATETIME date: {error}")))?;
    let (clock, fraction) = time.split_once('.').unwrap_or((time, ""));
    let clock = clock
        .split(':')
        .map(str::parse::<u8>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ConnError::Session(format!("parse DATETIME time: {error}")))?;
    if date.len() != 3 || clock.len() != 3 {
        return Err(ConnError::Session(format!("invalid DATETIME {value}")));
    }
    let micros = if fraction.is_empty() {
        0
    } else {
        format!("{fraction:0<6}")[..6]
            .parse::<u32>()
            .map_err(|error| ConnError::Session(format!("parse DATETIME fraction: {error}")))?
    };
    packet.push(if micros == 0 { 7 } else { 11 });
    packet.extend_from_slice(&date[0].to_le_bytes());
    packet.extend_from_slice(&[date[1] as u8, date[2] as u8, clock[0], clock[1], clock[2]]);
    if micros != 0 {
        packet.extend_from_slice(&micros.to_le_bytes());
    }
    Ok(())
}

fn encode_binary_time(packet: &mut Vec<u8>, value: &[u8]) -> ConnResult<()> {
    let value = std::str::from_utf8(value)
        .map_err(|_| ConnError::Session("TIME is not UTF-8".to_owned()))?;
    let (negative, value) = value
        .strip_prefix('-')
        .map_or((false, value), |value| (true, value));
    let (clock, fraction) = value.split_once('.').unwrap_or((value, ""));
    let clock = clock
        .split(':')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ConnError::Session(format!("parse TIME: {error}")))?;
    if clock.len() != 3 {
        return Err(ConnError::Session(format!("invalid TIME {value}")));
    }
    let days = clock[0] / 24;
    let hours = (clock[0] % 24) as u8;
    let micros = if fraction.is_empty() {
        0
    } else {
        format!("{fraction:0<6}")[..6]
            .parse::<u32>()
            .map_err(|error| ConnError::Session(format!("parse TIME fraction: {error}")))?
    };
    packet.push(if micros == 0 { 8 } else { 12 });
    packet.push(u8::from(negative));
    packet.extend_from_slice(&days.to_le_bytes());
    packet.extend_from_slice(&[hours, clock[1] as u8, clock[2] as u8]);
    if micros != 0 {
        packet.extend_from_slice(&micros.to_le_bytes());
    }
    Ok(())
}

impl Value {
    /// 编码为文本协议字段；`Null` 返回 `None`（写 0xfb）。
    pub(crate) fn encode_text(&self) -> Option<Vec<u8>> {
        match self {
            Self::Null => None,
            Self::Bytes(value) => Some(value.clone()),
            Self::Text(value) => Some(value.as_bytes().to_vec()),
            Self::Signed(value) => Some(value.to_string().into_bytes()),
            Self::Unsigned(value) => Some(value.to_string().into_bytes()),
            Self::Float(value) => Some(value.to_string().into_bytes()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 结果集列元信息（schema/table/name/类型等）。
pub struct ColumnInfo {
    pub schema: String,
    pub table: String,
    pub org_table: String,
    pub name: String,
    pub org_name: String,
    pub charset: u16,
    pub column_length: u32,
    pub column_type: u8,
    pub flags: u16,
    pub decimals: u8,
    pub default_value: Option<Vec<u8>>,
}

pub struct ResponseLifecycle {
    write_duration: Mutex<Duration>,
    finished: AtomicBool,
    finish: Box<dyn Fn(Duration) + Send + Sync>,
}

impl std::fmt::Debug for ResponseLifecycle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResponseLifecycle")
            .field("write_duration", &self.write_duration)
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl ResponseLifecycle {
    pub fn new(finish: impl Fn(Duration) + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            write_duration: Mutex::new(Duration::ZERO),
            finished: AtomicBool::new(false),
            finish: Box::new(finish),
        })
    }

    pub(crate) fn add_write_duration(&self, duration: Duration) {
        if let Ok(mut total) = self.write_duration.lock() {
            *total += duration;
        }
    }

    pub(crate) fn finish(&self) {
        if self
            .finished
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let duration = self
                .write_duration
                .lock()
                .map(|duration| *duration)
                .unwrap_or_default();
            (self.finish)(duration);
        }
    }
}

impl Drop for ResponseLifecycle {
    fn drop(&mut self) {
        if !self.finished.load(Ordering::Acquire) {
            let duration = self
                .write_duration
                .lock()
                .map(|duration| *duration)
                .unwrap_or_default();
            (self.finish)(duration);
        }
    }
}

#[derive(Debug, Clone, Default)]
/// 一次查询/命令产生的结果集（列、行与会话状态）。
pub struct QueryResult {
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<Value>>,
    pub state: SessionState,
    pub response_lifecycle: Option<Arc<ResponseLifecycle>>,
}

#[derive(Debug, Clone)]
pub struct PreparedMetadata {
    pub statement_id: u32,
    pub parameter_count: usize,
    pub columns: Vec<ColumnInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
/// MySQL COM_* 命令字节到枚举的映射。
pub enum Command {
    Sleep = 0x00,
    Quit = 0x01,
    InitDb = 0x02,
    Query = 0x03,
    FieldList = 0x04,
    CreateDb = 0x05,
    DropDb = 0x06,
    Refresh = 0x07,
    Shutdown = 0x08,
    Statistics = 0x09,
    ProcessInfo = 0x0a,
    Connect = 0x0b,
    ProcessKill = 0x0c,
    Debug = 0x0d,
    Ping = 0x0e,
    ChangeUser = 0x11,
    StmtPrepare = 0x16,
    StmtExecute = 0x17,
    StmtSendLongData = 0x18,
    StmtClose = 0x19,
    StmtReset = 0x1a,
    SetOption = 0x1b,
    StmtFetch = 0x1c,
    ResetConnection = 0x1f,
}

impl TryFrom<u8> for Command {
    type Error = ConnError;

    fn try_from(value: u8) -> ConnResult<Self> {
        Ok(match value {
            0x00 => Self::Sleep,
            0x01 => Self::Quit,
            0x02 => Self::InitDb,
            0x03 => Self::Query,
            0x04 => Self::FieldList,
            0x05 => Self::CreateDb,
            0x06 => Self::DropDb,
            0x07 => Self::Refresh,
            0x08 => Self::Shutdown,
            0x09 => Self::Statistics,
            0x0a => Self::ProcessInfo,
            0x0b => Self::Connect,
            0x0c => Self::ProcessKill,
            0x0d => Self::Debug,
            0x0e => Self::Ping,
            0x11 => Self::ChangeUser,
            0x16 => Self::StmtPrepare,
            0x17 => Self::StmtExecute,
            0x18 => Self::StmtSendLongData,
            0x19 => Self::StmtClose,
            0x1a => Self::StmtReset,
            0x1b => Self::SetOption,
            0x1c => Self::StmtFetch,
            0x1f => Self::ResetConnection,
            command => return Err(ConnError::UnsupportedCommand(command)),
        })
    }
}

#[derive(Debug)]
/// 可取消当前正在执行命令的令牌。
pub struct CancellationToken(AtomicBool);

impl CancellationToken {
    /// 创建未取消状态的令牌。
    pub fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    /// 标记为已取消。
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// 查询是否已取消。
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

/// 底层包 IO：读写、刷新、超时、TLS 升级与地址查询。
pub type PacketCloseHandle = Arc<dyn Fn() + Send + Sync>;

pub trait PacketIo: Send {
    fn read_packet(&mut self) -> ConnResult<Vec<u8>>;
    fn write_packet(&mut self, packet: &[u8]) -> ConnResult<()>;
    fn flush(&mut self) -> ConnResult<()>;
    fn reset_sequence(&mut self);
    fn set_read_timeout(&mut self, timeout: Duration) -> ConnResult<()>;
    fn set_compression(&mut self, algorithm: CompressionAlgorithm, zstd_level: i32);
    fn upgrade_to_tls(&mut self) -> ConnResult<TlsState>;
    fn peer_addr(&self) -> ConnResult<(String, String)>;
    fn local_addr(&self) -> ConnResult<(String, String)>;
    fn connection_alive(&self) -> bool;
    /// 返回无需获取 PacketIo 写锁即可关闭底层传输的控制句柄。
    fn close_handle(&self) -> Option<PacketCloseHandle> {
        None
    }
    fn close(&mut self) -> ConnResult<()>;
}

/// Session 侧上下文：鉴权、执行 SQL、切库与取消等。
#[derive(Clone, Debug)]
pub struct SessionProcessSnapshot {
    pub sql: String,
    pub command: u8,
    pub start_time: SystemTime,
}

pub trait TiDBContext: Send + Sync {
    fn state(&self) -> SessionState;
    fn set_connection_status(&self, status: i32);
    fn wait_timeout(&self) -> Duration;
    fn in_transaction(&self) -> bool;
    fn auth_plugin_for_user(&self, user: &str, host: &str) -> ConnResult<String>;
    fn authenticate(&self, request: &AuthRequest) -> ConnResult<()>;
    fn set_compression(&self, algorithm: CompressionAlgorithm, zstd_level: i32);
    fn set_process_info(&self, sql: &str, command: u8);
    fn clear_process_info(&self);
    fn is_connection_admin(&self) -> bool;
    fn execute_init_connect(&self, sql: &str) -> ConnResult<()>;
    fn use_db(&self, db: &str, cancel: &CancellationToken) -> ConnResult<()>;
    fn execute_query(
        &self,
        sql: &str,
        allow_multi_statements: bool,
        cancel: &CancellationToken,
    ) -> ConnResult<Vec<QueryResult>>;
    fn field_list(&self, table: &str, wildcard: &str) -> ConnResult<Vec<ColumnInfo>>;
    fn prepare_statement(
        &self,
        sql: &str,
        cancel: &CancellationToken,
    ) -> ConnResult<PreparedMetadata>;
    fn execute_prepared_statement(
        &self,
        statement_id: u32,
        arguments: &[crate::conn_stmt::BinaryParam],
        cancel: &CancellationToken,
    ) -> ConnResult<QueryResult>;
    fn close_prepared_statement(&self, statement_id: u32) -> ConnResult<()>;
    fn execute_command(
        &self,
        command: Command,
        payload: &[u8],
        cancel: &CancellationToken,
    ) -> ConnResult<Option<QueryResult>>;
    fn finish_protocol_response(&self, _write_duration: Duration) {}
    fn change_user(&self, payload: &[u8], cancel: &CancellationToken) -> ConnResult<()>;
    fn reset_connection(&self, cancel: &CancellationToken) -> ConnResult<()>;
    fn cancel(&self);
    fn close(&self) -> ConnResult<()>;
    fn last_statement(&self) -> String;
    fn process_snapshot(&self) -> SessionProcessSnapshot;
}

/// 根据连接参数打开 `TiDBContext` 的驱动接口。
pub trait SessionDriver: Send + Sync {
    fn open_ctx(
        &self,
        connection_id: u64,
        capability: u32,
        collation: u8,
        database: &str,
        tls_state: Option<&TlsState>,
    ) -> ConnResult<Arc<dyn TiDBContext>>;

    fn set_session_manager(&self, _manager: Weak<dyn astersql_session_sessmgr::Manager>) {}
}

/// 连接 ID 分配与回收（Domain）。
pub trait ConnectionDomain: Send + Sync {
    fn next_connection_id(&self) -> u64;
    fn release_connection_id(&self, connection_id: u64);
}

/// 由共享 `server::Server` 实现的适配器；本模块刻意不复制 Server 类型。
/// Adapter implemented by the shared `server::Server`; this module deliberately
/// does not duplicate the server type.
pub trait ConnectionServer: Send + Sync {
    fn register_connection(&self, connection: Arc<ClientConn>) -> ConnResult<()>;
    fn unregister_connection(&self, connection_id: u64);
    fn connection_count(&self) -> usize;
    fn config(&self) -> ServerConfig;
    fn driver(&self) -> Arc<dyn SessionDriver>;
    fn domain(&self) -> Arc<dyn ConnectionDomain>;
    fn is_shutdown(&self) -> bool;
    fn is_healthy(&self) -> bool;
    fn connection_active(&self, connection_id: u64);
    fn connection_closed(&self, connection_id: u64, reason: &str);
}

#[derive(Debug, Clone, Default)]
/// 客户端握手响应解析结果。
pub struct HandshakeResponse {
    pub capability: u32,
    pub collation: u8,
    pub user: String,
    pub database: String,
    pub auth: Vec<u8>,
    pub auth_plugin: String,
    pub attrs: BTreeMap<String, String>,
    pub zstd_level: i32,
}

/// 单个客户端连接的完整状态机实例。
pub struct ClientConn {
    packet: Mutex<Box<dyn PacketIo>>,
    server: Arc<dyn ConnectionServer>,
    capability: AtomicU32,
    connection_id: AtomicU64,
    user: RwLock<String>,
    database: RwLock<String>,
    salt: Vec<u8>,
    attributes: RwLock<BTreeMap<String, String>>,
    server_host: RwLock<String>,
    peer_host: RwLock<String>,
    peer_port: RwLock<String>,
    status: AtomicI32,
    last_code: AtomicU32,
    collation: AtomicU32,
    last_active: Mutex<Instant>,
    auth_plugin: RwLock<String>,
    unix_socket: bool,
    tls_state: RwLock<Option<TlsState>>,
    context: RwLock<Option<Arc<dyn TiDBContext>>>,
    current_cancel: Mutex<Option<Arc<CancellationToken>>>,
    prepared_statements: Mutex<HashMap<u32, crate::conn_stmt::PreparedStatement>>,
    prepared_columns: Mutex<HashMap<u32, Vec<ColumnInfo>>>,
    last_packet: Mutex<Vec<u8>>,
    packet_close: Option<PacketCloseHandle>,
    closed: AtomicBool,
    registered: AtomicBool,
}

/// 与 Go 侧 `clientConn` 命名对齐的类型别名。
pub type clientConn = ClientConn;

/// 分配连接 ID 并构造初始 `ClientConn`。
pub fn newClientConn(
    server: Arc<dyn ConnectionServer>,
    packet: Box<dyn PacketIo>,
    salt: Vec<u8>,
    unix_socket: bool,
) -> Arc<ClientConn> {
    let config = server.config();
    let connection_id = server.domain().next_connection_id();
    let packet_close = packet.close_handle();
    Arc::new(ClientConn {
        packet: Mutex::new(packet),
        server,
        capability: AtomicU32::new(0),
        connection_id: AtomicU64::new(connection_id),
        user: RwLock::new(String::new()),
        database: RwLock::new(String::new()),
        salt,
        attributes: RwLock::new(BTreeMap::new()),
        server_host: RwLock::new(String::new()),
        peer_host: RwLock::new(String::new()),
        peer_port: RwLock::new(String::new()),
        status: AtomicI32::new(connStatusDispatching),
        last_code: AtomicU32::new(0),
        collation: AtomicU32::new(config.default_collation as u32),
        last_active: Mutex::new(Instant::now()),
        auth_plugin: RwLock::new(config.default_auth_plugin),
        unix_socket,
        tls_state: RwLock::new(None),
        context: RwLock::new(None),
        current_cancel: Mutex::new(None),
        prepared_statements: Mutex::new(HashMap::new()),
        prepared_columns: Mutex::new(HashMap::new()),
        last_packet: Mutex::new(Vec::new()),
        packet_close,
        closed: AtomicBool::new(false),
        registered: AtomicBool::new(false),
    })
}

impl fmt::Debug for ClientConn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientConn")
            .field("connection_id", &self.connection_id.load(Ordering::Acquire))
            .field("capability", &self.capability.load(Ordering::Acquire))
            .field("status", &self.getStatus())
            .field("closed", &self.closed.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl ClientConn {
    /// 读取当前会话上下文。
    pub fn getCtx(&self) -> ConnResult<Option<Arc<dyn TiDBContext>>> {
        self.context
            .read()
            .map(|guard| guard.clone())
            .map_err(|_| ConnError::Poisoned("context"))
    }

    /// 设置或清空会话上下文。
    pub fn SetCtx(&self, context: Option<Arc<dyn TiDBContext>>) -> ConnResult<()> {
        *self
            .context
            .write()
            .map_err(|_| ConnError::Poisoned("context"))? = context;
        Ok(())
    }

    /// 返回 TLS 状态（若已升级）。
    pub fn getTLSState(&self) -> ConnResult<Option<TlsState>> {
        self.tls_state
            .read()
            .map(|state| state.clone())
            .map_err(|_| ConnError::Poisoned("tls_state"))
    }

    /// 更新连接状态并同步到 Session。
    pub fn setStatus(&self, status: i32) {
        self.status.store(status, Ordering::Release);
        if let Ok(Some(context)) = self.getCtx() {
            context.set_connection_status(status);
        }
    }

    /// 读取连接状态。
    pub fn getStatus(&self) -> i32 {
        self.status.load(Ordering::Acquire)
    }

    /// CAS 切换连接状态，失败表示并发关闭等路径已抢先。
    pub fn CompareAndSwapStatus(&self, old_status: i32, new_status: i32) -> bool {
        self.status
            .compare_exchange(old_status, new_status, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// 完整握手：初始包、可选 SSL、插件切换、鉴权、init_connect、注册连接。
    pub fn handshake(self: &Arc<Self>) -> ConnResult<()> {
        self.writeInitialHandshake()?;
        let mut response = self.readOptionalSSLRequestAndHandshakeResponse()?;
        self.handleAuthPlugin(&mut response)?;
        match response.auth_plugin.as_str() {
            AUTH_CACHING_SHA2_PASSWORD => response.auth = self.authSha(response.auth)?,
            AUTH_TIDB_SM3_PASSWORD => response.auth = self.authSM3(response.auth)?,
            AUTH_NATIVE_PASSWORD
            | AUTH_SOCKET
            | AUTH_TIDB_SESSION_TOKEN
            | AUTH_TIDB_AUTH_TOKEN
            | AUTH_CLEAR_PASSWORD
            | AUTH_LDAP_SASL
            | AUTH_LDAP_SIMPLE => {}
            plugin => return Err(ConnError::UnknownAuthPlugin(plugin.to_owned())),
        }
        self.openSessionAndDoAuth(&response)?;
        self.initConnect()?;
        self.writeOK()?;

        let algorithm = if response.capability & CLIENT_COMPRESS != 0 {
            CompressionAlgorithm::Zlib
        } else if response.capability & CLIENT_ZSTD_COMPRESSION_ALGORITHM != 0 {
            CompressionAlgorithm::Zstd
        } else {
            CompressionAlgorithm::None
        };
        self.packet
            .lock()
            .map_err(|_| ConnError::Poisoned("packet"))?
            .set_compression(algorithm, response.zstd_level);
        if let Some(context) = self.getCtx()? {
            context.set_compression(algorithm, response.zstd_level);
        }
        self.packet
            .lock()
            .map_err(|_| ConnError::Poisoned("packet"))?
            .reset_sequence();

        self.registered.store(true, Ordering::Release);
        if let Err(error) = self.server.register_connection(Arc::clone(self)) {
            self.registered.store(false, Ordering::Release);
            return Err(error);
        }
        Ok(())
    }

    /// 写入服务端初始握手包（版本、connection_id、salt、能力位等）。
    pub fn writeInitialHandshake(&self) -> ConnResult<()> {
        if self.salt.len() < 20 {
            return Err(ConnError::Session(
                "authentication salt must contain at least 20 bytes".into(),
            ));
        }
        let config = self.server.config();
        let capability = config.capability;
        let mut packet = Vec::with_capacity(128);
        packet.push(10);
        packet.extend_from_slice(config.server_version.as_bytes());
        packet.push(0);
        packet
            .extend_from_slice(&(self.connection_id.load(Ordering::Acquire) as u32).to_le_bytes());
        packet.extend_from_slice(&self.salt[..8]);
        packet.push(0);
        packet.extend_from_slice(&(capability as u16).to_le_bytes());
        packet.push(self.collation.load(Ordering::Acquire) as u8);
        packet.extend_from_slice(&SERVER_STATUS_AUTOCOMMIT.to_le_bytes());
        packet.extend_from_slice(&((capability >> 16) as u16).to_le_bytes());
        packet.push((self.salt.len() + 1) as u8);
        packet.extend_from_slice(&[0; 10]);
        packet.extend_from_slice(&self.salt[8..]);
        packet.push(0);
        packet.extend_from_slice(config.default_auth_plugin.as_bytes());
        packet.push(0);
        self.writePacket(&packet)?;
        self.flush()
    }

    /// 读取可选 SSLRequest，必要时升级 TLS，再解析握手响应。
    pub fn readOptionalSSLRequestAndHandshakeResponse(&self) -> ConnResult<HandshakeResponse> {
        let mut packet = self.readPacket()?;
        if packet.len() < 4 {
            return Err(ConnError::MalformedPacket("missing capability flags"));
        }
        let initial_capability = u32::from_le_bytes(packet[..4].try_into().unwrap_or([0; 4]));
        if initial_capability & CLIENT_PROTOCOL_41 == 0 {
            return Err(ConnError::UnsupportedProtocol);
        }
        if initial_capability & CLIENT_SSL != 0 && packet.len() == 32 {
            let tls_state = self
                .packet
                .lock()
                .map_err(|_| ConnError::Poisoned("packet"))?
                .upgrade_to_tls()?;
            *self
                .tls_state
                .write()
                .map_err(|_| ConnError::Poisoned("tls_state"))? = Some(tls_state);
            packet = self.readPacket()?;
        }
        let response = parse_handshake_response(&packet)?;
        let config = self.server.config();
        if config.require_secure_transport && !self.unix_socket && self.getTLSState()?.is_none() {
            return Err(ConnError::SecureTransportRequired);
        }
        let negotiated = response.capability & config.capability;
        self.capability.store(negotiated, Ordering::Release);
        self.collation
            .store(response.collation as u32, Ordering::Release);
        *self.user.write().map_err(|_| ConnError::Poisoned("user"))? = response.user.clone();
        *self
            .database
            .write()
            .map_err(|_| ConnError::Poisoned("database"))? = response.database.clone();
        *self
            .attributes
            .write()
            .map_err(|_| ConnError::Poisoned("attributes"))? = response.attrs.clone();
        self.PeerHost(true)?;
        Ok(response)
    }

    /// 按用户配置的插件决定是否发起 AuthSwitch。
    pub fn handleAuthPlugin(&self, response: &mut HandshakeResponse) -> ConnResult<()> {
        let context = self.openSession()?;
        let (host, _) = self.PeerHost(false)?;
        if response.auth_plugin == AUTH_TIDB_SESSION_TOKEN {
            return Ok(());
        }
        let user = self
            .user
            .read()
            .map_err(|_| ConnError::Poisoned("user"))?
            .clone();
        let configured = context.auth_plugin_for_user(&user, &host)?;
        let configured = if configured.is_empty() {
            AUTH_NATIVE_PASSWORD.to_owned()
        } else {
            configured
        };
        if configured == AUTH_SOCKET && !self.unix_socket {
            return Err(ConnError::AccessDenied { user, host });
        }
        let advertised = self
            .auth_plugin
            .read()
            .map_err(|_| ConnError::Poisoned("auth_plugin"))?
            .clone();
        if advertised != configured || response.auth_plugin != configured {
            if response.capability & CLIENT_PLUGIN_AUTH == 0 {
                if configured != AUTH_NATIVE_PASSWORD {
                    return Err(ConnError::UnsupportedProtocol);
                }
                response.auth_plugin = AUTH_NATIVE_PASSWORD.to_owned();
            } else {
                response.auth = self.authSwitchRequest(&configured)?;
                response.auth_plugin = configured;
            }
        }
        Ok(())
    }

    /// 发送 AuthSwitchRequest 并读取客户端新的认证数据。
    pub fn authSwitchRequest(&self, plugin: &str) -> ConnResult<Vec<u8>> {
        let client_plugin = match plugin {
            AUTH_LDAP_SASL => format!("{plugin}_client"),
            AUTH_LDAP_SIMPLE => AUTH_CLEAR_PASSWORD.to_owned(),
            _ => plugin.to_owned(),
        };
        let mut packet = Vec::with_capacity(client_plugin.len() + self.salt.len() + 3);
        packet.push(AUTH_SWITCH_REQUEST);
        packet.extend_from_slice(client_plugin.as_bytes());
        packet.push(0);
        packet.extend_from_slice(&self.salt);
        packet.push(0);
        self.writePacket(&packet)?;
        self.flush()?;
        let auth = self.readPacket()?;
        *self
            .auth_plugin
            .write()
            .map_err(|_| ConnError::Poisoned("auth_plugin"))? = plugin.to_owned();
        Ok(auth)
    }

    /// caching_sha2 完整密码认证路径。
    pub fn authSha(&self, auth: Vec<u8>) -> ConnResult<Vec<u8>> {
        self.fullPasswordAuth(auth)
    }

    /// SM3 完整密码认证路径。
    pub fn authSM3(&self, auth: Vec<u8>) -> ConnResult<Vec<u8>> {
        self.fullPasswordAuth(auth)
    }

    /// 向客户端请求完整密码（0x01 0x04）并读取响应。
    fn fullPasswordAuth(&self, auth: Vec<u8>) -> ConnResult<Vec<u8>> {
        if auth.is_empty() {
            return Ok(Vec::new());
        }
        self.writePacket(&[1, 4])?;
        self.flush()?;
        let mut response = self.readPacket()?;
        while response.last() == Some(&0) {
            response.pop();
        }
        Ok(response)
    }

    /// 懒打开 Session，并检查最大连接数限制。
    pub fn openSession(&self) -> ConnResult<Arc<dyn TiDBContext>> {
        if let Some(context) = self.getCtx()? {
            return Ok(context);
        }
        let config = self.server.config();
        if config.max_connections > 0 && self.server.connection_count() >= config.max_connections {
            return Err(ConnError::Session("too many connections".into()));
        }
        let database = self
            .database
            .read()
            .map_err(|_| ConnError::Poisoned("database"))?
            .clone();
        let tls_state = self.getTLSState()?;
        let context = self.server.driver().open_ctx(
            self.connection_id.load(Ordering::Acquire),
            self.capability.load(Ordering::Acquire),
            self.collation.load(Ordering::Acquire) as u8,
            &database,
            tls_state.as_ref(),
        )?;
        self.SetCtx(Some(context.clone()))?;
        Ok(context)
    }

    /// 打开 Session、执行鉴权，并在指定库时 USE DB。
    pub fn openSessionAndDoAuth(&self, response: &HandshakeResponse) -> ConnResult<()> {
        let context = self.openSession()?;
        let (host, _) = self.PeerHost(false)?;
        let user = self
            .user
            .read()
            .map_err(|_| ConnError::Poisoned("user"))?
            .clone();
        if !self.unix_socket && response.auth_plugin == AUTH_SOCKET {
            return Err(ConnError::AccessDenied { user, host });
        }
        context.authenticate(&AuthRequest {
            identity: AuthIdentity {
                username: user,
                hostname: host,
                plugin: response.auth_plugin.clone(),
            },
            auth_data: response.auth.clone(),
            salt: self.salt.clone(),
            tls_state: self.getTLSState()?,
            attributes: response.attrs.clone(),
        })?;
        if !response.database.is_empty() {
            context.use_db(&response.database, &CancellationToken::new())?;
        }
        Ok(())
    }

    /// 解析并缓存对端 host/port；Unix socket 固定为 localhost。
    pub fn PeerHost(&self, update: bool) -> ConnResult<(String, String)> {
        let cached_host = self
            .peer_host
            .read()
            .map_err(|_| ConnError::Poisoned("peer_host"))?
            .clone();
        let cached_port = self
            .peer_port
            .read()
            .map_err(|_| ConnError::Poisoned("peer_port"))?
            .clone();
        if !cached_host.is_empty() && !update {
            return Ok((cached_host, cached_port));
        }
        if self.unix_socket {
            *self
                .peer_host
                .write()
                .map_err(|_| ConnError::Poisoned("peer_host"))? = "localhost".into();
            *self
                .server_host
                .write()
                .map_err(|_| ConnError::Poisoned("server_host"))? = "localhost".into();
            return Ok(("localhost".into(), String::new()));
        }
        let packet = self
            .packet
            .lock()
            .map_err(|_| ConnError::Poisoned("packet"))?;
        let (host, port) = packet.peer_addr()?;
        let (server_host, _) = packet.local_addr()?;
        drop(packet);
        *self
            .peer_host
            .write()
            .map_err(|_| ConnError::Poisoned("peer_host"))? = host.clone();
        *self
            .peer_port
            .write()
            .map_err(|_| ConnError::Poisoned("peer_port"))? = port.clone();
        *self
            .server_host
            .write()
            .map_err(|_| ConnError::Poisoned("server_host"))? = server_host;
        Ok((host, port))
    }

    /// 连接管理员账号跳过 init_connect。
    pub fn skipInitConnect(&self) -> ConnResult<bool> {
        Ok(self.openSession()?.is_connection_admin())
    }

    /// 握手成功后执行配置的 init_connect SQL。
    pub fn initConnect(&self) -> ConnResult<()> {
        let statement = self.server.config().init_connect;
        if statement.is_empty() || self.skipInitConnect()? {
            return Ok(());
        }
        self.openSession()?.execute_init_connect(&statement)
    }

    /// 运行连接主循环，退出时确保 Close。
    pub fn Run(&self) -> ConnResult<()> {
        let outcome = self.run_loop();
        let close_result = self.Close();
        match (outcome, close_result) {
            (Err(ConnError::ClientQuit), Ok(())) | (Ok(()), Ok(())) => Ok(()),
            (Err(error), _) => Err(error),
            (Ok(()), Err(error)) => Err(error),
        }
    }

    /// 读包 → 分发 → 重置序号的循环，直到关闭或退出。
    fn run_loop(&self) -> ConnResult<()> {
        loop {
            let context = self.openSession()?;
            if self.server.is_shutdown() && !context.in_transaction() {
                return Ok(());
            }
            if !self.CompareAndSwapStatus(connStatusDispatching, connStatusReading)
                || self.getStatus() == connStatusWaitShutdown
            {
                return Ok(());
            }
            self.packet
                .lock()
                .map_err(|_| ConnError::Poisoned("packet"))?
                .set_read_timeout(context.wait_timeout())?;
            let data = match self.readPacket() {
                Ok(data) => data,
                Err(error) => {
                    self.server.connection_closed(
                        self.connection_id.load(Ordering::Acquire),
                        &format!("read packet failed: {error}"),
                    );
                    return Err(error);
                }
            };
            if !self.CompareAndSwapStatus(connStatusReading, connStatusDispatching) {
                return Ok(());
            }
            if self.server.is_shutdown() && !context.in_transaction() {
                return Ok(());
            }
            let dispatch_result = self.dispatch(&data);
            if let Err(error) = &dispatch_result {
                match error {
                    ConnError::ClientQuit | ConnError::ResultUndetermined(_) => {
                        return dispatch_result;
                    }
                    _ => {
                        let started = Instant::now();
                        let write_result = self.writeError(error);
                        context.finish_protocol_response(started.elapsed());
                        write_result?;
                    }
                }
            }
            self.packet
                .lock()
                .map_err(|_| ConnError::Poisoned("packet"))?
                .reset_sequence();
        }
    }

    /// 按 COM_* 命令分发到查询、预处理语句或管理命令处理函数。
    pub fn dispatch(&self, data: &[u8]) -> ConnResult<()> {
        let (&opcode, payload) = data
            .split_first()
            .ok_or(ConnError::MalformedPacket("empty command"))?;
        *self
            .last_packet
            .lock()
            .map_err(|_| ConnError::Poisoned("last_packet"))? = data.to_vec();
        let command = Command::try_from(opcode)?;
        let context = self.openSession()?;
        let cancel = Arc::new(CancellationToken::new());
        *self
            .current_cancel
            .lock()
            .map_err(|_| ConnError::Poisoned("current_cancel"))? = Some(cancel.clone());
        let sql = String::from_utf8_lossy(payload)
            .trim_end_matches('\0')
            .to_owned();
        let process_info = if command == Command::InitDb {
            format!("use {sql}")
        } else if command == Command::Query {
            sql.clone()
        } else {
            String::new()
        };
        context.set_process_info(&process_info, opcode);

        let result = match command {
            Command::Quit => Err(ConnError::ClientQuit),
            Command::InitDb => self.useDB(&sql, &cancel).and_then(|_| self.writeOK()),
            Command::Query => self.handleQuery(&sql, &cancel),
            Command::FieldList => self.handleFieldList(payload),
            Command::Refresh => self.handleRefresh(payload, &cancel),
            Command::Shutdown => self.handleQuery("SHUTDOWN", &cancel),
            Command::Statistics => self.writeStats(),
            Command::Ping => {
                if self.server.is_healthy() {
                    self.writeOK()
                } else {
                    Err(ConnError::ServerShutdown)
                }
            }
            Command::ChangeUser => context
                .change_user(payload, &cancel)
                .and_then(|_| self.writeOK()),
            Command::ResetConnection => context
                .reset_connection(&cancel)
                .and_then(|_| self.writeOK()),
            Command::StmtPrepare
            | Command::StmtExecute
            | Command::StmtSendLongData
            | Command::StmtClose
            | Command::StmtReset
            | Command::StmtFetch => self.handleStmt(command, payload, &cancel),
            Command::SetOption => self.handleSetOption(payload),
            unsupported => Err(ConnError::UnsupportedCommand(unsupported as u8)),
        };

        context.clear_process_info();
        *self
            .current_cancel
            .lock()
            .map_err(|_| ConnError::Poisoned("current_cancel"))? = None;
        *self
            .last_active
            .lock()
            .map_err(|_| ConnError::Poisoned("last_active"))? = Instant::now();
        self.server
            .connection_active(self.connection_id.load(Ordering::Acquire));
        result
    }

    /// 执行文本协议查询并写回一个或多个结果集。
    pub fn handleQuery(&self, sql: &str, cancel: &CancellationToken) -> ConnResult<()> {
        let context = self.openSession()?;
        let results = context.execute_query(
            sql,
            self.capability.load(Ordering::Acquire) & CLIENT_MULTI_STATEMENTS != 0,
            cancel,
        )?;
        if results.is_empty() {
            return self.writeOK();
        }
        let result_count = results.len();
        for (index, mut result) in results.into_iter().enumerate() {
            if index + 1 < result_count {
                result.state.status |= SERVER_MORE_RESULTS_EXISTS;
            }
            let lifecycle = result.response_lifecycle.clone();
            let started = Instant::now();
            let write_result = self.handleStmtResult(result).and_then(|_| self.flush());
            if let Some(lifecycle) = lifecycle {
                lifecycle.add_write_duration(started.elapsed());
                lifecycle.finish();
            }
            write_result?;
        }
        Ok(())
    }

    /// 将预处理相关 COM_STMT_* / COM_SET_OPTION 交给 Session 执行并写结果。
    pub fn handleStmt(
        &self,
        command: Command,
        payload: &[u8],
        cancel: &CancellationToken,
    ) -> ConnResult<()> {
        let context = self.openSession()?;
        match command {
            Command::StmtPrepare => {
                let sql = std::str::from_utf8(payload)
                    .map_err(|_| ConnError::MalformedPacket("prepared SQL is not UTF-8"))?;
                let metadata = context.prepare_statement(sql, cancel)?;
                let statement = crate::conn_stmt::PreparedStatement {
                    id: metadata.statement_id,
                    sql: sql.to_owned(),
                    num_params: metadata.parameter_count,
                    columns: metadata
                        .columns
                        .iter()
                        .map(|column| crate::conn_stmt::ColumnInfo {
                            name: column.name.clone(),
                            column_type: column.column_type,
                        })
                        .collect(),
                    bound_params: vec![None; metadata.parameter_count],
                    params_type: Vec::new(),
                    last_params: Vec::new(),
                    cursor: None,
                    cursor_active: false,
                    protocol_cursor: None,
                };
                self.prepared_statements
                    .lock()
                    .map_err(|_| ConnError::Poisoned("prepared statements"))?
                    .insert(metadata.statement_id, statement);
                self.prepared_columns
                    .lock()
                    .map_err(|_| ConnError::Poisoned("prepared columns"))?
                    .insert(metadata.statement_id, metadata.columns.clone());
                self.write_prepare_response(&metadata)?;
                self.flush()
            }
            Command::StmtExecute => {
                let statement_id = read_u32_le(payload, 0)?;
                let (arguments, use_cursor) = {
                    let mut statements = self
                        .prepared_statements
                        .lock()
                        .map_err(|_| ConnError::Poisoned("prepared statements"))?;
                    let statement = statements.get_mut(&statement_id).ok_or_else(|| {
                        ConnError::Session(format!("prepared statement {statement_id} not found"))
                    })?;
                    crate::conn_stmt::ParseExecuteParams(statement, payload)
                        .map_err(|error| ConnError::Session(error.to_string()))?
                };
                let mut result =
                    context.execute_prepared_statement(statement_id, &arguments, cancel)?;
                if use_cursor && !result.columns.is_empty() {
                    result.state.status |= SERVER_STATUS_CURSOR_EXISTS;
                    let lifecycle = result.response_lifecycle.clone();
                    let started = Instant::now();
                    self.write_binary_result_metadata(&result)?;
                    self.flush()?;
                    if let Some(lifecycle) = lifecycle {
                        lifecycle.add_write_duration(started.elapsed());
                    }
                    let mut statements = self
                        .prepared_statements
                        .lock()
                        .map_err(|_| ConnError::Poisoned("prepared statements"))?;
                    let statement = statements.get_mut(&statement_id).ok_or_else(|| {
                        ConnError::Session(format!("prepared statement {statement_id} not found"))
                    })?;
                    statement.cursor_active = true;
                    statement.protocol_cursor = Some(result);
                    return Ok(());
                }
                let lifecycle = result.response_lifecycle.clone();
                let started = Instant::now();
                let write_result = self.write_binary_result(result).and_then(|_| self.flush());
                if let Some(lifecycle) = lifecycle {
                    lifecycle.add_write_duration(started.elapsed());
                    lifecycle.finish();
                }
                write_result
            }
            Command::StmtSendLongData => {
                let statement_id = read_u32_le(payload, 0)?;
                let parameter = usize::from(read_u16_le(payload, 4)?);
                let mut statements = self
                    .prepared_statements
                    .lock()
                    .map_err(|_| ConnError::Poisoned("prepared statements"))?;
                let statement = statements.get_mut(&statement_id).ok_or_else(|| {
                    ConnError::Session(format!("prepared statement {statement_id} not found"))
                })?;
                statement
                    .bound_params
                    .get_mut(parameter)
                    .ok_or(ConnError::MalformedPacket("long-data parameter index"))?
                    .get_or_insert_with(Vec::new)
                    .extend_from_slice(&payload[6..]);
                Ok(())
            }
            Command::StmtReset => {
                let statement_id = read_u32_le(payload, 0)?;
                self.prepared_statements
                    .lock()
                    .map_err(|_| ConnError::Poisoned("prepared statements"))?
                    .get_mut(&statement_id)
                    .ok_or_else(|| {
                        ConnError::Session(format!("prepared statement {statement_id} not found"))
                    })?
                    .reset()
                    .map_err(|error| ConnError::Session(error.to_string()))?;
                self.writeOK()
            }
            Command::StmtClose => {
                let statement_id = read_u32_le(payload, 0)?;
                self.prepared_statements
                    .lock()
                    .map_err(|_| ConnError::Poisoned("prepared statements"))?
                    .remove(&statement_id);
                self.prepared_columns
                    .lock()
                    .map_err(|_| ConnError::Poisoned("prepared columns"))?
                    .remove(&statement_id);
                let _ = context.close_prepared_statement(statement_id);
                Ok(())
            }
            Command::StmtFetch => self.write_prepared_cursor_fetch(payload),
            _ => Err(ConnError::UnsupportedCommand(command as u8)),
        }
    }

    fn write_prepare_response(&self, metadata: &PreparedMetadata) -> ConnResult<()> {
        let column_count = u16::try_from(metadata.columns.len())
            .map_err(|_| ConnError::Session("too many prepared result columns".to_owned()))?;
        let parameter_count = u16::try_from(metadata.parameter_count)
            .map_err(|_| ConnError::Session("too many prepared parameters".to_owned()))?;
        let mut packet = vec![0x00];
        packet.extend_from_slice(&metadata.statement_id.to_le_bytes());
        packet.extend_from_slice(&column_count.to_le_bytes());
        packet.extend_from_slice(&parameter_count.to_le_bytes());
        packet.push(0);
        packet.extend_from_slice(&0_u16.to_le_bytes());
        self.writePacket(&packet)?;
        if parameter_count > 0 {
            for index in 0..parameter_count {
                self.writeColumnInfo(&ColumnInfo {
                    name: format!("param_{}", index + 1),
                    org_name: String::new(),
                    charset: self.collation.load(Ordering::Acquire) as u16,
                    column_length: 1024,
                    column_type: 0xfd,
                    ..empty_column_info()
                })?;
            }
            self.writeEOF(self.openSession()?.state().status)?;
        }
        if column_count > 0 {
            for column in &metadata.columns {
                self.writeColumnInfo(column)?;
            }
            self.writeEOF(self.openSession()?.state().status)?;
        }
        Ok(())
    }

    fn write_binary_result(&self, result: QueryResult) -> ConnResult<()> {
        if result.columns.is_empty() {
            return self.writeOkWith(OK_HEADER, false, &result.state);
        }
        let mut count = Vec::new();
        put_lenenc_int(&mut count, result.columns.len() as u64);
        self.writePacket(&count)?;
        for column in &result.columns {
            self.writeColumnInfo(column)?;
        }
        if self.capability.load(Ordering::Acquire) & CLIENT_DEPRECATE_EOF == 0 {
            self.writeEOF(result.state.status)?;
        }
        for row in &result.rows {
            self.write_binary_row(&result.columns, row)?;
        }
        self.writeEOF(result.state.status)
    }

    fn write_binary_result_metadata(&self, result: &QueryResult) -> ConnResult<()> {
        let mut count = Vec::new();
        put_lenenc_int(&mut count, result.columns.len() as u64);
        self.writePacket(&count)?;
        for column in &result.columns {
            self.writeColumnInfo(column)?;
        }
        if self.capability.load(Ordering::Acquire) & CLIENT_DEPRECATE_EOF == 0 {
            self.writeEOF(result.state.status)?;
        }
        Ok(())
    }

    fn write_prepared_cursor_fetch(&self, payload: &[u8]) -> ConnResult<()> {
        if payload.len() != 8 {
            return Err(ConnError::MalformedPacket("statement fetch payload"));
        }
        let statement_id = read_u32_le(payload, 0)?;
        let fetch_size = usize::try_from(read_u32_le(payload, 4)?.min(1024)).unwrap_or(1024);
        let mut statements = self
            .prepared_statements
            .lock()
            .map_err(|_| ConnError::Poisoned("prepared statements"))?;
        let statement = statements.get_mut(&statement_id).ok_or_else(|| {
            ConnError::Session(format!("prepared statement {statement_id} not found"))
        })?;
        if !statement.cursor_active {
            return Err(ConnError::Session(
                "prepared cursor is not active".to_owned(),
            ));
        }
        let cursor = statement
            .protocol_cursor
            .as_mut()
            .ok_or_else(|| ConnError::Session("prepared cursor is not active".to_owned()))?;
        let lifecycle = cursor.response_lifecycle.clone();
        let started = Instant::now();
        let count = fetch_size.min(cursor.rows.len());
        for row in cursor.rows.drain(..count) {
            self.write_binary_row(&cursor.columns, &row)?;
        }
        let exhausted = cursor.rows.is_empty();
        let mut status = cursor.state.status;
        if exhausted {
            status &= !SERVER_STATUS_CURSOR_EXISTS;
            status |= SERVER_STATUS_LAST_ROW_SENT;
        }
        self.writeEOF(status)?;
        self.flush()?;
        if let Some(lifecycle) = lifecycle {
            lifecycle.add_write_duration(started.elapsed());
            if exhausted {
                lifecycle.finish();
            }
        }
        if exhausted {
            statement.cursor_active = false;
            statement.protocol_cursor = None;
        }
        Ok(())
    }

    fn write_binary_row(&self, columns: &[ColumnInfo], row: &[Value]) -> ConnResult<()> {
        let mut packet = vec![0_u8; 1 + (columns.len() + 9) / 8];
        for (index, (column, value)) in columns.iter().zip(row).enumerate() {
            let bytes = value.encode_text();
            if bytes.as_deref() == Some(b"<nil>") || bytes.is_none() {
                packet[1 + ((index + 2) >> 3)] |= 1 << ((index + 2) & 7);
                continue;
            }
            encode_binary_column(&mut packet, column, &bytes.expect("non-NULL value"))?;
        }
        self.writePacket(&packet)
    }

    /// COM_SET_OPTION：按 Go/MySQL 语义动态开关 CLIENT_MULTI_STATEMENTS，并以 EOF 应答。
    pub fn handleSetOption(&self, payload: &[u8]) -> ConnResult<()> {
        let option = payload
            .get(..2)
            .ok_or(ConnError::MalformedPacket("set option value missing"))?;
        let option = u16::from_le_bytes([option[0], option[1]]);
        match option {
            0 => {
                self.capability
                    .fetch_or(CLIENT_MULTI_STATEMENTS, Ordering::AcqRel);
            }
            1 => {
                self.capability
                    .fetch_and(!CLIENT_MULTI_STATEMENTS, Ordering::AcqRel);
            }
            _ => return Err(ConnError::MalformedPacket("invalid set option value")),
        }
        let status = self.openSession()?.state().status;
        self.writeEOF(status)?;
        self.flush()
    }

    /// 无列则写 OK，否则写完整结果集。
    fn handleStmtResult(&self, result: QueryResult) -> ConnResult<()> {
        if result.columns.is_empty() {
            self.writeOkWith(OK_HEADER, false, &result.state)
        } else {
            self.writeResultSet(&result)
        }
    }

    /// 切换当前数据库并更新本地缓存名。
    pub fn useDB(&self, database: &str, cancel: &CancellationToken) -> ConnResult<()> {
        self.openSession()?.use_db(database, cancel)?;
        *self
            .database
            .write()
            .map_err(|_| ConnError::Poisoned("database"))? = database.to_owned();
        Ok(())
    }

    /// COM_FIELD_LIST：返回表字段元数据并以 EOF 结束。
    pub fn handleFieldList(&self, payload: &[u8]) -> ConnResult<()> {
        let split = payload
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(payload.len());
        let table = String::from_utf8_lossy(&payload[..split]);
        let wildcard = if split < payload.len() {
            String::from_utf8_lossy(&payload[split + 1..])
        } else {
            "".into()
        };
        for column in self.openSession()?.field_list(&table, &wildcard)? {
            self.writeFieldListColumnInfo(&column)?;
        }
        let status = self.openSession()?.state().status;
        self.writeEOF(status)?;
        self.flush()
    }

    /// COM_REFRESH：执行刷新标志对应的命令。
    pub fn handleRefresh(&self, payload: &[u8], cancel: &CancellationToken) -> ConnResult<()> {
        let flag = *payload
            .first()
            .ok_or(ConnError::MalformedPacket("refresh flag missing"))?;
        self.openSession()?
            .execute_command(Command::Refresh, &[flag], cancel)?;
        self.writeOK()
    }

    /// COM_STATISTICS：返回简化的运行统计文本。
    pub fn writeStats(&self) -> ConnResult<()> {
        let uptime = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let message = format!(
            "Uptime: {uptime}  Threads: {}  Questions: 0  Slow queries: 0  Opens: 0  Flush tables: 0  Open tables: 0  Queries per second avg: 0.000",
            self.server.connection_count()
        );
        self.writePacket(message.as_bytes())?;
        self.flush()
    }

    /// 写入并刷新 OK 包。
    pub fn writeOK(&self) -> ConnResult<()> {
        let state = self.openSession()?.state();
        self.writeOkWith(OK_HEADER, true, &state)
    }

    /// 按会话状态组装 OK/EOF 风格包，可选 flush。
    pub fn writeOkWith(&self, header: u8, flush: bool, state: &SessionState) -> ConnResult<()> {
        let capability = self.capability.load(Ordering::Acquire);
        let mut packet = vec![header];
        put_lenenc_int(&mut packet, state.affected_rows);
        put_lenenc_int(&mut packet, state.last_insert_id);
        if capability & CLIENT_PROTOCOL_41 != 0 {
            packet.extend_from_slice(&state.status.to_le_bytes());
            packet.extend_from_slice(&state.warning_count.to_le_bytes());
        }
        if capability & CLIENT_SESSION_TRACK != 0 {
            put_lenenc_bytes(&mut packet, state.last_message.as_bytes());
            if state.status & SERVER_SESSION_STATE_CHANGED != 0 {
                put_lenenc_bytes(&mut packet, &[]);
            }
        } else if !state.last_message.is_empty() {
            packet.extend_from_slice(state.last_message.as_bytes());
        }
        self.writePacket(&packet)?;
        if flush { self.flush() } else { Ok(()) }
    }

    /// 将 ConnError 映射为 MySQL ERR 包并写出。
    pub fn writeError(&self, error: &ConnError) -> ConnResult<()> {
        let (code, sql_state) = mysql_error_code_and_state(error);
        self.last_code.store(code as u32, Ordering::Release);
        let mut packet = vec![ERR_HEADER];
        packet.extend_from_slice(&code.to_le_bytes());
        if self.capability.load(Ordering::Acquire) & CLIENT_PROTOCOL_41 != 0 {
            packet.push(b'#');
            packet.extend_from_slice(sql_state);
        }
        packet.extend_from_slice(error.to_string().as_bytes());
        self.writePacket(&packet)?;
        self.flush()
    }

    /// 写入 EOF；若客户端废弃 EOF 则改写 OK 形态。
    pub fn writeEOF(&self, server_status: u16) -> ConnResult<()> {
        if self.capability.load(Ordering::Acquire) & CLIENT_DEPRECATE_EOF != 0 {
            let mut state = self.openSession()?.state();
            state.status = server_status;
            return self.writeOkWith(EOF_HEADER, false, &state);
        }
        let mut packet = vec![EOF_HEADER];
        if self.capability.load(Ordering::Acquire) & CLIENT_PROTOCOL_41 != 0 {
            packet.extend_from_slice(&self.openSession()?.state().warning_count.to_le_bytes());
            packet.extend_from_slice(&server_status.to_le_bytes());
        }
        self.writePacket(&packet)
    }

    /// 请求客户端通过 LOCAL INFILE 发送指定路径文件。
    pub fn writeReq(&self, file_path: &str) -> ConnResult<()> {
        let mut packet = vec![LOCAL_IN_FILE_HEADER];
        packet.extend_from_slice(file_path.as_bytes());
        self.writePacket(&packet)?;
        self.flush()
    }

    /// 发起 LOCAL INFILE 请求并拼接客户端回传的数据块直至空包。
    pub fn getDataFromPath(&self, file_path: &str) -> ConnResult<Vec<u8>> {
        self.writeReq(file_path)?;
        let mut all = Vec::new();
        loop {
            let packet = self.readPacket()?;
            if packet.is_empty() {
                return Ok(all);
            }
            all.extend_from_slice(&packet);
        }
    }

    /// 写列数、列定义、EOF、行数据与结束 EOF。
    pub fn writeResultSet(&self, result: &QueryResult) -> ConnResult<()> {
        let mut count = Vec::new();
        put_lenenc_int(&mut count, result.columns.len() as u64);
        self.writePacket(&count)?;
        for column in &result.columns {
            self.writeColumnInfo(column)?;
        }
        // CLIENT_DEPRECATE_EOF removes the metadata terminator entirely; the
        // next packet must be the first row (or the final OK packet). Sending
        // an OK-shaped 0xfe packet here makes current MySQL clients treat an
        // otherwise valid result set as empty.
        if self.capability.load(Ordering::Acquire) & CLIENT_DEPRECATE_EOF == 0 {
            self.writeEOF(result.state.status)?;
        }
        self.writeChunks(&result.rows)?;
        self.writeEOF(result.state.status)
    }

    /// 按文本协议编码单列定义包。
    pub fn writeColumnInfo(&self, column: &ColumnInfo) -> ConnResult<()> {
        self.write_column_info(column, false)
    }

    /// Write one COM_FIELD_LIST column definition including its default suffix.
    fn writeFieldListColumnInfo(&self, column: &ColumnInfo) -> ConnResult<()> {
        self.write_column_info(column, true)
    }

    fn write_column_info(&self, column: &ColumnInfo, with_default: bool) -> ConnResult<()> {
        let mut packet = Vec::new();
        for value in [
            "def",
            column.schema.as_str(),
            column.table.as_str(),
            column.org_table.as_str(),
            column.name.as_str(),
            column.org_name.as_str(),
        ] {
            put_lenenc_bytes(&mut packet, value.as_bytes());
        }
        packet.push(0x0c);
        packet.extend_from_slice(&column.charset.to_le_bytes());
        packet.extend_from_slice(&column.column_length.to_le_bytes());
        packet.push(column.column_type);
        packet.extend_from_slice(&column.flags.to_le_bytes());
        packet.push(column.decimals);
        packet.extend_from_slice(&[0, 0]);
        if with_default {
            if let Some(default_value) = column.default_value.as_ref() {
                put_lenenc_bytes(&mut packet, default_value);
            } else {
                packet.push(0xfb);
            }
        }
        self.writePacket(&packet)
    }

    /// 将结果行按文本协议逐行写出。
    pub fn writeChunks(&self, rows: &[Vec<Value>]) -> ConnResult<()> {
        for row in rows {
            let mut packet = Vec::new();
            for value in row {
                match value.encode_text() {
                    Some(bytes) => put_lenenc_bytes(&mut packet, &bytes),
                    None => packet.push(0xfb),
                }
            }
            self.writePacket(&packet)?;
        }
        Ok(())
    }

    /// 安装当前分发用的取消令牌并返回共享引用。
    pub fn installConnAliveInfo(&self) -> ConnResult<Arc<CancellationToken>> {
        let cancel = Arc::new(CancellationToken::new());
        *self
            .current_cancel
            .lock()
            .map_err(|_| ConnError::Poisoned("current_cancel"))? = Some(cancel.clone());
        Ok(cancel)
    }

    /// 取消当前命令并通知 Session 取消。
    pub fn cancelDispatch(&self) {
        if let Ok(guard) = self.current_cancel.lock() {
            if let Some(cancel) = guard.as_ref() {
                cancel.cancel();
            }
        }
        if let Ok(Some(context)) = self.getCtx() {
            context.cancel();
        }
    }

    /// 连接未关闭且底层 socket 仍存活。
    pub fn connectionAlive(&self) -> bool {
        !self.closed.load(Ordering::Acquire)
            && self
                .packet
                .lock()
                .map(|packet| packet.connection_alive())
                .unwrap_or(false)
    }

    /// 将事务中/自动提交状态格式化为诊断字符串。
    pub fn SessionStatusToString(&self) -> ConnResult<String> {
        let status = self.openSession()?.state().status;
        Ok(format!(
            "inTxn:{}, autocommit:{}",
            u8::from(status & SERVER_STATUS_IN_TRANS != 0),
            u8::from(status & SERVER_STATUS_AUTOCOMMIT != 0)
        ))
    }

    /// 当前资源组名，缺省为 `default`。
    pub fn currentResourceGroupName(&self) -> String {
        self.getCtx()
            .ok()
            .flatten()
            .map(|context| context.state().resource_group)
            .filter(|group| !group.is_empty())
            .unwrap_or_else(|| "default".into())
    }

    /// 汇总压缩启用状态与算法，供 SHOW STATUS。
    pub fn compressionStats(&self) -> BTreeMap<&'static str, String> {
        let capability = self.capability.load(Ordering::Acquire);
        let algorithm = if capability & CLIENT_COMPRESS != 0 {
            CompressionAlgorithm::Zlib
        } else if capability & CLIENT_ZSTD_COMPRESSION_ALGORITHM != 0 {
            CompressionAlgorithm::Zstd
        } else {
            CompressionAlgorithm::None
        };
        let mut stats = BTreeMap::new();
        stats.insert(
            statusCompression,
            (algorithm != CompressionAlgorithm::None).to_string(),
        );
        stats.insert(statusCompressionAlgorithm, format!("{algorithm:?}"));
        stats
    }

    /// 返回 Session 记录的最近一条语句文本。
    pub fn getLastStmtInConn(&self) -> String {
        self.getCtx()
            .ok()
            .flatten()
            .map(|ctx| ctx.last_statement())
            .unwrap_or_default()
    }

    /// 返回稳定的连接 ID 快照，供 Server 登记和 worker 跟踪。
    pub fn connection_id(&self) -> u64 {
        self.connection_id.load(Ordering::Acquire)
    }

    /// 返回握手协商后的 capability。
    pub fn negotiated_capability(&self) -> u32 {
        self.capability.load(Ordering::Acquire)
    }

    /// 返回当前用户、数据库与连接属性的只读快照。
    pub fn identity_snapshot(&self) -> (String, String, BTreeMap<String, String>) {
        let user = self
            .user
            .read()
            .map(|value| value.clone())
            .unwrap_or_default();
        let database = self
            .database
            .read()
            .map(|value| value.clone())
            .unwrap_or_default();
        let attributes = self
            .attributes
            .read()
            .map(|value| value.clone())
            .unwrap_or_default();
        (user, database, attributes)
    }

    /// 返回进程列表展示所需的远端 host/port 快照。
    pub fn peer_address_snapshot(&self) -> (String, String) {
        let host = self
            .peer_host
            .read()
            .map(|value| value.clone())
            .unwrap_or_default();
        let port = self
            .peer_port
            .read()
            .map(|value| value.clone())
            .unwrap_or_default();
        (host, port)
    }

    /// 加锁读取一个 MySQL 包。
    pub fn readPacket(&self) -> ConnResult<Vec<u8>> {
        self.packet
            .lock()
            .map_err(|_| ConnError::Poisoned("packet"))?
            .read_packet()
    }

    /// 加锁写出一个 MySQL 包。
    pub fn writePacket(&self, data: &[u8]) -> ConnResult<()> {
        self.packet
            .lock()
            .map_err(|_| ConnError::Poisoned("packet"))?
            .write_packet(data)
    }

    /// 刷新写缓冲。
    pub fn flush(&self) -> ConnResult<()> {
        self.packet
            .lock()
            .map_err(|_| ConnError::Poisoned("packet"))?
            .flush()
    }

    /// 幂等关闭：注销连接、释放 ID、关闭 packet 与 Session。
    pub fn Close(&self) -> ConnResult<()> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        // Match Go clientConn.Close: remove the connection from the server's
        // live-client registry before transport and session cleanup, which may
        // block. Performance Schema and graceful shutdown must stop observing
        // the connection as soon as Close begins.
        let connection_id = self.connection_id.swap(0, Ordering::AcqRel);
        if self.registered.swap(false, Ordering::AcqRel) {
            self.server.unregister_connection(connection_id);
        }
        // Close the transport before session cancellation/cleanup: those paths
        // may block, but protocol errors must be observable by the peer at once.
        if let Some(close) = &self.packet_close {
            close();
        }
        self.setStatus(connStatusShutdown);
        self.cancelDispatch();
        self.server.domain().release_connection_id(connection_id);

        let packet_result = self
            .packet
            .lock()
            .map_err(|_| ConnError::Poisoned("packet"))?
            .close();
        let session_result = match self.getCtx()? {
            Some(context) => context.close(),
            None => Ok(()),
        };
        self.SetCtx(None)?;
        packet_result.and(session_result)
    }

    /// 与 Close 相同的关闭入口（保留 Go 命名）。
    pub fn closeWithoutLock(&self) -> ConnResult<()> {
        self.Close()
    }
}

/// 包级关闭入口，委托 `ClientConn::Close`。
pub fn closeConn(connection: &ClientConn) -> ConnResult<()> {
    connection.Close()
}

/// 解析客户端 HandshakeResponse41 及可选连接属性/zstd 级别。
pub(crate) fn parse_handshake_response(data: &[u8]) -> ConnResult<HandshakeResponse> {
    if data.len() < 32 {
        return Err(ConnError::MalformedPacket(
            "response header shorter than 32 bytes",
        ));
    }
    let capability = u32::from_le_bytes(data[0..4].try_into().unwrap_or([0; 4]));
    if capability & CLIENT_PROTOCOL_41 == 0 {
        return Err(ConnError::UnsupportedProtocol);
    }
    let collation = data[8];
    let mut position = 32;
    let user = read_nul_string(data, &mut position)?;
    let auth = if capability & CLIENT_PLUGIN_AUTH_LENENC_CLIENT_DATA != 0 {
        let length = read_lenenc_int(data, &mut position)? as usize;
        take_bytes(data, &mut position, length)?.to_vec()
    } else if capability & (1 << 15) != 0 {
        let length = *data
            .get(position)
            .ok_or(ConnError::MalformedPacket("auth length missing"))?
            as usize;
        position += 1;
        take_bytes(data, &mut position, length)?.to_vec()
    } else {
        read_nul_bytes(data, &mut position)?.to_vec()
    };
    let database = if capability & CLIENT_CONNECT_WITH_DB != 0 {
        read_nul_string(data, &mut position)?
    } else {
        String::new()
    };
    let auth_plugin = if capability & CLIENT_PLUGIN_AUTH != 0 && position < data.len() {
        read_nul_string(data, &mut position)?
    } else {
        AUTH_NATIVE_PASSWORD.to_owned()
    };
    let mut attrs = BTreeMap::new();
    if capability & CLIENT_CONNECT_ATTRS != 0 && position < data.len() {
        let total = read_lenenc_int(data, &mut position)?;
        if total > MAX_CONNECTION_ATTRIBUTES_SIZE {
            return Err(ConnError::Session(
                "connection refused: session connection attributes exceed the 1 MiB hard limit"
                    .to_owned(),
            ));
        }
        let total = total as usize;
        let end = position
            .checked_add(total)
            .filter(|end| *end <= data.len())
            .ok_or(ConnError::MalformedPacket("connection attributes overflow"))?;
        while position < end {
            let key_len = read_lenenc_int(data, &mut position)? as usize;
            let key =
                String::from_utf8_lossy(take_bytes(data, &mut position, key_len)?).into_owned();
            let value_len = read_lenenc_int(data, &mut position)? as usize;
            let value =
                String::from_utf8_lossy(take_bytes(data, &mut position, value_len)?).into_owned();
            attrs.insert(key, value);
        }
    }
    let zstd_level = if capability & CLIENT_ZSTD_COMPRESSION_ALGORITHM != 0 && position < data.len()
    {
        data[position] as i32
    } else {
        0
    };
    Ok(HandshakeResponse {
        capability,
        collation,
        user,
        database,
        auth,
        auth_plugin,
        attrs,
        zstd_level,
    })
}

/// 读取 NUL 结尾字符串并推进游标。
fn read_nul_string(data: &[u8], position: &mut usize) -> ConnResult<String> {
    Ok(String::from_utf8_lossy(read_nul_bytes(data, position)?).into_owned())
}

/// 读取 NUL 结尾字节切片并推进游标。
fn read_nul_bytes<'a>(data: &'a [u8], position: &mut usize) -> ConnResult<&'a [u8]> {
    let rest = data
        .get(*position..)
        .ok_or(ConnError::MalformedPacket("field starts outside packet"))?;
    let length = rest
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(ConnError::MalformedPacket("unterminated string"))?;
    let value = &rest[..length];
    *position += length + 1;
    Ok(value)
}

/// 按固定长度切片并推进游标。
fn take_bytes<'a>(data: &'a [u8], position: &mut usize, length: usize) -> ConnResult<&'a [u8]> {
    let end = position
        .checked_add(length)
        .ok_or(ConnError::MalformedPacket("field length overflow"))?;
    let value = data
        .get(*position..end)
        .ok_or(ConnError::MalformedPacket("field exceeds packet"))?;
    *position = end;
    Ok(value)
}

/// 解析 MySQL length-encoded integer。
fn read_lenenc_int(data: &[u8], position: &mut usize) -> ConnResult<u64> {
    let first = *data
        .get(*position)
        .ok_or(ConnError::MalformedPacket("length-encoded integer missing"))?;
    *position += 1;
    match first {
        0xfc => {
            let bytes = take_bytes(data, position, 2)?;
            Ok(u16::from_le_bytes(bytes.try_into().unwrap_or([0; 2])) as u64)
        }
        0xfd => {
            let bytes = take_bytes(data, position, 3)?;
            Ok(bytes[0] as u64 | (bytes[1] as u64) << 8 | (bytes[2] as u64) << 16)
        }
        0xfe => {
            let bytes = take_bytes(data, position, 8)?;
            Ok(u64::from_le_bytes(bytes.try_into().unwrap_or([0; 8])))
        }
        0xfb => Err(ConnError::MalformedPacket("NULL is not a valid length")),
        value => Ok(value as u64),
    }
}

/// 将 u64 编码为 length-encoded integer。
pub(crate) fn put_lenenc_int(output: &mut Vec<u8>, value: u64) {
    match value {
        0..=250 => output.push(value as u8),
        251..=65_535 => {
            output.push(0xfc);
            output.extend_from_slice(&(value as u16).to_le_bytes());
        }
        65_536..=16_777_215 => {
            output.push(0xfd);
            let bytes = value.to_le_bytes();
            output.extend_from_slice(&bytes[..3]);
        }
        _ => {
            output.push(0xfe);
            output.extend_from_slice(&value.to_le_bytes());
        }
    }
}

/// 先写长度再写字节内容。
fn put_lenenc_bytes(output: &mut Vec<u8>, value: &[u8]) {
    put_lenenc_int(output, value.len() as u64);
    output.extend_from_slice(value);
}
