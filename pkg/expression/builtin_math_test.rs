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
// 数学内建函数标量行为的常规单元测试。
//
// 覆盖 ABS/CEIL/FLOOR/ROUND、对数定义域、随机数与溢出、CONV/CRC/SIGN、
// 三角函数以及 TRUNCATE/签名选择，作为与 Go 回归对照的精简用例集。

use crate::builtin_math::*;

/// 测试辅助：解析十进制字面量为 MyDecimal。
fn decimal(value: &str) -> MyDecimal {
    let mut result = MyDecimal::default();
    result.FromString(value.as_bytes()).unwrap();
    result
}

/// 测试辅助：MyDecimal → 字符串。
fn decimal_string(value: &MyDecimal) -> String {
    String::from_utf8(value.ToString()).unwrap()
}

/// ABS 溢出、CEIL/FLOOR 方向与 ROUND 小数位（含负位数）的常规与边界。
#[test]
fn abs_ceil_floor_and_round_cover_normal_boundary_and_overflow() {
    assert_eq!(abs_int(-1).unwrap(), 1);
    assert!(abs_int(i64::MIN).is_err());
    assert_eq!(abs_uint(u64::MAX), u64::MAX);
    assert_eq!(abs_real(-3.14), 3.14);
    assert_eq!(
        decimal_string(&abs_decimal(&decimal("-1.23")).unwrap()),
        "1.23"
    );

    assert_eq!(ceil_real(1.23), 2.0);
    assert_eq!(ceil_real(-1.23), -1.0);
    assert_eq!(floor_real(1.23), 1.0);
    assert_eq!(floor_real(-1.23), -2.0);
    assert_eq!(ceil_decimal_to_int(&decimal("1.23")).unwrap(), 2);
    assert_eq!(floor_decimal_to_int(&decimal("-1.23")).unwrap(), -2);

    assert_eq!(round_real(-1.5), -2.0);
    assert_eq!(round_with_frac_real(1.298, 1), 1.3);
    assert_eq!(round_with_frac_real(23.298, -1), 20.0);
    assert_eq!(round_with_frac_int(15, -1), 20);
}

/// 对数/平方根/反三角定义域：非法输入返回 None，并记录对数告警。
#[test]
fn logarithm_root_and_inverse_trig_domains_match_go_null_semantics() {
    assert_eq!(log(100.0), Some(100.0_f64.ln()));
    assert_eq!(log(-1.0), None);
    assert_eq!(log_base(10.0, 100.0), Some(2.0));
    assert_eq!(log_base(1.0, 2.0), None);
    assert_eq!(log2(16.0), Some(4.0));
    assert_eq!(log10(100.0), Some(2.0));
    assert_eq!(sqrt(16.0), Some(4.0));
    assert_eq!(sqrt(-16.0), None);
    assert_eq!(acos(2.0), None);
    assert_eq!(asin(-2.0), None);
    assert!(sqrt(f64::NAN).unwrap().is_nan());
    assert!(acos(f64::NAN).unwrap().is_nan());
    assert!(asin(f64::NAN).unwrap().is_nan());
    assert_eq!(
        eval_log(-1.0).warnings,
        vec![MathWarning::InvalidArgumentForLogarithm]
    );
}

/// 随机序列、POW/EXP/COT 的成功值与溢出错误。
#[test]
fn random_power_exponential_and_cot_cover_state_and_errors() {
    let rng = MysqlRand::with_seed(0);
    assert_eq!(rng.generate(), 0.15522042769493574);
    assert_eq!(rng.generate(), 0.620881741513388);
    assert_eq!(rand_with_seed_first_gen(Some(1)), 0.40540353712197724);

    assert_eq!(pow(4.0, -2.0).unwrap(), 0.0625);
    assert!(pow(10.0, 700.0).is_err());
    assert_eq!(exp(0.0).unwrap(), 1.0);
    assert!(exp(100_000.0).is_err());
    assert_eq!(cot(1.0).unwrap(), 0.6420926159343308);
    assert!(cot(0.0).is_err());
}

