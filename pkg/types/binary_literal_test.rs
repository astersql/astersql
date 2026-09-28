// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// BinaryLiteral（二进制字面量）解析、格式化与比较的单元测试。
//
// 覆盖 BIT/HEX 字面量解析、去前导零、整数转换溢出截断，
// 以及字节级 Compare / ToString 行为，对齐 Go `types` 包测试。

use types_group_1::*;

/// 汇总驱动各 BinaryLiteral 子用例。
#[test]
fn test_binary_literal() {
    test_trim_leading_zero_bytes();
    test_parse_bit_str_table();
    test_parse_bit_str_empty_error();
    test_parse_hex_str_table();
    test_parse_hex_str_empty_error();
    test_binary_literal_string();
    test_to_bit_literal_string();
    test_to_int();
    test_new_binary_literal_from_uint();
    test_compare_binary_literal();
    test_to_string();
}

/// 去除前导零字节，但全零时保留一个零字节。
fn test_trim_leading_zero_bytes() {
    let cases: &[(&[u8], &[u8])] = &[
        (&[], &[]),
        (&[0x0], &[0x0]),
        (&[0x1], &[0x1]),
        (&[0x1, 0x0], &[0x1, 0x0]),
        (&[0x0, 0x1], &[0x1]),
        (&[0x0, 0x0, 0x0], &[0x0]),
        (&[0x1, 0x0, 0x0], &[0x1, 0x0, 0x0]),
        (
            &[0x0, 0x1, 0x0, 0x0, 0x1, 0x0, 0x0],
            &[0x1, 0x0, 0x0, 0x1, 0x0, 0x0],
        ),
        (
            &[0x0, 0x0, 0x0, 0x0, 0x0, 0x1, 0x0, 0x0, 0x1, 0x0, 0x0],
            &[0x1, 0x0, 0x0, 0x1, 0x0, 0x0],
        ),
    ];

    for (input, expected) in cases {
        assert_eq!(trimLeadingZeroBytes(input), *expected, "input={input:?}");
    }
}

/// 表驱动校验 `b'...'` / `0b...` / `B'...'` 等 BIT 字面量解析。
fn test_parse_bit_str_table() {
    // hello / 左侧补 0 的等价比特串，用于验证按字节对齐解析
    let hello_bits = "1101000011001010110110001101100011011110010000001110111011011110111001001101100011001000010000001100110011011110110111100100000011000100110000101110010";
    let padded_hello_bits = "01101000011001010110110001101100011011110010000001110111011011110111001001101100011001000010000001100110011011110110111100100000011000100110000101110010";
    let cases: Vec<(String, Option<Vec<u8>>)> = vec![
        ("b''".into(), Some(vec![])),
        ("B''".into(), Some(vec![])),
        ("0b''".into(), None),
        ("0b0".into(), Some(vec![0x0])),
        ("b'0'".into(), Some(vec![0x0])),
        ("B'0'".into(), Some(vec![0x0])),
        ("0B0".into(), None),
        ("0b123".into(), None),
        ("b'123'".into(), None),
        ("b'é0000000'".into(), None),
        ("0b'1010'".into(), None),
        ("0b0000000".into(), Some(vec![0x0])),
        ("b'0000000'".into(), Some(vec![0x0])),
        ("B'0000000'".into(), Some(vec![0x0])),
        ("0b00000000".into(), Some(vec![0x0])),
        ("b'00000000'".into(), Some(vec![0x0])),
        ("B'00000000'".into(), Some(vec![0x0])),
        ("0b000000000".into(), Some(vec![0x0, 0x0])),
        ("b'000000000'".into(), Some(vec![0x0, 0x0])),
        ("B'000000000'".into(), Some(vec![0x0, 0x0])),
        ("0b1".into(), Some(vec![0x1])),
        ("b'1'".into(), Some(vec![0x1])),
        ("B'1'".into(), Some(vec![0x1])),
        ("0b00000001".into(), Some(vec![0x1])),
        ("b'00000001'".into(), Some(vec![0x1])),
        ("B'00000001'".into(), Some(vec![0x1])),
        ("0b000000010".into(), Some(vec![0x0, 0x2])),
        ("b'000000010'".into(), Some(vec![0x0, 0x2])),
        ("B'000000010'".into(), Some(vec![0x0, 0x2])),
        ("0b000000001".into(), Some(vec![0x0, 0x1])),
        ("b'000000001'".into(), Some(vec![0x0, 0x1])),
        ("B'000000001'".into(), Some(vec![0x0, 0x1])),
        ("0b11111111".into(), Some(vec![0xff])),
        ("b'11111111'".into(), Some(vec![0xff])),
        ("B'11111111'".into(), Some(vec![0xff])),
        ("0b111111111".into(), Some(vec![0x1, 0xff])),
        ("b'111111111'".into(), Some(vec![0x1, 0xff])),
        ("B'111111111'".into(), Some(vec![0x1, 0xff])),
        (
            format!("0b{hello_bits}"),
            Some(b"hello world foo bar".to_vec()),
        ),
        (
            format!("b'{hello_bits}'"),
            Some(b"hello world foo bar".to_vec()),
        ),
        (
            format!("B'{hello_bits}'"),
            Some(b"hello world foo bar".to_vec()),
        ),
        (
            format!("0b{padded_hello_bits}"),
            Some(b"hello world foo bar".to_vec()),
        ),
        (
            format!("b'{padded_hello_bits}'"),
            Some(b"hello world foo bar".to_vec()),
        ),
        (
            format!("B'{padded_hello_bits}'"),
            Some(b"hello world foo bar".to_vec()),
        ),
    ];

    for (input, expected) in cases {
        match (ParseBitStr(input.clone()), expected) {
            (Err(_), None) => {}
            (Ok(actual), Some(expected)) => assert_eq!(actual.as_ref(), expected, "{input}"),
            (result, expected) => panic!("input={input}, result={result:?}, expected={expected:?}"),
        }
    }
}

