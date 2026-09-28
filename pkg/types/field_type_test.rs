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

// FieldType（字段类型描述符）格式化、默认推断与聚合规则的单元测试。
//
// 对照 Go `pkg/types/field_type_test.go`：覆盖 `String`/`InfoSchemaStr`、
// `DefaultTypeForValue`、`AggFieldType` 与 `AggregateEvalType`。
// FieldType 描述列的 MySQL 类型、显示宽度、小数位、字符集与标志位。

// 对照 pkg/types/field_type_test.go，覆盖 FieldType 格式化、推断和聚合规则。
//

#![allow(dead_code)]
#![allow(non_snake_case)]

use crate::field::*;
use crate::metadata::{charset, mysql};

/// 各类 MySQL 类型的 `FieldType::String` / `InfoSchemaStr` 回归。
// TestFieldType 对应 Go 的 FieldType 字符串化回归测试。
// 每个代码段都保留原 Go 中对 flen、decimal、flag、charset/collation 或 elems 的设置顺序。
#[test]
fn TestFieldType() {
    let mut ft = NewFieldType(mysql::TypeDuration);
    assert_eq!(UnspecifiedLength, ft.GetFlen());
    assert_eq!(UnspecifiedLength, ft.GetDecimal());

    ft.SetDecimal(5);
    assert_eq!("time(5)", ft.String());

    ft = NewFieldType(mysql::TypeLong);
    ft.SetFlen(5);
    ft.SetFlen(5);
    ft.SetFlag(mysql::UnsignedFlag | mysql::ZerofillFlag);
    assert_eq!("int(5) UNSIGNED ZEROFILL", ft.String());
    assert_eq!("int(5) unsigned", ft.InfoSchemaStr());

    // Float/Double 的默认 flen 和默认 decimal 组合在 Go 中需要分别覆盖。
    let float_cases = vec![
        (mysql::TypeFloat, 12, 3, "float(12,3)"),
        (mysql::TypeFloat, 12, -1, "float"),
        (mysql::TypeFloat, 5, -1, "float"),
        (mysql::TypeFloat, 7, 3, "float(7,3)"),
        (mysql::TypeDouble, 22, 3, "double(22,3)"),
        (mysql::TypeDouble, 22, -1, "double"),
        (mysql::TypeDouble, 5, -1, "double"),
        (mysql::TypeDouble, 7, 3, "double(7,3)"),
    ];
    for (tp, flen, decimal, expect) in float_cases {
        ft = NewFieldType(tp);
        ft.SetFlen(flen);
        ft.SetDecimal(decimal);
        assert_eq!(expect, ft.String());
    }

    ft = NewFieldType(mysql::TypeBlob);
    ft.SetFlen(10);
    ft.SetCharset("UTF8".to_owned());
    ft.SetCollate("UTF8_UNICODE_GI".to_owned());
    assert_eq!(
        "text CHARACTER SET UTF8 COLLATE UTF8_UNICODE_GI",
        ft.String()
    );

    ft = NewFieldType(mysql::TypeVarchar);
    ft.SetFlen(10);
    ft.AddFlag(mysql::BinaryFlag);
    assert_eq!(
        "varchar(10) BINARY CHARACTER SET utf8mb4 COLLATE utf8mb4_bin",
        ft.String()
    );

    ft = NewFieldType(mysql::TypeString);
    ft.SetCharset(charset::CollationBin.to_owned());
    ft.AddFlag(mysql::BinaryFlag);
    assert_eq!("binary(1) COLLATE utf8mb4_bin", ft.String());

    let elem_cases = vec![
        (mysql::TypeEnum, vec!["a", "b"], "enum('a','b')"),
        (mysql::TypeEnum, vec!["'a'", "'b'"], "enum('''a''','''b''')"),
        (
            mysql::TypeEnum,
            vec!["a\nb", "a\tb", "a\rb"],
            "enum('a\\nb','a\tb','a\\rb')",
        ),
        (
            mysql::TypeEnum,
            vec!["a\nb", "a'\t\r\nb", "a\rb"],
            "enum('a\\nb','a''\t\\r\\nb','a\\rb')",
        ),
        (mysql::TypeSet, vec!["a", "b"], "set('a','b')"),
        (mysql::TypeSet, vec!["'a'", "'b'"], "set('''a''','''b''')"),
        (
            mysql::TypeSet,
            vec!["a\nb", "a'\t\r\nb", "a\rb"],
            "set('a\\nb','a''\t\\r\\nb','a\\rb')",
        ),
        (
            mysql::TypeSet,
            vec!["a'\nb", "a'b\tc"],
            "set('a''\\nb','a''b\tc')",
        ),
    ];
    for (tp, elems, expect) in elem_cases {
        // Go 这里用 SetElems 校验 enum/set 元素转义，包括引号、制表符、回车和换行。
        ft = NewFieldType(tp);
        ft.SetElems(elems.into_iter().map(str::to_owned).collect());
        assert_eq!(expect, ft.String());
    }

    let temporal_cases = vec![
        (mysql::TypeTimestamp, 8, 2, "timestamp(2)"),
        (mysql::TypeTimestamp, 8, 0, "timestamp"),
        (mysql::TypeDatetime, 8, 2, "datetime(2)"),
        (mysql::TypeDatetime, 8, 0, "datetime"),
        (mysql::TypeDate, 8, 2, "date"),
        (mysql::TypeDate, 8, 0, "date"),
        (mysql::TypeYear, 4, 0, "year(4)"),
        (mysql::TypeYear, 2, 2, "year(2)"),
    ];
    for (tp, flen, decimal, expect) in temporal_cases {
        // 最后一组 year(2) 对应 Go 注释中的 invalid year 场景，仍按字符串化结果断言。
        ft = NewFieldType(tp);
        ft.SetFlen(flen);
        ft.SetDecimal(decimal);
        assert_eq!(expect, ft.String());
    }
}

