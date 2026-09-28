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

// 列类型格的 Unwrap / Compare / Join 单元测试。
//
// 对应 Go `type_test.go`：构造一组典型 `FieldType` fixture，验证类型序、
// 字符集/排序规则、标志位与 ENUM/SET 的兼容性规则。

use super::*;
use std::collections::HashMap;

/// binary charset/collation 字面量。
const BINARY: &str = "binary";

/// 用 builder 构造测试用 `FieldType`。
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

/// 命名 fixture：整数/字符串/小数/枚举等典型列类型。
fn fixtures() -> HashMap<&'static str, Box<types::FieldType>> {
    let mut map = HashMap::new();
    map.insert(
        "typeInt",
        field_type(mysql::TypeLong, 0, 11, 0, BINARY, BINARY, &[]),
    );
    map.insert(
        "typeIntNotNull",
        field_type(
            mysql::TypeLong,
            mysql::NoDefaultValueFlag | mysql::NotNullFlag,
            10,
            0,
            BINARY,
            BINARY,
            &[],
        ),
    );
    map.insert(
        "typeIntAutoIncrementUnique",
        field_type(
            mysql::TypeLong,
            mysql::AutoIncrementFlag | mysql::UniqueKeyFlag,
            11,
            0,
            BINARY,
            BINARY,
            &[],
        ),
    );
    map.insert(
        "typeIntNotNullKey",
        field_type(
            mysql::TypeLong,
            mysql::NoDefaultValueFlag | mysql::MultipleKeyFlag | mysql::NotNullFlag,
            11,
            0,
            BINARY,
            BINARY,
            &[],
        ),
    );
    map.insert(
        "typeInt1",
        field_type(mysql::TypeLong, 0, 1, 0, BINARY, BINARY, &[]),
    );
    map.insert(
        "typeInt22",
        field_type(mysql::TypeLong, 0, 22, 0, BINARY, BINARY, &[]),
    );
    map.insert(
        "typeBit4",
        field_type(
            mysql::TypeBit,
            mysql::UnsignedFlag,
            4,
            0,
            BINARY,
            BINARY,
            &[],
        ),
    );
    map.insert(
        "typeBigInt22ZeroFill",
        field_type(
            mysql::TypeLonglong,
            mysql::ZerofillFlag | mysql::UnsignedFlag,
            22,
            0,
            BINARY,
            BINARY,
            &[],
        ),
    );
    map.insert(
        "typeDate",
        field_type(
            mysql::TypeDate,
            mysql::BinaryFlag,
            10,
            0,
            BINARY,
            BINARY,
            &[],
        ),
    );
    map.insert(
        "typeDateTime3",
        field_type(
            mysql::TypeDatetime,
            mysql::BinaryFlag,
            23,
            3,
            BINARY,
            BINARY,
            &[],
        ),
    );
    map.insert(
        "typeTimestamp",
        field_type(
            mysql::TypeTimestamp,
            mysql::BinaryFlag,
            19,
            0,
            BINARY,
            BINARY,
            &[],
        ),
    );
    map.insert(
        "typeTime6",
        field_type(
            mysql::TypeDuration,
            mysql::BinaryFlag,
            17,
            6,
            BINARY,
            BINARY,
            &[],
        ),
    );
    map.insert(
        "typeYear4",
        field_type(
            mysql::TypeYear,
            mysql::ZerofillFlag | mysql::UnsignedFlag,
            4,
            0,
            BINARY,
            BINARY,
            &[],
        ),
    );
    map.insert(
        "typeChar123",
        field_type(mysql::TypeString, 0, 123, 0, "utf8mb4", "utf8mb4_bin", &[]),
    );
    map.insert(
        "typeVarchar10UTF8Bin",
        field_type(mysql::TypeVarchar, 0, 10, 0, "utf8", "utf8_bin", &[]),
    );
    map.insert(
        "typeVarchar10UTF8GeneralCI",
        field_type(mysql::TypeVarchar, 0, 10, 0, "utf8", "utf8_general_ci", &[]),
    );
    map.insert(
        "typeVarchar10UTF8MB4Bin",
        field_type(mysql::TypeVarchar, 0, 10, 0, "utf8mb4", "utf8mb4_bin", &[]),
    );
    map.insert(
        "typeVarchar10Latin1Bin",
        field_type(mysql::TypeVarchar, 0, 10, 0, "latin1", "latin1_bin", &[]),
    );
    map.insert(
        "typeVarchar65432CharsetASCII",
        field_type(mysql::TypeVarchar, 0, 65432, 0, "ascii", "ascii_bin", &[]),
    );
    map.insert(
        "typeVarBinary420",
        field_type(
            mysql::TypeVarchar,
            mysql::BinaryFlag,
            420,
            0,
            BINARY,
            BINARY,
            &[],
        ),
    );
    map.insert(
        "typeBinary69",
        field_type(
            mysql::TypeString,
            mysql::BinaryFlag,
            69,
            0,
            BINARY,
            BINARY,
            &[],
        ),
    );
    map.insert(
        "typeLongBlob",
        field_type(
            mysql::TypeLongBlob,
            mysql::BinaryFlag,
            0xffff_ffff,
            0,
            BINARY,
            BINARY,
            &[],
        ),
    );
    map.insert(
        "typeMediumText",
        field_type(
            mysql::TypeMediumBlob,
            0,
            0xffff_ffff,
            0,
            "utf8mb4",
            "utf8mb4_bin",
            &[],
        ),
    );
    map.insert(
        "typeDecimal16_8",
        field_type(mysql::TypeNewDecimal, 0, 16, 8, BINARY, BINARY, &[]),
    );
    map.insert(
        "typeDecimal",
        field_type(mysql::TypeNewDecimal, 0, 11, 0, BINARY, BINARY, &[]),
    );
    map.insert(
        "typeEnum5",
        field_type(
            mysql::TypeEnum,
            0,
            types::UnspecifiedLength,
            0,
            "utf8mb4",
            "utf8mb4_bin",
            &["tidb", "tikv", "tiflash", "golang", "rust"],
        ),
    );
    map.insert(
        "typeEnum2",
        field_type(
            mysql::TypeEnum,
            0,
            types::UnspecifiedLength,
            0,
            "utf8mb4",
            "utf8mb4_bin",
            &["tidb", "tikv"],
        ),
    );
    map.insert(
        "typeSet5",
        field_type(
            mysql::TypeSet,
            0,
            types::UnspecifiedLength,
            0,
            "utf8mb4",
            "utf8mb4_bin",
            &["tidb", "tikv", "tiflash", "golang", "rust"],
        ),
    );
    map.insert(
        "typeSet2",
        field_type(
            mysql::TypeSet,
            0,
            types::UnspecifiedLength,
            0,
            "utf8mb4",
            "utf8mb4_bin",
            &["tidb", "tikv"],
        ),
    );
    map.insert(
        "typeJSON",
        field_type(
            mysql::TypeJSON,
            mysql::BinaryFlag,
            0xffff_ffff,
            0,
            BINARY,
            BINARY,
            &[],
        ),
    );
    map
}

