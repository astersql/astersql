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

// Parse 模块迁移对照单元测试。
//
// 覆盖 COM_STMT_FETCH 端序与封顶、握手头固定布局、各 capability 分支的
// 握手体解析、畸形包错误路径，以及连接属性策略与指标（Lost/LongestSeen）。

use std::sync::{Mutex, MutexGuard};

use super::parse::{
    ParseError, Response41, handshake_response_body, handshake_response_header, parse_attrs,
    stmt_fetch_cmd,
};
use super::parser::mysql::{
    ClientConnectAtts, ClientConnectWithDB, ClientPluginAuth, ClientPluginAuthLenencClientData,
    ClientSecureConnection, ClientZstdCompressionAlgorithm,
};
use super::sessionctx::vardef::{ConnectAttrsLongestSeen, ConnectAttrsLost, ConnectAttrsSize};

/// 串行化依赖全局 ConnectAttrs* 指标的测试，避免互相干扰。
static GLOBALS_LOCK: Mutex<()> = Mutex::new(());

/// 获取全局指标互斥锁；毒锁时吞掉 poison 继续（测试容错）。
pub(crate) fn globals_lock() -> MutexGuard<'static, ()> {
    GLOBALS_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 按 MySQL 长度编码整数规则编码 u64。
fn encode_lenenc_int(value: u64) -> Vec<u8> {
    match value {
        0..=250 => vec![value as u8],
        251..=0xffff => {
            let bytes = value.to_le_bytes();
            vec![0xfc, bytes[0], bytes[1]]
        }
        0x1_0000..=0xff_ffff => {
            let bytes = value.to_le_bytes();
            vec![0xfd, bytes[0], bytes[1], bytes[2]]
        }
        _ => {
            let mut encoded = vec![0xfe];
            encoded.extend_from_slice(&value.to_le_bytes());
            encoded
        }
    }
}

/// 长度编码字节串：先写长度整数再追加内容。
fn encode_lenenc_bytes(value: &[u8]) -> Vec<u8> {
    let mut encoded = encode_lenenc_int(value.len() as u64);
    encoded.extend_from_slice(value);
    encoded
}

/// 将多组键值对编码为连接属性载荷。
fn attrs_payload(items: &[(&str, &str)]) -> Vec<u8> {
    let mut payload = Vec::new();
    for (key, value) in items {
        payload.extend(encode_lenenc_bytes(key.as_bytes()));
        payload.extend(encode_lenenc_bytes(value.as_bytes()));
    }
    payload
}

/// 构造带固定 32 字节头与给定尾部的握手包，返回解析后的头偏移。
fn body_packet(capability: u32, tail: &[u8]) -> (Response41, Vec<u8>, usize) {
    let mut packet = Response41::default();
    let mut data = Vec::with_capacity(32 + tail.len());
    data.extend_from_slice(&capability.to_le_bytes());
    data.extend_from_slice(&0_u32.to_le_bytes());
    data.push(45);
    data.extend_from_slice(&[0; 23]);
    data.extend_from_slice(tail);
    let offset = handshake_response_header(&(), &mut packet, &data).unwrap();
    (packet, data, offset)
}

/// stmt_fetch：小端解析 statement_id/fetch_size，超长封顶 1024，长度不对报错。
#[test]
fn stmt_fetch_matches_go_length_endian_and_cap_behavior() {
    let cases = [
        (&[3, 0, 0, 0, 50, 0, 0, 0][..], Ok((3, 50))),
        (&[5, 0, 0, 0, 232, 3, 0, 0][..], Ok((5, 1000))),
        (&[5, 0, 0, 0, 0, 8, 0, 0][..], Ok((5, 1024))),
        (&[5, 0, 0][..], Err(ParseError::MalformedPacket)),
        (
            &[1, 0, 0, 0, 3, 2, 0, 0, 3, 5, 6][..],
            Err(ParseError::MalformedPacket),
        ),
        (&[][..], Err(ParseError::MalformedPacket)),
    ];

    for (input, expected) in cases {
        assert_eq!(stmt_fetch_cmd(input), expected);
    }
}

/// 握手头：读 capability/collation，短于 32 字节为畸形包。
#[test]
fn handshake_header_matches_go_fixed_layout() {
    let mut packet = Response41::default();
    let mut data = vec![0; 32];
    data[..4].copy_from_slice(&0x1234_5678_u32.to_le_bytes());
    data[8] = 224;

    assert_eq!(handshake_response_header(&(), &mut packet, &data), Ok(32));
    assert_eq!(packet.capability, 0x1234_5678);
    assert_eq!(packet.collation, 224);
    assert_eq!(
        handshake_response_header(&(), &mut packet, &data[..31]),
        Err(ParseError::MalformedPacket)
    );
}

