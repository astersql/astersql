// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 调度策略单元测试。
//
// 覆盖间隔（interval）与 Cron 表达式策略的下次触发时间计算，
// 以及非法表达式的错误信息。

use super::*;
use chrono::{DateTime, Duration, FixedOffset, TimeZone, Utc};

/// 按固定偏移构造测试用时间戳。
fn date(
    offset: FixedOffset,
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> Timestamp {
    offset
        .with_ymd_and_hms(year, month, day, hour, minute, second)
        .single()
        .unwrap()
}

#[test]
/// 验证 interval 表达式解析与相对 watermark 的下次时间。
fn test_interval_policy() {
    let watermark1 = Utc::now().fixed_offset();
    let watermark2 = DateTime::parse_from_rfc3339("2021-11-21T11:21:31Z").unwrap();
    let cases = [
        ("6m", Some(Duration::minutes(6))),
        ("7h", Some(Duration::hours(7))),
        ("8d", Some(Duration::hours(8 * 24))),
        ("11", None),
    ];

    // interval 为 None 表示期望解析失败。
    for (expr, interval) in cases {
        let result = CreateSchedEventPolicy(SchedEventInterval, expr.to_string());
        let Some(interval) = interval else {
            let error = match result {
                Err(error) => error,
                Ok(_) => panic!("expression {expr} should be rejected"),
            };
            assert!(
                error
                    .to_string()
                    .contains(&format!("invalid schedule event expr '{expr}'"))
            );
            continue;
        };

        let policy = result.unwrap();
        let _: SchedIntervalPolicy = NewSchedIntervalPolicy(expr.to_string()).unwrap();
        let (next, ok) = policy.NextEventTime(Some(watermark1));
        assert!(ok);
        assert_eq!(next, Some(watermark1 + interval));
        let (next, ok) = policy.NextEventTime(Some(watermark2));
        assert!(ok);
        assert_eq!(next, Some(watermark2 + interval));
    }
}

#[test]
/// 验证 cron/宏表达式在不同时区下的下次触发时间与非法表达式拒绝。
fn test_cron_policy() {
    let utc = FixedOffset::east_opt(0).unwrap();
    let local = FixedOffset::east_opt(8 * 60 * 60).unwrap();
    let loc_e2 = FixedOffset::east_opt(2 * 60 * 60).unwrap();
    let loc_w2 = FixedOffset::west_opt(2 * 60 * 60).unwrap();
    let cases = vec![
        ("", None, None),
        ("aaa", None, None),
        ("61 1 * * *", None, None),
        (
            "@hourly",
            Some(date(utc, 2021, 11, 21, 11, 21, 31)),
            Some(date(utc, 2021, 11, 21, 12, 0, 0)),
        ),
        (
            "@hourly",
            Some(date(local, 2021, 11, 21, 12, 0, 0)),
            Some(date(local, 2021, 11, 21, 13, 0, 0)),
        ),
        (
            "@daily",
            Some(date(local, 2021, 11, 21, 11, 21, 31)),
            Some(date(local, 2021, 11, 22, 0, 0, 0)),
        ),
        (
            "@weekly",
            Some(date(loc_e2, 2021, 11, 19, 11, 21, 31)),
            Some(date(loc_e2, 2021, 11, 21, 0, 0, 0)),
        ),
        (
            "@monthly",
            Some(date(loc_w2, 2021, 12, 19, 11, 21, 31)),
            Some(date(loc_w2, 2022, 1, 1, 0, 0, 0)),
        ),
        (
            "@yearly",
            Some(date(utc, 2021, 12, 19, 11, 21, 31)),
            Some(date(utc, 2022, 1, 1, 0, 0, 0)),
        ),
        (
            "12 12 * * *",
            Some(date(local, 2021, 12, 19, 11, 21, 31)),
            Some(date(local, 2021, 12, 19, 12, 12, 0)),
        ),
        (
            "5 4 21 2 *",
            Some(date(loc_e2, 2021, 12, 19, 11, 21, 31)),
            Some(date(loc_e2, 2022, 2, 21, 4, 5, 0)),
        ),
        (
            "55 16 * 12 0",
            Some(date(loc_w2, 2021, 12, 21, 11, 21, 31)),
            Some(date(loc_w2, 2021, 12, 26, 16, 55, 0)),
        ),
        (
            "12 8,16,19 * * *",
            Some(date(loc_w2, 2021, 12, 21, 2, 21, 31)),
            Some(date(loc_w2, 2021, 12, 21, 8, 12, 0)),
        ),
        (
            "12 8,16,19 * * *",
            Some(date(loc_w2, 2021, 12, 21, 9, 21, 31)),
            Some(date(loc_w2, 2021, 12, 21, 16, 12, 0)),
        ),
        (
            "12 8,16,19 * * *",
            Some(date(loc_w2, 2021, 12, 21, 19, 21, 31)),
            Some(date(loc_w2, 2021, 12, 22, 8, 12, 0)),
        ),
        (
            "12 8,16,19 * * *",
            Some(date(local, 2021, 12, 21, 16, 12, 0)),
            Some(date(local, 2021, 12, 21, 19, 12, 0)),
        ),
        (
            "* * 29 2 *",
            Some(date(local, 2021, 12, 21, 16, 12, 0)),
            Some(date(local, 2024, 2, 29, 0, 0, 0)),
        ),
        (
            "* * 30 2 *",
            Some(date(local, 2021, 12, 21, 16, 12, 0)),
            None,
        ),
    ];

    // watermark 为 None 表示非法 cron，应返回错误。
    for (expr, watermark, expected) in cases {
        let result = CreateSchedEventPolicy(SchedEventCron, expr.to_string());
        let Some(watermark) = watermark else {
            let error = match result {
                Err(error) => error,
                Ok(_) => panic!("expression {expr} should be rejected"),
            };
            assert!(
                error
                    .to_string()
                    .contains(&format!("invalid cron expr '{expr}'"))
            );
            continue;
        };

        let policy = result.unwrap();
        let _: CronPolicy = NewCronPolicy(expr.to_string()).unwrap();
        let (next, ok) = policy.NextEventTime(Some(watermark));
        assert_eq!(next, expected, "expression: {expr}");
        assert_eq!(ok, next.is_some(), "expression: {expr}");
    }
}

/// Day-of-week step divisors are intervals, not weekday numbers.
#[test]
fn cron_day_of_week_step_matches_robfig_standard_parser() {
    let utc = FixedOffset::east_opt(0).unwrap();
    let sunday = date(utc, 2021, 11, 21, 0, 0, 0);
    let expected_tuesday = date(utc, 2021, 11, 23, 0, 0, 0);
    let policy = CreateSchedEventPolicy(SchedEventCron, "0 0 * * */2".to_string()).unwrap();
    assert_eq!(
        policy.NextEventTime(Some(sunday)),
        (Some(expected_tuesday), true)
    );
}

#[test]
fn cron_rejects_day_seven_like_robfig() {
    assert!(CreateSchedEventPolicy(SchedEventCron, "0 0 * * 7".to_string()).is_err());
}
