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

use super::*;
use std::process::Command;

/// Go `types.IsTypeInteger` includes YEAR in addition to the five protocol
/// integer types, unlike `mysql.IsIntegerType`.
#[test]
fn integer_type_classification_includes_year() {
    for tp in [
        mysql::TypeTiny,
        mysql::TypeShort,
        mysql::TypeInt24,
        mysql::TypeLong,
        mysql::TypeLonglong,
        mysql::TypeYear,
    ] {
        assert!(IsTypeInteger(tp), "type {tp} must be an integer");
    }
    assert!(!IsTypeInteger(mysql::TypeFloat));
}

/// The internal crate must retain the Go package's basic string-family
/// classification while it breaks the dependency cycle.
#[test]
fn string_type_classification_matches_go() {
    for tp in [
        mysql::TypeString,
        mysql::TypeVarchar,
        mysql::TypeVarString,
        mysql::TypeTinyBlob,
        mysql::TypeMediumBlob,
        mysql::TypeBlob,
        mysql::TypeLongBlob,
        mysql::TypeUnspecified,
    ] {
        assert!(IsString(tp), "type {tp} must be string-like");
    }
    assert!(!IsString(mysql::TypeLong));
    assert!(IsStringKind(KindString));
    assert!(IsStringKind(KindBytes));
}

/// Port of the core `fsp_test.go` normalization and rounding cases.
#[test]
fn fsp_normalization_and_rounding_match_go() {
    for (input, expected, expected_error) in [
        (i64::from(UnspecifiedFsp), DefaultFsp, None),
        (-2019, DefaultFsp, Some("Invalid fsp -2019")),
        (
            i64::from(MinFsp) - 4_294_967_296,
            DefaultFsp,
            Some("Invalid fsp -4294967296"),
        ),
        (-1, DefaultFsp, None),
        (i64::from(MaxFsp) + 1, MaxFsp, None),
        (i64::from(MaxFsp) + 2019, MaxFsp, None),
        (i64::from(MaxFsp) + 4_294_967_296, MaxFsp, None),
        (i64::from(MaxFsp + MinFsp) / 2, 3, None),
        (5, 5, None),
    ] {
        let (actual, error) = CheckFsp(input);
        assert_eq!(expected, actual, "fsp {input}");
        assert_eq!(
            expected_error,
            error.as_ref().map(ToString::to_string).as_deref()
        );
    }

    for (input, fsp, expected, overflow) in [
        ("", 5, 0, false),
        ("1235", 6, 123_500, false),
        ("123456", 4, 123_500, false),
        ("1234567", 6, 123_457, false),
        ("1234567", 4, 123_500, false),
        ("1236", 3, 124_000, false),
        ("0312", 2, 30_000, false),
        ("999", 2, 0, true),
    ] {
        let (actual, actual_overflow, error) = ParseFrac(input, fsp);
        assert_eq!(expected, actual, "input {input}");
        assert_eq!(overflow, actual_overflow, "input {input}");
        assert!(error.is_none(), "input {input}: {error:?}");
    }

    let (actual, overflow, error) = ParseFrac("999", -56);
    assert_eq!((0, false), (actual, overflow));
    assert_eq!("Invalid fsp -56", error.unwrap().to_string());
    let (actual, overflow, error) = ParseFrac("NotNum", MaxFsp);
    assert_eq!((0, false), (actual, overflow));
    assert!(error.unwrap().to_string().starts_with("strconv.ParseInt:"));

    for (input, expected) in [
        ("100", "100000"),
        ("10000000000", "10000000000"),
        ("-100", "-100000"),
        ("-10000000000", "-10000000000"),
    ] {
        assert_eq!(expected, AlignFracForTest(input, 6));
    }
}

