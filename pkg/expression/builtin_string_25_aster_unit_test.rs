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

// 字符串内建标量语义与 Go 对齐的单元测试。
//
// 覆盖反转/修剪/填充、ORD/QUOTE/HEX、子串定位插入、集合选择与翻译、
// Base64、数值格式化、CONCAT/REPEAT，以及构造元数据与字符集转换。
// 多数函数区分二进制（按字节）与 UTF-8（按字符/rune）两条签名路径。

use crate::expression_builtin_string::*;

#[test]
/// 验证反转、TRIM、LPAD/RPAD 的字符边界与 Go 一致。
fn reverse_trim_and_padding_match_go_character_boundaries() {
    assert_eq!(reverseBytes(b"TiDB".to_vec()), b"BDiT");
    assert_eq!(reverseRunes("数据库".chars().collect()), ['库', '据', '数']);

    assert_eq!(trimLeft("xxxbarxxx", "x"), "barxxx");
    assert_eq!(trimRight("barxxyz", "xyz"), "barx");
    assert_eq!(trimBoth("xxxbarxxx", "x"), "bar");
    assert_eq!(trimBoth("   \tbar\n     ", " "), "\tbar\n");
    assert_eq!(trimLeft("unchanged", ""), "unchanged");

    assert_eq!(lpadUtf8("你好", 5, "ab"), Some("aba你好".to_owned()));
    assert_eq!(rpadUtf8("你好", 5, "ab"), Some("你好aba".to_owned()));
    assert_eq!(lpadUtf8("abc", 2, "x"), Some("ab".to_owned()));
    assert_eq!(rpadUtf8("abc", 5, ""), Some(String::new()));
}

#[test]
/// 验证 ORD/QUOTE/HEX/BIN/OCT 等格式化向量。
fn ord_quote_hex_and_binary_formats_match_go_vectors() {
    assert_eq!(calcOrd(b"2"), 50);
    assert_eq!(calcOrd("你".as_bytes()), 14_990_752);
    assert_eq!(calcOrd("👍".as_bytes()), 4_036_989_325);
    assert_eq!(calcOrd(b""), 0);

    assert_eq!(Quote(r"Don\'t!"), r"'Don\\\'t!'");
    assert_eq!(Quote("\0\u{1a}"), r"'\0\Z'");
    assert_eq!(Quote("萌萌哒😊"), "'萌萌哒😊'");

    assert_eq!(hexString("你好".as_bytes()), "E4BDA0E5A5BD");
    assert_eq!(hexInt(-1), "FFFFFFFFFFFFFFFF");
    assert_eq!(unhex("126"), Some(vec![0x01, 0x26]));
    assert_eq!(unhex("string"), None);
    assert_eq!(bin(-1), "1".repeat(64));
    assert_eq!(oct(-1), "1777777777777777777777");
}

#[test]
/// 验证子串、定位、插入与 INSTR 的一基下标规则。
fn substring_locate_insert_and_instr_keep_go_one_based_rules() {
    assert_eq!(substringUtf8("你好世界", 2, Some(2)), "好世");
    assert_eq!(substringUtf8("abcdef", -2, None), "ef");
    assert_eq!(substringUtf8("abcdef", 0, Some(3)), "");
    assert_eq!(substringIndex("www.mysql.com", ".", 2), "www.mysql");
    assert_eq!(substringIndex("www.mysql.com", ".", -2), "mysql.com");

    assert_eq!(locateUtf8("好", "你好吗好", 1, false), 2);
    assert_eq!(locateUtf8("A", "你a好", 1, true), 2);
    assert_eq!(locateBinary(b"a", b"\0a", 1), 2);

    assert_eq!(
        insertUtf8("这是TiDB", 3, 4, "AsterSQL", 64),
        Some("这是AsterSQL".to_owned())
    );
    assert_eq!(insertUtf8("abc", 0, 1, "x", 64), Some("abc".to_owned()));
    assert_eq!(insertUtf8("abc", 2, -1, "x", 64), Some("ax".to_owned()));
    assert_eq!(insertUtf8("abc", 2, 1, "toolong", 4), None);
    assert_eq!(instrUtf8("数据库TiDB", "TiDB", false), 4);
    assert_eq!(instrUtf8("Database", "BASE", true), 5);
    assert_eq!(instrBinary(b"\0abc", b"ab"), 2);
}

#[test]
/// 验证 EXPORT_SET/FIND_IN_SET/ELT/TRANSLATE 的顺序语义。
fn set_selection_and_translation_follow_go_ordering() {
    assert_eq!(exportSet(5, "Y", "N", ",", 4), "Y,N,Y,N");
    assert_eq!(exportSet(-6, "Y", "N", ",", 5), "N,Y,N,Y,Y");
    assert_eq!(
        exportSet(5, "Y", "N", ",", -1),
        exportSet(5, "Y", "N", ",", 64)
    );

    assert_eq!(findInSet("b", "a,b,c", false), 2);
    assert_eq!(findInSet("B", "a,b,c", true), 2);
    assert_eq!(findInSet("a,b", "a,b,c", false), 0);
    assert_eq!(makeSet(0b101, &[Some("a"), None, Some("c")]), "a,c");
    assert_eq!(
        elt(3, &[Some("Hej"), Some("ej"), Some("Heja")]),
        Some("Heja")
    );
    assert_eq!(elt(0, &[Some("Hej")]), None);

    assert_eq!(translateUtf8("12345", "143", "ax"), "a2x5");
    assert_eq!(translateUtf8("aaaa", "aa", "xy"), "xxxx");
    assert_eq!(translateBinary(b"12345", b"143", b"ax"), b"a2x5");
}

