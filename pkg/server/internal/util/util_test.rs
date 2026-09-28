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

// util 包单测：长度编码与 NUL 终止串解析。
//
// 对齐 Go `util_test.go` 用例表，覆盖成功边界与 UnexpectedEof 路径。

#![allow(non_snake_case)]

use astersql_server_internal_util::{
    LengthEncodedIntSize, NewInputDecoder, ParseLengthEncodedBytes, ParseLengthEncodedInt,
    ParseNullTermString,
};
use std::io::ErrorKind;

/// 表驱动校验 `ParseLengthEncodedInt` 与成功路径上的 `LengthEncodedIntSize`。
#[test]
fn TestParseLengthEncodedInt() {
    // (输入, 期望值, 是否 NULL, 消费字节, 是否报错)
    let test_cases: &[(&[u8], u64, bool, usize, bool)] = &[
        (&[0xfb], 0, true, 1, false),
        (&[0x00], 0, false, 1, false),
        (&[0xfc, 0x01, 0x02], 513, false, 3, false),
        (&[0xfd, 0x01, 0x02, 0x03], 197_121, false, 4, false),
        (
            &[0xfe, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08],
            578_437_695_752_307_201,
            false,
            9,
            false,
        ),
        (&[], 0, false, 0, true),
        (&[0xfc, 0x01], 0, false, 0, true),
        (&[0xfd, 0x01, 0x02], 0, false, 0, true),
        (&[0xfe], 0, false, 0, true),
    ];

    for &(buffer, expected_num, expected_null, expected_n, expected_error) in test_cases {
        let (num, is_null, n, error) = ParseLengthEncodedInt(buffer);
        assert_eq!(expected_num, num);
        assert_eq!(expected_null, is_null);
        assert_eq!(expected_n, n);
        assert_eq!(expected_error, error.is_some());
        if let Some(error) = error {
            assert_eq!(ErrorKind::UnexpectedEof, error.kind());
        } else {
            assert_eq!(expected_n, LengthEncodedIntSize(num));
        }
    }
}

/// 校验 `ParseLengthEncodedBytes`：NULL、空串、载荷不足与仅头部不足等场景。
#[test]
fn TestParseLengthEncodedBytes() {
    let (bytes, is_null, n, error) = ParseLengthEncodedBytes(&[0xfb]);
    assert!(bytes.is_none());
    assert!(is_null);
    assert_eq!(1, n);
    assert!(error.is_none());

    let (bytes, is_null, n, error) = ParseLengthEncodedBytes(&[0]);
    assert!(bytes.is_none());
    assert!(!is_null);
    assert_eq!(1, n);
    assert!(error.is_none());

    // 声明长度 1 但无后续字节：consumed 为声明 end=2。
    let (bytes, is_null, n, error) = ParseLengthEncodedBytes(&[0x01]);
    assert!(bytes.is_none());
    assert!(!is_null);
    assert_eq!(2, n);
    assert_eq!(ErrorKind::UnexpectedEof, error.unwrap().kind());

    // 0xfe 后缺 8 字节载荷：整数解析即失败，consumed=0。
    let (bytes, is_null, n, error) = ParseLengthEncodedBytes(&[0xfe]);
    assert!(bytes.is_none());
    assert!(!is_null);
    assert_eq!(0, n);
    assert_eq!(ErrorKind::UnexpectedEof, error.unwrap().kind());
}

/// 校验 `ParseNullTermString` 的切分与无终结符时的 Go nil 语义。
#[test]
fn TestParseNullTermString() {
    let cases: &[(&[u8], Option<&[u8]>, &[u8])] = &[
        (b"abc\0def", Some(b"abc"), b"def"),
        (b"\0def", Some(b""), b"def"),
        (b"def\0hig\0k", Some(b"def"), b"hig\0k"),
        (b"abcdef", None, b"abcdef"),
    ];

    for &(input, expected_string, expected_remain) in cases {
        let (string, remain) = ParseNullTermString(input);
        assert_eq!(expected_string, string);
        assert_eq!(expected_remain, remain);
    }
}

/// Go 的字符集查找只接受精确的 TiDB 字符集名；大小写、空白和额外编码别名均回退为二进制透传。
#[test]
fn TestNewInputDecoderUsesExactCharsetNames() {
    for charset in ["LATIN1", " latin1 ", "windows-1252"] {
        assert_eq!(
            vec![0xe9],
            NewInputDecoder(charset).DecodeInput(&[0xe9]),
            "unexpected decoding for unsupported charset label {charset:?}"
        );
    }
}
