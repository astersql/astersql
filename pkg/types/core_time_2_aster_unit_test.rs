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

// `CoreTime` 日历算法与 `ComputePlus` 的迁移期单元测试。
//
// 覆盖字段打包、儒略日换算、周数、闰年、时间差、
// MySQL 月末进位规则的 AddDate，以及 Datum 加法类型分派。

use super::*;

#[test]
/// 校验 FromDate 打包后各字段与 String 表示与 Go 一致。
fn core_time_2_packs_all_fields_like_go() {
    let value = FromDate(2024, 2, 29, 23, 58, 57, 654_321);

    assert_eq!(value.Year(), 2024);
    assert_eq!(value.Month(), 2);
    assert_eq!(value.Day(), 29);
    assert_eq!(value.Hour(), 23);
    assert_eq!(value.Minute(), 58);
    assert_eq!(value.Second(), 57);
    assert_eq!(value.Microsecond(), 654_321);
    assert_eq!(value.String(), "{2024 2 29 23 58 57 654321}");
}

#[test]
/// 对照 Go 表验证 daynr、周数、闰年、比较与时间差算法。
fn core_time_2_calendar_algorithms_match_go_tables() {
    // calcDaynr：年月日 → 累计日序号（对齐 MySQL 内部日历）
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

    // getDateFromDaynr：日序号反解为年月日
    for (daynr, expected) in [
        (730_669, (2000, 7, 3)),
        (719_528, (1970, 1, 1)),
        (730_544, (2000, 2, 29)),
        (366, (1, 1, 1)),
        (3_652_424, (9999, 12, 31)),
    ] {
        assert_eq!(getDateFromDaynr(daynr), expected);
    }

    assert_eq!(
        calcWeek(FromDate(2008, 2, 20, 0, 0, 0, 0), weekMode(0)).1,
        7
    );
    assert_eq!(
        calcWeek(FromDate(2008, 2, 20, 0, 0, 0, 0), weekMode(1)).1,
        8
    );
    assert_eq!(
        calcWeek(FromDate(2008, 12, 31, 0, 0, 0, 0), weekMode(1)).1,
        53
    );

    assert!(FromDate(2000, 1, 1, 0, 0, 0, 0).IsLeapYear());
    assert!(!FromDate(1900, 1, 1, 0, 0, 0, 0).IsLeapYear());
    assert_eq!(GetLastDay(2000, 2), 29);
    assert_eq!(GetLastDay(1900, 2), 28);
    assert_eq!(GetLastDay(2000, 13), 0);
    assert_eq!(FromDate(2008, 2, 20, 0, 0, 0, 0).YearDay(), 51);
    assert_eq!(FromDate(2008, 0, 20, 0, 0, 0, 0).Week(0), 0);

    let earlier = FromDate(2006, 1, 2, 3, 4, 5, 6);
    let later = FromDate(2016, 1, 2, 3, 4, 5, 0);
    assert_eq!(compareTime(earlier, later), -1);
    assert_eq!(compareTime(later, earlier), 1);

    let (seconds, micros, negative) = calcTimeTimeDiff(
        FromDate(2006, 0, 1, 12, 23, 21, 0),
        FromDate(2006, 0, 3, 21, 23, 22, 0),
        1,
    );
    assert_eq!((seconds, micros, negative), (57 * 3_600 + 1, 0, true));
}

#[test]
/// timestampDiff 在 DAY/SECOND/MICROSECOND 单位下保持 Go 整数宽度语义。
fn core_time_2_timestamp_diff_keeps_go_int_width() {
    for (unit, start, end, expected) in [
        (
            intervalMONTH,
            FromDate(2002, 5, 30, 0, 0, 0, 0),
            FromDate(2001, 1, 1, 0, 0, 0, 0),
            -16,
        ),
        (
            intervalYEAR,
            FromDate(2002, 5, 1, 0, 0, 0, 0),
            FromDate(2001, 1, 1, 0, 0, 0, 0),
            -1,
        ),
        (
            intervalMINUTE,
            FromDate(2003, 2, 1, 0, 0, 0, 0),
            FromDate(2003, 5, 1, 12, 5, 55, 0),
            128_885,
        ),
        (
            intervalMICROSECOND,
            FromDate(2002, 5, 30, 0, 0, 0, 0),
            FromDate(2002, 5, 30, 0, 13, 25, 0),
            805_000_000,
        ),
        (
            intervalMICROSECOND,
            FromDate(2000, 1, 1, 0, 0, 0, 12_345),
            FromDate(2000, 1, 1, 0, 0, 45, 32),
            44_987_687,
        ),
        (
            intervalQUARTER,
            FromDate(2000, 1, 12, 0, 0, 0, 0),
            FromDate(2016, 1, 1, 0, 0, 0, 0),
            63,
        ),
        (
            intervalQUARTER,
            FromDate(2016, 1, 1, 0, 0, 0, 0),
            FromDate(2000, 1, 12, 0, 0, 0, 0),
            -63,
        ),
    ] {
        assert_eq!(timestampDiff(unit, start, end), expected);
    }

    let start = FromDate(1000, 1, 1, 0, 0, 0, 0);
    let end = FromDate(9999, 12, 31, 23, 59, 59, 999_999);
    let days = i64::from(calcDaynr(9999, 12, 31) - calcDaynr(1000, 1, 1));
    let seconds = days * 86_400 + 86_399;

    assert_eq!(timestampDiff(intervalDAY, start, end), days);
    assert_eq!(timestampDiff(intervalSECOND, start, end), seconds);
    assert_eq!(
        timestampDiff(intervalMICROSECOND, start, end),
        seconds * 1_000_000 + 999_999
    );
    assert_eq!(
        timestampDiff(intervalMICROSECOND, end, start),
        -(seconds * 1_000_000 + 999_999)
    );
}