/// `DefaultTypeForValue` 表驱动用例期望的 FieldType 属性。
struct DefaultTypeForValueCase {
    tp: u8,
    flen: isize,
    decimal: isize,
    charset: &'static str,
    collation: &'static str,
    flag: usize,
}

/// 按值推断默认 FieldType（类型、flen、charset、flag 等）。
// TestDefaultTypeForValue 保留 Go 表驱动测试的每个输入表达式和期望 FieldType 属性。
// value 用字符串标记原 Go 表达式，后续接线时再替换成真实 datum/字面量对象。
#[test]
fn TestDefaultTypeForValue() {
    use std::any::Any;
    let bin = mysql::BinaryFlag;
    let not_null = mysql::NotNullFlag;
    let unsigned = mysql::UnsignedFlag;
    let case = |tp, flen, decimal, charset, collation, flag| DefaultTypeForValueCase {
        tp,
        flen,
        decimal,
        charset,
        collation,
        flag,
    };
    let tests: Vec<(Option<Box<dyn Any>>, DefaultTypeForValueCase)> = vec![
        (
            None,
            case(
                mysql::TypeNull,
                0,
                0,
                charset::CharsetBin,
                charset::CharsetBin,
                bin,
            ),
        ),
        (
            Some(Box::new(1_i32)),
            case(
                mysql::TypeLonglong,
                1,
                0,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | not_null,
            ),
        ),
        (
            Some(Box::new(0_i32)),
            case(
                mysql::TypeLonglong,
                1,
                0,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | not_null,
            ),
        ),
        (
            Some(Box::new(432_i32)),
            case(
                mysql::TypeLonglong,
                3,
                0,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | not_null,
            ),
        ),
        (
            Some(Box::new(4321_i32)),
            case(
                mysql::TypeLonglong,
                4,
                0,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | not_null,
            ),
        ),
        (
            Some(Box::new(1_234_567_i32)),
            case(
                mysql::TypeLonglong,
                7,
                0,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | not_null,
            ),
        ),
        (
            Some(Box::new(12_345_678_i32)),
            case(
                mysql::TypeLonglong,
                8,
                0,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | not_null,
            ),
        ),
        (
            Some(Box::new(12_345_678_901_234_567_i64)),
            case(
                mysql::TypeLonglong,
                17,
                0,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | not_null,
            ),
        ),
        (
            Some(Box::new(-42_i32)),
            case(
                mysql::TypeLonglong,
                3,
                0,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | not_null,
            ),
        ),
        (
            Some(Box::new(1_u64)),
            case(
                mysql::TypeLonglong,
                1,
                0,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | unsigned | not_null,
            ),
        ),
        (
            Some(Box::new(123_u64)),
            case(
                mysql::TypeLonglong,
                3,
                0,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | unsigned | not_null,
            ),
        ),
        (
            Some(Box::new(1234_u64)),
            case(
                mysql::TypeLonglong,
                4,
                0,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | unsigned | not_null,
            ),
        ),
        (
            Some(Box::new(1_234_567_u64)),
            case(
                mysql::TypeLonglong,
                7,
                0,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | unsigned | not_null,
            ),
        ),
        (
            Some(Box::new(12_345_678_u64)),
            case(
                mysql::TypeLonglong,
                8,
                0,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | unsigned | not_null,
            ),
        ),
        (
            Some(Box::new(12_345_678_901_234_567_u64)),
            case(
                mysql::TypeLonglong,
                17,
                0,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | unsigned | not_null,
            ),
        ),
        (
            Some(Box::new("abc".to_owned())),
            case(
                mysql::TypeVarString,
                3,
                UnspecifiedLength,
                charset::CharsetUTF8MB4,
                charset::CollationUTF8MB4,
                not_null,
            ),
        ),
        (
            Some(Box::new(1.1_f64)),
            case(
                mysql::TypeDouble,
                3,
                -1,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | not_null,
            ),
        ),
        (
            Some(Box::new(b"abc".to_vec())),
            case(
                mysql::TypeBlob,
                3,
                UnspecifiedLength,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | not_null,
            ),
        ),
        (
            Some(Box::new(HexLiteral::default())),
            case(
                mysql::TypeVarString,
                0,
                0,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | unsigned | not_null,
            ),
        ),
        (
            Some(Box::new(BitLiteral::default())),
            case(
                mysql::TypeVarString,
                0,
                0,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | not_null,
            ),
        ),
        (
            Some(Box::new(Time::new(mysql::TypeDatetime, DefaultFsp))),
            case(
                mysql::TypeDatetime,
                19,
                0,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | not_null,
            ),
        ),
        (
            Some(Box::new(Time::new(mysql::TypeDatetime, 3))),
            case(
                mysql::TypeDatetime,
                23,
                3,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | not_null,
            ),
        ),
        (
            Some(Box::new(Duration::default())),
            case(
                mysql::TypeDuration,
                8,
                0,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | not_null,
            ),
        ),
        (
            Some(Box::new(MyDecimal::default())),
            case(
                mysql::TypeNewDecimal,
                2,
                0,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | not_null,
            ),
        ),
        (
            Some(Box::new(Enum {
                Name: "a".to_owned(),
                Value: 1,
            })),
            case(
                mysql::TypeEnum,
                1,
                UnspecifiedLength,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | not_null,
            ),
        ),
        (
            Some(Box::new(Set {
                Name: "a".to_owned(),
                Value: 1,
            })),
            case(
                mysql::TypeSet,
                1,
                UnspecifiedLength,
                charset::CharsetBin,
                charset::CharsetBin,
                bin | not_null,
            ),
        ),
    ];

    for (i, (value, tt)) in tests.iter().enumerate() {
        let mut ft = FieldType::default();
        // Go 调用 DefaultTypeForValue(tt.value, &ft, mysql.DefaultCharset, mysql.DefaultCollationName)。
        DefaultTypeForValue(
            value.as_deref(),
            &mut ft,
            mysql::DefaultCharset,
            mysql::DefaultCollationName,
        );
        assert_eq!(tt.tp, ft.GetType(), "{} {:?} {:?}", i, ft.GetType(), tt.tp);
        assert_eq!(
            tt.flen,
            ft.GetFlen(),
            "{} {:?} {:?}",
            i,
            ft.GetFlen(),
            tt.flen
        );
        assert_eq!(
            tt.charset,
            ft.GetCharset(),
            "{} {:?} {:?}",
            i,
            ft.GetCharset(),
            tt.charset
        );
        assert_eq!(
            tt.decimal,
            ft.GetDecimal(),
            "{} {:?} {:?}",
            i,
            ft.GetDecimal(),
            tt.decimal
        );
        assert_eq!(
            tt.collation,
            ft.GetCollate(),
            "{} {:?} {:?}",
            i,
            ft.GetCollate(),
            tt.collation
        );
        assert_eq!(
            tt.flag,
            ft.GetFlag(),
            "{} {:?} {:?}",
            i,
            ft.GetFlag(),
            tt.flag
        );
    }
}

