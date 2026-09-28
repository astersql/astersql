// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// Datum 标量存取、克隆、比较与 JSON 往返的迁移期单元测试。
//
// 校验 setter/getter Kind 语义、字符串/字节所有权、二进制字面量比较形态、
// 行格式化、数值转换、忽略截断下的 decimal 比较，以及 JSON/时间序列化。

use astersql_types_datum::*;

#[test]
/// 整数/浮点 setter 写入后 Kind 与取值与 Go 存储语义一致。
fn datum_scalar_setters_match_go_storage_semantics() {
    let mut datum = Datum::default();
    assert!(datum.IsNull());

    datum.SetInt64(-42);
    assert_eq!(datum.Kind(), KindInt64);
    assert_eq!(datum.GetInt64(), -42);

    datum.SetUint64(u64::MAX);
    assert_eq!(datum.Kind(), KindUint64);
    assert_eq!(datum.GetUint64(), u64::MAX);

    datum.SetFloat64(12345.678);
    assert_eq!(datum.Kind(), KindFloat64);
    assert_eq!(datum.GetFloat64(), 12345.678);

    datum.SetFloat32(281.37);
    assert_eq!(datum.Kind(), KindFloat32);
    assert_eq!(datum.GetFloat32(), 281.37_f32);
}

#[test]
/// Datum.String 保留 Go 调试输出中的 Kind 名称、原始标量与字符串转义。
fn datum_debug_string_matches_go_kind_and_value_format() {
    assert_eq!(Datum::default().String(), "KindNull <nil>");
    assert_eq!(NewIntDatum(-42).String(), "KindInt64 -42");
    assert_eq!(
        NewUintDatum(u64::MAX).String(),
        "KindUint64 18446744073709551615"
    );
    assert_eq!(
        NewStringDatum("a\nb".to_string()).String(),
        r"KindString a\nb"
    );
    assert_eq!(NewBytesDatum(b"a\nb".to_vec()).String(), r"KindBytes a\nb");
}

#[test]
/// 字符串、字节与 interface 克隆为独立所有权，修改原值不影响副本。
fn datum_strings_bytes_and_clone_are_owned() {
    let mut original = NewCollationStringDatum("hello, 世界".to_owned(), "utf8mb4_bin".to_owned());
    let cloned = original.Clone();

    original.SetString("changed".to_owned(), "binary".to_owned());
    assert_eq!(cloned.GetString(), "hello, 世界");
    assert_eq!(cloned.Collation(), "utf8mb4_bin");

    let mut bytes = NewBytesDatum(vec![0, 1, 2, 255]);
    let bytes_clone = bytes.Clone();
    bytes.SetBytes(vec![9]);
    assert_eq!(bytes_clone.GetBytes(), vec![0, 1, 2, 255]);

    let mut interface = Datum::default();
    interface.SetInterface(Box::new("shared".to_owned()));
    let interface_clone = interface.Clone();
    assert_eq!(
        interface_clone
            .GetInterface()
            .unwrap()
            .as_ref()
            .downcast_ref::<String>()
            .map(String::as_str),
        Some("shared")
    );
}

#[test]
/// GetBinaryLiteral4Cmp 去掉前导零，供 BIT/HEX 比较。
fn binary_literal_comparison_form_matches_go() {
    assert_eq!(
        NewBinaryLiteralDatum(BinaryLiteral(vec![])).GetBinaryLiteral4Cmp(),
        BinaryLiteral(vec![])
    );
    assert_eq!(
        NewBinaryLiteralDatum(BinaryLiteral(vec![0, 0, 0])).GetBinaryLiteral4Cmp(),
        BinaryLiteral(vec![0])
    );
    assert_eq!(
        NewBinaryLiteralDatum(BinaryLiteral(vec![0, 0, 1, 2])).GetBinaryLiteral4Cmp(),
        BinaryLiteral(vec![1, 2])
    );
}

