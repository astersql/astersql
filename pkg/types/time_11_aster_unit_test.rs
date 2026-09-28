// Copyright 2026 AsterSQL.
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

// MySQL 时间/时长类型核心行为的 Aster 迁移单元测试。
//
// 覆盖 CoreTime 位布局、FSP/时区后缀、日期拆分、EXTRACT、解析打包比较、
// 零日期、Duration 运算、区间单位分类、TIMESTAMP 边界与数值时间回归。

use astersql_types_time::{
    AdjustYear, BasicTimeContext, CheckTimestampTypeForTest, DateFSP, Duration, ExtractDurationNum,
    ExtractDurationValue, FromDate, FromDateChecked, GetFormatType, GetFracIndex, GetFsp,
    GetTimezone, IsClockUnit, IsDateFormat, IsDateUnit, IsMicrosecondUnit, NewDuration, NewTime,
    ParseDateFormat, ParseDatetime, ParseDuration, ParseDurationValue, ParseTime,
    ParseTimeFromDecimal, ParseTimeFromFloat64, ParseTimeFromFloatString, ParseTimeFromInt64,
    StrictContext, Time, TimeFlags, mysql,
};
use chrono::TimeZone;
use rust_decimal::Decimal;
use std::str::FromStr;

#[test]
/// CoreTime 字段打包与 Time 元数据（类型/FSP）对齐 Go。
fn core_time_and_metadata_match_go_bit_layout() {
    let core = FromDate(2024, 2, 29, 23, 58, 57, 654_321);
    assert_eq!((core.Year(), core.Month(), core.Day()), (2024, 2, 29));
    assert_eq!((core.Hour(), core.Minute(), core.Second()), (23, 58, 57));
    assert_eq!(core.Microsecond(), 654_321);

    assert!(FromDateChecked(16_383, 15, 31, 31, 63, 63, 1_048_575).1);
    assert!(!FromDateChecked(16_384, 1, 1, 0, 0, 0, 0).1);
    assert!(!FromDateChecked(-1, 1, 1, 0, 0, 0, 0).1);

    let mut t = NewTime(core, mysql::TypeDatetime, 6);
    assert_eq!(t.Type(), mysql::TypeDatetime);
    assert_eq!(t.Fsp(), 6);
    t.SetType(mysql::TypeTimestamp);
    assert_eq!(t.Type(), mysql::TypeTimestamp);
    t.SetFsp(3);
    assert_eq!(t.Fsp(), 3);
    t.SetType(mysql::TypeDate);
    assert_eq!(t.Fsp(), 0);
}

#[test]
/// FSP 推断、小数点位置与时区后缀解析对齐 Go。
fn fractional_precision_and_timezone_suffix_match_go() {
    assert_eq!(GetFsp("2020-01-01 00:00:00"), 0);
    assert_eq!(GetFsp("2020-01-01 00:00:00.123456789+05:30"), 6);
    assert_eq!(GetFracIndex("2019.01.01 00:00:00"), -1);
    assert_eq!(GetFracIndex("2020-01-01 12:00:00.123456-05:00"), 19);
    assert_eq!(DateFSP("12:00:00.123"), 3);

    assert_eq!(
        GetTimezone("2020-01-01 12:00:00Z"),
        (19, "".into(), "".into(), "".into(), "".into())
    );
    assert_eq!(
        GetTimezone("2020-01-01 12:00:00+08:30"),
        (19, "+".into(), "08".into(), ":".into(), "30".into())
    );
    assert_eq!(
        GetTimezone("2020-01-01"),
        (-1, "".into(), "".into(), "".into(), "".into())
    );
}

#[test]
/// ParseDateFormat 拆分与 GetFormatType 判定对齐 Go。
fn date_component_splitting_matches_go() {
    assert_eq!(
        ParseDateFormat(" 2024-02-29T12:34:56 "),
        vec!["2024", "02", "29", "12", "34", "56"]
    );
    assert!(ParseDateFormat("x2024-02-29").is_empty());
    assert_eq!(GetFormatType("%Y-%m-%d %H:%i:%s"), (true, true));
    assert_eq!(GetFormatType("%H:%i:%s"), (true, false));
}

#[test]
/// EXTRACT(WEEK) 使用 MySQL week mode 0。
fn extract_week_uses_mysql_mode_zero() {
    let value = NewTime(FromDate(2019, 4, 12, 14, 0, 0, 0), mysql::TypeTimestamp, 0);
    assert_eq!(
        astersql_types_time::ExtractDatetimeNum(&value, "week").unwrap(),
        14
    );
}

