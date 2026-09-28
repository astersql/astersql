// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 字符集编码集成测试（对应 Go `encoding_test.go`）。
//
// 覆盖 GBK/GB18030 编解码往返、替换错误路径，以及多字符集的 IsValid/OpReplaceNoErr 行为。

use parser_charset::*;
use parser_charset::{DecoderTrap, EncoderTrap};

/// 执行 Transform 并按 `valid` 断言成功或失败，返回最终输出字节。
fn transformed(
    enc: parser_charset::encoding::EncodingRef,
    input: &[u8],
    op: Op,
    valid: bool,
) -> Vec<u8> {
    match enc.Transform(&mut Vec::new(), input, op) {
        Ok(output) => {
            assert!(valid, "expected transform error for {input:02x?}");
            output
        }
        Err(error) => {
            assert!(!valid, "unexpected transform error: {error}");
            error.output().to_vec()
        }
    }
}

/// GBK 往返与解码/编码替换用例，对齐 Go TestEncoding。
#[test]
fn test_encoding() {
    let enc = FindEncoding(CharsetGBK);
    assert_eq!(enc.Name(), CharsetGBK);

    let text = "一二三四".as_bytes();
    let table = Lookup("gbk").unwrap();
    let expected = table
        .encoding
        .encode("一二三四", EncoderTrap::Strict)
        .unwrap();
    assert_eq!(
        enc.Transform(&mut Vec::new(), &expected, OpDecode).unwrap(),
        text
    );
    let encoded = enc.Transform(&mut Vec::new(), text, OpEncode).unwrap();
    assert_eq!(encoded, expected);
    assert_eq!(
        enc.Transform(&mut Vec::new(), &encoded, OpDecode).unwrap(),
        text
    );

    // 解码用例：(输入, 期望输出, 是否合法)；非法时使用 OpDecodeReplace。
    let decode_cases: &[(&[u8], &[u8], bool)] = &[
        ("一二三".as_bytes(), "涓?簩涓?".as_bytes(), false),
        ("一二三123".as_bytes(), "涓?簩涓?23".as_bytes(), false),
        ("测试".as_bytes(), "娴嬭瘯".as_bytes(), true),
        ("案1案2".as_bytes(), "妗?妗?".as_bytes(), false),
        ("焊䏷菡釬".as_bytes(), "鐒婁彿鑿￠嚞".as_bytes(), true),
        (
            "鞍杏以伊位依".as_bytes(),
            "闉嶆潖浠ヤ紛浣嶄緷".as_bytes(),
            true,
        ),
        (
            "移維緯胃萎衣謂違".as_bytes(),
            "绉荤董绶?儍钀庤。璎傞仌".as_bytes(),
            false,
        ),
        (
            "仆仂仗仞仭仟价伉佚估".as_bytes(),
            "浠嗕粋浠椾粸浠?粺浠蜂級浣氫及".as_bytes(),
            false,
        ),
        (
            "佝佗佇佶侈侏侘佻佩佰侑佯".as_bytes(),
            "浣濅綏浣囦蕉渚堜緩渚樹交浣╀桨渚戜蒋".as_bytes(),
            true,
        ),
        (b"\x80", b"?", false),
        (b"\x80a", b"?", false),
        (b"\x80aa", b"?a", false),
        (b"aa\x80ab", b"aa?b", false),
        (
            b"a\xe4\xbd\xa0\xe5\xa5\xbd\x80a\xe6\xb5\x8b\xe8\xaf\x95",
            "a浣犲ソ?娴嬭瘯".as_bytes(),
            false,
        ),
        (b"aa\x80", b"aa?", false),
    ];
    for &(input, expected, valid) in decode_cases {
        assert_eq!(
            transformed(enc, input, OpDecodeReplace, valid),
            expected,
            "{input:02x?}"
        );
    }

    // 编码用例：含欧元符与平面外字符的替换路径。
    let encode_cases: &[(&str, &[u8], bool)] = &[
        ("一二三", b"\xd2\xbb\xb6\xfe\xc8\xfd", true),
        ("🀁", b"?", false),
        ("valid_string_🀁", b"valid_string_?", false),
        ("€", b"?", false),
        ("€a", b"?a", false),
        ("a€aa", b"a?aa", false),
        ("aaa€", b"aaa?", false),
    ];
    for &(input, expected, valid) in encode_cases {
        assert_eq!(
            transformed(enc, input.as_bytes(), OpEncodeReplace, valid),
            expected,
            "{input}"
        );
    }
}