#[test]
/// NULL/-inf/+inf 等特殊值与行字符串格式化、CloneRow 语义。
fn special_values_and_rows_match_go_formatting() {
    let values = vec![
        Datum::default(),
        MinNotNullDatum(),
        MaxValueDatum(),
        NewStringDatum("abc".to_owned()),
        NewIntDatum(7),
    ];

    assert!(DatumsContainNull(&values));
    assert_eq!(
        DatumsToString(&values, true).unwrap(),
        "(NULL, -inf, +inf, \"abc\", 7)"
    );

    let cloned = CloneRow(&values);
    assert_eq!(cloned.len(), values.len());
    assert_eq!(cloned[3].GetString(), "abc");
    assert_eq!(cloned[4].GetInt64(), 7);
}

#[test]
/// ConvertTo 按目标 FieldType 将字符串/整数转为对应 Kind。
fn datum_numeric_conversion_matches_go_kind_rules() {
    let context = (*DefaultStmtNoWarningContext).clone();
    let signed = NewStringDatum("42".to_owned())
        .ConvertTo(context.clone(), &NewFieldType(mysql::TypeLonglong))
        .unwrap();
    assert_eq!(signed.Kind(), KindInt64);
    assert_eq!(signed.GetInt64(), 42);

    let float = NewIntDatum(-7)
        .ConvertTo(context, &NewFieldType(mysql::TypeDouble))
        .unwrap();
    assert_eq!(float.Kind(), KindFloat64);
    assert_eq!(float.GetFloat64(), -7.0);
}

#[test]
/// IgnoreTruncate 上下文下非法字符串与 decimal 比较视为相等（对齐 Go）。
fn decimal_comparison_honors_ignore_truncate_context() {
    let base = (*DefaultStmtNoWarningContext).clone();
    let context = base.WithFlags(base.Flags().WithIgnoreTruncateErr(true));
    let string = NewStringDatum("hello".to_owned());
    let mut decimal = MyDecimal::default();
    decimal.FromInt(0);
    let decimal = NewDecimalDatum(decimal);

    let collator = collate::GetBinaryCollator();
    assert_eq!(
        string
            .Compare(context.clone(), &decimal, collator.as_ref())
            .unwrap(),
        0
    );
    let collator = collate::GetBinaryCollator();
    assert_eq!(
        decimal
            .Compare(context, &string, collator.as_ref())
            .unwrap(),
        0
    );
}

#[test]
/// MarshalJSON/UnmarshalJSON 往返保留字符串校对规则与 decimal 字面量。
fn datum_json_round_trip_preserves_owned_payloads() {
    let source = NewCollationStringDatum("round-trip".to_owned(), "utf8mb4_bin".to_owned());
    let encoded = source.MarshalJSON().unwrap();
    let mut decoded = Datum::default();
    decoded.UnmarshalJSON(&encoded).unwrap();

    assert_eq!(decoded.Kind(), KindString);
    assert_eq!(decoded.GetString(), "round-trip");
    assert_eq!(decoded.Collation(), "utf8mb4_bin");

    let mut decimal = MyDecimal::default();
    decimal.FromString(b"123.450").unwrap();
    let source = NewDecimalDatum(decimal);
    let encoded = source.MarshalJSON().unwrap();
    decoded.UnmarshalJSON(&encoded).unwrap();
    assert_eq!(decoded.Kind(), KindMysqlDecimal);
    assert_eq!(decoded.GetMysqlDecimal().String(), "123.450");

    assert_eq!(
        CompareBinaryJSON(
            &ParseBinaryJSONFromString("9.0").unwrap(),
            &ParseBinaryJSONFromString("9").unwrap(),
        ),
        0
    );

    let time = NewTime(FromDate(2026, 7, 14, 12, 34, 56, 0), mysql::TypeDatetime, 0);
    let mut time_datum = Datum::default();
    time_datum.SetMysqlTime(time);
    assert_eq!(
        time_datum.ToMysqlJSON().unwrap().TypeCode,
        JSONTypeCodeDatetime
    );
}
