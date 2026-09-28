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

// 字符集/排序规则查询 API 的单元测试，对照 `charset_test.go`。
//
// 覆盖 ValidCharsetAndCollation、默认排序规则、字符集描述查询、
// 按名取排序规则、自定义字符集增删，以及 utf8mb3 别名归一化。

use parser_charset::charset::*;
use std::collections::HashMap;

/// TestValidCharset：表驱动校验字符集与排序规则组合是否合法。
#[test]
fn test_valid_charset() {
    for (charset, collation, expected) in [
        ("utf8", "utf8_general_ci", true),
        ("", "utf8_general_ci", true),
        ("utf8mb4", "utf8mb4_bin", true),
        ("latin1", "latin1_bin", true),
        ("utf8", "utf8_invalid_ci", false),
        ("utf16", "utf16_bin", false),
        ("gb2312", "gb2312_chinese_ci", false),
        ("UTF8", "UTF8_BIN", true),
        ("UTF8", "utf8_bin", true),
        ("UTF8MB4", "utf8mb4_bin", true),
        ("UTF8MB4", "UTF8MB4_bin", true),
        ("UTF8MB4", "UTF8MB4_general_ci", true),
        ("Utf8", "uTf8_bIN", true),
        ("utf8mb3", "", true),
        ("utf8mb3", "utf8mb3_bin", true),
        ("utf8mb3", "utf8mb3_general_ci", true),
        ("utf8mb3", "utf8mb3_unicode_ci", true),
    ] {
        assert_eq!(
            ValidCharsetAndCollation(charset, collation),
            expected,
            "{charset}/{collation}"
        );
    }
}

/// TestGetDefaultCollation：校验默认排序规则查询，并核对 IsDefault 与目录一致性。
#[test]
fn test_get_default_collation() {
    for (charset, expected) in [
        ("utf8", Some("utf8_bin")),
        ("UTF8", Some("utf8_bin")),
        ("utf8mb4", Some("utf8mb4_bin")),
        ("ascii", Some("ascii_bin")),
        ("binary", Some("binary")),
        ("latin1", Some("latin1_bin")),
        ("invalid_cs", None),
        ("", None),
    ] {
        let actual = GetDefaultCollation(charset);
        match expected {
            Some(expected) => assert_eq!(actual.unwrap(), expected),
            None => assert!(actual.is_err(), "{charset}"),
        }
    }

    let charsets = CharacterSetInfos.read().unwrap();
    let defaults: Vec<_> = GetSupportedCollations()
        .into_iter()
        .filter(|collation| collation.IsDefault)
        .collect();
    for collation in &defaults {
        assert_eq!(
            charsets[&collation.CharsetName].DefaultCollation,
            collation.Name
        );
    }
    assert_eq!(charsets.len(), defaults.len());
}

/// TestGetCharsetDesc：按名称取字符集元数据（大小写不敏感）。
#[test]
fn test_get_charset_desc() {
    for (charset, expected) in [
        ("utf8", Some("utf8")),
        ("UTF8", Some("utf8")),
        ("utf8mb4", Some("utf8mb4")),
        ("ascii", Some("ascii")),
        ("binary", Some("binary")),
        ("latin1", Some("latin1")),
        ("invalid_cs", None),
        ("", None),
    ] {
        let actual = GetCharsetInfo(charset);
        match expected {
            Some(expected) => assert_eq!(actual.unwrap().Name, expected),
            None => assert!(actual.is_err(), "{charset}"),
        }
    }
}

/// TestGetCollationByName：全量排序规则字段对齐，未知名返回 ddl:1273。
#[test]
fn test_get_collation_by_name() {
    for expected in all_collations_for_test() {
        let actual = GetCollationByName(&expected.Name).unwrap();
        assert_eq!(actual.ID, expected.ID);
        assert_eq!(actual.CharsetName, expected.CharsetName);
        assert_eq!(actual.Name, expected.Name);
        assert_eq!(actual.IsDefault, expected.IsDefault);
        assert_eq!(actual.Sortlen, expected.Sortlen);
        assert_eq!(actual.PadAttribute, expected.PadAttribute);
    }
    assert_eq!(
        GetCollationByName("non_exist").unwrap_err().to_string(),
        "[ddl:1273]Unknown collation: 'non_exist'"
    );
}

/// TestValidCustomCharset：在子进程中增删自定义字符集，避免污染全局注册表。
#[test]
fn test_valid_custom_charset() {
    const CHILD: &str = "TASK265_CUSTOM_CHARSET_CHILD";
    // 父进程再拉起自身一次，子进程内真正执行 Add/Remove，防止并行测试互相干扰。
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "charset_test::test_valid_custom_charset",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }

    AddCharset(Charset {
        Name: "custom".into(),
        DefaultCollation: "custom_collation".into(),
        Collations: HashMap::new(),
        Desc: "Custom".into(),
        Maxlen: 4,
    });
    AddCollation(Collation {
        ID: 99999,
        CharsetName: "custom".into(),
        Name: "custom_collation".into(),
        IsDefault: true,
        Sortlen: 8,
        PadAttribute: PadNone.into(),
    });

    assert!(ValidCharsetAndCollation("custom", "custom_collation"));
    assert!(!ValidCharsetAndCollation("utf8", "utf8_invalid_ci"));
    RemoveCharset("custom");
}

/// TestUTF8MB3：utf8mb3 作为 utf8 别名的默认排序规则与排序规则名归一化。
#[test]
fn test_utf8mb3() {
    assert_eq!(GetDefaultCollationLegacy("utf8mb3").unwrap(), "utf8_bin");
    assert_eq!(GetCharsetInfo("utf8mb3").unwrap().Name, "utf8");
    for (name, alias) in [
        ("utf8mb3_bin", "utf8_bin"),
        ("utf8mb3_general_ci", "utf8_general_ci"),
        ("utf8mb3_unicode_ci", "utf8_unicode_ci"),
    ] {
        assert_eq!(GetCollationByName(name).unwrap().Name, alias);
    }
}

/// 冒烟：对常见受支持字符集调用 GetCharsetInfo，对应 Go benchmark 入口形状。
#[test]
fn benchmark_get_charset_desc_smoke() {
    for charset in [
        CharsetUTF8,
        CharsetUTF8MB4,
        CharsetASCII,
        CharsetLatin1,
        CharsetBin,
    ] {
        assert!(GetCharsetInfo(charset).is_ok());
    }
}