/// 空字符串 BIT 解析应返回 invalid empty 错误。
fn test_parse_bit_str_empty_error() {
    let error = ParseBitStr(String::new()).unwrap_err();
    assert!(error.to_string().contains("invalid empty "));
}

/// 表驱动校验 `x'...'` / `0x...` / `X'...'` 等 HEX 字面量解析。
fn test_parse_hex_str_table() {
    let cases: &[(&str, Option<&[u8]>)] = &[
        ("x'1'", None),
        ("x'01'", Some(&[0x1])),
        ("X'01'", Some(&[0x1])),
        ("0x1", Some(&[0x1])),
        ("0x-1", None),
        ("0X11", None),
        ("x'01+'", None),
        ("0x123", Some(&[0x01, 0x23])),
        ("0x10", Some(&[0x10])),
        ("0x4D7953514C", Some(b"MySQL")),
        (
            "0x4920616D2061206C6F6E672068657820737472696E67",
            Some(b"I am a long hex string"),
        ),
        (
            "x'4920616D2061206C6F6E672068657820737472696E67'",
            Some(b"I am a long hex string"),
        ),
        (
            "X'4920616D2061206C6F6E672068657820737472696E67'",
            Some(b"I am a long hex string"),
        ),
        ("x''", Some(&[])),
    ];

    for (input, expected) in cases {
        match (ParseHexStr((*input).to_owned()), expected) {
            (Err(_), None) => {}
            (Ok(actual), Some(expected)) => assert_eq!(actual.as_ref(), *expected, "{input}"),
            (result, expected) => panic!("input={input}, result={result:?}, expected={expected:?}"),
        }
    }
}

/// 空字符串 HEX 解析路径的错误信息（此处复用 ParseBitStr 空串断言）。
fn test_parse_hex_str_empty_error() {
    let error = ParseBitStr(String::new()).unwrap_err();
    assert!(error.to_string().contains("invalid empty "));
}

/// 校验 BinaryLiteral 的 `0x` 十六进制字符串表示。
fn test_binary_literal_string() {
    let cases = [
        (BinaryLiteral(vec![]), ""),
        (BinaryLiteral(vec![0x0]), "0x00"),
        (BinaryLiteral(vec![0x1]), "0x01"),
        (BinaryLiteral(vec![0xff, 0x01]), "0xff01"),
    ];
    for (input, expected) in cases {
        assert_eq!(input.String(), expected);
    }
}

