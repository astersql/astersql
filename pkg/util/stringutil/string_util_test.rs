// Copyright 2015 PingCAP, Inc.
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

// `string_util` 单元测试：反引号解析、LIKE 编译/匹配、标签与 Memoize 等。
//
// 对应用例表与 Go `string_util_test.go` 对齐；基准相关用例保留为可执行断言。

use std::cell::Cell;
use std::collections::HashMap;

/// 覆盖 UnquoteBytes 合法/非法输入与各类转义序列。
#[test]
fn test_unquote() {
    struct Case {
        input: &'static [u8],
        expected: &'static [u8],
        ok: bool,
    }

    let cases = [
        Case {
            input: b"",
            expected: b"",
            ok: false,
        },
        Case {
            input: b"'",
            expected: b"",
            ok: false,
        },
        Case {
            input: b"'abc\"",
            expected: b"",
            ok: false,
        },
        Case {
            input: b"abcdea",
            expected: b"",
            ok: false,
        },
        Case {
            input: b"'abc'def'",
            expected: b"",
            ok: false,
        },
        Case {
            input: b"\"abc\\\"",
            expected: b"",
            ok: false,
        },
        Case {
            input: b"\"abcdef\"",
            expected: b"abcdef",
            ok: true,
        },
        Case {
            input: b"\"abc'def\"",
            expected: b"abc'def",
            ok: true,
        },
        Case {
            input: "\"\\a汉字测试\"".as_bytes(),
            expected: "a汉字测试".as_bytes(),
            ok: true,
        },
        Case {
            input: "\"☺\"".as_bytes(),
            expected: "☺".as_bytes(),
            ok: true,
        },
        Case {
            input: b"\"\\xFF\"",
            expected: b"xFF",
            ok: true,
        },
        Case {
            input: b"\"\\U00010111\"",
            expected: b"U00010111",
            ok: true,
        },
        Case {
            input: b"\"\\U0001011111\"",
            expected: b"U0001011111",
            ok: true,
        },
        Case {
            input: b"\"\\a\\b\\f\\n\\r\\t\\v\\\\\\\"\"",
            expected: b"a\x08f\n\r\tv\\\"",
            ok: true,
        },
        Case {
            input: b"\"\\Z\\%\\_\"",
            expected: b"\x1a\\%\\_",
            ok: true,
        },
        Case {
            input: b"\"abc\\0\"",
            expected: b"abc\0",
            ok: true,
        },
        Case {
            input: b"\"abc\\\"abc\"",
            expected: b"abc\"abc",
            ok: true,
        },
        Case {
            input: b"'abcdef'",
            expected: b"abcdef",
            ok: true,
        },
        Case {
            input: b"'\"'",
            expected: b"\"",
            ok: true,
        },
        Case {
            input: b"'\\a\\b\\f\\n\\r\\t\\v\\\\\\''",
            expected: b"a\x08f\n\r\tv\\'",
            ok: true,
        },
        Case {
            input: b"' '",
            expected: b" ",
            ok: true,
        },
        Case {
            input: "'\\a汉字'".as_bytes(),
            expected: "a汉字".as_bytes(),
            ok: true,
        },
        Case {
            input: b"'\\a\x90'",
            expected: b"a\x90",
            ok: true,
        },
        Case {
            input: b"\"\\a\x18\xc3\xa8\xc3\xa0\xc3\xb8\xc2\xbb\x05\"",
            expected: b"a\x18\xc3\xa8\xc3\xa0\xc3\xb8\xc2\xbb\x05",
            ok: true,
        },
    ];

    for case in cases {
        let result = UnquoteBytes(case.input);
        assert_eq!(result.is_ok(), case.ok, "input: {:?}", case.input);
        match result {
            Ok(value) => assert_eq!(value, case.expected, "input: {:?}", case.input),
            Err(_) => assert!(case.expected.is_empty(), "input: {:?}", case.input),
        }
    }
}

/// 覆盖 CompilePattern + DoMatch 的通配符、转义与自定义 escape 字符用例。
#[test]
fn test_pattern_match() {
    let cases = [
        ("", "a", b'\\', false),
        ("a", "a", b'\\', true),
        ("a", "b", b'\\', false),
        ("aA", "aA", b'\\', true),
        ("_", "a", b'\\', true),
        ("_", "ab", b'\\', false),
        ("__", "b", b'\\', false),
        ("%", "abcd", b'\\', true),
        ("%", "", b'\\', true),
        ("%b", "AAA", b'\\', false),
        ("%a%", "BBB", b'\\', false),
        ("a%", "BBB", b'\\', false),
        (r"\%a", "%a", b'\\', true),
        (r"\%a", "aa", b'\\', false),
        (r"\_a", "_a", b'\\', true),
        (r"\_a", "aa", b'\\', false),
        (r"\\_a", r"\xa", b'\\', true),
        (r"\a\b", r"\a\b", b'\\', false),
        (r"\a\b", "ab", b'\\', true),
        ("%%_", "abc", b'\\', true),
        ("%_%_aA", "aaaA", b'\\', true),
        ("+_a", "_a", b'+', true),
        ("+%a", "%a", b'+', true),
        (r"\%a", "%a", b'+', false),
        ("++a", "+a", b'+', true),
        ("+a", "a", b'+', true),
        ("++_a", "+xa", b'+', true),
        ("___Հ", "䇇Հ", b'\\', false),
    ];

    for (pattern, input, escape, expected) in cases {
        let (chars, types) = CompilePattern(pattern, escape);
        assert_eq!(
            DoMatch(input, &chars, &types),
            expected,
            "pattern: {pattern}"
        );
    }
}

