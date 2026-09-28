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

// Go 时间兼容层的迁移回归测试。
//
// 覆盖 DST 跳变空洞的修正、完整 IANA 时区边界查询，以及固定时区的名称、
// 偏移和绝对时间语义，防止 Rust 实现与 Go `time` 的行为发生偏差。

use super::{FromDate, gotime};

/// 依次表示日期、时钟、纳秒、时区名和 UTC 偏移秒数的预期值。
type ExpectedTime = (i32, i32, i32, i32, i32, i32, i32, &'static str, i32);

/// 按 Go `time.Time` 的可观察字段核对完整时间值。
fn assert_time(value: gotime::Time, expected: ExpectedTime) {
    let (year, month, day) = value.Date();
    let (hour, minute, second) = value.Clock();
    let (zone_name, zone_offset) = value.Zone();
    assert_eq!(
        (
            year,
            month,
            day,
            hour,
            minute,
            second,
            value.Nanosecond(),
            zone_name.as_str(),
            zone_offset,
        ),
        expected
    );
}

#[test]
/// 验证 DST 空洞中的墙上时间会修正到最近的真实跳变边界。
fn adjusted_go_time_matches_go_dst_boundaries() {
    for (zone_name, input, expected) in [
        (
            "Australia/Lord_Howe",
            FromDate(2020, 10, 4, 1, 59, 59, 997),
            (2020, 10, 4, 1, 59, 59, 997_000, "+1030", 37_800),
        ),
        (
            "Australia/Lord_Howe",
            FromDate(2020, 10, 4, 2, 0, 0, 0),
            (2020, 10, 4, 2, 30, 0, 0, "+11", 39_600),
        ),
        (
            "Australia/Lord_Howe",
            FromDate(2020, 10, 4, 2, 15, 0, 0),
            (2020, 10, 4, 2, 30, 0, 0, "+11", 39_600),
        ),
        (
            "Australia/Lord_Howe",
            FromDate(2020, 10, 4, 2, 29, 59, 999_999),
            (2020, 10, 4, 2, 30, 0, 0, "+11", 39_600),
        ),
        (
            "Australia/Lord_Howe",
            FromDate(2020, 10, 4, 2, 30, 0, 1),
            (2020, 10, 4, 2, 30, 0, 1_000, "+11", 39_600),
        ),
        (
            "Australia/Lord_Howe",
            FromDate(2020, 6, 29, 3, 45, 0, 0),
            (2020, 6, 29, 3, 45, 0, 0, "+1030", 37_800),
        ),
        (
            "Australia/Lord_Howe",
            FromDate(2020, 4, 4, 1, 45, 0, 0),
            (2020, 4, 4, 1, 45, 0, 0, "+11", 39_600),
        ),
        (
            "Europe/Vilnius",
            FromDate(2020, 3, 29, 3, 45, 0, 0),
            (2020, 3, 29, 4, 0, 0, 0, "EEST", 10_800),
        ),
        (
            "Europe/Vilnius",
            FromDate(2020, 3, 29, 3, 59, 59, 456_789),
            (2020, 3, 29, 4, 0, 0, 0, "EEST", 10_800),
        ),
        (
            "Europe/Vilnius",
            FromDate(2020, 3, 29, 4, 0, 1, 130_000),
            (2020, 3, 29, 4, 0, 1, 130_000_000, "EEST", 10_800),
        ),
        (
            "Europe/Vilnius",
            FromDate(2020, 10, 25, 3, 45, 0, 0),
            (2020, 10, 25, 3, 45, 0, 0, "EET", 7_200),
        ),
        (
            "Europe/Vilnius",
            FromDate(2020, 6, 29, 3, 45, 0, 0),
            (2020, 6, 29, 3, 45, 0, 0, "EEST", 10_800),
        ),
        (
            "Europe/Amsterdam",
            FromDate(2020, 3, 29, 2, 45, 0, 0),
            (2020, 3, 29, 3, 0, 0, 0, "CEST", 7_200),
        ),
        (
            "Europe/Amsterdam",
            FromDate(2020, 10, 25, 2, 35, 0, 0),
            (2020, 10, 25, 2, 35, 0, 0, "CET", 3_600),
        ),
    ] {
        let location = gotime::LoadLocation(zone_name).unwrap();
        let (actual, error) = input.AdjustedGoTime(location);
        assert!(error.is_none(), "{zone_name}: {error:?}");
        assert_time(actual, expected);
    }

    // 普通日历非法值不属于 DST 跳变，不能借边界修正掩盖错误。
    let invalid = FromDate(2020, 2, 31, 2, 35, 0, 0);
    assert!(invalid.AdjustedGoTime(gotime::UTC).1.is_some());
}

#[test]
/// 验证时区边界来自完整规则历史，而非当前时刻附近的有限搜索窗口。
fn zone_bounds_are_not_limited_to_nearby_dst_transitions() {
    let shanghai = gotime::LoadLocation("Asia/Shanghai").unwrap();
    let value = gotime::Date(2020, 1, 1, 0, 0, 0, 0, shanghai);
    let (start, end) = value.ZoneBounds();

    assert_eq!(start.Year(), 1991);
    assert_eq!(end.Date(), (1, 1, 1));
}

#[test]
/// 验证固定时区不产生跳变边界，并保持名称、偏移及对应的绝对时间差。
fn fixed_zone_preserves_name_offset_and_absolute_time() {
    let nepal = gotime::FixedZone("NPT", 5 * 3_600 + 45 * 60);
    let local = gotime::Date(2020, 1, 1, 0, 0, 0, 0, nepal);
    let utc = gotime::Date(2020, 1, 1, 0, 0, 0, 0, gotime::UTC);

    assert_eq!(local.Zone(), ("NPT".to_owned(), 20_700));
    assert_eq!(utc.Sub(local).Hours(), 5.75);
    assert_eq!(local.ZoneBounds().0.Date(), (1, 1, 1));
    assert_eq!(local.ZoneBounds().1.Date(), (1, 1, 1));
}
