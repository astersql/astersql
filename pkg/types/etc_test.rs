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

// `etc` 类型分类与浮点舍入/截断行为的单元测试。
//
// 对照 Go `etc_test.go`，覆盖类型名映射、EOF 归一化、
// MaxFloat/Round/TruncateFloat，以及 NeedRestoredData 校对规则判定。

// 对照 pkg/types/etc_test.go，覆盖类型分类、EOF 归一化及浮点舍入/截断。
//

#![allow(dead_code)]
#![allow(non_snake_case)]

use crate::field::{
    ErrOverflow, FieldType, GetMaxFloat, IsBinaryStr, NewFieldType, Round, RoundFloat,
    TruncateFloat,
};
use crate::metadata::{
    EOFAsNil, IsNonBinaryStr, IsTemporalWithDate, IsTypeBlob, IsTypeChar, IsTypeFractionable,
    IsTypeNumeric, IsTypePrefixable, IsTypeTemporal, NeedRestoredData, TypeStr, TypeToStr, charset,
    errors, mysql, terror,
};

// testIsTypeBlob 对应 Go 辅助函数，保留 IsTypeBlob 的单例断言形状。
fn testIsTypeBlob(tp: u8, expect: bool) {
    let v = IsTypeBlob(tp);
    assert_eq!(expect, v);
}

// testIsTypeChar 对应 Go 辅助函数，保留 IsTypeChar 的单例断言形状。
fn testIsTypeChar(tp: u8, expect: bool) {
    let v = IsTypeChar(tp);
    assert_eq!(expect, v);
}

// TestIsType 覆盖 blob/char 类型判定，最后一个用非 blob/char 类型确认 false 分支。
#[test]
fn TestIsType() {
    testIsTypeBlob(mysql::TypeTinyBlob, true);
    testIsTypeBlob(mysql::TypeMediumBlob, true);
    testIsTypeBlob(mysql::TypeBlob, true);
    testIsTypeBlob(mysql::TypeLongBlob, true);
    testIsTypeBlob(mysql::TypeInt24, false);

    testIsTypeChar(mysql::TypeString, true);
    testIsTypeChar(mysql::TypeVarchar, true);
    testIsTypeChar(mysql::TypeLong, false);
}

// testTypeStr 对应 Go 辅助函数，直接检查 TypeStr 映射。
fn testTypeStr(tp: u8, expect: &str) {
    let v = TypeStr(tp);
    assert_eq!(expect, v);
}

// testTypeToStr 对应 Go 辅助函数，额外保留 charset 对 blob/string 显示名的影响。
fn testTypeToStr(tp: u8, charset: &str, expect: &str) {
    let v = TypeToStr(tp, charset);
    assert_eq!(expect, v);
}

// TestTypeToStr covers TypeStr and TypeToStr MySQL type name mappings.
#[test]
fn TestTypeToStr() {
    testTypeStr(mysql::TypeYear, "year");
    testTypeStr(0xdd, "");

    testTypeToStr(mysql::TypeBlob, "utf8", "text");
    testTypeToStr(mysql::TypeLongBlob, "utf8", "longtext");
    testTypeToStr(mysql::TypeTinyBlob, "utf8", "tinytext");
    testTypeToStr(mysql::TypeMediumBlob, "utf8", "mediumtext");
    testTypeToStr(mysql::TypeVarchar, "binary", "varbinary");
    testTypeToStr(mysql::TypeString, "binary", "binary");
    testTypeToStr(mysql::TypeTiny, "binary", "tinyint");
    testTypeToStr(mysql::TypeBlob, "binary", "blob");
    testTypeToStr(mysql::TypeLongBlob, "binary", "longblob");
    testTypeToStr(mysql::TypeTinyBlob, "binary", "tinyblob");
    testTypeToStr(mysql::TypeMediumBlob, "binary", "mediumblob");
    testTypeToStr(mysql::TypeVarchar, "utf8", "varchar");
    testTypeToStr(mysql::TypeString, "utf8", "char");
    testTypeToStr(mysql::TypeShort, "binary", "smallint");
    testTypeToStr(mysql::TypeInt24, "binary", "mediumint");
    testTypeToStr(mysql::TypeLong, "binary", "int");
    testTypeToStr(mysql::TypeLonglong, "binary", "bigint");
    testTypeToStr(mysql::TypeFloat, "binary", "float");
    testTypeToStr(mysql::TypeDouble, "binary", "double");
    testTypeToStr(mysql::TypeYear, "binary", "year");
    testTypeToStr(mysql::TypeDuration, "binary", "time");
    testTypeToStr(mysql::TypeDatetime, "binary", "datetime");
    testTypeToStr(mysql::TypeDate, "binary", "date");
    testTypeToStr(mysql::TypeTimestamp, "binary", "timestamp");
    testTypeToStr(mysql::TypeNewDecimal, "binary", "decimal");
    testTypeToStr(mysql::TypeUnspecified, "binary", "unspecified");
    testTypeToStr(0xdd, "binary", "");
    testTypeToStr(mysql::TypeBit, "binary", "bit");
    testTypeToStr(mysql::TypeEnum, "binary", "enum");
    testTypeToStr(mysql::TypeSet, "binary", "set");
}