/// 验证 LIKE 模式到正则文本的转换（含 QuoteMeta 转义）。
#[test]
fn test_compile_like_2_regexp() {
    let cases = [
        ("", "^$"),
        ("a", "^a$"),
        ("aA", "^aA$"),
        ("$a$%", r"^\$a\$.*$"),
        ("a.b%", r"^a\.b.*$"),
        ("a+b", r"^a\+b$"),
        ("_", "^.$"),
        ("__", "^..$"),
        ("%", "^.*$"),
        ("%b", "^.*b$"),
        ("%a%", "^.*a.*$"),
        ("a%", "^a.*$"),
        (r"\%a", "^%a$"),
        (r"\_a", "^_a$"),
        (r"\\_a", r"^\\.a$"),
        (r"\a\b", "^ab$"),
        ("%%_", "^..*$"),
        ("%_%_aA", "^...*aA$"),
    ];
    for (pattern, expected) in cases {
        assert_eq!(CompileLike2Regexp(pattern), expected, "pattern: {pattern}");
    }
}

/// 验证编译后模式是否全为精确匹配（无 '_' / '%'）。
#[test]
fn test_is_exact_match() {
    let cases = [
        ("", b'\\', true),
        ("_", b'\\', false),
        ("%", b'\\', false),
        ("a", b'\\', true),
        ("a_", b'\\', false),
        ("a%", b'\\', false),
        (r"a\_", b'\\', true),
        (r"a\%", b'\\', true),
        (r"a\\", b'\\', true),
        (r"a\\_", b'\\', false),
        ("a+%", b'+', true),
        (r"a\%", b'+', false),
        ("a++", b'+', true),
        ("a++_", b'+', false),
    ];
    for (pattern, escape, expected) in cases {
        let (_, types) = CompilePattern(pattern, escape);
        assert_eq!(IsExactMatch(&types), expected, "pattern: {pattern}");
    }
}

/// 由键值对切片构造标签 map，供 BuildStringFromLabels 用例复用。
fn labels(entries: &[(&str, &str)]) -> HashMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| ((*key).into(), (*value).into()))
        .collect()
}

/// 验证标签按 key 排序后格式化为 `k=v,k=v`。
#[test]
fn test_build_string_from_labels() {
    let cases = [
        ("nil map", labels(&[]), ""),
        ("one label", labels(&[("aaa", "bbb")]), "aaa=bbb"),
        (
            "two labels",
            labels(&[("aaa", "bbb"), ("ccc", "ddd")]),
            "aaa=bbb,ccc=ddd",
        ),
    ];
    for (name, input, expected) in cases {
        assert_eq!(BuildStringFromLabels(&input), expected, "case: {name}");
    }
}

/// 验证 glob 路径中 '?' 被转义为 `\?`，其它字符不变。
#[test]
fn test_escape_glob_question_mark() {
    let cases = [
        ("123", "123"),
        ("12*3", "12*3"),
        ("12?", r"12\?"),
        ("[1-2]", "[1-2]"),
    ];
    for (input, expected) in cases {
        assert_eq!(EscapeGlobQuestionMark(input), expected);
    }
}

/// Go's utf8.RuneCountInString counts an incomplete UTF-8 prefix as one RuneError.
#[test]
fn test_convert_pos_in_utf8_inside_multibyte_character() {
    assert_eq!(ConvertPosInUtf8("你好", 4), 3);
}

/// 验证 MemoizeStr 对非空返回值只计算一次。
#[test]
fn test_memoize_str() {
    let count = Cell::new(0);
    let stringer = MemoizeStr(|| {
        count.set(count.get() + 1);
        "slow".to_owned()
    });
    assert_eq!(stringer.String(), "slow");
    assert_eq!(stringer.String(), "slow");
    assert_eq!(count.get(), 1);
}

/// 保留 Go 正例基准输入：复杂 '%' / '_' 模式应对目标串匹配成功。
#[test]
fn benchmark_do_match_cases_remain_true() {
    let cases = [
        ("a%_%_%_%_b", "aababab"),
        ("%_%_a%_%_b", "bbbaaabb"),
        ("a%_%_a%_%_b", "aaaabbbbbbaaaaaaaaabbbbb"),
    ];
    for (pattern, target) in cases {
        let (chars, types) = CompilePattern(pattern, b'\\');
        assert!(DoMatch(target, &chars, &types), "pattern: {pattern}");
    }
}

/// 保留 Go 负例基准输入：过长 '%' 链对纯 'a' 串应匹配失败。
#[test]
fn benchmark_do_match_negative_case_remains_false() {
    let pattern = "a%a%a%a%a%a%a%a%b";
    let (chars, types) = CompilePattern(pattern, b'\\');
    assert!(!DoMatch(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        &chars,
        &types
    ));
}

/// 保留标签拼接基准输入的期望输出。
#[test]
fn benchmark_build_string_from_labels_case() {
    let input = labels(&[("aaa", "bbb"), ("foo", "bar")]);
    assert_eq!(BuildStringFromLabels(&input), "aaa=bbb,foo=bar");
}
