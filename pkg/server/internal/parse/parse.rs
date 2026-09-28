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

// MySQL 协议包解析：握手响应体与 COM_STMT_FETCH。
//
// 按客户端 capability 标志解析 Response41 可变体（用户名、认证数据、库名、
// 认证插件、连接属性、zstd 级别）；连接属性受大小限制与指标策略约束，
// 畸形属性行仅告警不中断握手（对齐 Go recover 语义）。

use std::collections::HashMap;
use std::sync::Mutex;

use super::parser::mysql::{
    ClientConnectAtts, ClientConnectWithDB, ClientPluginAuth, ClientPluginAuthLenencClientData,
    ClientSecureConnection, ClientZstdCompressionAlgorithm,
};
pub use super::server::internal::handshake::Response41;
use super::server::internal::util::{ParseLengthEncodedBytes, ParseLengthEncodedInt};
use super::sessionctx::vardef::{ConnectAttrsLongestSeen, ConnectAttrsLost, ConnectAttrsSize};

/// COM_STMT_FETCH 请求行数上限（与 Go 一致）。
const MAX_FETCH_SIZE: u32 = 1024;
/// 连接属性硬上限 1 MiB；超过则拒绝连接。
const MAX_CONNECTION_ATTRIBUTES_SIZE: u64 = 1 << 20;
/// 正常连接属性观测上限 64 KiB（limit<0 时的有效上限）。
const MAX_NORMAL_CONNECTION_ATTRIBUTES_SIZE: i64 = 65_536;
/// 截断时写入的保留属性键，值为丢弃字节数。
const RESERVED_CONN_ATTR_TRUNCATED: &[u8] = b"_truncated";
/// 自定义以下划线开头的连接属性弃用警告文案。
const DEPRECATED_UNDERSCORE_WARNING: &str = "custom connection attributes with leading underscore are deprecated and will be rejected in a future release";

/// 保护 ConnectAttrsLost / LongestSeen 指标的并发更新。
static CONNECT_ATTRS_METRICS_LOCK: Mutex<()> = Mutex::new(());

/// 包解析错误：畸形包或连接属性超过硬上限。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseError {
    /// 包长度或字段边界不合法。
    MalformedPacket,
    /// 连接属性超过 1 MiB 硬限制。
    ConnectionAttributesTooLarge,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedPacket => formatter.write_str("malform packet error"),
            Self::ConnectionAttributesTooLarge => formatter.write_str(
                "connection refused: session connection attributes exceed the 1 MiB hard limit",
            ),
        }
    }
}

impl std::error::Error for ParseError {}

/// Parses COM_STMT_FETCH and caps the requested row count like the Go server.
/// 解析 COM_STMT_FETCH：statement_id + fetch_size，并将行数上限封顶为 MAX_FETCH_SIZE。
pub fn stmt_fetch_cmd(data: &[u8]) -> Result<(u32, u32), ParseError> {
    if data.len() != 8 {
        return Err(ParseError::MalformedPacket);
    }
    let statement_id = u32::from_le_bytes(data[0..4].try_into().unwrap());
    let fetch_size = u32::from_le_bytes(data[4..8].try_into().unwrap()).min(MAX_FETCH_SIZE);
    Ok((statement_id, fetch_size))
}

/// Parses the fixed Protocol::SSLRequest/Response41 prefix.
/// 解析固定 32 字节 SSLRequest/Response41 前缀：capability 与 collation。
pub fn handshake_response_header<C: ?Sized>(
    _ctx: &C,
    packet: &mut Response41,
    data: &[u8],
) -> Result<usize, ParseError> {
    if data.len() < 32 {
        log::warn!("got malformed handshake response: packetData={data:02x?}");
        return Err(ParseError::MalformedPacket);
    }

    packet.capability = u32::from_le_bytes(data[0..4].try_into().unwrap());
    // 字节 8 为字符集/排序规则编号；中间 max_packet_size 等字段此处不读入结构。
    packet.collation = data[8];
    Ok(32)
}