/// 聚合测试用的全量 MySQL 字段类型列表（顺序影响相邻提升断言）。
// all_field_type_cases 对应 Go 测试中反复使用的 fts 列表，顺序决定聚合测试的相邻类型提升。
fn all_field_type_cases() -> Vec<Box<FieldType>> {
    vec![
        NewFieldType(mysql::TypeUnspecified),
        NewFieldType(mysql::TypeTiny),
        NewFieldType(mysql::TypeShort),
        NewFieldType(mysql::TypeLong),
        NewFieldType(mysql::TypeFloat),
        NewFieldType(mysql::TypeDouble),
        NewFieldType(mysql::TypeNull),
        NewFieldType(mysql::TypeTimestamp),
        NewFieldType(mysql::TypeLonglong),
        NewFieldType(mysql::TypeInt24),
        NewFieldType(mysql::TypeDate),
        NewFieldType(mysql::TypeDuration),
        NewFieldType(mysql::TypeDatetime),
        NewFieldType(mysql::TypeYear),
        NewFieldType(mysql::TypeNewDate),
        NewFieldType(mysql::TypeVarchar),
        NewFieldType(mysql::TypeBit),
        NewFieldType(mysql::TypeJSON),
        NewFieldType(mysql::TypeNewDecimal),
        NewFieldType(mysql::TypeEnum),
        NewFieldType(mysql::TypeSet),
        NewFieldType(mysql::TypeTinyBlob),
        NewFieldType(mysql::TypeMediumBlob),
        NewFieldType(mysql::TypeLongBlob),
        NewFieldType(mysql::TypeBlob),
        NewFieldType(mysql::TypeVarString),
        NewFieldType(mysql::TypeString),
        NewFieldType(mysql::TypeGeometry),
    ]
}

