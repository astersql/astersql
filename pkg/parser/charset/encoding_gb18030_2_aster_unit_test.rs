// Copyright 2021 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// GB18030 / 多字符集编码的 Aster 单元测试。
//
// 对应 Go 侧 encoding 相关测试用例的 Rust 移植，覆盖查找回退、UTF-8 边界、
// Latin1 透传、GBK/GB18030 往返编解码以及 HTML 标签规范化。

use parser_charset::*;

/// 验证支持列表、查找回退到 binary，以及 UTF-8-as-noop 行为。
#[test]
fn test_encoding_lookup_and_fallback() {
    assert!(IsSupportedEncoding(CharsetGB18030));
    assert!(!IsSupportedEncoding("GB18030"));
    assert_eq!(FindEncoding(CharsetGBK).Name(), CharsetGBK);
    assert_eq!(FindEncoding("").Tp(), EncodingTpBin);
    assert_eq!(FindEncoding("unknown").Tp(), EncodingTpBin);
    assert_eq!(FindEncodingTakeUTF8AsNoop(CharsetUTF8).Tp(), EncodingTpBin);
}

/// 验证 utf8mb4 接受四字节表情，而 utf8mb3 严格模式拒绝并替换。
#[test]
fn test_utf8_and_utf8mb3_boundaries() {
    let utf8 = FindEncoding(CharsetUTF8MB4);
    assert!(utf8.IsValid("😂".as_bytes()));
    assert_eq!(utf8.Peek("😂x".as_bytes()), "😂".as_bytes());
    assert_eq!(utf8.MbLen("😂".as_bytes()), 4);

    let strict = EncodingUTF8MB3StrictImpl();
    assert!(!strict.IsValid("😂".as_bytes()));
    let err = strict.Transform(&mut Vec::new(), "valid😂tail".as_bytes(), OpReplace);
    assert!(err.is_err());
    assert_eq!(err.unwrap_err().output(), b"valid?tail");
}

/// 验证 Latin1 接受任意字节并原样透传。
#[test]
fn test_latin1_preserves_arbitrary_bytes() {
    let latin1 = FindEncoding(CharsetLatin1);
    let input = [0xff, 0x80, b'a'];
    assert!(latin1.IsValid(&input));
    assert_eq!(latin1.Peek(&input), &[0xff]);
    assert_eq!(
        latin1.Transform(&mut Vec::new(), &input, OpDecode).unwrap(),
        input
    );
}

/// 验证 GBK 中文往返，以及欧元符/0x80 的替换错误路径。
#[test]
fn test_gbk_round_trip_and_replacement() {
    let gbk = FindEncoding(CharsetGBK);
    let encoded = gbk
        .Transform(&mut Vec::new(), "一二三".as_bytes(), OpEncode)
        .unwrap();
    assert_eq!(encoded, b"\xd2\xbb\xb6\xfe\xc8\xfd");
    assert_eq!(
        gbk.Transform(&mut Vec::new(), &encoded, OpDecode).unwrap(),
        "一二三".as_bytes()
    );

    let euro = gbk.Transform(&mut Vec::new(), "€a".as_bytes(), OpEncodeReplace);
    assert!(euro.is_err());
    assert_eq!(euro.unwrap_err().output(), b"?a");
    let invalid = gbk.Transform(&mut Vec::new(), b"aa\x80ab", OpDecodeReplace);
    assert!(invalid.is_err());
    // GBK Peek groups the invalid 0x80 with the following byte, matching the Go fixture `aa?b`.
    // GBK Peek 会把非法 0x80 与后续字节成组，对齐 Go 期望输出 `aa?b`。
    assert_eq!(invalid.unwrap_err().output(), b"aa?b");
}

/// 验证 GB18030-2022 相关码点往返、Peek/MbLen 四字节边界及替换字符解码。
#[test]
fn test_gb18030_2022_and_rune_error_cases() {
    let enc = FindEncoding(CharsetGB18030);
    for (text, expected) in [
        ("€", b"\xa2\xe3".as_slice()),
        ("ḿ", b"\xa8\xbc".as_slice()),
        ("🀁", b"\x94\x38\xe1\x31".as_slice()),
    ] {
        let encoded = enc
            .Transform(&mut Vec::new(), text.as_bytes(), OpEncode)
            .unwrap();
        assert_eq!(encoded, expected);
        assert_eq!(
            enc.Transform(&mut Vec::new(), &encoded, OpDecode).unwrap(),
            text.as_bytes()
        );
    }

    assert_eq!(enc.Peek(b"\x81\x30\x81\x30x"), b"\x81\x30\x81\x30");
    assert_eq!(enc.MbLen(b"\x81\x30\x81\x30"), 4);
    let decoded = enc
        .Transform(&mut Vec::new(), b"\xb0\xb2\x84\x31\xa4\x37", OpDecode)
        .unwrap();
    assert_eq!(decoded, "安�".as_bytes());
}

/// 验证 Lookup 对 HTML 标签空白/大小写规范化，以及编解码往返。
#[test]
fn test_lookup_normalizes_html_labels() {
    use parser_charset::{DecoderTrap, EncoderTrap};

    assert_eq!(Lookup("  UTF8MB4\n").unwrap().name, "utf-8");
    assert_eq!(Lookup("GB_2312-80").unwrap().name, "gbk");
    let gb18030 = Lookup("gb18030").unwrap();
    assert_eq!(gb18030.name, "gb18030");
    let encoded = gb18030
        .encoding
        .encode("中文", EncoderTrap::Strict)
        .unwrap();
    assert_eq!(
        gb18030
            .encoding
            .decode(&encoded, DecoderTrap::Strict)
            .unwrap(),
        "中文"
    );
    assert!(Lookup("not-an-encoding").is_none());
}
