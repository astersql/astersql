// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 向量化比较内核（`builtin_compare_vec`）与 Go 语义对齐的单元测试。
//
// 覆盖 GREATEST/LEAST（极值聚合）、有符号/无符号整数比较、比较结果映射、
// INTERVAL 查找、字符串按时间比较，以及各签名的 `vectorized()` 声明。
// SQL NULL 经列的 null 位图传播；任一侧为 NULL 时比较结果为 NULL（`<=>` 除外）。

use crate::builtin_compare_vec_kernel::*;
use rust_decimal::Decimal;

/// 构造带 NULL 位图的可空列；`nulls` 为 NULL 行下标。
fn column<T>(values: Vec<T>, nulls: &[usize]) -> NullableColumn<T> {
    NullableColumn::with_nulls(values, nulls).unwrap()
}

/// 整型 / DECIMAL / 浮点 GREATEST、LEAST 应与 Go 一致，并按行传播 NULL。
#[test]
fn numeric_extrema_match_go_and_propagate_nulls() {
    let first = column(vec![3_i64, 8, 1, 9], &[2]);
    let second = column(vec![7_i64, 2, 6, 4], &[1]);
    let third = column(vec![5_i64, 5, 5, 5], &[]);

    // 任一侧为 NULL 的行，极值结果也为 NULL。
    assert_eq!(
        greatest_i64(&[first.clone(), second.clone(), third.clone()]).unwrap(),
        column(vec![7, 8, 1, 9], &[1, 2])
    );
    assert_eq!(
        least_i64(&[first, second, third]).unwrap(),
        column(vec![3, 8, 1, 4], &[1, 2])
    );

    let decimals = [
        column(vec![Decimal::new(125, 2), Decimal::new(-5, 0)], &[]),
        column(vec![Decimal::new(13, 1), Decimal::new(-6, 0)], &[1]),
    ];
    assert_eq!(
        greatest_decimal(&decimals).unwrap(),
        column(vec![Decimal::new(13, 1), Decimal::new(-5, 0)], &[1])
    );
    assert_eq!(
        least_decimal(&decimals).unwrap(),
        column(vec![Decimal::new(125, 2), Decimal::new(-5, 0)], &[1])
    );

    // 浮点保留 -0.0 的位模式语义（与 Go cmp 路径一致处由内核保证）。
    let reals = [column(vec![1.5, -0.0], &[]), column(vec![2.5, 0.0], &[])];
    let greatest = greatest_f64(&reals).unwrap();
    let least = least_f64(&reals).unwrap();
    assert_eq!(greatest.values[0], 2.5);
    assert_eq!(least.values[0], 1.5);
    assert_eq!(greatest.values[1].to_bits(), (-0.0_f64).to_bits());
    assert_eq!(least.values[1].to_bits(), (-0.0_f64).to_bits());
}

/// 字符串极值使用校对规则（collation）；偶数参数走拷贝路径仍应对齐。
#[test]
fn string_extrema_use_collation_and_match_even_argument_copy_path() {
    let inputs = [
        column(vec!["Beta".to_owned(), "x".to_owned()], &[]),
        column(vec!["alpha".to_owned(), "z".to_owned()], &[1]),
    ];
    // 模拟不区分大小写的校对比较器。
    let case_insensitive = |left: &str, right: &str| left.to_lowercase().cmp(&right.to_lowercase());

    assert_eq!(
        greatest_string_by(&inputs, case_insensitive).unwrap(),
        column(vec!["Beta".to_owned(), "x".to_owned()], &[1])
    );
    assert_eq!(
        least_string_by(&inputs, case_insensitive).unwrap(),
        column(vec!["alpha".to_owned(), "x".to_owned()], &[1])
    );
}