/// 将 `Box<FieldType>` 切片转为 `&FieldType` 引用切片。
fn refs(values: &[Box<FieldType>]) -> Vec<&FieldType> {
    values.iter().map(Box::as_ref).collect()
}

/// 聚合 FieldType：单类型、同型成对、与 Long / JSON 配对的提升规则。
// TestAggFieldType 对应 Go 的聚合 FieldType 测试：单类型、同类型成对、与 Long 聚合、与 JSON 聚合。
#[test]
fn TestAggFieldType() {
    let fts = all_field_type_cases();

    for i in 0..fts.len() {
        let mut aggTp = AggFieldType(&[fts[i].as_ref()]);
        assert_eq!(fts[i].GetType(), aggTp.GetType());

        aggTp = AggFieldType(&[fts[i].as_ref(), fts[i].as_ref()]);
        match fts[i].GetType() {
            mysql::TypeDate => assert_eq!(mysql::TypeDate, aggTp.GetType()),
            mysql::TypeJSON => assert_eq!(mysql::TypeJSON, aggTp.GetType()),
            mysql::TypeEnum | mysql::TypeSet | mysql::TypeVarString => {
                assert_eq!(mysql::TypeVarchar, aggTp.GetType())
            }
            mysql::TypeUnspecified => assert_eq!(mysql::TypeNewDecimal, aggTp.GetType()),
            _ => assert_eq!(fts[i].GetType(), aggTp.GetType()),
        }

        let long = NewFieldType(mysql::TypeLong);
        aggTp = AggFieldType(&[fts[i].as_ref(), long.as_ref()]);
        match fts[i].GetType() {
            mysql::TypeTiny
            | mysql::TypeShort
            | mysql::TypeLong
            | mysql::TypeYear
            | mysql::TypeInt24
            | mysql::TypeNull => assert_eq!(mysql::TypeLong, aggTp.GetType()),
            mysql::TypeLonglong => assert_eq!(mysql::TypeLonglong, aggTp.GetType()),
            mysql::TypeFloat | mysql::TypeDouble => assert_eq!(mysql::TypeDouble, aggTp.GetType()),
            mysql::TypeTimestamp
            | mysql::TypeDate
            | mysql::TypeDuration
            | mysql::TypeDatetime
            | mysql::TypeNewDate
            | mysql::TypeVarchar
            | mysql::TypeJSON
            | mysql::TypeEnum
            | mysql::TypeSet
            | mysql::TypeVarString
            | mysql::TypeGeometry => assert_eq!(mysql::TypeVarchar, aggTp.GetType()),
            mysql::TypeBit => assert_eq!(mysql::TypeLonglong, aggTp.GetType()),
            mysql::TypeString => assert_eq!(mysql::TypeString, aggTp.GetType()),
            mysql::TypeUnspecified | mysql::TypeNewDecimal => {
                assert_eq!(mysql::TypeNewDecimal, aggTp.GetType())
            }
            mysql::TypeTinyBlob => assert_eq!(mysql::TypeTinyBlob, aggTp.GetType()),
            mysql::TypeBlob => assert_eq!(mysql::TypeBlob, aggTp.GetType()),
            mysql::TypeMediumBlob => assert_eq!(mysql::TypeMediumBlob, aggTp.GetType()),
            mysql::TypeLongBlob => assert_eq!(mysql::TypeLongBlob, aggTp.GetType()),
            other => unreachable!("unexpected MySQL type {other}"),
        }

        let json = NewFieldType(mysql::TypeJSON);
        aggTp = AggFieldType(&[fts[i].as_ref(), json.as_ref()]);
        match fts[i].GetType() {
            mysql::TypeJSON | mysql::TypeNull => assert_eq!(mysql::TypeJSON, aggTp.GetType()),
            mysql::TypeLongBlob | mysql::TypeMediumBlob | mysql::TypeTinyBlob | mysql::TypeBlob => {
                assert_eq!(mysql::TypeLongBlob, aggTp.GetType())
            }
            mysql::TypeString => assert_eq!(mysql::TypeString, aggTp.GetType()),
            _ => assert_eq!(mysql::TypeVarchar, aggTp.GetType()),
        }
    }
}

