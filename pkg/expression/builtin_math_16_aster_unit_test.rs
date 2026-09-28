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
// 数学内建函数（ABS/ROUND/LOG/RAND/CONV 等）的 Aster 单元测试。
//
// 直接调用 `builtin_math` 中的标量实现，核对与 Go `builtin_math_test.go`
// 一致的边界、溢出错误文案、定义域 NULL 与签名选择。

use crate::builtin_math::*;

/// 从十进制字符串构造 MyDecimal（MySQL 高精度十进制类型）。
fn decimal(value: &str) -> MyDecimal {
    let mut result = MyDecimal::default();
    result.FromString(value.as_bytes()).unwrap();
    result
}

/// 将 MyDecimal 格式化为规范化字符串，便于断言比较。
fn decimal_string(value: &MyDecimal) -> String {
    String::from_utf8(value.ToString()).unwrap()
}

/// ABS/ROUND 数值结果，以及 ROUND/FLOOR/CEIL/RAND/TRUNCATE 的签名元数据分支。
#[test]
fn abs_round_and_metadata_match_go_branches() {
    assert_eq!(abs_int(-1).unwrap(), 1);
    assert_eq!(
        abs_int(i64::MIN).unwrap_err().to_string(),
        "[types:1690]BIGINT value is out of range in 'abs(-9223372036854775808)'"
    );
    assert_eq!(abs_real(-3.14), 3.14);
    assert_eq!(abs_uint(u64::MAX), u64::MAX);

    assert_eq!(round_real(-1.5), -2.0);
    assert_eq!(round_with_frac_real(1.298, 1), 1.3);
    assert_eq!(round_with_frac_real(23.298, -1), 20.0);
    assert_eq!(round_with_frac_int(15, -1), 20);

    assert_eq!(
        calculate_decimal_for_round_and_truncate(
            EvalType::Decimal,
            4,
            FractionMetadata::Constant(Some(99)),
        ),
        MAX_DECIMAL_SCALE,
    );
    assert_eq!(
        calculate_decimal_for_round_and_truncate(EvalType::Decimal, 4, FractionMetadata::Dynamic,),
        4,
    );
    assert_eq!(
        get_eval_type_for_floor_and_ceil(FieldTypeMeta::decimal(22, 2)),
        (EvalType::Decimal, EvalType::Decimal),
    );
    assert_eq!(
        get_eval_type_for_floor_and_ceil(FieldTypeMeta::decimal(10, 2)),
        (EvalType::Int, EvalType::Decimal),
    );
    assert_eq!(
        abs_signature(FieldTypeMeta::int(true)),
        ScalarFuncSignature::AbsUInt
    );
    assert_eq!(
        round_signature(FieldTypeMeta::decimal(10, 2), true),
        ScalarFuncSignature::RoundWithFracDecimal,
    );
    assert_eq!(
        ceil_signature(FieldTypeMeta::decimal(22, 2)),
        ScalarFuncSignature::CeilDecimalToDecimal,
    );
    assert_eq!(
        floor_signature(FieldTypeMeta::decimal(10, 2)),
        ScalarFuncSignature::FloorDecimalToInt,
    );
    assert_eq!(
        rand_signature(true, false),
        ScalarFuncSignature::RandWithSeedFirstGen
    );
    assert_eq!(
        truncate_signature(FieldTypeMeta::int(true)),
        ScalarFuncSignature::TruncateUInt,
    );
}

/// DECIMAL 路径的 ABS/ROUND/CEIL/FLOOR/TRUNCATE，含负值截断方向。
#[test]
fn decimal_abs_round_ceil_floor_and_truncate_match_go() {
    assert_eq!(
        decimal_string(&abs_decimal(&decimal("-1.23")).unwrap()),
        "1.23"
    );
    assert_eq!(
        decimal_string(&round_decimal(&decimal("-1.58")).unwrap()),
        "-2"
    );
    assert_eq!(
        decimal_string(&round_with_frac_decimal(&decimal("23.298"), -1, 0).unwrap()),
        "20",
    );
    assert_eq!(ceil_decimal_to_int(&decimal("1.23")).unwrap(), 2);
    assert_eq!(ceil_decimal_to_int(&decimal("-1.23")).unwrap(), -1);
    assert_eq!(floor_decimal_to_int(&decimal("1.23")).unwrap(), 1);
    assert_eq!(floor_decimal_to_int(&decimal("-1.23")).unwrap(), -2);
    assert_eq!(
        decimal_string(&ceil_decimal(&decimal("1.23")).unwrap()),
        "2"
    );
    assert_eq!(
        decimal_string(&floor_decimal(&decimal("-1.23")).unwrap()),
        "-2"
    );
    assert_eq!(
        decimal_string(&truncate_decimal(&decimal("23.298"), -1, 0).unwrap()),
        "20",
    );
}