// TestEOFAsNil 对应 Go 测试：io.EOF 被转换为 nil，普通错误保持原消息。
#[test]
fn TestEOFAsNil() {
    let eof = std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "EOF");
    assert!(EOFAsNil(Some(errors::SharedError::new(eof))).is_none());
    let err = EOFAsNil(Some(errors::New("test"))).expect("ordinary error must be preserved");
    assert_eq!("test", err.to_string());
}

struct MaxFloatCase {
    flen: i32,
    decimal: i32,
    expect: f64,
}

// TestMaxFloat 保留 Go 表驱动测试，用 flen/decimal 计算可表示最大浮点值。
#[test]
fn TestMaxFloat() {
    let tests = vec![
        MaxFloatCase {
            flen: 3,
            decimal: 2,
            expect: 9.99,
        },
        MaxFloatCase {
            flen: 5,
            decimal: 2,
            expect: 999.99,
        },
        MaxFloatCase {
            flen: 10,
            decimal: 1,
            expect: 999999999.9,
        },
        MaxFloatCase {
            flen: 5,
            decimal: 5,
            expect: 0.99999,
        },
    ];

    for test in tests {
        assert_eq!(test.expect, GetMaxFloat(test.flen, test.decimal));
    }
}

struct RoundFloatCase {
    input: f64,
    expect: f64,
}

// TestRoundFloat 验证 Go RoundFloat 的 bankers rounding 行为，正负 .5 都按原期望保留。
#[test]
fn TestRoundFloat() {
    let tests = vec![
        RoundFloatCase {
            input: 2.5,
            expect: 2.0,
        },
        RoundFloatCase {
            input: 1.5,
            expect: 2.0,
        },
        RoundFloatCase {
            input: 0.5,
            expect: 0.0,
        },
        RoundFloatCase {
            input: 0.49999999999999997,
            expect: 0.0,
        },
        RoundFloatCase {
            input: 0.0,
            expect: 0.0,
        },
        RoundFloatCase {
            input: -0.49999999999999997,
            expect: 0.0,
        },
        RoundFloatCase {
            input: -0.5,
            expect: 0.0,
        },
        RoundFloatCase {
            input: -2.5,
            expect: -2.0,
        },
        RoundFloatCase {
            input: -1.5,
            expect: -2.0,
        },
    ];

    for test in tests {
        assert_eq!(test.expect, RoundFloat(test.input));
    }
}

struct RoundCase {
    input: f64,
    dec: i32,
    expect: f64,
}