/// NotNullFlag 仅在两侧皆有时才保留到聚合结果。
// TestAggFieldTypeForTypeFlag 对应 Go 测试：两个 Longlong 只有都带 NotNullFlag 时才聚合出 NotNullFlag。
#[test]
fn TestAggFieldTypeForTypeFlag() {
    let mut types = vec![
        NewFieldType(mysql::TypeLonglong),
        NewFieldType(mysql::TypeLonglong),
    ];

    let mut aggTp = AggFieldType(&refs(&types));
    assert_eq!(mysql::TypeLonglong, aggTp.GetType());
    assert_eq!(0, aggTp.GetFlag());

    types[0].SetFlag(mysql::NotNullFlag);
    aggTp = AggFieldType(&refs(&types));
    assert_eq!(mysql::TypeLonglong, aggTp.GetType());
    assert_eq!(0, aggTp.GetFlag());

    types[0].SetFlag(0);
    types[1].SetFlag(mysql::NotNullFlag);
    aggTp = AggFieldType(&refs(&types));
    assert_eq!(mysql::TypeLonglong, aggTp.GetType());
    assert_eq!(0, aggTp.GetFlag());

    types[0].SetFlag(mysql::NotNullFlag);
    aggTp = AggFieldType(&refs(&types));
    assert_eq!(mysql::TypeLonglong, aggTp.GetType());
    assert_eq!(mysql::NotNullFlag, aggTp.GetFlag());
}

/// 相邻整型聚合时 unsigned 标志的提升与类型拓宽。
// TestAggFieldTypeForIntegralPromotion 保留 Go 对相邻整型聚合的 unsigned flag 提升规则。
#[test]
fn TestAggFieldTypeForIntegralPromotion() {
    let mut fts = vec![
        NewFieldType(mysql::TypeTiny),
        NewFieldType(mysql::TypeShort),
        NewFieldType(mysql::TypeInt24),
        NewFieldType(mysql::TypeLong),
        NewFieldType(mysql::TypeLonglong),
        NewFieldType(mysql::TypeNewDecimal),
    ];

    for i in 1..fts.len() - 1 {
        fts[i - 1].SetFlag(0);
        fts[i].SetFlag(0);
        let mut aggTp = AggFieldType(&[fts[i - 1].as_ref(), fts[i].as_ref()]);
        assert_eq!(fts[i].GetType(), aggTp.GetType());
        assert_eq!(0, aggTp.GetFlag());

        fts[i - 1].SetFlag(mysql::UnsignedFlag);
        aggTp = AggFieldType(&[fts[i - 1].as_ref(), fts[i].as_ref()]);
        assert_eq!(fts[i].GetType(), aggTp.GetType());
        assert_eq!(0, aggTp.GetFlag());

        fts[i - 1].SetFlag(mysql::UnsignedFlag);
        fts[i].SetFlag(mysql::UnsignedFlag);
        aggTp = AggFieldType(&[fts[i - 1].as_ref(), fts[i].as_ref()]);
        assert_eq!(fts[i].GetType(), aggTp.GetType());
        assert_eq!(mysql::UnsignedFlag, aggTp.GetFlag());

        fts[i - 1].SetFlag(0);
        fts[i].SetFlag(mysql::UnsignedFlag);
        aggTp = AggFieldType(&[fts[i - 1].as_ref(), fts[i].as_ref()]);
        assert_eq!(fts[i + 1].GetType(), aggTp.GetType());
        assert_eq!(0, aggTp.GetFlag());
    }
}

