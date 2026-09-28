// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 标量转换与区间枚举的单元测试。

use crate::*;

fn decimal_datum(value: f64) -> types::Datum {
    let mut decimal = types::MyDecimal::default();
    decimal.FromFloat64(value).unwrap();
    types::NewDecimalDatum(decimal)
}

fn time_datum(year: i32, month: i32, day: i32, time_type: u8) -> types::Datum {
    types::NewTimeDatum(types::NewTime(
        types::FromDate(year, month, day, 0, 0, 0, 0),
        time_type,
        types::DefaultFsp,
    ))
}

/// 验证 calcFraction 边界、公共前缀长度与字节标量序。
#[test]
fn scalar_conversion_and_fraction_match_boundaries() {
    assert_eq!(calcFraction(0.0, 10.0, -1.0), 0.0);
    assert_eq!(calcFraction(0.0, 10.0, 5.0), 0.5);
    assert_eq!(calcFraction(0.0, 10.0, 12.0), 1.0);
    assert_eq!(calcFraction(0.0, 10.0, f64::NAN), 0.5);
    assert_eq!(commonPrefixLength(&[b"abcd".to_vec(), b"abxy".to_vec()]), 2);
    assert!(convertBytesToScalar(b"ab") < convertBytesToScalar(b"ac"));
}

/// 覆盖 Go `TestCalcFraction` 的 Datum 类型矩阵和期望比例。
#[test]
fn datum_fraction_matches_go_type_matrix() {
    let cases = [
        (
            types::NewIntDatum(0),
            types::NewIntDatum(4),
            types::NewIntDatum(1),
            0.25,
        ),
        (
            types::NewUintDatum(0),
            types::NewUintDatum(4),
            types::NewUintDatum(1),
            0.25,
        ),
        (
            types::NewFloat64Datum(0.0),
            types::NewFloat64Datum(4.0),
            types::NewFloat64Datum(1.0),
            0.25,
        ),
        (
            types::NewFloat32Datum(0.0),
            types::NewFloat32Datum(4.0),
            types::NewFloat32Datum(1.0),
            0.25,
        ),
        (
            decimal_datum(0.0),
            decimal_datum(4.0),
            decimal_datum(1.0),
            0.25,
        ),
        (
            types::NewDurationDatum(types::Duration {
                Duration: 0,
                Fsp: 0,
            }),
            types::NewDurationDatum(types::Duration {
                Duration: 14_400_000_000_000,
                Fsp: 0,
            }),
            types::NewDurationDatum(types::Duration {
                Duration: 3_600_000_000_000,
                Fsp: 0,
            }),
            0.25,
        ),
        (
            time_datum(2017, 1, 1, types::mysql::TypeTimestamp),
            time_datum(2017, 4, 1, types::mysql::TypeTimestamp),
            time_datum(2017, 2, 1, types::mysql::TypeTimestamp),
            0.344_444_444_444_444_44,
        ),
        (
            time_datum(2017, 1, 1, types::mysql::TypeDatetime),
            time_datum(2017, 4, 1, types::mysql::TypeDatetime),
            time_datum(2017, 2, 1, types::mysql::TypeDatetime),
            0.344_444_444_444_444_44,
        ),
        (
            time_datum(2017, 1, 1, types::mysql::TypeDate),
            time_datum(2017, 4, 1, types::mysql::TypeDate),
            time_datum(2017, 2, 1, types::mysql::TypeDate),
            0.344_444_444_444_444_44,
        ),
        (
            types::NewStringDatum("aasad".to_owned()),
            types::NewStringDatum("addad".to_owned()),
            types::NewStringDatum("abfsd".to_owned()),
            0.322_802_539_840_637_45,
        ),
        (
            types::NewBytesDatum(b"aasad".to_vec()),
            types::NewBytesDatum(b"asdff".to_vec()),
            types::NewBytesDatum(b"abfsd".to_vec()),
            0.052_921_680_221_726_9,
        ),
    ];

    for (lower, upper, value, expected) in cases {
        let actual = calcFraction4Datums(&lower, &upper, &value);
        assert!(
            (actual - expected).abs() <= 1e-9,
            "expected {expected}, got {actual}"
        );
    }
}

/// 小整数开区间应枚举中间值；过大区间返回 None。
#[test]
fn small_integer_ranges_are_enumerated_with_exclusions() {
    let values = EnumRangeValues(types::NewIntDatum(1), types::NewIntDatum(5), true, true).unwrap();
    assert_eq!(
        values
            .iter()
            .map(types::Datum::GetInt64)
            .collect::<Vec<_>>(),
        vec![2, 3, 4]
    );
    assert!(
        EnumRangeValues(types::NewIntDatum(1), types::NewIntDatum(100), false, false,).is_none()
    );
}