/// 对数族与 SQRT/ACOS/ASIN：定义域外为 None，并产生对数非法参数告警。
#[test]
fn logarithms_and_domain_nulls_match_go() {
    assert_eq!(log(100.0), Some(100.0_f64.ln()));
    assert_eq!(log(-1.0), None);
    assert_eq!(log_base(10.0, 100.0), Some(2.0));
    assert_eq!(log_base(1.0, 2.0), None);
    assert_eq!(log2(16.0), Some(4.0));
    assert_eq!(log10(100.0), Some(2.0));
    assert_eq!(
        eval_log(-1.0).warnings,
        vec![MathWarning::InvalidArgumentForLogarithm],
    );
    assert_eq!(sqrt(-16.0), None);
    assert_eq!(acos(2.0), None);
    assert_eq!(asin(-2.0), None);
}

/// 随机数生成器状态、POW/EXP/COT 溢出错误信息与 Go 文案对齐。
#[test]
fn rand_pow_exp_and_cot_preserve_state_and_overflow() {
    // Clone 后应各自推进 RNG 状态，与 Go MysqlRng 共享语义不同处由实现定义。
    let rng = MysqlRand::with_seed(0);
    let cloned = rng.clone();
    assert_eq!(rng.generate(), 0.15522042769493574);
    assert_eq!(cloned.generate(), 0.620881741513388);
    assert_eq!(rand_with_seed_first_gen(Some(1)), 0.40540353712197724);
    assert_eq!(rand_with_seed_first_gen(None), 0.15522042769493574);

    assert_eq!(pow(4.0, -2.0).unwrap(), 0.0625);
    assert!(pow(10.0, 700.0).is_err());
    assert_eq!(exp(0.0).unwrap(), 1.0);
    assert_eq!(
        exp(100_000.0).unwrap_err().to_string(),
        "[types:1690]DOUBLE value is out of range in 'exp(100000)'",
    );
    assert_eq!(cot(1.0).unwrap(), 0.6420926159343308);
    assert_eq!(
        cot(0.0).unwrap_err().to_string(),
        "[types:1690]DOUBLE value is out of range in 'cot(0)'",
    );
}

/// CONV 有效前缀扫描与有符号/无符号进制转换规则（负基数表示有符号）。
#[test]
fn conv_and_valid_prefix_match_go_signed_and_unsigned_rules() {
    assert_eq!(get_valid_prefix("-123456D1f", 5), "-1234");
    assert_eq!(get_valid_prefix("+12azD", 16), "12a");
    assert_eq!(get_valid_prefix("+", 12), "");

    assert_eq!(conv("a", 16, 2).unwrap(), Some("1010".to_owned()));
    assert_eq!(conv("6E", 18, 8).unwrap(), Some("172".to_owned()));
    assert_eq!(conv("-17", 10, -18).unwrap(), Some("-H".to_owned()));
    assert_eq!(
        conv("-17", 10, 18).unwrap(),
        Some("2D3FGB0B9CG4BD1H".to_owned()),
    );
    assert_eq!(
        conv("18446744073709551615", -10, 16).unwrap(),
        Some("7FFFFFFFFFFFFFFF".to_owned()),
    );
    assert_eq!(conv("a6a", 1, 8).unwrap(), None);
    assert_eq!(conv("TIDB", 10, 8).unwrap(), Some("0".to_owned()));
}

/// CRC32（含 Unicode 字节）、SIGN 与三角函数/角度换算的精确值。
#[test]
fn crc_sign_and_trigonometry_match_go_values() {
    assert_eq!(crc32(b"mysql"), 2_501_908_538);
    assert_eq!(crc32("一二三".as_bytes()), 1_785_250_883);
    assert_eq!(sign(0.4), 1);
    assert_eq!(sign(0.0), 0);
    assert_eq!(sign(-0.4), -1);

    assert_eq!(atan(1.0), std::f64::consts::FRAC_PI_4);
    assert_eq!(atan2(0.0, -2.0), std::f64::consts::PI);
    assert_eq!(cos(std::f64::consts::PI), -1.0);
    assert_eq!(degrees(std::f64::consts::PI), 180.0);
    assert_eq!(pi(), std::f64::consts::PI);
    assert_eq!(radians(180.0), std::f64::consts::PI);
    assert_eq!(sin(std::f64::consts::FRAC_PI_2), 1.0);
    assert_eq!(tan(std::f64::consts::FRAC_PI_4), 0.9999999999999999);
}

/// TRUNCATE 在极大/极小小数位、NaN 以及有符号/无符号整数上的边界行为。
#[test]
fn truncate_numeric_boundaries_match_go() {
    assert_eq!(truncate_real(123.2, -1), 120.0);
    assert_eq!(truncate_real(1.1, 400), 1.1);
    assert_eq!(truncate_real(1.1, -400), 0.0);
    assert!(truncate_real(f64::NAN, 3).is_nan());

    assert_eq!(
        truncate_int(9_223_372_036_854_775_807, -7, false),
        9_223_372_036_850_000_000
    );
    assert_eq!(truncate_int(1, i64::MIN, false), 0);
    assert_eq!(truncate_int(123, -2, true), 123);
    assert_eq!(
        truncate_uint(u64::MAX, -10, false),
        18_446_744_070_000_000_000
    );
    assert_eq!(truncate_uint(123, -2, true), 123);
}