/// Parses the variable Response41 body according to the negotiated capabilities.
/// Every indexing path is checked explicitly, matching the Go recover guard without
/// allowing malformed client input to panic the Rust server.
/// 按协商 capability 解析 Response41 可变体；所有下标访问显式检查，避免畸形输入 panic。
pub fn handshake_response_body<C: ?Sized>(
    _ctx: &C,
    packet: &mut Response41,
    data: &[u8],
    mut offset: usize,
) -> Result<(), ParseError> {
    packet.user = read_nul_terminated(data, &mut offset)?.to_vec();

    // 认证数据有三种形态：长度编码、1 字节长度前缀、或 NUL 结尾。
    if packet.capability & ClientPluginAuthLenencClientData != 0 {
        let first = *data.get(offset).ok_or(ParseError::MalformedPacket)?;
        if first == 1 {
            // MySQL 5.7 can set the lenenc capability while sending the
            // historical two-byte "no auth data" representation.
            // MySQL 5.7 可能声明 lenenc 能力却仍发历史两字节“无认证数据”。
            offset = offset.checked_add(2).ok_or(ParseError::MalformedPacket)?;
        } else {
            let (auth_length, is_null) = read_lenenc_int(data, &mut offset)?;
            if !is_null {
                let auth_length =
                    usize::try_from(auth_length).map_err(|_| ParseError::MalformedPacket)?;
                packet.auth = take_bytes(data, &mut offset, auth_length)?.to_vec();
            }
        }
    } else if packet.capability & ClientSecureConnection != 0 {
        let auth_length = *data.get(offset).ok_or(ParseError::MalformedPacket)? as usize;
        offset += 1;
        packet.auth = take_bytes(data, &mut offset, auth_length)?.to_vec();
    } else {
        packet.auth = read_nul_terminated(data, &mut offset)?.to_vec();
    }

    if packet.capability & ClientConnectWithDB != 0 {
        let remaining = data.get(offset..).ok_or(ParseError::MalformedPacket)?.len();
        if remaining > 0 {
            packet.db_name = read_nul_terminated(data, &mut offset)?.to_vec();
        }
    }

    if packet.capability & ClientPluginAuth != 0 {
        let rest = data.get(offset..).ok_or(ParseError::MalformedPacket)?;
        if let Some(length) = rest.iter().position(|byte| *byte == 0) {
            if length > 0 {
                packet.auth_plugin = rest[..length].to_vec();
            }
            offset += length + 1;
        }
    }

    if packet.capability & ClientConnectAtts != 0 {
        if data
            .get(offset..)
            .ok_or(ParseError::MalformedPacket)?
            .is_empty()
        {
            return Ok(());
        }

        let (attributes_length, is_null) = read_lenenc_int(data, &mut offset)?;
        if !is_null {
            if attributes_length > MAX_CONNECTION_ATTRIBUTES_SIZE {
                return Err(ParseError::ConnectionAttributesTooLarge);
            }
            let attributes_length =
                usize::try_from(attributes_length).map_err(|_| ParseError::MalformedPacket)?;
            let row = take_bytes(data, &mut offset, attributes_length)?;
            match parse_attrs(row) {
                Ok((attributes, warning)) => {
                    if !warning.is_empty() {
                        log::debug!("{warning}");
                    }
                    packet.attrs = attributes;
                }
                Err(error) => {
                    // Connection attributes are optional metadata. Go logs a
                    // malformed row and continues the handshake successfully.
                    // 连接属性为可选元数据；畸形行仅告警并继续握手（对齐 Go）。
                    log::warn!("parse attrs failed: {error}");
                    return Ok(());
                }
            }
        }
    }

    if packet.capability & ClientZstdCompressionAlgorithm != 0 {
        packet.zstd_level = *data.get(offset).ok_or(ParseError::MalformedPacket)? as isize;
    }

    Ok(())
}

/// 读取到 NUL 为止的字节切片，并推进 offset 越过终止符。
fn read_nul_terminated<'a>(data: &'a [u8], offset: &mut usize) -> Result<&'a [u8], ParseError> {
    let rest = data.get(*offset..).ok_or(ParseError::MalformedPacket)?;
    let length = rest
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(ParseError::MalformedPacket)?;
    *offset = offset
        .checked_add(length + 1)
        .ok_or(ParseError::MalformedPacket)?;
    Ok(&rest[..length])
}

/// 从当前 offset 取出固定长度字节并推进。
fn take_bytes<'a>(
    data: &'a [u8],
    offset: &mut usize,
    length: usize,
) -> Result<&'a [u8], ParseError> {
    let end = offset
        .checked_add(length)
        .ok_or(ParseError::MalformedPacket)?;
    let value = data.get(*offset..end).ok_or(ParseError::MalformedPacket)?;
    *offset = end;
    Ok(value)
}

/// 解析长度编码整数并推进 offset。
fn read_lenenc_int(data: &[u8], offset: &mut usize) -> Result<(u64, bool), ParseError> {
    let input = data.get(*offset..).ok_or(ParseError::MalformedPacket)?;
    let (value, is_null, consumed, error) = ParseLengthEncodedInt(input);
    if error.is_some() {
        return Err(ParseError::MalformedPacket);
    }
    *offset = offset
        .checked_add(consumed)
        .ok_or(ParseError::MalformedPacket)?;
    Ok((value, is_null))
}

/// 单条连接属性键值及其原始字节占用。
#[derive(Clone)]
struct ConnAttrKv {
    key: Vec<u8>,
    value: Vec<u8>,
    byte_size: i64,
}

/// 解码后的连接属性集合及累计大小、弃用下划线标记。
struct DecodedConnAttrs {
    items: Vec<ConnAttrKv>,
    total_size: i64,
    has_deprecated_underscore_attr: bool,
}

/// Package-visible for the package-local Rust unit tests, like Go's parseAttrs.
/// 包内可见的连接属性解析入口（对齐 Go parseAttrs），供单元测试直接调用。
pub(crate) fn parse_attrs(data: &[u8]) -> Result<(HashMap<Vec<u8>, Vec<u8>>, String), ParseError> {
    let limit = ConnectAttrsSize.Load();
    // limit==0 表示禁用收集连接属性。
    if limit == 0 {
        return Ok((HashMap::new(), String::new()));
    }

    let decoded = decode_conn_attrs(data)?;
    Ok(apply_conn_attrs_policy_and_metrics(decoded, limit))
}

