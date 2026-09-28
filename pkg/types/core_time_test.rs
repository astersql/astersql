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

// `CoreTime` 日历与时区调整的单元测试。
//
// 覆盖周行为位标志、周数、儒略日、时间差比较、日期与 Duration 混合、
// 闰年/月末、AddDate 溢出，以及夏令时跳变区间的 adjusted GoTime。

use crate::core_time::gotime;
use crate::core_time::*;

#[test]
/// 周行为位标志常量与 weekBehaviourTest 判定。
fn test_week_behaviour() {
    assert_eq!(1_u32, weekBehaviourMondayFirst);
    assert_eq!(2_u32, weekBehaviourYear);
    assert_eq!(4_u32, weekBehaviourFirstWeekday);
    assert!(weekBehaviourTest(1, weekBehaviourMondayFirst));
    assert!(weekBehaviourTest(2, weekBehaviourYear));
    assert!(weekBehaviourTest(4, weekBehaviourFirstWeekday));
}

#[test]
/// calcWeek 在不同 mode 下的周序号。
fn test_week() {
    for (input, mode, expected) in [
        (FromDate(2008, 2, 20, 0, 0, 0, 0), 0, 7),
        (FromDate(2008, 2, 20, 0, 0, 0, 0), 1, 8),
        (FromDate(2008, 12, 31, 0, 0, 0, 0), 1, 53),
    ] {
        assert_eq!(calcWeek(input, weekMode(mode)).1, expected);
    }
}

#[test]
/// 年月日 → 累计日序号对照表。
fn test_calc_daynr() {
    for (year, month, day, expected) in [
        (0, 0, 0, 0),
        (9999, 12, 31, 3_652_424),
        (1970, 1, 1, 719_528),
        (2006, 12, 16, 733_026),
        (10, 1, 2, 3_654),
        (2008, 2, 20, 733_457),
    ] {
        assert_eq!(calcDaynr(year, month, day), expected);
    }
}

#[test]
/// 两时间点差值（秒/微秒）及符号。
fn test_calc_time_time_diff() {
    for (t1, t2, sign, seconds, micros) in [
        (
            FromDate(2006, 0, 1, 12, 23, 21, 0),
            FromDate(2006, 0, 3, 21, 23, 22, 0),
            1,
            57 * 3_600 + 1,
            0,
        ),
        (
            FromDate(0, 0, 0, 21, 23, 24, 0),
            FromDate(0, 0, 0, 11, 23, 22, 0),
            1,
            10 * 3_600 + 2,
            0,
        ),
        (
            FromDate(0, 0, 0, 1, 2, 3, 0),
            FromDate(0, 0, 0, 5, 2, 0, 0),
            -1,
            6 * 3_600 + 4 * 60 + 3,
            0,
        ),
    ] {
        let actual = calcTimeTimeDiff(t1, t2, sign);
        assert_eq!((actual.0, actual.1), (seconds, micros));
    }
}

#[test]
/// compareTime 反对称性：cmp(a,b) == -cmp(b,a)。
fn test_compare_time() {
    for (left, right, expected) in [
        (
            FromDate(0, 0, 0, 0, 0, 0, 0),
            FromDate(0, 0, 0, 0, 0, 0, 0),
            0,
        ),
        (
            FromDate(0, 0, 0, 0, 1, 0, 0),
            FromDate(0, 0, 0, 0, 0, 0, 0),
            1,
        ),
        (
            FromDate(2006, 1, 2, 3, 4, 5, 6),
            FromDate(2016, 1, 2, 3, 4, 5, 0),
            -1,
        ),
        (
            FromDate(0, 0, 0, 11, 22, 33, 0),
            FromDate(0, 0, 0, 12, 21, 33, 0),
            -1,
        ),
        (
            FromDate(9999, 12, 30, 23, 59, 59, 999_999),
            FromDate(0, 1, 2, 3, 4, 5, 6),
            1,
        ),
    ] {
        assert_eq!(compareTime(left, right), expected);
        assert_eq!(compareTime(right, left), -expected);
    }
}

#[test]
/// 日序号反解年月日，含边界与非法 daynr。
fn test_get_date_from_daynr() {
    for (daynr, expected) in [
        (730_669, (2000, 7, 3)),
        (720_195, (1971, 10, 30)),
        (719_528, (1970, 1, 1)),
        (719_892, (1970, 12, 31)),
        (730_850, (2000, 12, 31)),
        (730_544, (2000, 2, 29)),
        (204_960, (561, 2, 28)),
        (0, (0, 0, 0)),
        (32, (0, 0, 0)),
        (366, (1, 1, 1)),
        (744_729, (2038, 12, 31)),
        (3_652_424, (9999, 12, 31)),
    ] {
        assert_eq!(getDateFromDaynr(daynr), expected);
    }
}

