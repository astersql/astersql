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
// MyDecimal 核心行为的 Aster 单元测试：解析、舍入、转换、四则、编解码与 JSON。
//
// MyDecimal 是 MySQL DECIMAL/NUMERIC 的内存表示（按 9 位一组的 word 缓冲）。

use super::mydecimal::*;

/// 从文本构造 MyDecimal。
fn decimal(text: &str) -> MyDecimal {
    let mut value = MyDecimal::default();
    value.FromString(text.as_bytes()).unwrap();
    value
}

/// 将 ToString 字节转为 UTF-8 文本便于断言。
fn rendered(value: &MyDecimal) -> String {
    String::from_utf8(value.ToString()).unwrap()
}

#[test]
/// 解析科学计数法/前导零，以及 Shift 小数点移位。
fn parses_formats_and_shifts_like_go() {
    let cases = [
        ("00123.1230", "123.1230"),
        ("123E5", "12300000"),
        ("123E-2", "1.23"),
        ("-.000000012345000098765", "-0.000000012345000098765"),
    ];
    for (input, expected) in cases {
        assert_eq!(rendered(&decimal(input)), expected);
    }

    // 负向 Shift：小数点左移 10 位
    let mut shifted = decimal("123987654321.123456789000");
    shifted.Shift(-10).unwrap();
    assert_eq!(rendered(&shifted), "12.3987654321123456789");
}

#[test]
/// 非法后缀截断时保留已解析前缀；完全非法则归零并报 TruncatedWrongValue。
fn reports_go_parse_errors_without_losing_valid_prefix() {
    let mut value = MyDecimal::default();
    assert_eq!(value.FromString(b"123.45."), Err(DecimalError::Truncated));
    assert_eq!(rendered(&value), "123.45");

    assert_eq!(
        value.FromString(b"not-a-decimal"),
        Err(DecimalError::TruncatedWrongValue)
    );
    assert_eq!(rendered(&value), "0");
}

#[test]
/// HalfUp / Truncate / Ceiling 三种舍入模式对照 Go。
fn rounds_half_up_truncate_and_ceiling_like_go() {
    let input = decimal("123456789.987654321");
    for (mode, scale, expected) in [
        (ModeHalfUp, 1, "123456790.0"),
        (ModeTruncate, 1, "123456789.9"),
        (ModeCeiling, 1, "123456790.0"),
        (ModeHalfUp, -1, "123456790"),
    ] {
        let mut output = MyDecimal::default();
        input.Round(&mut output, scale, mode).unwrap();
        assert_eq!(rendered(&output), expected);
    }
}

#[test]
/// int64/u64 边界转换及小数截断错误。
fn converts_integer_boundaries_like_go() {
    let mut min = MyDecimal::default();
    min.FromInt(i64::MIN);
    assert_eq!(rendered(&min), "-9223372036854775808");
    assert_eq!(min.ToInt(), (i64::MIN, Ok(())));

    let max_unsigned = decimal("18446744073709551615");
    assert_eq!(max_unsigned.ToUint(), (u64::MAX, Ok(())));
    assert_eq!(
        max_unsigned.ToInt(),
        (i64::MAX, Err(DecimalError::Overflow))
    );

    let fractional = decimal("-1.23");
    assert_eq!(fractional.ToInt(), (-1, Err(DecimalError::Truncated)));
}

#[test]
/// 加减乘向量及极端指数相乘截断。
fn add_subtract_and_multiply_match_go_vectors() {
    let a = decimal("1234500009876.5");
    let b = decimal(".00012345000098765");
    let mut output = MyDecimal::default();
    DecimalAdd(&a, &b, &mut output).unwrap();
    assert_eq!(rendered(&output), "1234500009876.50012345000098765");

    DecimalSub(&a, &b, &mut output).unwrap();
    assert_eq!(rendered(&output), "1234500009876.49987654999901235");

    DecimalMul(&decimal("-123.456"), &decimal("98765.4321"), &mut output).unwrap();
    assert_eq!(rendered(&output), "-12193185.1853376");

    // 极小 × 极大会超出固定缓冲，期望 Truncated
    let tiny = decimal("-0.0000000000000000000000000000000000000000000000000017382578996420603");
    let huge =
        decimal("-13890436710184412000000000000000000000000000000000000000000000000000000000000");
    assert_eq!(
        DecimalMul(&tiny, &huge, &mut output),
        Err(DecimalError::Truncated)
    );
    assert_eq!(output.String(), "0.000000000000000000000000000000");
}