#[test]
/// 解析、DateFormat、打包/解包与 RoundFrac 对齐 Go。
fn datetime_parse_format_pack_and_compare_match_go() {
    let ctx = StrictContext;
    let t = ParseTime(&ctx, "2024-02-29 23:59:58.123456", mysql::TypeDatetime, 6).unwrap();
    assert_eq!(t.String(), "2024-02-29 23:59:58.123456");
    assert_eq!(
        t.DateFormat("%W, %M %D %Y %r").unwrap(),
        "Thursday, February 29th 2024 11:59:58 PM"
    );

    let packed = t.ToPackedUint().unwrap();
    let mut unpacked = Time::default();
    unpacked.SetType(mysql::TypeDatetime);
    unpacked.SetFsp(6);
    unpacked.FromPackedUint(packed).unwrap();
    assert_eq!(t.Compare(unpacked), 0);

    let rounded = t.RoundFrac(&ctx, 3).unwrap();
    assert_eq!(rounded.String(), "2024-02-29 23:59:58.123");
}

#[test]
/// 零日期与 ignore_zero_in_date 标志行为对齐 Go。
fn zero_date_validation_matches_go_context_flags() {
    let zero = ParseDatetime(&StrictContext, "0000-00-00 00:00:00").unwrap();
    assert_eq!(zero.String(), "0000-00-00 00:00:00");

    let zero_in_date_context = BasicTimeContext {
        flags: TimeFlags {
            ignore_zero_in_date: true,
            ..TimeFlags::default()
        },
        location: chrono_tz::UTC,
    };
    let partial_zero = ParseDatetime(&zero_in_date_context, "2017-00-05 23:59:58.575601").unwrap();
    assert_eq!(partial_zero.String(), "2017-00-05 23:59:58.575601");
}

#[test]
/// Duration 解析、加减与格式化对齐 Go。
fn duration_parse_arithmetic_and_format_match_go() {
    let ctx = StrictContext;
    let (d, is_null) = ParseDuration(&ctx, "-2 03:04:05.600000", 6).unwrap();
    assert!(!is_null);
    assert_eq!(d.String(), "-51:04:05.600000");
    assert_eq!(
        (d.Hour(), d.Minute(), d.Second(), d.MicroSecond()),
        (51, 4, 5, 600_000)
    );
    assert_eq!(d.DurationFormat("%H:%i:%s.%f").unwrap(), "51:04:05.600000");

    let sum = d.Add(Duration::from_parts(52, 0, 0, 0, 6)).unwrap();
    assert_eq!(sum.String(), "00:55:54.400000");
    assert_eq!(sum.Neg().String(), "-00:55:54.400000");
}

#[test]
/// 区间值解析、EXTRACT Duration 与两位年份调整。
fn interval_value_parsing_and_year_adjustment_match_go() {
    assert_eq!(AdjustYear(69, false).unwrap(), 2069);
    assert_eq!(AdjustYear(70, false).unwrap(), 1970);
    assert_eq!(AdjustYear(0, false).unwrap(), 0);
    assert_eq!(AdjustYear(0, true).unwrap(), 2000);
    assert!(AdjustYear(2156, false).is_err());

    assert_eq!(
        ParseDurationValue("DAY_SECOND", "2 03:04:05").unwrap(),
        (0, 0, 2, 11_045_000_000_000, 0)
    );
    let extracted = ExtractDurationValue("HOUR_MICROSECOND", "12:34:56.1234").unwrap();
    assert_eq!(extracted.String(), "12:34:56.123400");

    assert_eq!(
        ParseDurationValue("MINUTE_SECOND", "35:10567890").unwrap(),
        (0, 0, 122, 29_190_000_000_000, 0)
    );
    assert_eq!(
        ParseDurationValue("DAY_MICROSECOND", "12 14:00:00.345").unwrap(),
        (0, 0, 12, 50_400_345_000_000, 6)
    );
    assert_eq!(
        ExtractDurationValue("SECOND_MICROSECOND", "61.01")
            .unwrap()
            .String(),
        "00:01:01.010000"
    );
    assert_eq!(
        ExtractDurationValue("MONTH", "1").unwrap().String(),
        "720:00:00"
    );
    assert!(ExtractDurationValue("DAY", "-35").is_err());
}

