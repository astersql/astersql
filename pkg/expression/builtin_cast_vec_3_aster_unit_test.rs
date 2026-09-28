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

// 向量化 CAST 与 Go 对齐的单元测试。
//
// 校验支持的 (源, 目标) 签名矩阵、整列非空/NULL 传播、Real→Time 有效与无效样例、
// UNION 无符号钳制、时长/小数/字符串舍入，以及 JSON 布尔与 parse_to_json 模式。

use crate::expression_builtin_cast_vec::*;

/// 取出非空 CAST 结果；若为 NULL 则 panic 便于定位失败行。
fn value(column: &[Option<ScalarValue>], index: usize) -> &ScalarValue {
    column[index]
        .as_ref()
        .unwrap_or_else(|| panic!("non-NULL cast result at row {index}: {column:?}"))
}

/// 支持矩阵须包含 Go 全部 49 个向量化 CAST 签名。
#[test]
fn supported_matrix_keeps_every_go_vectorized_cast_signature() {
    let casts = supported_casts();
    assert_eq!(casts.len(), 49);
    for required in [
        (EvalKind::Int, EvalKind::Duration),
        (EvalKind::Real, EvalKind::Time),
        (EvalKind::String, EvalKind::Decimal),
        (EvalKind::Json, EvalKind::Duration),
        (EvalKind::Decimal, EvalKind::Json),
        (EvalKind::Duration, EvalKind::Json),
    ] {
        assert!(casts.contains(&required), "missing {required:?}");
    }
}