/// 有符号与无符号混比遵循 MySQL 整数序；`<=>`（NULL-safe 相等）永不返回 SQL NULL。
#[test]
fn signed_unsigned_comparison_matches_mysql_integer_ordering() {
    let signed = IntColumn::signed(vec![-1, 0, i64::MAX], &[2]).unwrap();
    let unsigned =
        IntColumn::unsigned(vec![0, i64::MAX as u64 + 1, i64::MAX as u64], &[1]).unwrap();

    assert_eq!(
        compare_int_columns(&signed, &unsigned, CompareOp::Lt).unwrap(),
        column(vec![1, 1, 0], &[1, 2])
    );
    assert_eq!(
        compare_int_columns(&unsigned, &signed, CompareOp::Gt).unwrap(),
        column(vec![1, 1, 0], &[1, 2])
    );

    // NullEq：两侧皆 NULL 为 1，一侧 NULL 为 0，结果列无 NULL。
    let both_null = IntColumn::signed(vec![1, 2], &[0]).unwrap();
    let right = IntColumn::signed(vec![1, 2], &[0, 1]).unwrap();
    assert_eq!(
        compare_int_columns(&both_null, &right, CompareOp::NullEq).unwrap(),
        column(vec![1, 0], &[])
    );
}

/// 将 cmp 原始序（负/零/正）映射为各比较算子的 0/1 结果，应对齐 Go。
#[test]
fn every_compare_result_mapping_matches_go() {
    let raw = [-2, 0, 3];
    assert_eq!(map_compare_results(&raw, CompareOp::Lt), vec![1, 0, 0]);
    assert_eq!(map_compare_results(&raw, CompareOp::Le), vec![1, 1, 0]);
    assert_eq!(map_compare_results(&raw, CompareOp::Eq), vec![0, 1, 0]);
    assert_eq!(map_compare_results(&raw, CompareOp::Ne), vec![1, 0, 1]);
    assert_eq!(map_compare_results(&raw, CompareOp::Gt), vec![0, 0, 1]);
    assert_eq!(map_compare_results(&raw, CompareOp::Ge), vec![0, 1, 1]);
}

/// INTERVAL(N, …)：无 NULL 边界可二分；含 NULL 时退回线性扫描；目标 NULL 返回 -1。
#[test]
fn interval_int_uses_linear_for_nullable_and_binary_otherwise() {
    let targets = IntColumn::signed(vec![-1, 3, 7, 0], &[3]).unwrap();
    let non_nullable = vec![
        IntColumn::signed(vec![0; 4], &[]).unwrap(),
        IntColumn::signed(vec![4; 4], &[]).unwrap(),
        IntColumn::signed(vec![8; 4], &[]).unwrap(),
    ];
    assert_eq!(
        interval_int(&targets, &non_nullable, false).unwrap(),
        vec![0, 1, 2, -1]
    );

    let nullable = vec![
        IntColumn::signed(vec![0; 4], &[1]).unwrap(),
        IntColumn::signed(vec![4; 4], &[]).unwrap(),
        IntColumn::signed(vec![8; 4], &[]).unwrap(),
    ];
    assert_eq!(
        interval_int(&targets, &nullable, true).unwrap(),
        vec![0, 1, 2, -1]
    );
}

/// 浮点 INTERVAL 的 NULL 跳过与边界语义应对齐 Go。
#[test]
fn interval_real_matches_go_null_and_boundary_semantics() {
    let targets = column(vec![-1.0, 3.0, 9.0, 0.0], &[3]);
    let boundaries = vec![
        column(vec![0.0; 4], &[]),
        column(vec![4.0; 4], &[1]),
        column(vec![8.0; 4], &[]),
    ];
    assert_eq!(
        interval_real(&targets, &boundaries, true).unwrap(),
        vec![0, 2, 3, -1]
    );
}

