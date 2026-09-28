// Copyright 2024 PingCAP, Inc.
// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// `topLevelJSONTokenIter` 单元测试。
//
// 对照 Go `TestIter`：非法输入报错、顶层字段名/标量/嵌套值遍历，以及对象结束 EOF。

use super::{Token, newTopLevelJSONTokenIter};

/// 构造字符串 Token 的测试辅助。
fn string(value: &str) -> Token {
    Token::String(value.to_owned())
}

/// 构造数字 Token 的测试辅助（保留原文）。
fn number(value: &str) -> Token {
    Token::Number(value.to_owned())
}

/// 对应 Go TestIter：畸形输入、顶层名、标量/嵌套值与对象结束行为。
// TestIter mirrors Go's TestIter: malformed input handling, top-level names,
// scalar values, nested values, and end-of-object behavior.
#[test]
fn test_iter() {
    // 失败用例：消息子串需出现在错误文本中。
    let fail_cases = [
        ("{", "unexpected EOF"),
        (
            "[]",
            "expected '{' for topLevelJSONTokenIter, got Delim('[')",
        ),
        ("{a}", "expected value at line 1 column 1 at byte 1"),
        ("{]", "mismatched closing delimiter at byte 1"),
    ];

    for (content, expected_error) in fail_cases {
        let mut iter = newTopLevelJSONTokenIter(content.as_bytes());
        // 持续 next 直到拿到错误。
        let error = loop {
            match iter.next(false) {
                Ok(_) => continue,
                Err(error) => break error,
            }
        };
        assert!(
            error.to_string().contains(expected_error),
            "content: {content}; expected error containing {expected_error:?}, got {error}"
        );
    }

    // 成功用例：交替断言字段名与字段值 token 序列。
    let success_cases = [
        ("{}", vec![]),
        (
            r#"{"a": 1, "b": "val"}"#,
            vec![
                vec![string("a")],
                vec![number("1")],
                vec![string("b")],
                vec![string("val")],
            ],
        ),
        (
            r#"{"a": 1, "long1": {"skip": "skip"}, "b": "val", "long2": [0,0,{"skip":2}]}"#,
            vec![
                vec![string("a")],
                vec![number("1")],
                vec![string("long1")],
                vec![
                    Token::Delim('{'),
                    string("skip"),
                    string("skip"),
                    Token::Delim('}'),
                ],
                vec![string("b")],
                vec![string("val")],
                vec![string("long2")],
                vec![
                    Token::Delim('['),
                    number("0"),
                    number("0"),
                    Token::Delim('{'),
                    string("skip"),
                    number("2"),
                    Token::Delim('}'),
                    Token::Delim(']'),
                ],
            ],
        ),
    ];

    for (content, expected) in success_cases {
        let mut iter = newTopLevelJSONTokenIter(content.as_bytes());
        let mut expected_index = 0;
        while expected_index < expected.len() {
            let name = iter
                .readName()
                .unwrap_or_else(|error| panic!("content: {content}; readName failed: {error}"));
            assert_eq!(
                vec![string(&name)],
                expected[expected_index],
                "content: {content}"
            );
            expected_index += 1;

            let tokens = iter
                .next(false)
                .unwrap_or_else(|error| panic!("content: {content}; next failed: {error}"));
            assert_eq!(tokens, expected[expected_index], "content: {content}");
            expected_index += 1;
        }

        // 耗尽顶层对象后必须返回正常 EOF。
        let error = iter
            .next(false)
            .expect_err("exhausted top-level object must return EOF");
        assert!(error.is_eof(), "content: {content}; got {error}");
    }
}

/// Go encoding/json only accepts space, tab, CR, and LF as JSON whitespace.
#[test]
fn test_iter_rejects_non_json_ascii_whitespace() {
    for content in [b"{\x0b}".as_slice(), b"{\x0c}".as_slice()] {
        let mut iter = newTopLevelJSONTokenIter(content);
        let error = iter
            .next(false)
            .expect_err("vertical tab and form feed must not be accepted as JSON whitespace");
        assert!(
            !error.is_eof(),
            "non-JSON whitespace must be a syntax error, got {error}"
        );
    }
}
