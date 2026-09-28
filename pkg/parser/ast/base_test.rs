// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// `AstNode` 文本设置与二进制字面量转换的单元测试。
//
// 覆盖 UTF-8/GBK 解码、可打印保留、不可打印转 `0x`、注释跳过、
// NO_BACKSLASH_ESCAPES，以及较大 WHERE 子句的转换工作负载。
use crate::base::AstNode;
use parser_charset::{CharsetGBK, CharsetUTF8, FindEncoding};

/// 辅助：按编码设置原文后断言 Text() 结果。
fn assert_text(encoding: &str, input: &[u8], expected: &str) {
    let mut node = AstNode::default();
    node.SetText(FindEncoding(encoding), input);
    assert_eq!(node.Text(), expected, "input={input:?}");
}

#[test]
/// 核对 SetText 后 Text/OriginalText 在 UTF-8 与 GBK 下正确。
fn test_node_set_text() {
    let cases: &[(&str, &[u8], &str, &[u8])] = &[
        (CharsetUTF8, "你好".as_bytes(), "你好", "你好".as_bytes()),
        (CharsetGBK, b"\xd2\xbb", "一", b"\xd2\xbb"),
        (CharsetGBK, b"\xc1\xd0", "列", b"\xc1\xd0"),
    ];
    let mut node = AstNode::default();
    for (encoding, input, expected_utf8, expected_original) in cases {
        node.SetText(FindEncoding(encoding), input);
        assert_eq!(node.Text(), *expected_utf8);
        assert_eq!(node.OriginalText(), *expected_original);
    }
}

#[test]
/// Go 的 nil encoding 契约：不解码、不规范化二进制字面量，原样返回文本。
fn test_node_set_text_without_encoding() {
    let mut node = AstNode::default();
    node.SetText(None, b"SELECT '\x00'");
    assert_eq!(node.Text().as_bytes(), b"SELECT '\x00'");
    assert_eq!(node.OriginalText(), b"SELECT '\x00'");
}

#[test]
/// 核对可打印字面量保持不变、不可打印转为十六进制，含前缀与转义。
fn test_binary_string_literal_conversion() {
    let printable: &[(&str, &[u8], &str)] = &[
        (
            "single-quoted",
            b"SELECT 'hello world'",
            "SELECT 'hello world'",
        ),
        (
            "double-quoted",
            b"SELECT \"hello world\"",
            "SELECT \"hello world\"",
        ),
        (
            "_binary prefix",
            b"SELECT _binary 'hello world'",
            "SELECT _binary 'hello world'",
        ),
        (
            "_utf8 prefix",
            b"SELECT _utf8'hello world'",
            "SELECT _utf8'hello world'",
        ),
        (
            "_utf8mb4 prefix",
            b"SELECT _utf8mb4'hello world'",
            "SELECT _utf8mb4'hello world'",
        ),
        (
            "N prefix",
            b"SELECT N'hello world'",
            "SELECT N'hello world'",
        ),
        (
            "escaped quotes",
            b"SELECT 'it''s here'",
            "SELECT 'it''s here'",
        ),
        (
            "escaped slash quote",
            b"SELECT 'it\\'s here'",
            "SELECT 'it\\'s here'",
        ),
        (
            "escaped double quotes",
            b"SELECT \"say \"\"hi\"\"\"",
            "SELECT \"say \"\"hi\"\"\"",
        ),
        (
            "backtick",
            b"SELECT 'has `backtick` inside'",
            "SELECT 'has `backtick` inside'",
        ),
        (
            "binary word",
            b"SELECT 'the word _binary appears'",
            "SELECT 'the word _binary appears'",
        ),
        (
            "backslashes",
            b"SELECT 'path\\\\to\\\\file'",
            "SELECT 'path\\\\to\\\\file'",
        ),
    ];
    for (name, input, expected) in printable {
        assert_text(CharsetUTF8, input, expected);
        assert!(!name.is_empty());
    }

    let binary: &[(&str, &[u8], &str)] = &[
        ("single", b"SELECT '\xd2\xe4\xa6\xb8'", "SELECT 0xd2e4a6b8"),
        (
            "double",
            b"SELECT \"\xd2\xe4\xa6\xb8\"",
            "SELECT 0xd2e4a6b8",
        ),
        (
            "binary prefix",
            b"SELECT _binary '\xd2\xe4\xa6\xb8'",
            "SELECT _binary 0xd2e4a6b8",
        ),
        (
            "binary no space",
            b"SELECT _binary'\x01'",
            "SELECT _binary 0x01",
        ),
        ("utf8 no space", b"SELECT _utf8'\x01'", "SELECT _utf8 0x01"),
        (
            "utf8mb4 no space",
            b"SELECT _utf8mb4'\x01'",
            "SELECT _utf8mb4 0x01",
        ),
        ("doubled quote", b"SELECT '\xd2''\xe4'", "SELECT 0xd227e4"),
        ("slash quote", b"SELECT '\xd2\\'\xe4'", "SELECT 0xd227e4"),
        (
            "double doubled",
            b"SELECT \"\xd2\"\"\xe4\"",
            "SELECT 0xd222e4",
        ),
        ("backtick", b"SELECT '\xd2`\xe4'", "SELECT 0xd260e4"),
        (
            "mixed",
            b"SELECT '\xd2\xe4', 'hello', _binary '\xa1\xb2'",
            "SELECT 0xd2e4, 'hello', _binary 0xa1b2",
        ),
        (
            "truncated utf8",
            b"SELECT '\xf0\x9f\x98'",
            "SELECT 0xf09f98",
        ),
        ("continuation", b"SELECT '\x80\x81'", "SELECT 0x8081"),
        ("nul", b"SELECT '\x00'", "SELECT 0x00"),
        (
            "mixed control",
            b"SELECT 'hello\x00world'",
            "SELECT 0x68656c6c6f00776f726c64",
        ),
        (
            "controls",
            b"SELECT '\x01\x02\x03\x04\x05'",
            "SELECT 0x0102030405",
        ),
    ];
    for (name, input, expected) in binary {
        assert_text(CharsetUTF8, input, expected);
        assert!(!name.is_empty());
    }
}