/// 聚合 EvalType（求值类型）及 BinaryFlag 推导。
// TestAggregateEvalType 对应 Go 的聚合 EvalType 测试，保留三轮输入组合和每类类型的期望 eval type/flag。
#[test]
fn TestAggregateEvalType() {
    let fts = all_field_type_cases();

    for i in 0..fts.len() {
        let mut flag = 0;
        let mut aggregatedEvalType = AggregateEvalType(&[fts[i].as_ref()], &mut flag);
        assert_aggregate_eval_type_for_single_or_same(fts[i].GetType(), aggregatedEvalType, flag);

        flag = 0;
        aggregatedEvalType = AggregateEvalType(&[fts[i].as_ref(), fts[i].as_ref()], &mut flag);
        assert_aggregate_eval_type_for_single_or_same(fts[i].GetType(), aggregatedEvalType, flag);

        flag = 0;
        let long = NewFieldType(mysql::TypeLong);
        aggregatedEvalType = AggregateEvalType(&[fts[i].as_ref(), long.as_ref()], &mut flag);
        match fts[i].GetType() {
            mysql::TypeTimestamp
            | mysql::TypeDate
            | mysql::TypeDuration
            | mysql::TypeDatetime
            | mysql::TypeNewDate
            | mysql::TypeVarchar
            | mysql::TypeJSON
            | mysql::TypeEnum
            | mysql::TypeSet
            | mysql::TypeTinyBlob
            | mysql::TypeMediumBlob
            | mysql::TypeLongBlob
            | mysql::TypeBlob
            | mysql::TypeVarString
            | mysql::TypeString
            | mysql::TypeGeometry => {
                assert!(aggregatedEvalType.IsStringKind());
                assert_eq!(0, flag);
            }
            mysql::TypeUnspecified
            | mysql::TypeTiny
            | mysql::TypeShort
            | mysql::TypeLong
            | mysql::TypeNull
            | mysql::TypeBit
            | mysql::TypeLonglong
            | mysql::TypeYear
            | mysql::TypeInt24 => {
                assert_eq!(ETInt, aggregatedEvalType);
                assert_eq!(mysql::BinaryFlag, flag);
            }
            mysql::TypeFloat | mysql::TypeDouble => {
                assert_eq!(ETReal, aggregatedEvalType);
                assert_eq!(mysql::BinaryFlag, flag);
            }
            mysql::TypeNewDecimal => {
                assert_eq!(ETDecimal, aggregatedEvalType);
                assert_eq!(mysql::BinaryFlag, flag);
            }
            other => unreachable!("unexpected MySQL type {other}"),
        }
    }
}

/// 断言单类型或同型成对聚合时的 EvalType / flag。
// assert_aggregate_eval_type_for_single_or_same 抽出 Go 中前两轮 switch 的重复逻辑。
fn assert_aggregate_eval_type_for_single_or_same(
    tp: u8,
    aggregatedEvalType: EvalType,
    flag: usize,
) {
    match tp {
        mysql::TypeUnspecified
        | mysql::TypeNull
        | mysql::TypeTimestamp
        | mysql::TypeDate
        | mysql::TypeDuration
        | mysql::TypeDatetime
        | mysql::TypeNewDate
        | mysql::TypeVarchar
        | mysql::TypeJSON
        | mysql::TypeEnum
        | mysql::TypeSet
        | mysql::TypeTinyBlob
        | mysql::TypeMediumBlob
        | mysql::TypeLongBlob
        | mysql::TypeBlob
        | mysql::TypeVarString
        | mysql::TypeString
        | mysql::TypeGeometry => {
            assert!(aggregatedEvalType.IsStringKind());
            assert_eq!(0, flag);
        }
        mysql::TypeTiny
        | mysql::TypeShort
        | mysql::TypeLong
        | mysql::TypeLonglong
        | mysql::TypeBit
        | mysql::TypeInt24
        | mysql::TypeYear => {
            assert_eq!(ETInt, aggregatedEvalType);
            assert_eq!(mysql::BinaryFlag, flag);
        }
        mysql::TypeFloat | mysql::TypeDouble => {
            assert_eq!(ETReal, aggregatedEvalType);
            assert_eq!(mysql::BinaryFlag, flag);
        }
        mysql::TypeNewDecimal => {
            assert_eq!(ETDecimal, aggregatedEvalType);
            assert_eq!(mysql::BinaryFlag, flag);
        }
        other => unreachable!("unexpected MySQL type {other}"),
    }
}
