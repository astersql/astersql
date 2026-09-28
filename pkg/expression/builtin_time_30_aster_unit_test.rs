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

// 日期时间标量内核的 Aster 单元测试。
//
// 对照 Go `builtin_time`：覆盖 duration 识别与 FSP、加减时间、PERIOD 换算、
// GET_FORMAT/TIME_FORMAT、MAKEDATE/MAKETIME、TIMESTAMPADD、时区转换、
// TSO（Timestamp Oracle，全局时间戳）解析与有界陈旧读辅助。

use crate::builtin_time::*;
use chrono::{NaiveDate, NaiveDateTime};

/// 解析日期时间字符串为 NaiveDateTime，失败则 panic。
fn dt(value: &str) -> NaiveDateTime {
    parse_datetime(value).unwrap()
}

#[test]
/// duration 词法识别与 ADDTIME/SUBTIME 的 FSP 规则。
fn duration_detection_and_fsp_match_go_cases() {
    for value in ["110:00:00", "1 01:00:00", "01:00:00.999999"] {
        assert!(is_duration(value), "{value}");
    }
    for value in [
        "aa:bb:cc",
        "071231235959.999999",
        "20171231235959.999999",
        "2017-01-01 01:01:01.11",
    ] {
        assert!(!is_duration(value), "{value}");
    }
    assert_eq!(get_fsp_for_time_add_sub("10:11:12"), 0);
    assert_eq!(get_fsp_for_time_add_sub("10:11:12.000000"), 0);
    assert_eq!(get_fsp_for_time_add_sub("10:11:12.000001"), 6);
}

#[test]
/// DURATION 加减保留符号、天与小数秒。
fn mysql_duration_arithmetic_preserves_sign_days_and_fraction() {
    assert_eq!(
        add_time_strings("01:00:00.999999", "02:00:00.999998").unwrap(),
        "03:00:01.999997"
    );
    assert_eq!(
        add_time_strings("110:00:00", "1 02:00:00").unwrap(),
        "136:00:00"
    );
    assert_eq!(
        add_time_strings("-110:00:00", "1 02:00:00").unwrap(),
        "-84:00:00"
    );
    assert_eq!(time_to_sec("-02:00:05").unwrap(), -7205);
    assert_eq!(time_to_sec("020005").unwrap(), 7205);
    assert_eq!(sec_to_time(86_400.25).unwrap(), "24:00:00.250000");
}

#[test]
/// DATETIME 加时间跨日滚动与非法双 DATETIME 报错。
fn datetime_addition_matches_go_rollover_cases() {
    assert_eq!(
        add_time_strings("2017-12-31 23:59:59", "00:00:01").unwrap(),
        "2018-01-01 00:00:00"
    );
    assert_eq!(
        add_time_strings("2007-12-31 23:59:59.999999", "1 1:1:1.000002").unwrap(),
        "2008-01-02 01:01:01.000001"
    );
    assert!(add_time_strings("2020-05-13 14:01:24", "2020-04-29 05:11:19").is_err());
}

#[test]
/// PERIOD_ADD/DIFF 与非法 period 校验。
fn period_conversion_and_validation_match_mysql() {
    assert_eq!(period_add(201611, 2).unwrap(), 201701);
    assert_eq!(period_add(1611, 3).unwrap(), 201702);
    assert_eq!(period_add(7011, 3).unwrap(), 197102);
    assert_eq!(period_diff(201702, 201611).unwrap(), 3);
    assert!(period_add(12323, 10).is_err());
    assert!(period_add(0, 3).is_err());
}

#[test]
/// GET_FORMAT 地区族与 TIME_FORMAT 掩码。
fn get_format_and_time_format_cover_all_location_families() {
    assert_eq!(get_format("DATE", "USA"), Some("%m.%d.%Y"));
    assert_eq!(get_format("DATETIME", "EUR"), Some("%Y-%m-%d %H.%i.%s"));
    assert_eq!(get_format("TIME", "INTERNAL"), Some("%H%i%s"));
    assert_eq!(get_format("DATE", "unknown"), None);
    assert_eq!(
        time_format("17:42:03.000001", "%r %T %h:%i%p %f").unwrap(),
        "05:42:03 PM 17:42:03 05:42PM 000001"
    );
}