#[test]
/// 核对行/块注释内的引号不触发转换；可执行注释 `/*!`/`/*+` 内仍转换。
fn test_binary_string_literal_skips_comments() {
    let cases: &[(&[u8], &str)] = &[
        (b"-- don't do this\nSELECT 'hello' FROM t", "-- don't do this\nSELECT 'hello' FROM t"),
        (b"-- SELECT * FROM t WHERE name='John'\nSELECT 1", "-- SELECT * FROM t WHERE name='John'\nSELECT 1"),
        (b"-- see table \"users\"\nSELECT \"bar\" FROM t", "-- see table \"users\"\nSELECT \"bar\" FROM t"),
        (b"SELECT 1 -- don't", "SELECT 1 -- don't"),
        (b"-- ending with '\nSELECT 'hello'", "-- ending with '\nSELECT 'hello'"),
        (b"SELECT 1 --1", "SELECT 1 --1"),
        (b"# user's config\nSELECT 'value' FROM t", "# user's config\nSELECT 'value' FROM t"),
        (b"/* it's a test */ SELECT 'value' FROM t", "/* it's a test */ SELECT 'value' FROM t"),
        (b"/*\n * don't modify\n */ SELECT 'value' FROM t", "/*\n * don't modify\n */ SELECT 'value' FROM t"),
        (b"--\x0c don't\nSELECT 'hello' FROM t", "--\u{c} don't\nSELECT 'hello' FROM t"),
        (b"--\x0b don't\nSELECT 'hello' FROM t", "--\u{b} don't\nSELECT 'hello' FROM t"),
        (b"/*!80000 SELECT '\xd2\xe4' */", "/*!80000 SELECT 0xd2e4 */"),
        (b"/*+ SET_VAR(charset='\xd2\xe4') */ SELECT 1", "/*+ SET_VAR(charset=0xd2e4) */ SELECT 1"),
        (b"/*T![unsupported] don't */ SELECT 'hello' FROM t", "/*T![unsupported] don't */ SELECT 'hello' FROM t"),
        (b"/*M! don't */ SELECT 'hello' FROM t", "/*M! don't */ SELECT 'hello' FROM t"),
        (b"-- (don't use parenthesis)\n\nCREATE OR REPLACE VIEW v AS SELECT 'Attribute' AS t FROM t1 UNION ALL SELECT 'Reference' AS t FROM t2", "-- (don't use parenthesis)\n\nCREATE OR REPLACE VIEW v AS SELECT 'Attribute' AS t FROM t1 UNION ALL SELECT 'Reference' AS t FROM t2"),
        (b"-- don't\nSELECT '\xd2\xe4' FROM t", "-- don't\nSELECT 0xd2e4 FROM t"),
    ];
    for (input, expected) in cases {
        assert_text(CharsetUTF8, input, expected);
    }
}