#[test]
/// AddDate 采用 MySQL 月末规则（如 1/31 +1 月 → 2/28），并检测溢出。
fn core_time_2_add_date_uses_mysql_month_end_rule() {
    let january_31 = gotime::Date(2018, 1, 31, 0, 0, 0, 0, gotime::UTC);
    let (result, err) = AddDate(0, 1, 0, january_31);

    assert!(err.is_none());
    assert_eq!(result.Date(), (2018, 2, 28));

    let valid = FromDate(2019, 1, 1, 1, 2, 3, 4);
    let (converted, err) = valid.GoTime(gotime::UTC);
    assert!(err.is_none());
    assert_eq!(converted.Date(), (2019, 1, 1));
    assert_eq!(converted.Clock(), (1, 2, 3));

    let invalid = FromDate(2019, 2, 31, 0, 0, 0, 0);
    assert!(invalid.GoTime(gotime::UTC).1.is_some());

    let (_, overflow) = AddDate(
        8_000,
        1,
        1,
        gotime::Date(2000, 1, 1, 0, 0, 0, 0, gotime::UTC),
    );
    assert!(overflow.is_some());
}

#[test]
/// ComputePlus 按 Kind 分派 int/uint/float/decimal，非法组合报错。
fn core_time_2_compute_plus_matches_go_dispatch() {
    let int_sum = ComputePlus(NewIntDatum(72), NewIntDatum(28)).unwrap();
    assert_eq!(int_sum.Kind(), KindInt64);
    assert_eq!(int_sum.GetInt64(), 100);

    let mixed_sum = ComputePlus(NewIntDatum(72), NewUintDatum(28)).unwrap();
    assert_eq!(mixed_sum.Kind(), KindUint64);
    assert_eq!(mixed_sum.GetUint64(), 100);

    let uint_sum = ComputePlus(NewUintDatum(72), NewIntDatum(-28)).unwrap();
    assert_eq!(uint_sum.Kind(), KindUint64);
    assert_eq!(uint_sum.GetUint64(), 44);

    let positive_mixed_sum = ComputePlus(NewUintDatum(72), NewIntDatum(28)).unwrap();
    assert_eq!(positive_mixed_sum.Kind(), KindUint64);
    assert_eq!(positive_mixed_sum.GetUint64(), 100);

    let unsigned_sum = ComputePlus(NewUintDatum(72), NewUintDatum(28)).unwrap();
    assert_eq!(unsigned_sum.Kind(), KindUint64);
    assert_eq!(unsigned_sum.GetUint64(), 100);

    let float_sum = ComputePlus(NewFloat64Datum(72.5), NewFloat64Datum(27.5)).unwrap();
    assert_eq!(float_sum.Kind(), KindFloat64);
    assert_eq!(float_sum.GetFloat64(), 100.0);

    let decimal_sum = ComputePlus(
        NewDecimalDatum(NewDecFromStringForTest("72.5")),
        NewDecimalDatum(NewDecFromStringForTest("3")),
    )
    .unwrap();
    assert_eq!(decimal_sum.Kind(), KindMysqlDecimal);
    assert_eq!(decimal_sum.GetMysqlDecimal().to_string(), "75.5");

    assert!(ComputePlus(NewIntDatum(i64::MAX), NewIntDatum(1)).is_err());
    assert!(ComputePlus(NewUintDatum(0), NewIntDatum(-1)).is_err());
    assert!(ComputePlus(NewUintDatum(u64::MAX), NewUintDatum(1)).is_err());
    assert!(ComputePlus(NewFloat64Datum(1.0), NewIntDatum(1)).is_err());
    assert!(ComputePlus(NewStringDatum("abcd"), NewIntDatum(42)).is_err());
}