// TestRound 覆盖带小数位参数的 Round，包括负精度向十位舍入的分支。
#[test]
fn TestRound() {
    let tests = vec![
        RoundCase {
            input: -1.23,
            dec: 0,
            expect: -1.0,
        },
        RoundCase {
            input: -1.58,
            dec: 0,
            expect: -2.0,
        },
        RoundCase {
            input: 1.58,
            dec: 0,
            expect: 2.0,
        },
        RoundCase {
            input: 1.298,
            dec: 1,
            expect: 1.3,
        },
        RoundCase {
            input: 1.298,
            dec: 0,
            expect: 1.0,
        },
        RoundCase {
            input: 23.298,
            dec: -1,
            expect: 20.0,
        },
    ];

    for test in tests {
        assert_eq!(test.expect, Round(test.input, test.dec));
    }
}

struct TruncateFloatCase {
    input: f64,
    flen: i32,
    decimal: i32,
    expect: f64,
    overflow: bool,
}

// TestTruncateFloat 保留数值结果和 terror.ErrorEqual 的错误等价检查。
#[test]
fn TestTruncateFloat() {
    let tests = vec![
        TruncateFloatCase {
            input: 100.114,
            flen: 10,
            decimal: 2,
            expect: 100.11,
            overflow: false,
        },
        TruncateFloatCase {
            input: 100.115,
            flen: 10,
            decimal: 2,
            expect: 100.12,
            overflow: false,
        },
        TruncateFloatCase {
            input: 100.1156,
            flen: 10,
            decimal: 3,
            expect: 100.116,
            overflow: false,
        },
        TruncateFloatCase {
            input: 100.1156,
            flen: 3,
            decimal: 1,
            expect: 99.9,
            overflow: true,
        },
        TruncateFloatCase {
            input: 1.36,
            flen: 10,
            decimal: 2,
            expect: 1.36,
            overflow: false,
        },
    ];

    for test in tests {
        let (f, err) = TruncateFloat(test.input, test.flen, test.decimal);
        assert_eq!(test.expect, f);
        assert_eq!(test.overflow, err.is_some(), "err: {err:?}");
        if let Some(err) = err {
            let expected = errors::SharedError::new((**ErrOverflow).clone());
            assert!(terror::ErrorEqual(Some(&err), Some(&expected)));
        }
    }
}

// TestIsTypeTemporal 对应 Go 测试，四个时间类型和 NewDate 返回 true，普通字符返回 false。
#[test]
fn TestIsTypeTemporal() {
    assert!(IsTypeTemporal(mysql::TypeDuration));
    assert!(IsTypeTemporal(mysql::TypeDatetime));
    assert!(IsTypeTemporal(mysql::TypeTimestamp));
    assert!(IsTypeTemporal(mysql::TypeDate));
    assert!(IsTypeTemporal(mysql::TypeNewDate));
    assert!(!IsTypeTemporal(b't'));
}

// TestIsBinaryStr 复现 Go 中 FieldType 的逐步变更：bit 类型即使 bin collation 也不是 binary string，blob 才是。
#[test]
fn TestIsBinaryStr() {
    let mut input = FieldType::default();
    input.SetType(mysql::TypeBit);
    input.SetFlag(mysql::UnsignedFlag);
    input.SetFlen(1);
    input.SetDecimal(0);
    input.SetCharset(charset::CharsetUTF8.to_owned());
    input.SetCollate(charset::CollationUTF8.to_owned());

    input.SetCollate(charset::CollationUTF8.to_owned());
    assert!(!IsBinaryStr(&input));

    input.SetCollate(charset::CollationBin.to_owned());
    assert!(!IsBinaryStr(&input));

    input.SetType(mysql::TypeBlob);
    assert!(IsBinaryStr(&input));
}

// TestIsNonBinaryStr 修正 Go 原测试误调 IsBinaryStr 的覆盖缺口，直接验证目标函数。
#[test]
fn TestIsNonBinaryStr() {
    let mut input = NewFieldType(mysql::TypeBit);
    input.SetFlag(mysql::UnsignedFlag);
    input.SetFlen(1);
    input.SetDecimal(0);
    input.SetCharset(charset::CharsetUTF8.to_owned());
    input.SetCollate(charset::CollationUTF8.to_owned());

    input.SetCollate(charset::CollationBin.to_owned());
    assert!(!IsNonBinaryStr(&input));

    input.SetCollate(charset::CollationUTF8.to_owned());
    assert!(!IsNonBinaryStr(&input));

    input.SetType(mysql::TypeBlob);
    assert!(IsNonBinaryStr(&input));
}