/// 组合多种 capability，校验用户/认证/库名/插件/属性/zstd 全部解析正确。
#[test]
fn handshake_body_matches_all_go_capability_branches() {
    let _guard = globals_lock();
    let original_size = ConnectAttrsSize.Load();
    // -1 表示使用默认 64 KiB 正常上限。
    ConnectAttrsSize.Store(-1);

    let capability = ClientSecureConnection
        | ClientConnectWithDB
        | ClientPluginAuth
        | ClientConnectAtts
        | ClientZstdCompressionAlgorithm;
    let attrs = attrs_payload(&[("_client_name", "libmysql"), ("app_name", "myapp")]);
    let mut tail = b"root\0".to_vec();
    tail.extend_from_slice(&[3, 1, 2, 3]);
    tail.extend_from_slice(b"test_db\0caching_sha2_password\0");
    tail.extend(encode_lenenc_int(attrs.len() as u64));
    tail.extend(attrs);
    tail.push(7);
    let (mut packet, data, offset) = body_packet(capability, &tail);

    handshake_response_body(&(), &mut packet, &data, offset).unwrap();
    assert_eq!(packet.user, b"root");
    assert_eq!(packet.auth, [1, 2, 3]);
    assert_eq!(packet.db_name, b"test_db");
    assert_eq!(packet.auth_plugin, b"caching_sha2_password");
    assert_eq!(
        packet
            .attrs
            .get(b"_client_name".as_slice())
            .map(Vec::as_slice),
        Some(b"libmysql".as_slice())
    );
    assert_eq!(
        packet.attrs.get(b"app_name".as_slice()).map(Vec::as_slice),
        Some(b"myapp".as_slice())
    );
    assert_eq!(packet.zstd_level, 7);

    ConnectAttrsSize.Store(original_size);
}

/// 覆盖 lenenc 认证数据、无 capability 的 NUL 认证，以及缺 NUL 的插件名截断。
#[test]
fn handshake_body_preserves_lenenc_and_nul_auth_shapes() {
    let lenenc_capability = ClientPluginAuthLenencClientData | ClientPluginAuth;
    let auth = vec![0x5a; 300];
    let mut tail = b"alice\0".to_vec();
    tail.extend(encode_lenenc_int(auth.len() as u64));
    tail.extend_from_slice(&auth);
    tail.extend_from_slice(b"mysql_native_password\0");
    let (mut packet, data, offset) = body_packet(lenenc_capability, &tail);
    handshake_response_body(&(), &mut packet, &data, offset).unwrap();
    assert_eq!(packet.auth, auth);
    assert_eq!(packet.auth_plugin, b"mysql_native_password");

    let mut tail = b"bob\0secret\0".to_vec();
    tail.extend_from_slice(b"ignored without capability");
    let (mut packet, data, offset) = body_packet(0, &tail);
    handshake_response_body(&(), &mut packet, &data, offset).unwrap();
    assert_eq!(packet.user, b"bob");
    assert_eq!(packet.auth, b"secret");

    // 无 NUL 终止的插件名：不写入 auth_plugin（与 Go 行为一致）。
    let tail = b"carol\0\x01\x00plugin-without-nul";
    let (mut packet, data, offset) = body_packet(lenenc_capability, tail);
    handshake_response_body(&(), &mut packet, &data, offset).unwrap();
    assert!(packet.auth.is_empty());
    assert!(packet.auth_plugin.is_empty());
}

/// Go string 可无损承载任意字节；握手文本字段与属性键值不得用 U+FFFD 改写非法 UTF-8。
#[test]
fn handshake_body_preserves_non_utf8_string_bytes_like_go() {
    let _guard = globals_lock();
    let original_size = ConnectAttrsSize.Load();
    ConnectAttrsSize.Store(-1);

    let capability =
        ClientSecureConnection | ClientConnectWithDB | ClientPluginAuth | ClientConnectAtts;
    let mut attrs = Vec::new();
    attrs.extend(encode_lenenc_bytes(b"\xfc"));
    attrs.extend(encode_lenenc_bytes(b"\xfb"));

    let mut tail = b"\xff\0".to_vec();
    tail.push(0);
    tail.extend_from_slice(b"\xfe\0\xfd\0");
    tail.extend(encode_lenenc_int(attrs.len() as u64));
    tail.extend(attrs);
    let (mut packet, data, offset) = body_packet(capability, &tail);

    handshake_response_body(&(), &mut packet, &data, offset).unwrap();
    assert_eq!(packet.user, b"\xff");
    assert_eq!(packet.db_name, b"\xfe");
    assert_eq!(packet.auth_plugin, b"\xfd");
    let (key, value) = packet.attrs.iter().next().expect("one connection attr");
    assert_eq!(key, b"\xfc");
    assert_eq!(value, b"\xfb");

    ConnectAttrsSize.Store(original_size);
}

