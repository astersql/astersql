// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 二进制 JSON 函数相关单元测试：Unicode 解码、Unquote、Compare。
//
// 对齐 Go `json_binary_functions_test.go`；含基准桩函数与 opaque 比较用例。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use crate::json_functions::*;

/// `\uXXXX` / 代理对解码用例：输入十六进制、期望 UTF-8、字节数与合法性。
struct DecodeCase {
    input: &'static str,
    expected_result: &'static str,
    size: usize,
    in_surrogate_range: bool,
    expected_valid: bool,
}

/// 表驱动校验 `DecodeOneEscapedUnicodeForTest`。
#[test]
fn TestDecodeEscapedUnicode() {
    let test_cases = [
        DecodeCase {
            input: "597d",
            expected_result: "好\0",
            size: 3,
            in_surrogate_range: false,
            expected_valid: true,
        },
        DecodeCase {
            input: "fffd",
            expected_result: "�\0",
            size: 3,
            in_surrogate_range: false,
            expected_valid: true,
        },
        DecodeCase {
            input: "D83DDE0A",
            expected_result: "😊",
            size: 4,
            in_surrogate_range: false,
            expected_valid: true,
        },
        DecodeCase {
            input: "D83D",
            expected_result: "",
            size: 0,
            in_surrogate_range: true,
            expected_valid: false,
        },
        DecodeCase {
            input: "D83D11",
            expected_result: "",
            size: 0,
            in_surrogate_range: false,
            expected_valid: false,
        },
        DecodeCase {
            input: "ZZZZ",
            expected_result: "",
            size: 0,
            in_surrogate_range: false,
            expected_valid: false,
        },
        DecodeCase {
            input: "D83DDE0A597d",
            expected_result: "",
            size: 0,
            in_surrogate_range: false,
            expected_valid: false,
        },
    ];

    for case in test_cases {
        match DecodeOneEscapedUnicodeForTest(case.input.as_bytes()) {
            Ok((result, size, in_surrogate_range)) => {
                assert!(case.expected_valid, "{} should be invalid", case.input);
                assert_eq!(
                    case.in_surrogate_range, in_surrogate_range,
                    "{}",
                    case.input
                );
                assert_eq!(
                    case.expected_result,
                    String::from_utf8_lossy(&result),
                    "{}",
                    case.input
                );
                assert_eq!(case.size, size, "{}", case.input);
            }
            Err(error) => {
                assert!(!case.expected_valid, "{}: {error}", case.input);
                assert_eq!(
                    case.in_surrogate_range,
                    error.to_string().starts_with("surrogate:"),
                    "{}",
                    case.input
                );
            }
        }
    }
}

/// Unquote 转义序列用例。
struct UnquoteCase {
    input: &'static str,
    expected_result: &'static str,
    expected_valid: bool,
}

/// 表驱动校验 `UnquoteJSONStringForTest` 对常用转义与非法输入的处理。
#[test]
fn TestUnquoteJSONString() {
    let test_cases = [
        UnquoteCase {
            input: "\\b",
            expected_result: "\x08",
            expected_valid: true,
        },
        UnquoteCase {
            input: "\\f",
            expected_result: "\x0c",
            expected_valid: true,
        },
        UnquoteCase {
            input: "\\n",
            expected_result: "\n",
            expected_valid: true,
        },
        UnquoteCase {
            input: "\\r",
            expected_result: "\r",
            expected_valid: true,
        },
        UnquoteCase {
            input: "\\t",
            expected_result: "\t",
            expected_valid: true,
        },
        UnquoteCase {
            input: "\\\\",
            expected_result: "\\",
            expected_valid: true,
        },
        UnquoteCase {
            input: "\\u597d",
            expected_result: "好",
            expected_valid: true,
        },
        UnquoteCase {
            input: "0\\u597d0",
            expected_result: "0好0",
            expected_valid: true,
        },
        UnquoteCase {
            input: "\\a",
            expected_result: "a",
            expected_valid: true,
        },
        UnquoteCase {
            input: "[",
            expected_result: "[",
            expected_valid: true,
        },
        UnquoteCase {
            input: "\\ud83e\\udd21",
            expected_result: "🤡",
            expected_valid: true,
        },
        UnquoteCase {
            input: "\\ufffd",
            expected_result: "�",
            expected_valid: true,
        },
        UnquoteCase {
            input: "\\",
            expected_result: "",
            expected_valid: false,
        },
        UnquoteCase {
            input: "\\u59",
            expected_result: "",
            expected_valid: false,
        },
    ];

    for case in test_cases {
        let result = UnquoteJSONStringForTest(case.input.to_owned());
        if case.expected_valid {
            assert_eq!(case.expected_result, result.unwrap(), "{}", case.input);
        } else {
            assert!(result.is_err(), "{} should be invalid", case.input);
        }
    }
}

