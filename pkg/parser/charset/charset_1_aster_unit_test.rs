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
// See the License for the specific language governing permissions and
// limitations under the License.

// 字符集元数据与编码实现的 Aster 集成单元测试。
//
// 覆盖默认/别名查询、受支持列表排序、Unsupported vs Unknown 错误区分，
// 以及 ASCII/二进制编码与 GB18030 补充映射、大小写特例。

use parser_charset::charset::*;
use parser_charset::encoding_ascii::*;
use parser_charset::encoding_bin::*;
use parser_charset::encoding_gb18030_data::*;
use parser_charset::{OpReplaceNoErr, TransformResult};

/// 校验默认排序规则、utf8mb3 别名与大小写不敏感查询。
#[test]
fn charset_metadata_matches_go_defaults_and_aliases() {
    assert_eq!(GetDefaultCollation("UTF8").unwrap(), "utf8_bin");
    assert_eq!(GetDefaultCollationLegacy("utf8mb3").unwrap(), "utf8_bin");
    assert!(ValidCharsetAndCollation("", "utf8_general_ci"));
    assert!(ValidCharsetAndCollation("utf8mb3", "UTF8MB3_UNICODE_CI"));
    assert!(!ValidCharsetAndCollation("utf16", "utf16_bin"));
    assert_eq!(GetCollationByName("UTF8MB3_BIN").unwrap().Name, "utf8_bin");
}

/// 校验受支持字符集按名称排序，且受支持排序规则数量与 ID 查询正确。
#[test]
fn supported_metadata_is_initialized_and_sorted() {
    let charsets = GetSupportedCharsets();
    let names: Vec<_> = charsets
        .iter()
        .map(|charset| charset.Name.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "ascii", "binary", "gb18030", "gbk", "latin1", "utf8", "utf8mb4"
        ]
    );

    let collations = GetSupportedCollations();
    assert_eq!(collations.len(), 7);
    assert!(
        collations
            .iter()
            .any(|collation| collation.Name == "gb18030_bin")
    );
    assert_eq!(GetCollationByID(46).unwrap().Name, "utf8mb4_bin");
}

/// 校验“已知但不支持”与“完全未知”返回不同错误文案，对齐 Go 语义。
#[test]
fn unknown_charset_and_collation_errors_keep_go_distinction() {
    assert!(
        GetCharsetInfo("utf16")
            .unwrap_err()
            .to_string()
            .contains("Unsupported charset utf16")
    );
    assert!(
        GetCharsetInfo("not-a-charset")
            .unwrap_err()
            .to_string()
            .contains("Unknown charset not-a-charset")
    );
    assert!(
        GetCollationByName("not-a-collation")
            .unwrap_err()
            .to_string()
            .contains("Unknown collation")
    );
}

/// 校验 ASCII 编码的合法性、foreach 分块与非法字符替换为 `?`。
#[test]
fn ascii_validation_foreach_and_replacement_match_go() {
    init_encoding_ascii();
    let ascii = ENCODING_ASCII_IMPL.get().unwrap();
    assert!(ascii.is_valid(b"qwerty"));
    assert!(!ascii.is_valid("qwÊrty".as_bytes()));

    let mut chunks = Vec::new();
    ascii.foreach("a中b".as_bytes(), OpReplaceNoErr, |from, to, ok| {
        chunks.push((from.to_vec(), to.to_vec(), ok));
        true
    });
    assert_eq!(
        chunks,
        vec![
            (b"a".to_vec(), b"a".to_vec(), true),
            ("中".as_bytes().to_vec(), "中".as_bytes().to_vec(), false),
            (b"b".to_vec(), b"b".to_vec(), true)
        ]
    );

    let transformed = ascii
        .transform(None, "qwÊrty".as_bytes(), OpReplaceNoErr)
        .unwrap();
    assert_eq!(transformed.as_slice(), b"qw?rty");
}

/// 校验二进制编码对任意字节零拷贝透传（Borrowed 结果指针相同）。
#[test]
fn binary_encoding_is_a_zero_copy_byte_passthrough() {
    init_encoding_bin();
    let binary = ENCODING_BIN_IMPL.get().unwrap();
    let input = [0, 0xff, b'a'];
    assert!(binary.is_valid(&input));
    match binary.transform(None, &input, OpReplaceNoErr).unwrap() {
        TransformResult::Borrowed(output) => assert!(std::ptr::eq(output.as_ptr(), input.as_ptr())),
        TransformResult::Owned(_) => panic!("binary transform must borrow its input"),
    }
}

/// 校验 GB18030 补充映射表 Unicode↔码点双向一致。
#[test]
fn gb18030_supplemental_maps_are_bidirectional() {
    assert_eq!(unicode_to_gb18030().get(&'€'), Some(&0xA2E3));
    assert_eq!(gb18030_to_unicode().get(&0xA2E3), Some(&'€'));
    assert_eq!(unicode_to_gb18030().len(), GB18030_ENCODING_LIST.len());
}

/// 校验 GB18030 大小写转换对 µ/μ 等特例覆盖 Unicode 默认行为。
#[test]
fn gb18030_special_case_overrides_unicode_defaults() {
    assert_eq!(gb18030_case().to_upper("µa"), "µA");
    assert_eq!(gb18030_case().to_lower("µA"), "μa");
}