/// Duration 下界按 FSP 舍入；DATE 区间按天枚举并清除下界时分秒。
#[test]
fn temporal_ranges_follow_fractional_precision() {
    let first_day = types::NewTimeDatum(types::NewTime(
        types::FromDate(2024, 1, 1, 0, 0, 0, 0),
        types::mysql::TypeDatetime,
        0,
    ));
    let second_day = types::NewTimeDatum(types::NewTime(
        types::FromDate(2024, 1, 2, 0, 0, 0, 0),
        types::mysql::TypeDatetime,
        0,
    ));
    assert_eq!(
        convertDatumToScalar(&second_day, 0) - convertDatumToScalar(&first_day, 0),
        86_400_000_000_000.0
    );

    let durations = EnumRangeValues(
        types::NewDurationDatum(types::Duration {
            Duration: 600_000_000,
            Fsp: 0,
        }),
        types::NewDurationDatum(types::Duration {
            Duration: 3_000_000_000,
            Fsp: 0,
        }),
        false,
        true,
    )
    .unwrap();
    assert_eq!(
        durations
            .iter()
            .map(|value| value.GetMysqlDuration().Duration)
            .collect::<Vec<_>>(),
        vec![1_000_000_000, 2_000_000_000]
    );

    let dates = EnumRangeValues(
        types::NewTimeDatum(types::NewTime(
            types::FromDate(2024, 1, 1, 12, 34, 56, 0),
            types::mysql::TypeDate,
            0,
        )),
        types::NewTimeDatum(types::NewTime(
            types::FromDate(2024, 1, 4, 0, 0, 0, 0),
            types::mysql::TypeDate,
            0,
        )),
        false,
        true,
    )
    .unwrap();
    assert_eq!(
        dates
            .iter()
            .map(|value| {
                let value = value.GetMysqlTime();
                (value.Day(), value.Hour(), value.Minute(), value.Second())
            })
            .collect::<Vec<_>>(),
        vec![(1, 0, 0, 0), (2, 0, 0, 0), (3, 0, 0, 0)]
    );

    assert!(
        EnumRangeValues(
            types::NewTimeDatum(types::NewTime(
                types::FromDate(1, 1, 1, 0, 0, 0, 0),
                types::mysql::TypeDate,
                0,
            )),
            types::NewTimeDatum(types::NewTime(
                types::FromDate(9999, 12, 31, 0, 0, 0, 0),
                types::mysql::TypeDate,
                0,
            )),
            false,
            false,
        )
        .is_none()
    );
}

/// Go 整数运算会在边界回绕，小区间枚举也应保持该行为。
#[test]
fn integer_ranges_wrap_at_machine_boundaries() {
    let signed = EnumRangeValues(
        types::NewIntDatum(i64::MAX),
        types::NewIntDatum(i64::MIN),
        false,
        false,
    )
    .unwrap();
    assert_eq!(
        signed
            .iter()
            .map(types::Datum::GetInt64)
            .collect::<Vec<_>>(),
        vec![i64::MAX, i64::MIN]
    );

    let unsigned = EnumRangeValues(
        types::NewUintDatum(u64::MAX),
        types::NewUintDatum(1),
        false,
        false,
    )
    .unwrap();
    assert_eq!(
        unsigned
            .iter()
            .map(types::Datum::GetUint64)
            .collect::<Vec<_>>(),
        vec![u64::MAX, 0, 1]
    );
}

/// 补齐 Go `TestEnumRangeValues` 的时间类型和空区间场景。
#[test]
fn enum_ranges_match_go_temporal_and_empty_cases() {
    for time_type in [types::mysql::TypeTimestamp, types::mysql::TypeDatetime] {
        let values = EnumRangeValues(
            types::NewTimeDatum(types::NewTime(
                types::FromDate(2017, 1, 1, 0, 0, 0, 0),
                time_type,
                0,
            )),
            types::NewTimeDatum(types::NewTime(
                types::FromDate(2017, 1, 1, 0, 0, 5, 0),
                time_type,
                0,
            )),
            false,
            true,
        )
        .unwrap();
        assert_eq!(
            values
                .iter()
                .map(|value| value.GetMysqlTime().Second())
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );
    }

    assert!(
        EnumRangeValues(
            types::NewIntDatum(i64::MIN),
            types::NewIntDatum(0),
            false,
            false
        )
        .is_none()
    );
    assert!(
        EnumRangeValues(
            time_datum(2017, 1, 1, types::mysql::TypeDate),
            time_datum(2017, 1, 1, types::mysql::TypeDate),
            true,
            true,
        )
        .is_none()
    );
}