/// 由时分秒微秒构造 Duration 测试夹具（Fsp=6）。
fn duration(hour: i64, minute: i64, second: i64, microsecond: i64) -> Duration {
    Duration {
        Duration: ((hour * 3_600 + minute * 60 + second) * 1_000_000) + microsecond,
        Fsp: 6,
    }
}

#[test]
/// mixDateAndDuration：日期与时长正负叠加并进位。
fn test_mix_date_and_time() {
    for (mut date, dur, negative, expected) in [
        (
            FromDate(1896, 3, 4, 0, 0, 0, 0),
            duration(12, 23, 24, 5),
            false,
            FromDate(1896, 3, 4, 12, 23, 24, 5),
        ),
        (
            FromDate(1896, 3, 4, 0, 0, 0, 0),
            duration(24, 23, 24, 5),
            false,
            FromDate(1896, 3, 5, 0, 23, 24, 5),
        ),
        (
            FromDate(2016, 12, 31, 0, 0, 0, 0),
            duration(24, 0, 0, 0),
            false,
            FromDate(2017, 1, 1, 0, 0, 0, 0),
        ),
        (
            FromDate(2016, 12, 0, 0, 0, 0, 0),
            duration(24, 0, 0, 0),
            false,
            FromDate(2016, 12, 1, 0, 0, 0, 0),
        ),
        (
            FromDate(2017, 1, 12, 3, 23, 15, 0),
            duration(2, 21, 10, 0),
            true,
            FromDate(2017, 1, 12, 1, 2, 5, 0),
        ),
    ] {
        // negative 时对 Duration 取负再混入日期
        let applied = if negative {
            Duration {
                Duration: -dur.Duration,
                ..dur
            }
        } else {
            dur
        };
        mixDateAndDuration(&mut date, applied);
        assert_eq!(compareTime(date, expected), 0);
    }
}

#[test]
/// 闰年判定（含世纪年规则）。
fn test_is_leap_year() {
    for (year, expected) in [
        (1960, true),
        (1963, false),
        (2008, true),
        (2017, false),
        (1988, true),
        (2000, true),
        (1992, true),
        (2024, true),
        (2016, true),
        (2015, false),
        (2014, false),
        (2001, false),
        (1989, false),
    ] {
        assert_eq!(FromDate(year, 1, 1, 0, 0, 0, 0).IsLeapYear(), expected);
    }
}

#[test]
/// 指定年月的最后一天（含闰二月）。
fn test_get_last_day() {
    for (year, month, expected) in [
        (2000, 1, 31),
        (2000, 2, 29),
        (2000, 4, 30),
        (1900, 2, 28),
        (1996, 2, 29),
    ] {
        assert_eq!(GetLastDay(year, month), expected);
    }
}

#[test]
/// getFixDays：AddDate 月末修正天数。
fn test_get_fix_days() {
    for (year, month, day, old_time, expected) in [
        (
            2000,
            1,
            0,
            gotime::Date(2000, 1, 31, 0, 0, 0, 0, gotime::UTC),
            -2,
        ),
        (
            2000,
            1,
            12,
            gotime::Date(2000, 1, 31, 0, 0, 0, 0, gotime::UTC),
            0,
        ),
        (
            2000,
            1,
            12,
            gotime::Date(2000, 1, 0, 0, 0, 0, 0, gotime::UTC),
            0,
        ),
        (
            2000,
            2,
            24,
            gotime::Date(2000, 2, 10, 0, 0, 0, 0, gotime::UTC),
            0,
        ),
        (
            2019,
            4,
            5,
            gotime::Date(2019, 4, 1, 1, 2, 3, 4, gotime::UTC),
            0,
        ),
    ] {
        assert_eq!(getFixDays(year, month, day, old_time), expected);
    }
}

#[test]
/// AddDate 正常进位与超范围溢出错误。
fn test_add_date() {
    let base = gotime::Date(2000, 1, 1, 0, 0, 0, 0, gotime::UTC);
    for (year, month, day, old_time, has_error) in [
        (1, 1, 0, base, false),
        (2, 1, 12, base, false),
        (3, 1, 12, base, false),
        (
            4,
            2,
            24,
            gotime::Date(2000, 2, 10, 0, 0, 0, 0, gotime::UTC),
            false,
        ),
        (
            1,
            4,
            5,
            gotime::Date(2019, 4, 1, 1, 2, 3, 4, gotime::UTC),
            false,
        ),
        (7999, 1, 1, base, false),
        (-2000, 1, 1, base, false),
        (8000, 1, 1, base, true),
        (10_001 * 365, 1, 1, base, true),
        (1, 10_001 * 36, 1, base, true),
        (1, 1, 10_001 * 365, base, true),
        (-2001, 1, 1, base, true),
        (-10_001 * 365, 1, 1, base, true),
        (1, -10_001 * 36, 1, base, true),
        (1, 1, -10_001 * 365, base, true),
    ] {
        let (result, error) = AddDate(year, month, day, old_time);
        assert_eq!(error.is_some(), has_error);
        if !has_error {
            assert_eq!(result.Year(), year as i32 + old_time.Year());
        }
    }
}