// TestIsTemporalWithDate 检查带日期部分的时间类型，Duration 不在该测试范围内。
#[test]
fn TestIsTemporalWithDate() {
    assert!(IsTemporalWithDate(mysql::TypeDatetime));
    assert!(IsTemporalWithDate(mysql::TypeDate));
    assert!(IsTemporalWithDate(mysql::TypeTimestamp));
    assert!(!IsTemporalWithDate(b't'));
}

// TestIsTypePrefixable 对应 Go 测试：普通字符不可前缀索引，Blob 类型可前缀索引。
#[test]
fn TestIsTypePrefixable() {
    assert!(!IsTypePrefixable(b't'));
    assert!(IsTypePrefixable(mysql::TypeBlob));
}

/// 校验可带 FSP 的时间类型判定。
#[test]
fn TestIsTypeFractionable() {
    assert!(IsTypeFractionable(mysql::TypeDatetime));
    assert!(IsTypeFractionable(mysql::TypeDuration));
    assert!(IsTypeFractionable(mysql::TypeTimestamp));
    assert!(!IsTypeFractionable(b't'));
}

/// 校验数值族类型判定。
#[test]
fn TestIsTypeNumeric() {
    for tp in [
        mysql::TypeBit,
        mysql::TypeTiny,
        mysql::TypeInt24,
        mysql::TypeLong,
        mysql::TypeLonglong,
        mysql::TypeNewDecimal,
        mysql::TypeFloat,
        mysql::TypeDouble,
        mysql::TypeShort,
    ] {
        assert!(IsTypeNumeric(tp), "type {tp}");
    }
    assert!(!IsTypeNumeric(mysql::TypeUnspecified));
    assert!(!IsTypeNumeric(b't'));
}

/// 表驱动校验不同 charset/collation 下是否需要 restored data。
#[test]
fn TestNeedRestoredData() {
    let cases = [
        (mysql::TypeString, "binary", "binary", false),
        (mysql::TypeVarString, "binary", "binary", false),
        (mysql::TypeString, "utf8mb4", "utf8mb4_bin", false),
        (mysql::TypeVarString, "utf8mb4", "utf8mb4_bin", true),
        (mysql::TypeString, "utf8mb4", "utf8mb4_general_ci", true),
        (mysql::TypeVarString, "utf8mb4", "utf8mb4_general_ci", true),
        (mysql::TypeString, "utf8mb4", "utf8mb4_unicode_ci", true),
        (mysql::TypeVarString, "utf8mb4", "utf8mb4_unicode_ci", true),
        (mysql::TypeString, "utf8mb4", "utf8mb4_0900_ai_ci", true),
        (mysql::TypeVarString, "utf8mb4", "utf8mb4_0900_ai_ci", true),
        (mysql::TypeString, "utf8mb4", "utf8mb4_0900_bin", false),
        (mysql::TypeVarString, "utf8mb4", "utf8mb4_0900_bin", false),
        (mysql::TypeString, "gbk", "gbk_bin", true),
        (mysql::TypeVarString, "gbk", "gbk_bin", true),
        (mysql::TypeString, "gbk", "gbk_chinese_ci", true),
        (mysql::TypeVarString, "gbk", "gbk_chinese_ci", true),
        (mysql::TypeString, "gb18030", "gb18030_bin", true),
        (mysql::TypeVarString, "gb18030", "gb18030_bin", true),
        (mysql::TypeString, "gb18030", "gb18030_chinese_ci", true),
        (mysql::TypeVarString, "gb18030", "gb18030_chinese_ci", true),
    ];

    for (tp, charset, collate, expected) in cases {
        let mut ft = NewFieldType(tp);
        ft.SetCharset(charset.to_owned());
        ft.SetCollate(collate.to_owned());
        assert_eq!(
            expected,
            NeedRestoredData(&ft),
            "type {tp}, {charset}/{collate}"
        );
    }
}