/// 基准桩：单次调用 Unicode 解码。
fn BenchmarkDecodeEscapedUnicode() {
    let _ = DecodeOneEscapedUnicodeForTest(b"597d");
}

/// 构造 Merge / MergePatch 基准用的一对 JSON 文档。
fn benchmark_values() -> (BinaryJSON, BinaryJSON) {
    let value_a = CreateBinaryJSON(serde_json::from_str(r#"{"title":"Goodbye!","author":{"givenName":"John","familyName":"Doe"},"tags":["example","sample"],"content":"This will be unchanged"}"#).unwrap()).unwrap();
    let value_b = CreateBinaryJSON(serde_json::from_str(r#"{"title":"Hello!","phoneNumber":"+01-123-456-7890","author":{"familyName":null},"tags":["example"]}"#).unwrap()).unwrap();
    (value_a, value_b)
}

/// 基准桩：MergePatch。
fn BenchmarkMergePatchBinary() {
    let (value_a, value_b) = benchmark_values();
    let _ = MergePatchBinaryJSON(&[Some(&value_a), Some(&value_b)]);
}

/// 基准桩：MergeBinaryJSON。
fn BenchmarkMergeBinary() {
    let (value_a, value_b) = benchmark_values();
    let _ = MergeBinaryJSON(&[value_a, value_b]);
}

/// 构造 opaque 类型 BinaryJSON（type_code + 长度前缀 + 原始字节）。
fn opaque(type_code: u8, bytes: &[u8]) -> BinaryJSON {
    assert!(bytes.len() < 0x80);
    let mut value = vec![type_code, bytes.len() as u8];
    value.extend_from_slice(bytes);
    BinaryJSON {
        TypeCode: JSONTypeCodeOpaque,
        Value: value,
    }
}

/// 校验字符串与 opaque 的 CompareBinaryJSON 序关系。
#[test]
fn TestBinaryCompare() {
    let string =
        |value: &str| CreateBinaryJSON(serde_json::Value::String(value.to_owned())).unwrap();
    let tests = [
        (string("a"), string("b"), -1),
        (opaque(0, &[0, 1, 2, 3]), opaque(0, &[0, 1, 2]), 1),
        (opaque(0, &[0, 1, 2, 3]), opaque(0, &[0, 2, 1]), -1),
        (string("test"), opaque(0, &[0, 2, 1]), -1),
    ];
    let compare_message = |result| match result {
        1 => "greater than",
        0 => "equal with",
        -1 => "smaller than",
        _ => unreachable!(),
    };

    for (left, right, expected) in tests {
        assert_eq!(
            expected,
            CompareBinaryJSON(&left, &right),
            "{left:?} should be {} {right:?}",
            compare_message(expected)
        );
    }
}

/// JSON_SEARCH 的 `_` 通配符按 Unicode 字符而非 UTF-8 字节匹配。
#[test]
fn TestSearchLikeMatchesUnicodeRune() {
    let value = CreateBinaryJSON(serde_json::json!(["你", "ab"])).unwrap();
    let result = value.Search("all", "_", b'\\', &[]).unwrap().unwrap();

    assert_eq!(
        serde_json::json!("$[0]"),
        BinaryJSONToSerde(&result).unwrap()
    );
}