/// 校验按位字面量格式化，以及是否去除前导零位。
fn test_to_bit_literal_string() {
    let cases = [
        (vec![], true, "b''"),
        (vec![], false, "b''"),
        (vec![0x0], true, "b'0'"),
        (vec![0x0], false, "b'00000000'"),
        (vec![0x0, 0x0], true, "b'0'"),
        (vec![0x0, 0x0], false, "b'0000000000000000'"),
        (vec![0x1], true, "b'1'"),
        (vec![0x1], false, "b'00000001'"),
        (vec![0xff, 0x01], true, "b'1111111100000001'"),
        (vec![0xff, 0x01], false, "b'1111111100000001'"),
        (vec![0x0, 0xff, 0x01], true, "b'1111111100000001'"),
        (vec![0x0, 0xff, 0x01], false, "b'000000001111111100000001'"),
    ];
    for (bytes, trim_leading_zero, expected) in cases {
        assert_eq!(
            BinaryLiteral(bytes).ToBitLiteralString(trim_leading_zero),
            expected
        );
    }
}

/// 校验 HEX 转 u64：超过 8 字节时截断为 MAX 并返回错误。
fn test_to_int() {
    let cases = [
        ("x''", 0_u64, false),
        ("0x00", 0x0, false),
        ("0xff", 0xff, false),
        ("0x10ff", 0x10ff, false),
        ("0x1010ffff", 0x1010ffff, false),
        ("0x1010ffff8080", 0x1010ffff8080, false),
        ("0x1010ffff8080ff12", 0x1010ffff8080ff12, false),
        ("0x1010ffff8080ff12ff", u64::MAX, true),
    ];

    for (input, expected, has_error) in cases {
        let literal = ParseHexStr(input.to_owned()).unwrap();
        match literal.ToInt(DefaultStmtNoWarningContext.clone()) {
            Ok(actual) => {
                assert!(!has_error, "input={input}");
                assert_eq!(actual, expected, "input={input}");
            }
            Err(error) => {
                assert!(has_error, "input={input}: {error}");
                assert_eq!(error.value, expected, "input={input}");
            }
        }
    }
}

/// 从 u64 构造指定字节宽度的 BinaryLiteral；负宽度除 -1 外 panic。
fn test_new_binary_literal_from_uint() {
    let cases: &[(u64, isize, &[u8])] = &[
        (0x0, -1, &[0x0]),
        (0x0, 1, &[0x0]),
        (0x0, 2, &[0x0, 0x0]),
        (0x1, -1, &[0x1]),
        (0x1, 1, &[0x1]),
        (0x1, 2, &[0x0, 0x1]),
        (0x1, 3, &[0x0, 0x0, 0x1]),
        (0x10, -1, &[0x10]),
        (0x123, -1, &[0x1, 0x23]),
        (0x123, 2, &[0x1, 0x23]),
        (0x123, 1, &[0x23]),
        (0x123, 5, &[0x0, 0x0, 0x0, 0x1, 0x23]),
        (0x4D7953514C, -1, &[0x4D, 0x79, 0x53, 0x51, 0x4C]),
        (
            0x4D7953514C,
            8,
            &[0x0, 0x0, 0x0, 0x4D, 0x79, 0x53, 0x51, 0x4C],
        ),
        (
            0x4920616D2061206C,
            -1,
            &[0x49, 0x20, 0x61, 0x6D, 0x20, 0x61, 0x20, 0x6C],
        ),
        (
            0x4920616D2061206C,
            8,
            &[0x49, 0x20, 0x61, 0x6D, 0x20, 0x61, 0x20, 0x6C],
        ),
        (0x4920616D2061206C, 5, &[0x6D, 0x20, 0x61, 0x20, 0x6C]),
    ];

    for (input, byte_size, expected) in cases {
        assert_eq!(
            NewBinaryLiteralFromUint(*input, *byte_size).as_ref(),
            *expected,
            "input={input:#x}, byte_size={byte_size}"
        );
    }
    assert!(std::panic::catch_unwind(|| NewBinaryLiteralFromUint(0x123, -2)).is_err());
}

/// 按去前导零后的字节内容比较两个 BinaryLiteral。
fn test_compare_binary_literal() {
    let cases = [
        (vec![0, 0, 1], vec![2], -1),
        (vec![0, 1], vec![0, 0, 2], -1),
        (vec![0, 1], vec![1], 0),
        (vec![0, 2, 1], vec![1, 2], 1),
    ];
    for (left, right, expected) in cases {
        assert_eq!(BinaryLiteral(left).Compare(BinaryLiteral(right)), expected);
    }
}

/// 校验 HEX/BIT 字面量按原始字节解码为字符串。
fn test_to_string() {
    let hex = NewHexLiteral("x'3A3B'".to_owned()).unwrap();
    assert_eq!(hex.ToString(), ":;");
    let bit = NewBitLiteral("b'00101011'".to_owned()).unwrap();
    assert_eq!(bit.ToString(), "+");
}
