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
// Copyright 2026 AsterSQL.

// `CharsetConvertor` 单元测试：gb18030/utf8 往返与非法字节替换。

use crate::NewCharsetConvertor;

/// UTF-8 期望文本（含扩展字符）。
const UTF8: &str = "你好，世界！ḿ€龴";
/// 与 `UTF8` 对应的 GB18030 编码字节。
const GB18030: &[u8] = &[
    0xC4, 0xE3, 0xBA, 0xC3, 0xA3, 0xAC, 0xCA, 0xC0, 0xBD, 0xE7, 0xA3, 0xA1, 0xA8, 0xBC, 0xA2, 0xE3,
    0xFE, 0x59,
];

/// gb18030 与 utf8mb4 的 Decode/Encode 往返正确。
#[test]
fn TestCharsetConvertor() {
    let converter = NewCharsetConvertor("gb18030", "�").unwrap();
    assert_eq!(converter.Decode(GB18030).unwrap(), UTF8);
    assert_eq!(converter.Encode(UTF8).unwrap(), GB18030);

    let utf8 = NewCharsetConvertor("utf8mb4", "�").unwrap();
    assert_eq!(utf8.Decode(UTF8.as_bytes()).unwrap(), UTF8);
    assert_eq!(utf8.Encode(UTF8).unwrap(), UTF8.as_bytes());
}

/// 非法字节替换为配置串；未知字符集名返回错误。
#[test]
fn TestInvalidCharReplace() {
    let converter = NewCharsetConvertor("gb18030", "😅😅😅").unwrap();
    let mut input = GB18030.to_vec();
    // 插入 0xff 制造非法序列，两侧夹合法 GB18030
    input.push(0xff);
    input.extend_from_slice(GB18030);
    assert_eq!(
        converter.Decode(&input).unwrap(),
        format!("{UTF8}😅😅😅{UTF8}")
    );
    assert!(NewCharsetConvertor("unknown", "�").is_err());
}

/// 合法编码的 U+FFFD 不是解码错误，不应被配置的非法字符替换串改写。
#[test]
fn valid_gb18030_replacement_character_is_preserved() {
    let converter = NewCharsetConvertor("gb18030", "invalid").unwrap();
    let encoded_replacement_character = [0x84, 0x31, 0xa4, 0x37];

    assert_eq!(
        converter.Decode(&encoded_replacement_character).unwrap(),
        "\u{fffd}"
    );
    assert_eq!(
        converter.Encode("\u{fffd}").unwrap(),
        encoded_replacement_character
    );

    let mixed = [
        encoded_replacement_character.as_slice(),
        &[0xff],
        encoded_replacement_character.as_slice(),
    ]
    .concat();
    assert_eq!(converter.Decode(&mixed).unwrap(), "�invalid�");
}
