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

fn decimal(value: &str) -> MyDecimal {
    let mut decimal = MyDecimal::default();
    decimal.FromString(value.as_bytes()).unwrap();
    decimal
}

/// 对齐 Go DECIMAL 转无符号整数的四舍五入路径。
#[test]
fn decimal_to_uint_matches_go_rounding() {
    assert_eq!(
        ConvertDecimalToUint(&decimal("1.5"), u64::MAX, mysql::TypeLonglong).unwrap(),
        2
    );
}

/// 完整复现 Go `TestNumberToDuration`。
#[test]
fn number_to_duration_matches_go_cases() {
    let cases = [
        (20_171_222, 0, true, (0, 0, 0)),
        (171_222, 0, false, (17, 12, 22)),
        (20_171_222_020_005, 0, false, (2, 0, 5)),
        (10_000_000_000, 0, true, (0, 0, 0)),
        (171_222, 1, false, (17, 12, 22)),
        (176_022, 1, true, (0, 0, 0)),
        (8_391_222, 1, true, (0, 0, 0)),
        (8_381_222, 0, false, (838, 12, 22)),
        (1_001_222, 0, false, (100, 12, 22)),
        (171_260, 1, true, (0, 0, 0)),
    ];
    for (number, fsp, has_error, expected) in cases {
        match NumberToDuration(number, fsp) {
            Ok(duration) => {
                assert!(!has_error, "{number} should fail");
                assert_eq!(
                    (duration.Hour(), duration.Minute(), duration.Second()),
                    expected,
                    "number={number}, fsp={fsp}"
                );
            }
            Err(_) => assert!(has_error, "{number} should succeed"),
        }
    }

    let positive = NumberToDuration(171_222, 0).unwrap();
    let negative = NumberToDuration(-171_222, 0).unwrap();
    assert_eq!(negative.Duration, -positive.Duration);
}

/// 完整复现 Go `TestStrToDuration` 的 DATETIME/Duration 判别。
#[test]
fn str_to_duration_matches_go_cases() {
    let cases = [
        ("20190412120000", 4, false),
        ("20190101180000", 6, false),
        ("20190101180000", 1, false),
        ("20190101181234", 3, false),
        ("00:00:00.000000", 6, true),
        ("00:00:00", 0, true),
    ];
    for (value, fsp, expected) in cases {
        let (_, _, is_duration) =
            StrToDuration(DefaultStmtNoWarningContext.clone(), value, fsp).unwrap();
        assert_eq!(is_duration, expected, "value={value}, fsp={fsp}");
    }
}

/// 完整复现 Go `TestConvertJSONToInt`，包括失败时的最佳努力值。
#[test]
fn json_to_int_matches_go_cases() {
    let cases = [
        ("{}", 0, true),
        ("[]", 0, true),
        ("3", 3, false),
        ("-3", -3, false),
        ("4.5", 4, false),
        ("true", 1, false),
        ("false", 0, false),
        ("null", 0, true),
        (r#""hello""#, 0, true),
        (r#""123hello""#, 123, true),
        (r#""1234""#, 1234, false),
    ];
    for (input, expected, has_error) in cases {
        let json = ParseBinaryJSONFromString(input).unwrap();
        match ConvertJSONToInt64(DefaultStmtNoWarningContext.clone(), json, false) {
            Ok(value) => {
                assert!(!has_error, "{input} should fail");
                assert_eq!(value, expected, "input={input}");
            }
            Err(error) => {
                assert!(has_error, "{input} should succeed");
                assert_eq!(error.value, expected, "input={input}");
            }
        }
    }
}

/// 完整复现 Go `TestConvertJSONToFloat`。
#[test]
fn json_to_float_matches_go_cases() {
    let cases = [
        (CreateBinaryJSON(Vec::<JsonValue>::new()), 0.0, true),
        (CreateBinaryJSON(3_i64), 3.0, false),
        (CreateBinaryJSON(-3_i64), -3.0, false),
        (CreateBinaryJSON(1_u64 << 63), (1_u64 << 63) as f64, false),
        (CreateBinaryJSON(4.5_f64), 4.5, false),
        (CreateBinaryJSON(true), 1.0, false),
        (CreateBinaryJSON(false), 0.0, false),
        (CreateBinaryJSON(()), 0.0, true),
        (CreateBinaryJSON("hello"), 0.0, true),
        (CreateBinaryJSON("123.456hello"), 123.456, true),
        (CreateBinaryJSON("1234"), 1234.0, false),
    ];
    let object = ParseBinaryJSONFromString("{}").unwrap();
    for (json, expected, has_error) in std::iter::once((object, 0.0, true)).chain(cases) {
        match ConvertJSONToFloat(DefaultStmtNoWarningContext.clone(), json) {
            Ok(value) => {
                assert!(!has_error);
                assert_eq!(value, expected);
            }
            Err(error) => {
                assert!(has_error);
                assert_eq!(error.value, expected);
            }
        }
    }
}

/// 完整复现 Go `TestConvertJSONToDecimal`。
#[test]
fn json_to_decimal_matches_go_cases() {
    let cases = [
        ("3", "3", false),
        ("-3", "-3", false),
        ("4.5", "4.5", false),
        (r#""1234""#, "1234", false),
        (
            r#""1234567890123456789012345678901234567890123456789012345""#,
            "1234567890123456789012345678901234567890123456789012345",
            false,
        ),
        ("true", "1", false),
        ("false", "0", false),
        ("null", "0", true),
    ];
    for (input, expected, has_error) in cases {
        let json = ParseBinaryJSONFromString(input).unwrap();
        match ConvertJSONToDecimal(DefaultStmtNoWarningContext.clone(), json) {
            Ok(value) => {
                assert!(!has_error, "{input} should fail");
                assert_eq!(value.String(), expected, "input={input}");
            }
            Err(error) => {
                assert!(has_error, "{input} should succeed");
                assert_eq!(error.value.String(), expected, "input={input}");
            }
        }
    }
}