/// 从格元素解包出 `FieldType`。
fn unwrap_type(value: &dyn Lattice) -> types::FieldType {
    value
        .Unwrap()
        .downcast_ref::<types::FieldType>()
        .cloned()
        .expect("Type.Unwrap must return FieldType")
}

// TestTypeUnwrap 对应 Go 的 TestTypeUnwrap。
#[test]
/// 验证 `Type(...).Unwrap()` 保留原始字段类型。
fn test_type_unwrap() {
    for (name, field_type) in fixtures() {
        let wrapped = Type(field_type.as_ref());
        assert_eq!(unwrap_type(&wrapped), *field_type, "{name}");
    }
}

#[test]
/// Go 在 collation 的首个下划线前为空时，以完整 collation 作为 charset。
fn test_type_unwrap_leading_underscore_collation() {
    let field_type = field_type(mysql::TypeVarchar, 0, 10, 0, "_bin", "_bin", &[]);

    assert_eq!(unwrap_type(&Type(&field_type)).GetCharset(), "_bin");
}

// TestTypeCompareJoin 对应 Go 的 TestTypeCompareJoin。
#[test]
/// 表格驱动：覆盖 Compare/Join 成功与各类不相容错误。
fn test_type_compare_join() {
    let fixtures = fixtures();
    let get = |name: &str| fixtures.get(name).expect(name);

    let cases: &[(
        &str,
        &str,
        Option<i32>,
        Option<&str>,
        Option<&str>,
        Option<&str>,
    )] = &[
        (
            "typeInt",
            "typeInt22",
            Some(-1),
            None,
            Some("typeInt22"),
            None,
        ),
        ("typeInt1", "typeInt", Some(-1), None, Some("typeInt"), None),
        (
            "typeInt",
            "typeIntNotNull",
            Some(1),
            None,
            Some("typeInt"),
            None,
        ),
        (
            "typeVarchar10UTF8Bin",
            "typeVarchar10UTF8MB4Bin",
            Some(-1),
            None,
            Some("typeVarchar10UTF8MB4Bin"),
            None,
        ),
        (
            "typeVarchar10Latin1Bin",
            "typeVarchar10UTF8MB4Bin",
            Some(-1),
            None,
            Some("typeVarchar10UTF8MB4Bin"),
            None,
        ),
        (
            "typeVarchar10Latin1Bin",
            "typeVarchar10UTF8Bin",
            None,
            Some("incompatible charset"),
            Some("typeVarchar10UTF8MB4Bin"),
            None,
        ),
        (
            "typeVarchar10UTF8GeneralCI",
            "typeVarchar10UTF8MB4Bin",
            None,
            Some("incompatible collation"),
            None,
            Some("incompatible collation"),
        ),
        (
            "typeInt",
            "typeIntAutoIncrementUnique",
            None,
            Some("distinct singletons"),
            None,
            Some("distinct singletons"),
        ),
        (
            "typeIntNotNull",
            "typeIntAutoIncrementUnique",
            None,
            Some("combining contradicting orders"),
            None,
            Some("auto type but not defined as a key"),
        ),
        (
            "typeIntNotNullKey",
            "typeIntAutoIncrementUnique",
            None,
            Some("combining contradicting orders"),
            Some("__joined_auto_key__"),
            None,
        ),
        (
            "typeDecimal16_8",
            "typeDecimal",
            None,
            Some("distinct singletons"),
            None,
            Some("distinct singletons"),
        ),
        (
            "typeVarchar65432CharsetASCII",
            "typeVarBinary420",
            None,
            Some("distinct singletons"),
            None,
            Some("distinct singletons"),
        ),
        (
            "typeEnum5",
            "typeEnum2",
            Some(1),
            None,
            Some("typeEnum5"),
            None,
        ),
        (
            "typeSet2",
            "typeSet5",
            Some(-1),
            None,
            Some("typeSet5"),
            None,
        ),
        (
            "typeSet5",
            "typeEnum5",
            None,
            Some("incompatible mysql type"),
            None,
            Some("incompatible mysql type"),
        ),
    ];

    for (a_name, b_name, cmp, cmp_err, join, join_err) in cases {
        let a = Type(get(a_name));
        let b = Type(get(b_name));
        match (cmp, cmp_err) {
            (Some(expected), None) => {
                assert_eq!(a.Compare(&b).unwrap(), *expected, "{a_name} vs {b_name}");
                assert_eq!(b.Compare(&a).unwrap(), -*expected, "{b_name} vs {a_name}");
            }
            (None, Some(needle)) => {
                let err = a.Compare(&b).unwrap_err().to_string();
                assert!(err.contains(needle), "{a_name} vs {b_name}: {err}");
                let reverse_err = b.Compare(&a).unwrap_err().to_string();
                assert!(
                    reverse_err.contains(needle),
                    "{b_name} vs {a_name}: {reverse_err}"
                );
            }
            _ => panic!("invalid compare expectation"),
        }

        match (join, join_err) {
            (Some(join_name), None) if *join_name == "__joined_auto_key__" => {
                let joined = a.Join(&b).unwrap();
                let ft = unwrap_type(joined.as_ref());
                assert!(mysql::HasAutoIncrementFlag(ft.GetFlag()));
                assert!(
                    mysql::HasPriKeyFlag(ft.GetFlag())
                        || mysql::HasUniKeyFlag(ft.GetFlag())
                        || mysql::HasMultipleKeyFlag(ft.GetFlag())
                );
                let reverse_joined = b.Join(&a).unwrap();
                assert_eq!(
                    unwrap_type(reverse_joined.as_ref()),
                    unwrap_type(joined.as_ref())
                );
            }
            (Some(join_name), None) => {
                let expected = Type(get(join_name));
                let joined = a.Join(&b).unwrap();
                assert_eq!(unwrap_type(joined.as_ref()), unwrap_type(&expected));
                assert!(joined.Compare(&a).unwrap() >= 0);
                assert!(joined.Compare(&b).unwrap() >= 0);
                let reverse_joined = b.Join(&a).unwrap();
                assert_eq!(unwrap_type(reverse_joined.as_ref()), unwrap_type(&expected));
            }
            (None, Some(needle)) => {
                let err = match a.Join(&b) {
                    Ok(_) => panic!("{a_name} join {b_name} unexpectedly succeeded"),
                    Err(error) => error.to_string(),
                };
                assert!(err.contains(needle), "{a_name} join {b_name}: {err}");
                let reverse_err = match b.Join(&a) {
                    Ok(_) => panic!("{b_name} join {a_name} unexpectedly succeeded"),
                    Err(error) => error.to_string(),
                };
                assert!(
                    reverse_err.contains(needle),
                    "{b_name} join {a_name}: {reverse_err}"
                );
            }
            _ => panic!("invalid join expectation"),
        }
    }
}
