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

// 列类型格的 AsterSQL 迁移补充单元测试。
//
// 相对 Go 原测试，额外覆盖：整数/BLOB 语义序、编解码各维 round-trip、
// utf8mb3 规范化、AUTO_INCREMENT 无键拒绝、缺失列标志与标准默认值。

use super::*;

/// 键相关标志掩码，用于断言 join/缺失列调整结果。
const KEY_FLAGS: usize = mysql::PriKeyFlag | mysql::UniqueKeyFlag | mysql::MultipleKeyFlag;

/// `Ordering` → Go 风格 -1/0/1。
fn ordering(value: std::cmp::Ordering) -> i32 {
    match value {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// 构造测试用 `FieldType`。
fn field_type(
    tp: u8,
    flag: usize,
    flen: isize,
    decimal: isize,
    charset: &str,
    collate: &str,
    elems: &[&str],
) -> Box<types::FieldType> {
    Box::new(
        types::NewFieldTypeBuilder()
            .SetType(tp)
            .SetFlag(flag)
            .SetFlen(flen)
            .SetDecimal(decimal)
            .SetCharset(charset.to_owned())
            .SetCollate(collate.to_owned())
            .SetElems(elems.iter().map(|value| (*value).to_owned()).collect())
            .Build(),
    )
}

/// 从格元素解包 `FieldType`。
fn unwrap_type(value: &dyn Lattice) -> types::FieldType {
    value
        .Unwrap()
        .downcast_ref::<types::FieldType>()
        .cloned()
        .expect("Type.Unwrap must return FieldType")
}

#[test]
/// 整数与 BLOB 类型编号的语义序应与 Go 特殊分支一致。
fn mysql_type_orders_match_go_special_cases() {
    let integers = [
        mysql::TypeTiny,
        mysql::TypeShort,
        mysql::TypeInt24,
        mysql::TypeLong,
        mysql::TypeLonglong,
    ];
    for (left_index, left) in integers.iter().enumerate() {
        for (right_index, right) in integers.iter().enumerate() {
            assert_eq!(
                compareMySQLIntegerType(*left, *right),
                ordering(left_index.cmp(&right_index))
            );
        }
    }

    let blobs = [
        mysql::TypeTinyBlob,
        mysql::TypeBlob,
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
    ];
    for (left_index, left) in blobs.iter().enumerate() {
        for (right_index, right) in blobs.iter().enumerate() {
            assert_eq!(
                compareMySQLBlobType(*left, *right),
                ordering(left_index.cmp(&right_index))
            );
        }
    }
}

#[test]
/// `Type` 编解码后各维（含标志与 elems）应保持。
fn type_round_trips_all_encoded_dimensions() {
    let cases = [
        field_type(
            mysql::TypeLong,
            mysql::NoDefaultValueFlag | mysql::MultipleKeyFlag | mysql::NotNullFlag,
            11,
            0,
            "binary",
            "binary",
            &[],
        ),
        field_type(
            mysql::TypeNewDecimal,
            mysql::UnsignedFlag,
            16,
            8,
            "binary",
            "binary",
            &[],
        ),
        field_type(
            mysql::TypeEnum,
            0,
            -1,
            0,
            "utf8mb4",
            "utf8mb4_bin",
            &["tidb", "tikv", "tiflash"],
        ),
    ];

    for expected in cases {
        let wrapped = Type(&expected);
        assert_eq!(unwrap_type(&wrapped), *expected);
    }
}

#[test]
/// 排序规则大小写与 utf8mb3→utf8 规范化。
fn type_normalizes_collation_case_and_utf8mb3_like_go() {
    let original = field_type(mysql::TypeVarchar, 0, 10, 0, "UTF8MB3", "UTF8MB3_BIN", &[]);
    let unwrapped = unwrap_type(&Type(&original));
    assert_eq!(unwrapped.GetCharset(), "utf8");
    assert_eq!(unwrapped.GetCollate(), "utf8_bin");
}

#[test]
/// 标志位、字符集与 ENUM 前缀规则的 Compare/Join。
fn compare_and_join_cover_go_flag_charset_and_enum_rules() {
    let int = field_type(mysql::TypeLong, 0, 11, 0, "binary", "binary", &[]);
    let int_22 = field_type(mysql::TypeLong, 0, 22, 0, "binary", "binary", &[]);
    let left = Type(&int);
    let right = Type(&int_22);
    assert_eq!(left.Compare(&right).unwrap(), -1);
    assert_eq!(unwrap_type(left.Join(&right).unwrap().as_ref()), *int_22);

    let latin1 = field_type(mysql::TypeVarchar, 0, 10, 0, "latin1", "latin1_bin", &[]);
    let utf8 = field_type(mysql::TypeVarchar, 0, 10, 0, "utf8", "utf8_bin", &[]);
    let utf8mb4 = field_type(mysql::TypeVarchar, 0, 10, 0, "utf8mb4", "utf8mb4_bin", &[]);
    let latin1_type = Type(&latin1);
    let utf8_type = Type(&utf8);
    assert!(latin1_type.Compare(&utf8_type).is_err());
    assert_eq!(
        unwrap_type(latin1_type.Join(&utf8_type).unwrap().as_ref()),
        *utf8mb4
    );

    let enum_short = field_type(
        mysql::TypeEnum,
        0,
        -1,
        0,
        "utf8mb4",
        "utf8mb4_bin",
        &["tidb", "tikv"],
    );
    let enum_long = field_type(
        mysql::TypeEnum,
        0,
        -1,
        0,
        "utf8mb4",
        "utf8mb4_bin",
        &["tidb", "tikv", "tiflash"],
    );
    assert_eq!(Type(&enum_short).Compare(&Type(&enum_long)).unwrap(), -1);
    assert_eq!(
        unwrap_type(Type(&enum_short).Join(&Type(&enum_long)).unwrap().as_ref()),
        *enum_long
    );
}

#[test]
/// join 后仍无键的 AUTO_INCREMENT 必须报错。
fn auto_increment_without_joined_key_is_rejected() {
    let not_null = field_type(
        mysql::TypeLong,
        mysql::NoDefaultValueFlag | mysql::NotNullFlag,
        10,
        0,
        "binary",
        "binary",
        &[],
    );
    let auto_unique = field_type(
        mysql::TypeLong,
        mysql::AutoIncrementFlag | mysql::UniqueKeyFlag,
        11,
        0,
        "binary",
        "binary",
        &[],
    );
    let error = match Type(&not_null).Join(&Type(&auto_unique)) {
        Ok(_) => panic!("AUTO_INCREMENT without a joined key must fail"),
        Err(error) => error,
    };
    assert!(error.to_string().contains(ErrMsgAutoTypeWithoutKey));
}

#[test]
/// DECIMAL 精度/标度单点冲突与不同 collation 后缀不相容。
fn decimal_dimensions_and_collation_suffixes_remain_incompatible() {
    let decimal_16_8 = field_type(mysql::TypeNewDecimal, 0, 16, 8, "binary", "binary", &[]);
    let decimal_11_0 = field_type(mysql::TypeNewDecimal, 0, 11, 0, "binary", "binary", &[]);
    let decimal_error = Type(&decimal_16_8)
        .Compare(&Type(&decimal_11_0))
        .unwrap_err();
    assert!(decimal_error.to_string().contains("distinct singletons"));

    let general_ci = field_type(mysql::TypeVarchar, 0, 10, 0, "utf8", "utf8_general_ci", &[]);
    let utf8mb4_bin = field_type(mysql::TypeVarchar, 0, 10, 0, "utf8mb4", "utf8mb4_bin", &[]);
    let collation_error = Type(&general_ci)
        .Join(&Type(&utf8mb4_bin))
        .err()
        .expect("different collation suffixes must not join");
    assert!(
        collation_error
            .to_string()
            .contains("incompatible collation")
    );
}

#[test]
/// `setFlagForMissingColumn` / `setAntiKeyFlags` 与 Go 语义一致。
fn missing_column_flag_adjustments_match_go() {
    let source = field_type(
        mysql::TypeLong,
        mysql::NoDefaultValueFlag | mysql::NotNullFlag | mysql::UniqueKeyFlag,
        11,
        0,
        "binary",
        "binary",
        &[],
    );
    let mut wrapped = Type(&source);
    assert!(!wrapped.hasDefault());
    assert!(wrapped.isNotNull());
    assert!(wrapped.setFlagForMissingColumn());

    let adjusted = unwrap_type(&wrapped);
    assert_eq!(adjusted.GetFlag() & mysql::NoDefaultValueFlag, 0);
    assert_eq!(adjusted.GetFlag() & KEY_FLAGS, 0);

    wrapped.setAntiKeyFlags(mysql::MultipleKeyFlag);
    assert_eq!(
        unwrap_type(&wrapped).GetFlag() & KEY_FLAGS,
        mysql::MultipleKeyFlag
    );
}

#[test]
/// 标准默认值（datetime/duration/binary/enum）与 Go 一致。
fn missing_column_defaults_match_go_values() {
    let datetime = field_type(
        mysql::TypeDatetime,
        mysql::NotNullFlag,
        23,
        3,
        "binary",
        "binary",
        &[],
    );
    let duration = field_type(
        mysql::TypeDuration,
        mysql::NotNullFlag,
        17,
        6,
        "binary",
        "binary",
        &[],
    );
    let binary = field_type(
        mysql::TypeString,
        mysql::BinaryFlag,
        4,
        0,
        "binary",
        "binary",
        &[],
    );
    let enum_type = field_type(
        mysql::TypeEnum,
        0,
        -1,
        0,
        "utf8mb4",
        "utf8mb4_bin",
        &["tidb", "tikv"],
    );

    assert_eq!(
        *Type(&datetime)
            .getStandardDefaultValue()
            .downcast::<String>()
            .unwrap(),
        "0000-00-00 00:00:00.000"
    );
    assert_eq!(
        *Type(&duration)
            .getStandardDefaultValue()
            .downcast::<String>()
            .unwrap(),
        "00:00:00.000000"
    );
    assert_eq!(
        Type(&binary)
            .getStandardDefaultValue()
            .downcast::<String>()
            .unwrap()
            .as_bytes(),
        &[0, 0, 0, 0]
    );
    assert_eq!(
        *Type(&enum_type)
            .getStandardDefaultValue()
            .downcast::<String>()
            .unwrap(),
        "tidb"
    );
}