/// 多字符集 IsValid 与 OpReplaceNoErr 行为对照表。
#[test]
fn test_encoding_validate() {
    let invalid = b"\xff\xfe\xfd".as_slice();
    let mut chinese_invalid = "中文".as_bytes().to_vec();
    chinese_invalid.extend_from_slice(invalid);
    // (字符集, 输入, 替换后期望, 是否合法)。
    let cases: Vec<(&str, Vec<u8>, Vec<u8>, bool)> = vec![
        (CharsetASCII, b"".to_vec(), b"".to_vec(), true),
        (CharsetASCII, b"qwerty".to_vec(), b"qwerty".to_vec(), true),
        (CharsetASCII, "qwÊrty".into(), b"qw?rty".to_vec(), false),
        (CharsetASCII, "中文".into(), b"??".to_vec(), false),
        (
            CharsetASCII,
            "中文?qwert".into(),
            b"???qwert".to_vec(),
            false,
        ),
        (CharsetUTF8MB4, b"".to_vec(), b"".to_vec(), true),
        (CharsetUTF8MB4, b"qwerty".to_vec(), b"qwerty".to_vec(), true),
        (CharsetUTF8MB4, "qwÊrty".into(), "qwÊrty".into(), true),
        (
            CharsetUTF8MB4,
            "qwÊ合法字符串".into(),
            "qwÊ合法字符串".into(),
            true,
        ),
        (CharsetUTF8MB4, "😂".into(), "😂".into(), true),
        (CharsetUTF8MB4, invalid.to_vec(), b"???".to_vec(), false),
        (
            CharsetUTF8MB4,
            chinese_invalid.clone(),
            "中文???".into(),
            false,
        ),
        (CharsetUTF8MB4, "�".into(), "�".into(), true),
        (CharsetUTF8, b"".to_vec(), b"".to_vec(), true),
        (CharsetUTF8, b"qwerty".to_vec(), b"qwerty".to_vec(), true),
        (CharsetUTF8, "qwÊrty".into(), "qwÊrty".into(), true),
        (
            CharsetUTF8,
            "qwÊ合法字符串".into(),
            "qwÊ合法字符串".into(),
            true,
        ),
        (CharsetUTF8, "😂".into(), b"?".to_vec(), false),
        (
            CharsetUTF8,
            "valid_str😂".into(),
            b"valid_str?".to_vec(),
            false,
        ),
        (CharsetUTF8, invalid.to_vec(), b"???".to_vec(), false),
        (CharsetUTF8, chinese_invalid, "中文???".into(), false),
        (CharsetUTF8, "�".into(), "�".into(), true),
        (CharsetGBK, b"".to_vec(), b"".to_vec(), true),
        (CharsetGBK, b"asdf".to_vec(), b"asdf".to_vec(), true),
        (CharsetGBK, "中文".into(), "中文".into(), true),
        (CharsetGBK, "À".into(), b"?".to_vec(), false),
        (CharsetGBK, "中文À中文".into(), "中文?中文".into(), false),
        (CharsetGBK, "asdfÀ".into(), b"asdf?".to_vec(), false),
        (CharsetGB18030, b"".to_vec(), b"".to_vec(), true),
        (CharsetGB18030, b"asdf".to_vec(), b"asdf".to_vec(), true),
        (CharsetGB18030, "中文".into(), "中文".into(), true),
        (CharsetGB18030, "À".into(), "À".into(), true),
        (CharsetGB18030, "中文À中文".into(), "中文À中文".into(), true),
        (CharsetGB18030, "asdfÀ".into(), "asdfÀ".into(), true),
        (CharsetGB18030, "😂".into(), "😂".into(), true),
    ];
    for (charset, input, expected, valid) in cases {
        // CharsetUTF8 走 utf8mb3 严格实现，与 Go 测试一致。
        let enc = if charset == CharsetUTF8 {
            EncodingUTF8MB3StrictImpl()
        } else {
            FindEncoding(charset)
        };
        assert_eq!(enc.IsValid(&input), valid, "{charset}/{input:02x?}");
        assert_eq!(
            enc.Transform(&mut Vec::new(), &input, OpReplaceNoErr)
                .unwrap(),
            expected,
            "{charset}/{input:02x?}"
        );
    }
}

#[test]
/// ASCII 遇到高位字节时按 Go UTF8 Peek 的宽度整体报告，即使序列本身非法。
fn test_ascii_foreach_uses_utf8_peek_width() {
    for malformed in [&[0x80, 0x81][..], &[0xf5, 0x80, 0x80, 0x80][..]] {
        assert_eq!(malformed, encoding_utf8_impl().peek(malformed));
    }

    let encoding = FindEncoding(CharsetASCII);
    let malformed = b"\xe2(\xa1";
    let mut chunks = Vec::new();
    encoding.Foreach(malformed, OpReplaceNoErr, &mut |from, _, ok| {
        chunks.push((from.to_vec(), ok));
        true
    });
    assert_eq!(vec![(malformed.to_vec(), false)], chunks);
    assert_eq!(
        b"?",
        encoding
            .Transform(&mut Vec::new(), malformed, OpReplaceNoErr)
            .unwrap()
            .as_slice()
    );

    for malformed in [&[0x80, 0x81][..], &[0xf5, 0x80, 0x80, 0x80][..]] {
        let mut chunks = Vec::new();
        encoding.Foreach(malformed, OpReplaceNoErr, &mut |from, _, ok| {
            chunks.push((from.to_vec(), ok));
            true
        });
        assert_eq!(vec![(malformed.to_vec(), false)], chunks);
    }
}