#[test]
/// 核对 NO_BACKSLASH_ESCAPES 下反斜杠不作为转义，且仍可转换不可打印串。
fn test_binary_string_literal_no_backslash_escapes() {
    let mut node = AstNode::default();
    node.SetText(FindEncoding(CharsetUTF8), b"SELECT '\\n'");
    node.SetNoBackslashEscapes(true);
    assert_eq!(node.Text(), "SELECT '\\n'");
    node.SetText(FindEncoding(CharsetUTF8), b"SELECT '\\' , 'after'");
    node.SetNoBackslashEscapes(true);
    assert_eq!(node.Text(), "SELECT '\\' , 'after'");
    node.SetText(FindEncoding(CharsetUTF8), b"SELECT '\xd2\xe4'");
    node.SetNoBackslashEscapes(true);
    assert_eq!(node.Text(), "SELECT 0xd2e4");
}

#[test]
/// 核对 GBK 编码下可打印汉字保留、非法字节转十六进制。
fn test_binary_string_literal_gbk() {
    let cases: &[(&[u8], &str)] = &[
        (b"select '\xb1\xed1'", "select '表1'"),
        (b"select '\x80\xff'", "select 0x80ff"),
        (b"select '\xb9\x5c'", "select '筡'"),
        (b"select '\xb9\x5c\xc5\x5c'", "select '筡臷'"),
        (b"select '\xb9\x5c', 'after'", "select '筡', 'after'"),
    ];
    for (input, expected) in cases {
        assert_text(CharsetGBK, input, expected);
    }
}

/// 构造重复 OR 条件的大查询，用于转换性能/正确性冒烟。
fn build_query(clause: &[u8], count: usize) -> Vec<u8> {
    let mut query = b"SELECT * FROM t1 WHERE ".to_vec();
    for index in 0..count {
        if index > 0 {
            query.extend_from_slice(b" OR ");
        }
        query.extend_from_slice(clause);
    }
    query
}

#[test]
/// 对多种子句长度运行转换，断言结果仍以 SELECT 前缀开头。
fn benchmark_convert_binary_string_literals_workloads() {
    let clauses: &[&[u8]] = &[
        b"c1 = 12345",
        b"c1 = 'hello world'",
        b"c1 = _binary '\xd2\xe4\xa6\xb8\xc1\xf3\xe5\xd7\xa9\xb2\xc4\xd6\xe8\xf1\xa3\xb5'",
    ];
    for clause in clauses {
        for count in [1, 200] {
            let input = build_query(clause, count);
            let mut node = AstNode::default();
            node.SetText(FindEncoding(CharsetUTF8), &input);
            assert!(node.Text().starts_with("SELECT * FROM t1 WHERE "));
        }
    }
}

/// Public statement nodes own source text independently of parser lifetime.
#[test]
fn public_node_text_setters_preserve_encoding_and_clone_state() {
    use crate::Node;
    let mut node = crate::SetStmt::default();
    node.SetText(Some(FindEncoding(CharsetUTF8)), b"SELECT '\\n\x01'");
    assert_eq!(node.OriginalText(), b"SELECT '\\n\x01'");
    assert_eq!(node.Text(), "SELECT 0x0a01");
    node.SetNoBackslashEscapes(true);
    assert_eq!(node.Text(), "SELECT 0x5c6e01");
    let cloned = node.clone();
    node.SetText(Some(FindEncoding(CharsetUTF8)), b"SELECT 2");
    assert_eq!(cloned.Text(), "SELECT 0x5c6e01");
    assert_eq!(node.Text(), "SELECT 2");

    let mut erased: Box<dyn Node> = Box::new(crate::SelectStmt::default());
    erased.SetText(Some(FindEncoding(CharsetUTF8)), "SELECT 你好".as_bytes());
    assert_eq!(erased.Text(), "SELECT 你好");
    assert_eq!(erased.OriginalText(), "SELECT 你好".as_bytes());
    erased.SetText(None, "SELECT 你好".as_bytes());
    assert_eq!(erased.Text(), "SELECT 你好");
    let concrete = erased.into_any().downcast::<crate::SelectStmt>().unwrap();
    assert_eq!(concrete.Text(), "SELECT 你好");
}