#[test]
/// 超大指数溢出时短路为全 9 填充（对齐 Go 缓冲上界行为）。
fn extreme_exponents_short_circuit_at_the_go_buffer_boundary() {
    let mut value = MyDecimal::default();
    assert_eq!(
        value.FromString(b"1e1073741823"),
        Err(DecimalError::Overflow)
    );
    assert_eq!(rendered(&value), "9".repeat(81));
}

#[test]
/// 除法保留位数规则、取模与除零错误。
fn division_and_modulus_match_go_scale_rules() {
    let mut output = MyDecimal::default();
    DecimalDiv(&decimal("1"), &decimal("3"), &mut output, 5).unwrap();
    assert_eq!(rendered(&output), "0.333333333");

    DecimalDiv(&decimal("1.000000000000"), &decimal("3"), &mut output, 5).unwrap();
    assert_eq!(rendered(&output), "0.333333333333333333");

    DecimalMod(&decimal("234.567"), &decimal("10.555"), &mut output).unwrap();
    assert_eq!(rendered(&output), "2.357");

    assert_eq!(
        DecimalDiv(&decimal("1"), &decimal("0"), &mut output, 5),
        Err(DecimalError::DivByZero)
    );
}

#[test]
/// ToBin/FromBin 往返、精度截断/溢出，以及编码字节序可排序。
fn binary_encoding_round_trips_and_preserves_sort_order() {
    let cases = [
        ("-10.55", 4, 2, "-10.55", None),
        ("123.45", 10, 3, "123.450", None),
        (
            ".00012345000098765",
            15,
            14,
            "0.00012345000098",
            Some(DecimalError::Truncated),
        ),
        (
            "111111111.11",
            10,
            2,
            "11111111.11",
            Some(DecimalError::Overflow),
        ),
    ];
    for (input, precision, frac, expected, expected_error) in cases {
        let value = decimal(input);
        let (encoded, error) = value.ToBin(precision, frac);
        assert_eq!(error.err(), expected_error);
        let mut restored = MyDecimal::default();
        let (_, decode_status) = restored.FromBin(&encoded, precision, frac);
        decode_status.unwrap();
        assert_eq!(rendered(&restored), expected);
    }

    // 同精度二进制编码应保持数值序：负 < 零 < 正
    let low = decimal("-1").ToBin(4, 0).0;
    let mid = decimal("0").ToBin(4, 0).0;
    let high = decimal("1").ToBin(4, 0).0;
    assert!(low < mid && mid < high);
}

#[test]
/// Parquet 数组、PrecisionAndFrac、HashKey 与 JSON 编解码。
fn parquet_json_hash_and_precision_follow_go() {
    let mut parquet = MyDecimal::default();
    // Parquet 有符号大端整数 + scale 还原为十进制
    parquet.FromParquetArray(&mut [0xff, 0x85], 2).unwrap(); // -123 / 10^2
    assert_eq!(rendered(&parquet), "-1.23");

    let value = decimal("001.2300");
    assert_eq!(value.PrecisionAndFrac(), (5, 4));
    assert_eq!(
        value.HashKeySize().unwrap(),
        value.ToHashKey().unwrap().len()
    );

    let json = value.MarshalJSON().unwrap();
    let mut restored = MyDecimal::default();
    restored.UnmarshalJSON(&json).unwrap();
    assert_eq!(restored, value);
}