/// Go recover 路径在 Rust 中应映射为 MalformedPacket / AttributesTooLarge。
#[test]
fn handshake_body_converts_go_recover_paths_to_malformed_errors() {
    let cases: &[(u32, &[u8])] = &[
        (0, b"missing-user-terminator"),
        (ClientSecureConnection, b"user\0\x05ab"),
        (ClientConnectWithDB, b"user\0auth\0missing-db-terminator"),
        (ClientPluginAuthLenencClientData, b"user\0\xfc\x05"),
        (
            ClientPluginAuthLenencClientData | ClientConnectWithDB,
            b"user\0\x01",
        ),
        (ClientZstdCompressionAlgorithm, b"user\0auth\0"),
    ];
    for (capability, tail) in cases {
        let (mut packet, data, offset) = body_packet(*capability, tail);
        assert_eq!(
            handshake_response_body(&(), &mut packet, &data, offset),
            Err(ParseError::MalformedPacket)
        );
    }

    // 属性长度超过 1 MiB 硬上限应拒绝连接。
    let mut oversized = b"user\0auth\0".to_vec();
    oversized.extend(encode_lenenc_int((1_u64 << 20) + 1));
    let (mut packet, data, offset) = body_packet(ClientConnectAtts, &oversized);
    assert_eq!(
        handshake_response_body(&(), &mut packet, &data, offset),
        Err(ParseError::ConnectionAttributesTooLarge)
    );
}

/// 畸形属性行非致命：握手成功且 attrs 为空（对齐 Go 日志后继续）。
#[test]
fn malformed_connection_attribute_rows_are_non_fatal_like_go() {
    let _guard = globals_lock();
    let original_size = ConnectAttrsSize.Load();
    ConnectAttrsSize.Store(-1);

    let malformed_attrs = [5, b'a'];
    let mut tail = b"user\0auth\0".to_vec();
    tail.extend(encode_lenenc_int(malformed_attrs.len() as u64));
    tail.extend_from_slice(&malformed_attrs);
    let (mut packet, data, offset) = body_packet(ClientConnectAtts, &tail);
    handshake_response_body(&(), &mut packet, &data, offset).unwrap();
    assert!(packet.attrs.is_empty());

    ConnectAttrsSize.Store(original_size);
}

/// 校验弃用下划线警告、超限截断 `_truncated`、Lost 计数，以及 limit=0 跳过。
#[test]
fn connection_attribute_policy_and_metrics_match_go() {
    let _guard = globals_lock();
    let original_size = ConnectAttrsSize.Load();
    let original_lost = ConnectAttrsLost.Load();
    let original_longest = ConnectAttrsLongestSeen.Load();

    ConnectAttrsSize.Store(-1);
    ConnectAttrsLost.Store(0);
    ConnectAttrsLongestSeen.Store(0);
    let payload = attrs_payload(&[
        ("_client_name", "libmysql"),
        ("_custom", "val"),
        ("_program_name", "mysql"),
        ("app_name", "myapp"),
    ]);
    let (attrs, warning) = parse_attrs(&payload).unwrap();
    assert_eq!(
        attrs.get(b"_custom".as_slice()).map(Vec::as_slice),
        Some(b"val".as_slice())
    );
    assert_eq!(
        attrs.get(b"_program_name".as_slice()).map(Vec::as_slice),
        Some(b"mysql".as_slice())
    );
    assert_eq!(
        warning,
        "custom connection attributes with leading underscore are deprecated and will be rejected in a future release"
    );
    assert_eq!(ConnectAttrsLongestSeen.Load(), 61);
    assert_eq!(ConnectAttrsLost.Load(), 0);

    ConnectAttrsSize.Store(20);
    let payload = attrs_payload(&[("_truncated", "client-value"), ("app_name", "my_service")]);
    let (attrs, warning) = parse_attrs(&payload).unwrap();
    assert_eq!(
        attrs.get(b"_truncated".as_slice()).map(Vec::as_slice),
        Some(b"40".as_slice())
    );
    assert!(!attrs.contains_key(b"app_name".as_slice()));
    assert_eq!(ConnectAttrsLost.Load(), 1);
    assert!(warning.contains("total size 40 bytes exceeds performance_schema_session_connect_attrs_size (20), 40 bytes were discarded"));

    // limit=0：不收集属性，也不更新 LongestSeen。
    ConnectAttrsSize.Store(0);
    ConnectAttrsLongestSeen.Store(17);
    let (attrs, warning) = parse_attrs(&[0xfc]).unwrap();
    assert!(attrs.is_empty());
    assert!(warning.is_empty());
    assert_eq!(ConnectAttrsLongestSeen.Load(), 17);

    ConnectAttrsSize.Store(original_size);
    ConnectAttrsLost.Store(original_lost);
    ConnectAttrsLongestSeen.Store(original_longest);
}

/// ≥64 KiB 的属性总大小不更新 LongestSeen（与 Go 观测窗口一致）。
#[test]
fn longest_seen_ignores_payloads_at_or_above_sixty_four_kib() {
    let _guard = globals_lock();
    let original_size = ConnectAttrsSize.Load();
    let original_longest = ConnectAttrsLongestSeen.Load();
    ConnectAttrsSize.Store(-1);
    ConnectAttrsLongestSeen.Store(9);

    let value = "x".repeat(65_535);
    let payload = attrs_payload(&[("k", &value)]);
    let (attrs, _) = parse_attrs(&payload).unwrap();
    assert_eq!(attrs.get(b"k".as_slice()).map(Vec::len), Some(65_535));
    assert_eq!(ConnectAttrsLongestSeen.Load(), 9);

    ConnectAttrsSize.Store(original_size);
    ConnectAttrsLongestSeen.Store(original_longest);
}
