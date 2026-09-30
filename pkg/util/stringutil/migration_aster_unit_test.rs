// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// `stringutil` 迁移对齐单测：对照 Go 行为表验证 Unquote / LIKE / UTF-8 等 API。
//
// 覆盖转义解析、通配符编译匹配、标识符转义、MemoizeStr 缓存语义等关键路径。

use super::string_util::*;
use std::cell::Cell;
use std::collections::HashMap;

/// 验证 Unquote/UnquoteBytes 对转义序列与非法 UTF-8 字节的行为与 Go 一致。
#[test]
fn migration_unquote_matches_go_escape_and_raw_byte_behavior() {
    let cases = [
        (r#""abcdef""#, b"abcdef".as_slice()),
        (r#""\a汉字测试""#, "a汉字测试".as_bytes()),
        (r#""\a\b\f\n\r\t\v\\\"""#, b"a\x08f\n\r\tv\\\"".as_slice()),
        (r#""\Z\%\_""#, b"\x1a\\%\\_".as_slice()),
        (r#"'abc\0'"#, b"abc\0".as_slice()),
    ];
    for (source, expected) in cases {
        assert_eq!(
            UnquoteBytes(source.as_bytes()).unwrap(),
            expected,
            "{source}"
        );
        assert_eq!(Unquote(source).unwrap().as_bytes(), expected, "{source}");
    }

    assert_eq!(UnquoteBytes(b"'\x90'").unwrap(), b"\x90");
    for invalid in [b"".as_slice(), b"'", b"'abc\"", b"abcdea", b"'abc'def'"] {
        assert_eq!(UnquoteBytes(invalid).unwrap_err().to_string(), ErrSyntax);
    }
}

/// 验证 CompilePattern/DoMatch/CompileLike2Regexp/IsExactMatch 与 Go 用例表一致。
#[test]
fn migration_like_compilation_and_matching_match_go_tables() {
    let cases = [
        ("", "a", b'\\', false),
        ("_", "a", b'\\', true),
        ("%", "", b'\\', true),
        (r"\%a", "%a", b'\\', true),
        (r"\\_a", r"\xa", b'\\', true),
        ("%%_", "abc", b'\\', true),
        ("%_%_aA", "aaaA", b'\\', true),
        ("+_a", "_a", b'+', true),
        ("++_a", "+xa", b'+', true),
        ("___Հ", "䇇Հ", b'\\', false),
    ];
    for (pattern, input, escape, expected) in cases {
        let (weights, types) = CompilePattern(pattern, escape);
        assert_eq!(
            DoMatch(input, &weights, &types),
            expected,
            "{pattern:?} {input:?}"
        );
    }

    assert_eq!(CompileLike2Regexp(r"$a$%", b'\\'), r"^\$a\$.*$");
    assert_eq!(CompileLike2Regexp(r"\\_a", b'\\'), r"^\\.a$");
    let (_, exact) = CompilePattern(r"a\%", b'\\');
    assert!(IsExactMatch(&exact));
}

/// 验证二进制 LIKE 编译按字节保留权重，并对非 UTF-8 输入可匹配。
#[test]
fn migration_binary_like_preserves_bytes() {
    let (weights, types) = CompilePatternBinary("%_\\%", b'\\');
    assert_eq!(weights, b"_%%");
    assert_eq!(types, [PatOne, PatAny, PatMatch]);
    assert!(DoMatchBinary("\u{80}%", &weights, &types));
}

/// 验证标签拼接、标识符转义、尾空格计数、UTF-8 位置与 glob '?' 转义边界。
#[test]
fn migration_helpers_match_go_edge_behavior() {
    let labels = HashMap::from([
        ("ccc".to_owned(), "ddd".to_owned()),
        ("aaa".to_owned(), "bbb".to_owned()),
    ]);
    assert_eq!(BuildStringFromLabels(&labels), "aaa=bbb,ccc=ddd");
    assert_eq!(Escape("foo `bar`", 0), "`foo ``bar``` ".trim_end());
    assert_eq!(Escape("foo \"bar\"", ModeANSIQuotes), r#""foo ""bar""""#);
    assert_eq!(GetTailSpaceCount("你  "), 2);
    assert_eq!(ConvertPosInUtf8("你好", 3), 2);
    assert_eq!(EscapeGlobQuestionMark("12?[?]"), r"12\?[\?]");
}

/// 验证 Utf8Len、TrimUtf8String 与 ASCII 大小写（含 escape 保护）原地修改行为。
#[test]
fn migration_utf8_and_ascii_mutation_match_go() {
    assert_eq!([Utf8Len(b'a'), Utf8Len(0xe4), Utf8Len(0xff)], [1, 3, 8]);
    let mut value = "你好ab".to_owned();
    assert_eq!(TrimUtf8String(&mut value, 2), 6);
    assert_eq!(value, "ab");

    let mut plain = "AbC你".as_bytes().to_vec();
    LowerOneString(&mut plain);
    assert_eq!(plain, "abc你".as_bytes());

    let mut upper_escape = b"AAAA".to_vec();
    assert_eq!(
        LowerOneStringExcludeEscapeChar(&mut upper_escape, b'A'),
        b'A'
    );
    assert_eq!(upper_escape, b"AaAa");
    let mut lower_escape = b"ABC".to_vec();
    assert_eq!(
        LowerOneStringExcludeEscapeChar(&mut lower_escape, b'a'),
        b'A'
    );
    assert_eq!(lower_escape, b"abc");
}

/// 验证 MemoizeStr 只缓存非空结果；首次空串后仍会再次调用闭包。
#[test]
fn migration_memoize_only_caches_non_empty_values() {
    let calls = Cell::new(0);
    let stringer = MemoizeStr(|| {
        calls.set(calls.get() + 1);
        if calls.get() == 1 {
            String::new()
        } else {
            "slow".to_owned()
        }
    });
    assert_eq!(stringer.String(), "");
    assert_eq!(stringer.String(), "slow");
    assert_eq!(stringer.String(), "slow");
    assert_eq!(calls.get(), 2);
}