#[test]
/// Weekday 名称；含非法日历日的推算结果。
fn test_weekday() {
    for (input, expected) in [
        (FromDate(2019, 1, 1, 0, 0, 0, 0), "Tuesday"),
        (FromDate(2019, 2, 31, 0, 0, 0, 0), "Sunday"),
        (FromDate(2019, 4, 31, 0, 0, 0, 0), "Wednesday"),
    ] {
        assert_eq!(input.Weekday().to_string(), expected);
    }
}

/// 格式化为接近 Go 的日期时间+时区偏移字符串。
fn go_format(value: gotime::Time) -> String {
    let (year, month, day) = value.Date();
    let (hour, minute, second) = value.Clock();
    let mut text = format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}");
    let micros = value.Nanosecond() / 1_000;
    if micros != 0 {
        let fraction = format!("{micros:06}").trim_end_matches('0').to_owned();
        text.push('.');
        text.push_str(&fraction);
    }
    let (name, offset) = value.Zone();
    let sign = if offset < 0 { '-' } else { '+' };
    let absolute = offset.abs();
    format!(
        "{text} {name} {sign}{:02}{:02}",
        absolute / 3_600,
        absolute % 3_600 / 60
    )
}

#[test]
/// 多时区 DST 跳变前后的 adjusted 时刻与非法日期。
fn test_adjusted_go_time() {
    for (zone, value, expected, success) in [
        (
            "Australia/Lord_Howe",
            FromDate(2020, 10, 4, 1, 59, 59, 997),
            "2020-10-04 01:59:59.000997 +1030 +1030",
            true,
        ),
        (
            "Australia/Lord_Howe",
            FromDate(2020, 10, 4, 2, 0, 0, 0),
            "2020-10-04 02:30:00 +11 +1100",
            true,
        ),
        (
            "Australia/Lord_Howe",
            FromDate(2020, 10, 4, 2, 15, 0, 0),
            "2020-10-04 02:30:00 +11 +1100",
            true,
        ),
        (
            "Australia/Lord_Howe",
            FromDate(2020, 10, 4, 2, 29, 59, 999_999),
            "2020-10-04 02:30:00 +11 +1100",
            true,
        ),
        (
            "Australia/Lord_Howe",
            FromDate(2020, 10, 4, 2, 30, 0, 1),
            "2020-10-04 02:30:00.000001 +11 +1100",
            true,
        ),
        (
            "Australia/Lord_Howe",
            FromDate(2020, 6, 29, 3, 45, 0, 0),
            "2020-06-29 03:45:00 +1030 +1030",
            true,
        ),
        (
            "Australia/Lord_Howe",
            FromDate(2020, 4, 4, 1, 45, 0, 0),
            "2020-04-04 01:45:00 +11 +1100",
            true,
        ),
        (
            "Europe/Vilnius",
            FromDate(2020, 3, 29, 3, 45, 0, 0),
            "2020-03-29 04:00:00 EEST +0300",
            true,
        ),
        (
            "Europe/Vilnius",
            FromDate(2020, 3, 29, 3, 59, 59, 456_789),
            "2020-03-29 04:00:00 EEST +0300",
            true,
        ),
        (
            "Europe/Vilnius",
            FromDate(2020, 3, 29, 4, 0, 1, 130_000),
            "2020-03-29 04:00:01.13 EEST +0300",
            true,
        ),
        (
            "Europe/Vilnius",
            FromDate(2020, 10, 25, 3, 45, 0, 0),
            "2020-10-25 03:45:00 EET +0200",
            true,
        ),
        (
            "Europe/Vilnius",
            FromDate(2020, 6, 29, 3, 45, 0, 0),
            "2020-06-29 03:45:00 EEST +0300",
            true,
        ),
        (
            "Europe/Amsterdam",
            FromDate(2020, 3, 29, 2, 45, 0, 0),
            "2020-03-29 03:00:00 CEST +0200",
            true,
        ),
        (
            "Europe/Amsterdam",
            FromDate(2020, 10, 25, 2, 35, 0, 0),
            "2020-10-25 02:35:00 CET +0100",
            true,
        ),
        ("UTC", FromDate(2020, 2, 31, 2, 35, 0, 0), "", false),
    ] {
        let zone = gotime::LoadLocation(zone).unwrap();
        let (actual, error) = value.AdjustedGoTime(zone);
        assert_eq!(error.is_none(), success);
        if error.is_none() {
            assert_eq!(go_format(actual), expected);
        }
    }
}
