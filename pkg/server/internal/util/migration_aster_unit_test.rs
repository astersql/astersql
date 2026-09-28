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

// util 包迁移对照单测（Aster 补充）。
//
// 相对 Go 边界做更细回归：长度编码整数/字节、NUL 终止串、输入解码与 CORS，
// 以及 BufferedReadConn 的 Peek/读/探活不丢数据行为。

use astersql_server_internal_util::{
    LengthEncodedIntSize, NewBufferedReadConn, NewCorsHandler, NewInputDecoder, NewTestConfig,
    ParseLengthEncodedBytes, ParseLengthEncodedInt, ParseNullTermString,
};
use http::{Request, Response};
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;

/// 校验长度编码整数边界与 `LengthEncodedIntSize` 一致性，并覆盖截断输入报 UnexpectedEof。
#[test]
fn length_encoded_values_match_go_boundaries() {
    // 完整载荷：NULL(0xfb)、单字节、2/3/8 字节扩展及对应消费长度。
    for (input, expected) in [
        (&[0xfb][..], (0, true, 1)),
        (&[0][..], (0, false, 1)),
        (&[0xfc, 1, 2][..], (513, false, 3)),
        (&[0xfd, 1, 2, 3][..], (197_121, false, 4)),
        (
            &[0xfe, 1, 2, 3, 4, 5, 6, 7, 8][..],
            (578_437_695_752_307_201, false, 9),
        ),
    ] {
        let (num, is_null, consumed, error) = ParseLengthEncodedInt(input);
        assert_eq!((num, is_null, consumed), expected);
        assert!(error.is_none());
        assert_eq!(LengthEncodedIntSize(num), consumed);
    }

    // 截断输入：consumed 为 0 且错误为 UnexpectedEof。
    for input in [&[][..], &[0xfc, 1][..], &[0xfd, 1, 2][..], &[0xfe][..]] {
        let (_, _, consumed, error) = ParseLengthEncodedInt(input);
        assert_eq!(consumed, 0);
        assert_eq!(error.unwrap().kind(), ErrorKind::UnexpectedEof);
    }

    // NULL 字节串、长度不足、以及完整 "abc" 载荷。
    let (bytes, is_null, consumed, error) = ParseLengthEncodedBytes(&[0xfb]);
    assert!(bytes.is_none() && is_null && error.is_none());
    assert_eq!(consumed, 1);

    let (bytes, is_null, consumed, error) = ParseLengthEncodedBytes(&[1]);
    assert!(bytes.is_none() && !is_null);
    assert_eq!(consumed, 2);
    assert_eq!(error.unwrap().kind(), ErrorKind::UnexpectedEof);

    let (bytes, is_null, consumed, error) = ParseLengthEncodedBytes(&[3, b'a', b'b', b'c']);
    assert_eq!(bytes, Some(&b"abc"[..]));
    assert!(!is_null && error.is_none());
    assert_eq!(consumed, 4);
}

/// 校验 NUL 终止串解析：有终结符时切分前缀，无终结符时首值为 None（对齐 Go nil）。
#[test]
fn null_terminated_strings_match_go_nil_semantics() {
    assert_eq!(
        ParseNullTermString(b"abc\0def"),
        (Some(&b"abc"[..]), &b"def"[..])
    );
    assert_eq!(ParseNullTermString(b"\0def"), (Some(&b""[..]), &b"def"[..]));
    assert_eq!(ParseNullTermString(b"abcdef"), (None, &b"abcdef"[..]));
}

/// 校验 utf8mb4/latin1 输入解码、测试配置默认值，以及 CORS 头写入顺序。
#[test]
fn decoder_cors_and_test_config_match_go() {
    assert_eq!(NewInputDecoder("utf8mb4").DecodeInput(b"hello"), b"hello");
    assert_eq!(
        NewInputDecoder("latin1").DecodeInput(&[0xe9]),
        "é".as_bytes()
    );

    let mut cfg = NewTestConfig();
    assert_eq!(cfg.host, "127.0.0.1");
    assert_eq!(cfg.status.status_host, "127.0.0.1");
    assert!(!cfg.security.auto_tls);
    assert_eq!(cfg.socket, "");
    cfg.cors = "https://example.com".into();

    // CorsHandler 先写 Allow-Origin/Methods，再委托内层 handler。
    let handler = NewCorsHandler(
        |response: &mut Response<Vec<u8>>, _request: Request<Vec<u8>>| {
            *response.status_mut() = http::StatusCode::NO_CONTENT;
        },
        cfg,
    );
    let response = handler.ServeHTTP(Request::new(Vec::new()));
    assert_eq!(response.status(), 204);
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        "https://example.com"
    );
    assert_eq!(response.headers()["access-control-allow-methods"], "GET");
}

/// 校验 Peek 后再 Read 不丢数据，且空闲连接 IsAlive 返回 1。
#[test]
fn buffered_connection_peeks_then_reads_without_losing_data() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (release_tx, release_rx) = mpsc::channel();
    // 服务端写 "hello" 后阻塞，直到客户端探活完成再退出。
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.write_all(b"hello").unwrap();
        release_rx.recv().unwrap();
    });

    let stream = TcpStream::connect(address).unwrap();
    let mut conn = NewBufferedReadConn(stream).unwrap();
    assert_eq!(conn.Peek(2).unwrap(), b"he");
    let mut data = [0; 5];
    conn.read_exact(&mut data).unwrap();
    assert_eq!(&data, b"hello");
    assert_eq!(conn.IsAlive(), 1);

    release_tx.send(()).unwrap();
    server.join().unwrap();
}