#[test]
/// 时钟/日期/微秒单位分类（大小写折叠）对齐 Go。
fn interval_unit_classification_matches_go_case_folding_and_enums() {
    for unit in [
        "MICROSECOND",
        "SECOND",
        "MINUTE",
        "HOUR",
        "SECOND_MICROSECOND",
        "MINUTE_MICROSECOND",
        "MINUTE_SECOND",
        "HOUR_MICROSECOND",
        "HOUR_SECOND",
        "HOUR_MINUTE",
        "DAY_MICROSECOND",
        "DAY_SECOND",
        "DAY_MINUTE",
        "DAY_HOUR",
    ] {
        assert!(IsClockUnit(unit), "{unit}");
    }
    assert!(!IsClockUnit("TEST"));
    assert!(!IsClockUnit("SOME_MICROSECOND"));

    for unit in [
        "Day",
        "Week",
        "month",
        "quarter",
        "YEAR",
        "DAY_MICROSECOND",
        "DAY_SECOND",
        "DAY_MINUTE",
        "DAY_HOUR",
        "YEAR_MONTH",
    ] {
        assert!(IsDateUnit(unit), "{unit}");
    }
    assert!(!IsDateUnit("MICROSECOND"));
    assert!(!IsDateUnit("SOME_DAY"));

    for unit in [
        "Microsecond",
        "Second_microsecond",
        "minute_microsecond",
        "hour_microsecond",
        "DAY_MICROSECOND",
    ] {
        assert!(IsMicrosecondUnit(unit), "{unit}");
    }
    assert!(!IsMicrosecondUnit("SECOND"));
    assert!(!IsMicrosecondUnit("SOME_MICROSECOND"));
}

#[test]
/// IsDateFormat 字面量形态判定对齐 Go。
fn date_literal_classification_matches_go_split_rules() {
    assert!(!IsDateFormat("1234:321"));
    assert!(IsDateFormat("2019-04-01"));
    assert!(IsDateFormat("2019-4-1"));
    assert!(IsDateFormat("20129"));
}

#[test]
/// TIMESTAMP 上下界与 DST 空洞校验对齐 Go。
fn timestamp_bounds_and_dst_gaps_match_go() {
    let cases = [
        (
            chrono_tz::Asia::Shanghai,
            FromDate(2038, 1, 19, 11, 14, 7, 0),
            false,
        ),
        (
            chrono_tz::Asia::Shanghai,
            FromDate(2038, 1, 19, 12, 14, 7, 0),
            true,
        ),
        (chrono_tz::UTC, FromDate(2038, 1, 19, 3, 14, 7, 0), false),
        (chrono_tz::UTC, FromDate(2038, 1, 19, 4, 14, 7, 0), true),
        (
            chrono_tz::America::Los_Angeles,
            FromDate(2018, 3, 11, 1, 0, 50, 0),
            false,
        ),
        (
            chrono_tz::America::Los_Angeles,
            FromDate(2018, 3, 11, 2, 0, 16, 0),
            true,
        ),
        (
            chrono_tz::Europe::London,
            FromDate(2019, 3, 31, 1, 0, 20, 0),
            true,
        ),
    ];

    for (location, core, expect_error) in cases {
        assert_eq!(
            CheckTimestampTypeForTest(core, location).is_err(),
            expect_error,
            "{core:?} in {location}"
        );
    }
}

#[test]
/// 数值时间/时长与 ConvertToYearFromNow 等回归用例。
fn numeric_time_and_duration_regressions_match_go() {
    let negative = Duration {
        Duration: -39_541_000_000_000,
        Fsp: 0,
    };
    assert_eq!(ExtractDurationNum(&negative, "MICROSECOND").unwrap(), 0);
    assert_eq!(
        ExtractDurationNum(&negative, "MINUTE_SECOND").unwrap(),
        -5901
    );
    assert_eq!(ExtractDurationNum(&negative, "day_hour").unwrap(), -10);

    let (day_hour, _) = ParseDuration(&StrictContext, "-24 10", 0).unwrap();
    assert_eq!(day_hour.String(), "-586:00:00");

    let date = ParseTimeFromInt64(&StrictContext, 20_000_102).unwrap();
    assert_eq!(date.Type(), mysql::TypeDate);
    assert_eq!(date.String(), "2000-01-02");
    assert_eq!(
        ParseTimeFromFloat64(&StrictContext, 20_000_102.9)
            .unwrap()
            .String(),
        "2000-01-02"
    );
    let decimal = Decimal::from_str("20000102030405.0078125").unwrap();
    assert_eq!(
        ParseTimeFromDecimal(&StrictContext, &decimal)
            .unwrap()
            .String(),
        "2000-01-02 03:04:05.007812"
    );
    assert_eq!(
        ParseTimeFromFloatString(&StrictContext, "20170118.123", mysql::TypeDatetime, 3)
            .unwrap()
            .String(),
        "2017-01-18 00:00:00.000"
    );

    let concat_ctx = BasicTimeContext {
        flags: TimeFlags {
            cast_time_to_year_through_concat: true,
            ..Default::default()
        },
        location: chrono_tz::UTC,
    };
    let now = chrono_tz::UTC
        .with_ymd_and_hms(2023, 11, 13, 3, 9, 0)
        .unwrap();
    assert_eq!(
        NewDuration(0, 20, 12, 0, 0)
            .ConvertToYearFromNow(&concat_ctx, now)
            .unwrap(),
        2012
    );
}