/// CONV 有符号规则、CRC32（含中文 UTF-8）与 SIGN。
#[test]
fn conv_crc_and_sign_cover_signed_unsigned_invalid_and_unicode_cases() {
    assert_eq!(get_valid_prefix("-123456D1f", 5), "-1234");
    assert_eq!(get_valid_prefix("+12azD", 16), "12a");
    assert_eq!(conv("a", 16, 2).unwrap().as_deref(), Some("1010"));
    assert_eq!(conv("-17", 10, -18).unwrap().as_deref(), Some("-H"));
    assert_eq!(
        conv("18446744073709551615", -10, 16).unwrap().as_deref(),
        Some("7FFFFFFFFFFFFFFF")
    );
    assert_eq!(conv("a6a", 1, 8).unwrap(), None);
    assert_eq!(conv("TIDB", 10, 8).unwrap().as_deref(), Some("0"));

    assert_eq!(crc32(b""), 0);
    assert_eq!(crc32(b"mysql"), 2_501_908_538);
    assert_eq!(crc32("一二三".as_bytes()), 1_785_250_883);
    assert_eq!(sign(0.4), 1);
    assert_eq!(sign(0.0), 0);
    assert_eq!(sign(-0.4), -1);
}

/// PI、角度换算与基本三角函数结果对齐 Go。
#[test]
fn trigonometry_and_angle_conversion_match_go_values() {
    assert_eq!(pi(), std::f64::consts::PI);
    assert_eq!(degrees(std::f64::consts::PI), 180.0);
    assert_eq!(radians(180.0), std::f64::consts::PI);
    assert_eq!(sin(std::f64::consts::FRAC_PI_2), 1.0);
    assert_eq!(cos(std::f64::consts::PI), -1.0);
    assert_eq!(atan(1.0), std::f64::consts::FRAC_PI_4);
    assert_eq!(atan2(0.0, -2.0), std::f64::consts::PI);
    assert!((tan(std::f64::consts::FRAC_PI_4) - 1.0).abs() < 1e-15);
}

/// TRUNCATE 在实数/整数/DECIMAL 及 NaN、极端小数位上的行为。
#[test]
fn truncate_preserves_decimal_integer_nan_and_large_fraction_boundaries() {
    assert_eq!(truncate_real(123.2, -1), 120.0);
    assert_eq!(truncate_real(1.1, 400), 1.1);
    assert_eq!(truncate_real(1.1, -400), 0.0);
    assert!(truncate_real(f64::NAN, 3).is_nan());
    assert_eq!(truncate_int(i64::MAX, -7, false), 9_223_372_036_850_000_000);
    assert_eq!(truncate_int(1, i64::MIN, false), 0);
    assert_eq!(truncate_int(123, -2, true), 123);
    assert_eq!(
        truncate_uint(u64::MAX, -10, false),
        18_446_744_070_000_000_000
    );
    assert_eq!(
        decimal_string(&truncate_decimal(&decimal("23.298"), -1, 0).unwrap()),
        "20"
    );
}

/// 根据参数 FieldType 选择的 ScalarFuncSignature 必须与 Go 分支一致。
#[test]
fn signature_metadata_selects_the_same_go_implementations() {
    assert_eq!(
        abs_signature(FieldTypeMeta::int(true)),
        ScalarFuncSignature::AbsUInt
    );
    assert_eq!(
        round_signature(FieldTypeMeta::decimal(10, 2), true),
        ScalarFuncSignature::RoundWithFracDecimal
    );
    assert_eq!(
        ceil_signature(FieldTypeMeta::decimal(22, 2)),
        ScalarFuncSignature::CeilDecimalToDecimal
    );
    assert_eq!(
        floor_signature(FieldTypeMeta::decimal(10, 2)),
        ScalarFuncSignature::FloorDecimalToInt
    );
    assert_eq!(
        rand_signature(true, false),
        ScalarFuncSignature::RandWithSeedFirstGen
    );
    assert_eq!(
        truncate_signature(FieldTypeMeta::int(true)),
        ScalarFuncSignature::TruncateUInt
    );
}