#[test]
/// 验证 Base64 长度预算、换行与非法输入处理。
fn base64_lengths_wrapping_and_invalid_input_match_go() {
    assert_eq!(base64NeededDecodedLength(8), Some(6));
    assert_eq!(base64NeededEncodedLength(3), Some(4));
    assert_eq!(base64NeededEncodedLength(58), Some(81));

    assert_eq!(toBase64(b"abc", 4), Some("YWJj".to_owned()));
    assert_eq!(toBase64(b"abc", 3), None);
    let long = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    assert_eq!(
        toBase64(long, 89),
        Some("QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVphYmNkZWZnaGlqa2xtbm9wcXJzdHV2d3h5ejAxMjM0\nNTY3ODkrLw==".to_owned())
    );
    assert_eq!(fromBase64(" YW\tJj ", 8), Some(b"abc".to_vec()));
    assert_eq!(fromBase64("asc", 8), None);
    assert_eq!(splitToSubN("abcdefgh", 3), vec!["abc", "def", "gh"]);
}

#[test]
/// 验证 FORMAT 舍入、REPEAT/SPACE/CONCAT 边界情况。
fn numeric_rounding_repeat_and_concat_preserve_edge_cases() {
    assert_eq!(roundFormatArgs("1234.567", 2), "1234.577");
    assert_eq!(roundFormatArgs("-99.999", 2), "-100.009");
    assert_eq!(roundFormatArgs("12", 4), "12");

    assert_eq!(
        concat(&[Some("a"), Some(""), Some("中")]),
        Some("a中".to_owned())
    );
    assert_eq!(concat(&[Some("a"), None]), None);
    assert_eq!(
        concatWS(Some(","), &[Some("a"), None, Some("b")]),
        Some("a,b".to_owned())
    );
    assert_eq!(concatWS(None, &[Some("a")]), None);
    assert_eq!(repeat("ab", 3, 6), Some("ababab".to_owned()));
    assert_eq!(repeat("ab", 4, 7), None);
    assert_eq!(space(3, 3), Some("   ".to_owned()));
    assert_eq!(space(-1, 3), Some(String::new()));
}

#[test]
/// 验证函数规格元数据、CHAR、CONVERT、FORMAT locale 与 WEIGHT_STRING。
fn construction_metadata_char_format_load_and_weight_match_go() {
    assert_eq!(STRING_FUNCTION_SPECS.len(), 44);
    assert!(verifyStringFunctionArgs("substring", 2));
    assert!(verifyStringFunctionArgs("SUBSTRING", 3));
    assert!(!verifyStringFunctionArgs("substring", 4));
    let insert = stringFunctionSpec("insert").unwrap();
    assert!(insert.packet_sensitive);
    assert!(insert.has_binary_and_utf8_signatures);
    assert_eq!(insert.return_kind, StringReturnKind::String);

    assert_eq!(
        charFromInts(&[Some(77), Some(121), Some(83), Some(81), Some(76)]),
        b"MySQL"
    );
    assert_eq!(charFromInts(&[Some(0x4e2d)]), vec![0x4e, 0x2d]);
    assert_eq!(charFromInts(&[Some(-1)]), vec![0xff; 4]);

    assert_eq!(
        convertCharset(b"haha", "utf8", "binary"),
        Some(b"haha".to_vec())
    );
    assert_eq!(
        convertCharset(b"\xd6\xd0", "binary", "gbk"),
        Some("中".as_bytes().to_vec())
    );
    assert_eq!(
        convertCharset("中文\n".as_bytes(), "binary", "utf8"),
        Some("中文\n".as_bytes().to_vec())
    );
    assert_eq!(convertCharset(b"haha", "utf8", "wrongcharset"), None);

    assert_eq!(
        formatByLocale("1234567.89", 2, Some("en_US")),
        ("1,234,567.89".to_owned(), true)
    );
    assert_eq!(
        formatByLocale("7654321.98", 2, Some("de_DE")),
        ("7.654.321,98".to_owned(), true)
    );
    assert_eq!(
        formatByLocale("98765", 0, Some("sv_SE")),
        ("98 765".to_owned(), true)
    );
    assert_eq!(
        formatByLocale("4567890.123", 2, Some("de_CH")),
        ("4'567'890.12".to_owned(), true)
    );
    assert_eq!(
        formatByLocale("1234567890.123", 3, Some("en_IN")),
        ("1,23,45,67,890.123".to_owned(), true)
    );
    assert_eq!(
        formatByLocale("12345.67", 2, Some("unknown")),
        ("12,345.67".to_owned(), false)
    );
    assert_eq!(
        formatByLocale("9.999", 2, Some("es_ES")),
        ("10,00".to_owned(), true)
    );

    assert_eq!(loadFile("/etc/passwd"), None);
    assert_eq!(weightStringBinary(b"a ", None, false), b"a");
    assert_eq!(weightStringBinary(b"a", Some(5), true), b"a");
    assert_eq!(
        weightStringBinary("中".as_bytes(), Some(2), false),
        vec![0xe4, 0xb8]
    );
    assert_eq!(weightStringBinary(b"a", Some(5), false), b"a\0\0\0\0");
}

/// 供 Go 同名迁移入口复用的完整字符串标量回归集合。
pub(crate) fn run_string_parity_suite() {
    reverse_trim_and_padding_match_go_character_boundaries();
    ord_quote_hex_and_binary_formats_match_go_vectors();
    substring_locate_insert_and_instr_keep_go_one_based_rules();
    set_selection_and_translation_follow_go_ordering();
    base64_lengths_wrapping_and_invalid_input_match_go();
    numeric_rounding_repeat_and_concat_preserve_edge_cases();
    construction_metadata_char_format_load_and_weight_match_go();
}