/// Port of the non-error helper cases from `helper_test.go`.
#[test]
fn numeric_helpers_match_go() {
    for (value, decimal, expected) in [
        (123.45, 0, 123.0),
        (123.45, 1, 123.4),
        (123.45, 2, 123.45),
        (123.45, 3, 123.45),
        (123.45, -400, 0.0),
        (123.45, 400, 123.45),
    ] {
        assert_eq!(expected, Truncate(value, decimal));
    }
    for (value, decimal, expected) in [
        (12.13, -1, "10"),
        (13.15, 0, "13"),
        (0.0, 2, "0"),
        (0.001, 2, "0"),
        (0.539, 2, "0.53"),
        (0.9951, 2, "0.99"),
        (1.0, 2, "1"),
        (-0.456, 2, "-0.45"),
    ] {
        assert_eq!(expected, TruncateFloatToString(value, decimal));
    }
    for (input, expected, expects_error) in [
        ("9223372036854775806", i64::MAX - 1, None),
        ("9223372036854775807", i64::MAX, None),
        ("9223372036854775808", i64::MAX, Some(())),
        ("-9223372036854775807", i64::MIN + 1, None),
        ("-9223372036854775808", i64::MIN, None),
        ("-9223372036854775809", i64::MIN, Some(())),
    ] {
        let (actual, error) = StrToIntForTest(input);
        assert_eq!(expected, actual, "input {input}");
        assert_eq!(expects_error.is_some(), error.is_some(), "input {input}");
        if let Some(error) = error {
            let expected = errors::SharedError::new((**ErrBadNumber).clone());
            assert!(
                errors::ErrorEqual(Some(&error), Some(&expected)),
                "input {input}"
            );
        }
    }
}

/// Field construction and mixed-sign aggregation preserve Go defaults and
/// integer range promotion.
#[test]
fn field_type_defaults_and_aggregation_match_go() {
    let string_type = NewFieldType(mysql::TypeVarchar);
    assert_eq!(mysql::DefaultCharset, string_type.GetCharset());
    assert_eq!(mysql::DefaultCollationName, string_type.GetCollate());

    let signed = NewFieldType(mysql::TypeLong);
    let mut unsigned = NewFieldType(mysql::TypeLong);
    unsigned.SetFlag(mysql::UnsignedFlag);
    let aggregated = AggFieldType(&[&signed, &unsigned]);
    assert_eq!(mysql::TypeLonglong, aggregated.GetType());
    assert_eq!(0, aggregated.GetFlag() & mysql::UnsignedFlag);
}

/// Runtime-value inference covers representative Go type-switch branches.
#[test]
fn value_inference_matches_go_scalar_cases() {
    let mut field_type = FieldType::default();
    DefaultTypeForValue(
        Some(&true),
        &mut field_type,
        mysql::DefaultCharset,
        mysql::DefaultCollationName,
    );
    assert_eq!(mysql::TypeLonglong, field_type.GetType());
    assert_ne!(0, field_type.GetFlag() & mysql::IsBooleanFlag);

    field_type = FieldType::default();
    let text = "中文".to_owned();
    DefaultTypeForValue(
        Some(&text),
        &mut field_type,
        mysql::DefaultCharset,
        mysql::DefaultCollationName,
    );
    assert_eq!(mysql::TypeVarString, field_type.GetType());
    assert_eq!(text.len() as isize, field_type.GetFlen());

    field_type = FieldType::default();
    DefaultTypeForValue(
        None,
        &mut field_type,
        mysql::DefaultCharset,
        mysql::DefaultCollationName,
    );
    assert_eq!(mysql::TypeNull, field_type.GetType());
}

/// Go 的包级错误变量会在 `RegisterFinish` 前完成注册；独立进程保证
/// 本测试不会不可逆地冻结当前测试进程共享的错误注册表。
#[test]
fn standard_errors_are_registered_during_package_initialization() {
    const HELPER_ENV: &str = "TYPES_FIELD_PACKAGE_INIT_HELPER";

    if std::env::var_os(HELPER_ENV).is_some() {
        parser_types::terror::RegisterFinish();
        assert_eq!(ErrTruncated.Code(), errno::WarnDataTruncated as i32);
        assert_eq!(ErrOverflow.Code(), errno::ErrDataOutOfRange as i32);
        assert_eq!(ErrBadNumber.Code(), errno::ErrBadNumber as i32);
        assert_eq!(
            ErrTooBigFieldLength.Code(),
            errno::ErrTooBigFieldlength as i32
        );
        return;
    }

    let status = Command::new(std::env::current_exe().expect("current test executable"))
        .arg("--exact")
        .arg(
            "migration_aster_unit_test::standard_errors_are_registered_during_package_initialization",
        )
        .env(HELPER_ENV, "1")
        .status()
        .expect("run types/internal/field package-initialization helper");
    assert!(
        status.success(),
        "types/internal/field package-initialization helper failed with {status}"
    );
}