/// Go 的 binary Transform 与 ASCII 全合法快路径都会直接返回 src，且不重置调用方的 dest。
#[test]
fn test_noop_transform_preserves_destination_buffer() {
    for charset in [CharsetBin, CharsetASCII] {
        let mut dest = b"caller-owned-capacity".to_vec();
        let output = FindEncoding(charset)
            .Transform(&mut dest, b"plain ascii", OpEncode)
            .unwrap();
        assert_eq!(output, b"plain ascii", "{charset}");
        assert_eq!(dest, b"caller-owned-capacity", "{charset}");
    }
}

/// GB18030 往返与解码/编码用例，对齐 Go TestEncodingGB18030。
#[test]
fn test_encoding_gb18030() {
    let enc = FindEncoding(CharsetGB18030);
    assert_eq!(enc.Name(), CharsetGB18030);
    let table = Lookup("gb18030").unwrap();
    let text = "一二三四";
    let expected = table.encoding.encode(text, EncoderTrap::Strict).unwrap();
    assert_eq!(
        enc.Transform(&mut Vec::new(), &expected, OpDecode).unwrap(),
        text.as_bytes()
    );
    assert_eq!(
        enc.Transform(&mut Vec::new(), text.as_bytes(), OpEncode)
            .unwrap(),
        expected
    );
    assert_eq!(
        table
            .encoding
            .decode(&expected, DecoderTrap::Strict)
            .unwrap(),
        text
    );

    // 解码用例：含 0x80 与 U+FFFD 四字节序列。
    let decode_cases: &[(&[u8], &[u8], bool)] = &[
        ("一二三".as_bytes(), "涓?浜屼笁".as_bytes(), false),
        ("一二三123".as_bytes(), "涓?浜屼笁123".as_bytes(), false),
        ("测试".as_bytes(), "娴嬭瘯".as_bytes(), true),
        ("案1案2".as_bytes(), "妗?1妗?2".as_bytes(), false),
        ("焊䏷菡釬".as_bytes(), "鐒婁彿鑿￠嚞".as_bytes(), true),
        (
            "鞍杏以伊位依".as_bytes(),
            "闉嶆潖浠ヤ紛浣嶄緷".as_bytes(),
            true,
        ),
        (
            "移維緯胃萎衣謂違".as_bytes(),
            "绉荤董绶\u{e21d}儍钀庤。璎傞仌".as_bytes(),
            true,
        ),
        (
            "仆仂仗仞仭仟价伉佚估".as_bytes(),
            "浠嗕粋浠椾粸浠\u{e15d}粺浠蜂級浣氫及".as_bytes(),
            true,
        ),
        (
            "佝佗佇佶侈侏侘佻佩佰侑佯".as_bytes(),
            "浣濅綏浣囦蕉渚堜緩渚樹交浣╀桨渚戜蒋".as_bytes(),
            true,
        ),
        (b"\x80", b"?", false),
        (b"\x80a", b"?a", false),
        (b"\x80aa", b"?aa", false),
        (b"aa\x80ab", b"aa?ab", false),
        (
            b"a\xe4\xbd\xa0\xe5\xa5\xbd\x80a\xe6\xb5\x8b\xe8\xaf\x95",
            "a浣犲ソ?a娴嬭瘯".as_bytes(),
            false,
        ),
        (b"aa\x80", b"aa?", false),
        (b"\xb0\xb2", "安".as_bytes(), true),
        (
            b"\xb0\xb2\x84\x31\xa4\x37\x30\x84\x31\xa4\x37\x32",
            "安�0�2".as_bytes(),
            true,
        ),
        (
            b"\x80\x84\x31\xa4\x37\x80\x84\x31\xa4\x37",
            "?�?�".as_bytes(),
            false,
        ),
        (b"\x84\x31\xa4\x37\x81", "�?".as_bytes(), false),
    ];
    for &(input, expected, valid) in decode_cases {
        assert_eq!(
            transformed(enc, input, OpDecodeReplace, valid),
            expected,
            "{input:02x?}"
        );
    }

    // 编码用例：欧元符、ḿ、补充平面麻将符均可编码。
    let encode_cases: &[(&str, &[u8])] = &[
        ("一二三", b"\xd2\xbb\xb6\xfe\xc8\xfd"),
        ("🀁", b"\x94\x38\xe1\x31"),
        ("€", b"\xa2\xe3"),
        ("€a", b"\xa2\xe3a"),
        ("a€aa", b"a\xa2\xe3aa"),
        ("aaa€", b"aaa\xa2\xe3"),
        ("ḿ", b"\xa8\xbc"),
    ];
    for &(input, expected) in encode_cases {
        assert_eq!(
            transformed(enc, input.as_bytes(), OpEncodeReplace, true),
            expected,
            "{input}"
        );
    }
}