/// 字符串按时间比较：先转换再比；转换失败应立即返回错误。
#[test]
fn string_as_time_extrema_convert_each_value_and_stop_on_error() {
    let args = [
        column(
            vec!["2024-1-2".to_owned(), "bad".to_owned(), "x".to_owned()],
            &[2],
        ),
        column(
            vec![
                "2023-12-31".to_owned(),
                "2024-01-01".to_owned(),
                "y".to_owned(),
            ],
            &[],
        ),
    ];
    // 模拟时间字符串规范化；非法值返回 Conversion 错误。
    let converter = |value: &str| -> Result<String, CompareVecError> {
        match value {
            "2024-1-2" => Ok("2024-01-02".to_owned()),
            "bad" => Err(CompareVecError::Conversion("bad time".to_owned())),
            other => Ok(other.to_owned()),
        }
    };
    assert_eq!(
        greatest_string_as_time(&args, converter),
        Err(CompareVecError::Conversion("bad time".to_owned()))
    );

    let valid = [
        column(vec!["2024-1-2".to_owned(), "2024-02-01".to_owned()], &[]),
        column(vec!["2023-12-31".to_owned(), "2024-03-01".to_owned()], &[1]),
    ];
    assert_eq!(
        least_string_as_time(&valid, converter).unwrap(),
        column(vec!["2023-12-31".to_owned(), "2024-02-01".to_owned()], &[1])
    );
}

/// TIME / DURATION 极值：转换钩子与 NULL 传播规则应保留。
#[test]
fn time_and_duration_extrema_preserve_conversion_and_null_rules() {
    let times = [
        column(vec![30_i64, 20], &[]),
        column(vec![10_i64, 40], &[1]),
    ];
    assert_eq!(
        greatest_time_by(&times, |value| Ok(value + 1)).unwrap(),
        column(vec![31, 21], &[1])
    );
    assert_eq!(
        least_time_by(&times, |value| Ok(value - 1)).unwrap(),
        column(vec![9, 19], &[1])
    );

    let durations = [
        column(vec![30_i64, 20], &[]),
        column(vec![10_i64, 40], &[1]),
    ];
    assert_eq!(
        greatest_duration(&durations).unwrap(),
        column(vec![30, 20], &[1])
    );
    assert_eq!(
        least_duration(&durations).unwrap(),
        column(vec![10, 20], &[1])
    );
}

/// 空参数与列长不一致应返回显式错误，而非 panic 或静默截断。
#[test]
fn empty_or_mismatched_columns_return_explicit_errors() {
    assert_eq!(greatest_i64(&[]), Err(CompareVecError::NoArguments));
    assert_eq!(
        NullableColumn::with_nulls(vec![1_i64], &[1]),
        Err(CompareVecError::InvalidNullIndex { index: 1, len: 1 })
    );
    let mismatched = [column(vec![1_i64], &[]), column(vec![1_i64, 2], &[])];
    assert_eq!(
        least_i64(&mismatched),
        Err(CompareVecError::LengthMismatch {
            expected: 1,
            actual: 2
        })
    );
}

/// Go 侧每个比较相关签名均应声明已向量化（vectorized）。
#[test]
fn every_go_signature_reports_vectorized() {
    // 批量断言各 Builtin*Sig 的 vectorized() 为 true。
    macro_rules! assert_vectorized {
        ($($signature:expr),+ $(,)?) => {
            $(assert!($signature.vectorized());)+
        };
    }

    assert_vectorized!(
        BuiltinGreatestDecimalSig,
        BuiltinLeastDecimalSig,
        BuiltinLeastIntSig,
        BuiltinGreatestIntSig,
        BuiltinGeIntSig,
        BuiltinLeastRealSig,
        BuiltinLeastStringSig,
        BuiltinEqIntSig,
        BuiltinNeIntSig,
        BuiltinGtIntSig,
        BuiltinNullEqIntSig,
        BuiltinIntervalIntSig,
        BuiltinIntervalRealSig,
        BuiltinLeIntSig,
        BuiltinLtIntSig,
        BuiltinGreatestCmpStringAsTimeSig,
        BuiltinGreatestRealSig,
        BuiltinLeastCmpStringAsTimeSig,
        BuiltinGreatestStringSig,
        BuiltinGreatestTimeSig,
        BuiltinLeastTimeSig,
        BuiltinGreatestDurationSig,
        BuiltinLeastDurationSig,
    );
}