/// 按长度编码键值对顺序解码连接属性行。
fn decode_conn_attrs(data: &[u8]) -> Result<DecodedConnAttrs, ParseError> {
    let mut position = 0;
    let mut items = Vec::new();
    let mut total_size = 0_i64;
    let mut has_deprecated_underscore_attr = false;

    while position < data.len() {
        let (key, key_size) = read_lenenc_bytes(&data[position..])?;
        position = position
            .checked_add(key_size)
            .ok_or(ParseError::MalformedPacket)?;

        let (value, value_size) =
            read_lenenc_bytes(data.get(position..).ok_or(ParseError::MalformedPacket)?)?;
        position = position
            .checked_add(value_size)
            .ok_or(ParseError::MalformedPacket)?;

        let byte_size = i64::try_from(key.len().saturating_add(value.len()))
            .map_err(|_| ParseError::MalformedPacket)?;
        total_size = total_size
            .checked_add(byte_size)
            .ok_or(ParseError::MalformedPacket)?;

        // 非标准的 `_` 前缀键视为弃用自定义属性。
        if !has_deprecated_underscore_attr
            && key.starts_with(b"_")
            && !is_standard_connection_attribute(&key)
        {
            has_deprecated_underscore_attr = true;
        }

        items.push(ConnAttrKv {
            key,
            value,
            byte_size,
        });
    }

    Ok(DecodedConnAttrs {
        items,
        total_size,
        has_deprecated_underscore_attr,
    })
}

/// 解析一个长度编码字节串，返回值与消耗字节数。
fn read_lenenc_bytes(data: &[u8]) -> Result<(Vec<u8>, usize), ParseError> {
    let (value, _is_null, consumed, error) = ParseLengthEncodedBytes(data);
    if error.is_some() {
        return Err(ParseError::MalformedPacket);
    }
    Ok((value.unwrap_or_default().to_vec(), consumed))
}

/// 按大小限制截断属性、更新 Lost/LongestSeen 指标并组装警告。
fn apply_conn_attrs_policy_and_metrics(
    decoded: DecodedConnAttrs,
    limit: i64,
) -> (HashMap<Vec<u8>, Vec<u8>>, String) {
    let effective_limit = normalize_connect_attrs_limit(limit);
    let mut attributes = HashMap::new();
    let mut total_size = 0_i64;
    let mut accepted_size = 0_i64;
    let mut truncated = false;

    for item in decoded.items {
        total_size += item.byte_size;
        if total_size > effective_limit {
            // 首次超限时递增 Lost 计数；后续超限项直接丢弃。
            if !truncated {
                truncated = true;
                increment_connect_attrs_lost();
            }
            continue;
        }
        if !truncated {
            accepted_size += item.byte_size;
            attributes.insert(item.key, item.value);
        }
    }

    update_connect_attrs_longest_seen(decoded.total_size);

    let mut warnings = Vec::with_capacity(2);
    if decoded.has_deprecated_underscore_attr {
        warnings.push(DEPRECATED_UNDERSCORE_WARNING.to_owned());
    }
    if truncated {
        let truncated_bytes = decoded.total_size - accepted_size;
        attributes.insert(
            RESERVED_CONN_ATTR_TRUNCATED.to_vec(),
            truncated_bytes.to_string().into_bytes(),
        );
        warnings.push(format!(
            "session connection attributes truncated: total size {} bytes exceeds performance_schema_session_connect_attrs_size ({}), {} bytes were discarded",
            decoded.total_size, effective_limit, truncated_bytes
        ));
    }

    (attributes, warnings.join("; "))
}

/// 将负 limit 规范为 64 KiB 正常上限；非负则原样使用。
fn normalize_connect_attrs_limit(limit: i64) -> i64 {
    if limit < 0 {
        MAX_NORMAL_CONNECTION_ATTRIBUTES_SIZE
    } else {
        limit
    }
}

/// 原子递增 ConnectAttrsLost（持锁，对齐 Go 指标更新）。
fn increment_connect_attrs_lost() {
    let _guard = CONNECT_ATTRS_METRICS_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ConnectAttrsLost.Store(ConnectAttrsLost.Load().wrapping_add(1));
}

/// 更新所见最长连接属性总大小；≥64 KiB 的载荷不参与统计。
fn update_connect_attrs_longest_seen(total_size: i64) {
    if total_size >= MAX_NORMAL_CONNECTION_ATTRIBUTES_SIZE {
        return;
    }

    let _guard = CONNECT_ATTRS_METRICS_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if total_size > ConnectAttrsLongestSeen.Load() {
        ConnectAttrsLongestSeen.Store(total_size);
    }
}

/// 标准连接属性键（客户端名/版本/OS/PID/平台），不算弃用自定义键。
fn is_standard_connection_attribute(key: &[u8]) -> bool {
    matches!(
        key,
        b"_client_name" | b"_client_version" | b"_os" | b"_pid" | b"_platform"
    )
}