#[test]
/// MAKEDATE/MAKETIME 边界与日期部件提取。
fn make_date_make_time_and_date_parts_follow_go_boundaries() {
    assert_eq!(
        make_date(8, 1).unwrap(),
        NaiveDate::from_ymd_opt(2008, 1, 1).unwrap()
    );
    assert_eq!(
        make_date(99, 32).unwrap(),
        NaiveDate::from_ymd_opt(1999, 2, 1).unwrap()
    );
    assert!(make_date(2024, 0).is_err());
    assert_eq!(make_time(839, 0, 0.0, false).unwrap(), "838:59:59");
    assert!(make_time(12, 60, 0.0, false).is_err());
    let value = dt("2008-04-01");
    assert_eq!(quarter(value), 2);
    assert_eq!(day_name(value), "Tuesday");
    assert_eq!(day_of_week(value), 3);
    assert_eq!(day_of_year(value), 92);
}

#[test]
/// TIMESTAMPADD 月末钳制与溢出。
fn timestamp_add_clamps_month_ends_like_tidb() {
    assert_eq!(
        timestamp_add("MONTH", 1.0, dt("2024-01-31")).unwrap(),
        dt("2024-02-29")
    );
    assert_eq!(
        timestamp_add("MONTH", -1.0, dt("2024-03-31")).unwrap(),
        dt("2024-02-29")
    );
    assert_eq!(
        timestamp_add("SECOND", 1.1, dt("1995-05-01")).unwrap(),
        dt("1995-05-01 00:00:01.100000")
    );
    assert!(timestamp_add("MONTH", 3.0, dt("9999-10-29")).is_err());
}

#[test]
/// LAST_DAY、DATEDIFF、TIMESTAMPDIFF 日历感知。
fn last_day_date_diff_and_timestamp_diff_are_calendar_aware() {
    assert_eq!(
        last_day(dt("2004-02-05")),
        NaiveDate::from_ymd_opt(2004, 2, 29).unwrap()
    );
    assert_eq!(date_diff(dt("2024-03-01"), dt("2024-02-28")), 2);
    assert_eq!(
        timestamp_diff("MONTH", dt("2023-01-31"), dt("2024-01-30")).unwrap(),
        11
    );
    assert_eq!(
        timestamp_diff("YEAR", dt("2020-02-29"), dt("2024-02-28")).unwrap(),
        3
    );
}

#[test]
/// CONVERT_TZ 命名时区与固定偏移。
fn timezone_conversion_honors_named_and_fixed_offsets() {
    assert_eq!(
        convert_tz(dt("2024-01-15 12:00:00"), "+00:00", "Asia/Shanghai").unwrap(),
        dt("2024-01-15 20:00:00")
    );
    assert_eq!(
        convert_tz(dt("2024-07-15 12:00:00"), "America/New_York", "+00:00").unwrap(),
        dt("2024-07-15 16:00:00")
    );
    assert!(convert_tz(dt("2024-01-01"), "Not/AZone", "+00:00").is_err());
}

#[test]
/// TSO 解析与有界陈旧读时间夹取。
fn tso_and_bounded_staleness_helpers_match_go() {
    assert_eq!(
        parse_tso(404_411_537_129_996_288).unwrap(),
        dt("2018-11-20 09:53:04.877000")
    );
    assert_eq!(parse_tso_logical(404_411_537_129_996_290).unwrap(), 2);
    assert!(parse_tso(0).is_err());
    let min = dt("2024-01-01");
    let max = dt("2024-01-03");
    assert_eq!(cal_appropriate_time(min, max, dt("2023-12-31")), min);
    assert_eq!(
        cal_appropriate_time(min, max, dt("2024-01-02")),
        dt("2024-01-02")
    );
    assert_eq!(cal_appropriate_time(min, max, dt("2024-01-04")), max);
}

#[test]
/// TO_DAYS / TO_SECONDS 与 Go 样例一致。
fn mysql_day_numbers_and_seconds_match_go_examples() {
    assert_eq!(to_days(dt("2007-10-07")), 733321);
    assert_eq!(to_days(dt("2008-10-07")), 733687);
    assert_eq!(to_seconds(dt("2009-11-29")), 63_426_672_000);
    assert_eq!(to_seconds(dt("2009-11-29 13:43:32")), 63_426_721_412);
}

/// 供同名 Go 迁移入口复用的完整时间语义回归集合。
pub(crate) fn run_time_parity_suite() {
    duration_detection_and_fsp_match_go_cases();
    mysql_duration_arithmetic_preserves_sign_days_and_fraction();
    datetime_addition_matches_go_rollover_cases();
    period_conversion_and_validation_match_mysql();
    get_format_and_time_format_cover_all_location_families();
    make_date_make_time_and_date_parts_follow_go_boundaries();
    timestamp_add_clamps_month_ends_like_tidb();
    last_day_date_diff_and_timestamp_diff_are_calendar_aware();
    timezone_conversion_honors_named_and_fixed_offsets();
    tso_and_bounded_staleness_helpers_match_go();
    mysql_day_numbers_and_seconds_match_go_examples();
}