/// 每个签名对非空行产出值，并对 NULL 行保持传播。
#[test]
fn every_go_signature_executes_a_non_null_row_and_propagates_null() {
    let mut ctx = CastContext::warning();
    for &(source_kind, target_kind) in supported_casts() {
        let spec = match target_kind {
            EvalKind::Decimal => CastSpec::decimal(source_kind, 30, 3),
            EvalKind::String => CastSpec::string(source_kind, 128),
            EvalKind::Time => CastSpec::datetime(source_kind, 3),
            EvalKind::Duration => CastSpec::duration(source_kind, 3),
            _ => CastSpec::new(source_kind, target_kind),
        };
        // 为每种源类型挑选可成功转换到各目标的样例值；JSON→Time/Duration 需带引号字符串。
        let source = match (source_kind, target_kind) {
            (EvalKind::Int, _) => ScalarValue::Int(10_101),
            (EvalKind::Real, _) => ScalarValue::Real(10_101.0),
            (EvalKind::Decimal, _) => ScalarValue::Decimal(decimal("10101")),
            (EvalKind::String, _) => ScalarValue::String("10101".into()),
            (EvalKind::Time, _) => ScalarValue::Time(time("2024-05-06 01:02:03.456", 3)),
            (EvalKind::Duration, _) => ScalarValue::Duration(duration("01:02:03.456", 3)),
            (EvalKind::Json, EvalKind::Time) => {
                ScalarValue::Json(json(r#""2024-05-06 01:02:03.456""#))
            }
            (EvalKind::Json, EvalKind::Duration) => ScalarValue::Json(json(r#""01:02:03.456""#)),
            (EvalKind::Json, _) => ScalarValue::Json(json("10101")),
        };

        let result = cast_column(&mut ctx, &spec, &[Some(source), None])
            .unwrap_or_else(|failure| panic!("{source_kind:?}->{target_kind:?}: {failure}"));
        assert!(
            result[0].is_some(),
            "{source_kind:?}->{target_kind:?} unexpectedly returned NULL"
        );
        assert!(
            result[1].is_none(),
            "{source_kind:?}->{target_kind:?} lost NULL"
        );
    }
}

/// Real→Time：合法数值解析为日期时间，非法与越界变为 NULL 并累计 warning。
#[test]
fn real_as_time_matches_go_valid_invalid_and_null_cases() {
    let source = [
        Some(ScalarValue::Real(0.0)),
        Some(ScalarValue::Real(101.1)),
        Some(ScalarValue::Real(111.1)),
        Some(ScalarValue::Real(1122.1)),
        Some(ScalarValue::Real(31212.111)),
        Some(ScalarValue::Real(121212.1111)),
        Some(ScalarValue::Real(1121212.111111)),
        Some(ScalarValue::Real(11121212.111111)),
        Some(ScalarValue::Real(99991111.1111111)),
        Some(ScalarValue::Real(201212121212.1111111)),
        Some(ScalarValue::Real(20121212121212.1111111)),
        Some(ScalarValue::Real(1.1)),
        Some(ScalarValue::Real(48.1)),
        Some(ScalarValue::Real(100.1)),
        Some(ScalarValue::Real(1301.11)),
        Some(ScalarValue::Real(1131.111)),
        Some(ScalarValue::Real(100001111.111)),
        Some(ScalarValue::Real(20121212121260.1111111)),
        Some(ScalarValue::Real(20121212126012.1111111)),
        Some(ScalarValue::Real(20121212241212.1111111)),
        None,
    ];
    let mut ctx = CastContext::warning();
    let result = cast_column(&mut ctx, &CastSpec::datetime(EvalKind::Real, 0), &source)
        .expect("real to datetime");

    // 前 11 个 Real 可解析为合法 datetime；其后无效输入期望 NULL。
    let expected = [
        "0000-00-00 00:00:00",
        "2000-01-01 00:00:00",
        "2000-01-11 00:00:00",
        "2000-11-22 00:00:00",
        "2003-12-12 00:00:00",
        "2012-12-12 00:00:00",
        "0112-12-12 00:00:00",
        "1112-12-12 00:00:00",
        "9999-11-11 00:00:00",
        "2020-12-12 12:12:12",
        "2012-12-12 12:12:12",
    ];
    for (index, expected) in expected.iter().enumerate() {
        assert_eq!(value(&result, index).as_time().unwrap().String(), *expected);
    }
    assert!(result[11..].iter().all(Option::is_none));
    assert_eq!(ctx.warnings().len(), 9);
}

/// UNION 场景下无符号目标将负数钳为 0，且不改写 NULL。
#[test]
fn union_unsigned_clamps_negative_values_without_touching_nulls() {
    let mut ctx = CastContext::warning();
    let mut int_spec = CastSpec::new(EvalKind::Int, EvalKind::Int);
    int_spec.in_union = true;
    int_spec.target_unsigned = true;
    let ints = cast_column(
        &mut ctx,
        &int_spec,
        &[Some(ScalarValue::Int(-9)), Some(ScalarValue::Int(7)), None],
    )
    .unwrap();
    assert_eq!(
        ints,
        vec![Some(ScalarValue::Int(0)), Some(ScalarValue::Int(7)), None]
    );

    let mut decimal_spec = CastSpec::decimal(EvalKind::String, 10, 2);
    decimal_spec.in_union = true;
    decimal_spec.target_unsigned = true;
    let decimals = cast_column(
        &mut ctx,
        &decimal_spec,
        &[
            Some(ScalarValue::String(" -12.75 ".into())),
            Some(ScalarValue::String("3.125".into())),
            None,
        ],
    )
    .unwrap();
    assert_eq!(value(&decimals, 0).as_decimal().unwrap().String(), "0.00");
    assert_eq!(value(&decimals, 1).as_decimal().unwrap().String(), "3.13");
    assert!(decimals[2].is_none());
}

/// 时长小数秒四舍五入，并与字符串/DECIMAL 互转一致。
#[test]
fn temporal_duration_decimal_and_string_paths_match_go_rounding() {
    let mut ctx = CastContext::warning();
    let duration_spec = CastSpec::duration(EvalKind::String, 3);
    let durations = cast_column(
        &mut ctx,
        &duration_spec,
        &[
            Some(ScalarValue::String("12:34:56.7894".into())),
            Some(ScalarValue::String("-00:00:01.9996".into())),
            None,
        ],
    )
    .unwrap();
    assert_eq!(
        value(&durations, 0).as_duration().unwrap().String(),
        "12:34:56.789"
    );
    assert_eq!(
        value(&durations, 1).as_duration().unwrap().String(),
        "-00:00:02.000"
    );

    let strings = cast_column(
        &mut ctx,
        &CastSpec::string(EvalKind::Duration, 32),
        &durations,
    )
    .unwrap();
    assert_eq!(value(&strings, 0).as_string().unwrap(), "12:34:56.789");
    assert_eq!(value(&strings, 1).as_string().unwrap(), "-00:00:02.000");

    let decimals = cast_column(
        &mut ctx,
        &CastSpec::decimal(EvalKind::Duration, 12, 3),
        &durations,
    )
    .unwrap();
    assert_eq!(
        value(&decimals, 0).as_decimal().unwrap().String(),
        "123456.789"
    );
    assert_eq!(value(&decimals, 1).as_decimal().unwrap().String(), "-2.000");
}

/// JSON CAST：整型作布尔、字符串 parse_to_json 与默认引号包装。
#[test]
fn json_cast_modes_preserve_number_boolean_string_and_parse_semantics() {
    let mut ctx = CastContext::warning();

    let mut bool_spec = CastSpec::new(EvalKind::Int, EvalKind::Json);
    bool_spec.source_boolean = true;
    let booleans = cast_column(
        &mut ctx,
        &bool_spec,
        &[Some(ScalarValue::Int(0)), Some(ScalarValue::Int(3)), None],
    )
    .unwrap();
    assert_eq!(value(&booleans, 0).as_json().unwrap().String(), "false");
    assert_eq!(value(&booleans, 1).as_json().unwrap().String(), "true");

    let mut parse_spec = CastSpec::new(EvalKind::String, EvalKind::Json);
    parse_spec.parse_to_json = true;
    let parsed = cast_column(
        &mut ctx,
        &parse_spec,
        &[Some(ScalarValue::String(
            r#"{"a":1,"b":[true,null]}"#.into(),
        ))],
    )
    .unwrap();
    assert_eq!(
        value(&parsed, 0).as_json().unwrap().String(),
        r#"{"a": 1, "b": [true, null]}"#
    );

    let quoted = cast_column(
        &mut ctx,
        &CastSpec::new(EvalKind::String, EvalKind::Json),
        &[Some(ScalarValue::String("plain".into()))],
    )
    .unwrap();
    assert_eq!(value(&quoted, 0).as_json().unwrap().String(), r#""plain""#);
}

/// 各标量族反向转换保持数值字符串语义，并传播 NULL。
#[test]
fn all_scalar_families_keep_null_propagation_and_reverse_conversions() {
    let mut ctx = CastContext::warning();
    let decimal = decimal("42.6");
    let json = json("123.5");
    let time = time("2024-05-06 07:08:09.123456", 6);
    let duration = duration("01:02:03.456", 3);

    let cases = [
        (
            CastSpec::new(EvalKind::Decimal, EvalKind::Int),
            ScalarValue::Decimal(decimal),
        ),
        (
            CastSpec::new(EvalKind::Json, EvalKind::Real),
            ScalarValue::Json(json),
        ),
        (
            CastSpec::new(EvalKind::Time, EvalKind::Int),
            ScalarValue::Time(time),
        ),
        (
            CastSpec::new(EvalKind::Duration, EvalKind::Real),
            ScalarValue::Duration(duration),
        ),
    ];
    let expected = ["43", "123.5", "20240506070809", "10203.456"];
    for ((spec, source), expected) in cases.into_iter().zip(expected) {
        let output = cast_column(&mut ctx, &spec, &[Some(source), None]).unwrap();
        assert_eq!(value(&output, 0).numeric_string(), expected);
        assert!(output[1].is_none());
    }
}
