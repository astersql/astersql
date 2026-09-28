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

// TimerRecord / TimerSpec 校验与下次事件时间计算的单元测试。
//
// 覆盖必填字段、非法策略/表达式/时区报错，以及 INTERVAL 策略下
// 启用/禁用、Location 换算与非法表达式的 NextEventTime 行为。

use super::*;
use chrono::{Duration, FixedOffset, Utc};

/// 逐步补齐字段，断言 Validate 在各阶段的错误信息与最终成功路径。
#[test]
fn test_timer_validate() {
    let mut record = TimerRecord::default();
    assert_eq!(
        record.Validate().unwrap_err().to_string(),
        "field 'Namespace' should not be empty"
    );

    record.Namespace = "n1".to_string();
    assert_eq!(
        record.Validate().unwrap_err().to_string(),
        "field 'Key' should not be empty"
    );

    record.Key = "k1".to_string();
    assert_eq!(
        record.Validate().unwrap_err().to_string(),
        "field 'SchedPolicyType' should not be empty"
    );

    // 未知策略类型应报 invalid schedule event type。
    record.SchedPolicyType = "aa".to_string();
    assert_eq!(
        record.Validate().unwrap_err().to_string(),
        "schedule event configuration is not valid: invalid schedule event type: 'aa'"
    );

    // 合法类型但非法间隔单位。
    record.SchedPolicyType = SchedEventInterval.to_string();
    record.SchedPolicyExpr = "1x".to_string();
    assert_eq!(
        record.Validate().unwrap_err().to_string(),
        "schedule event configuration is not valid: invalid schedule event expr '1x': unknown unit x"
    );

    record.SchedPolicyExpr = "1h".to_string();
    assert!(record.Validate().is_ok());

    // 非法时区名应包含 Unknown or incorrect time zone。
    record.TimeZone = "a123".to_string();
    assert!(
        record
            .Validate()
            .unwrap_err()
            .to_string()
            .contains("Unknown or incorrect time zone: 'a123'")
    );

    record.TimeZone = "tidb".to_string();
    assert!(
        record
            .Validate()
            .unwrap_err()
            .to_string()
            .contains("Unknown or incorrect time zone: 'tidb'")
    );

    // 数值偏移、IANA 名、空串均应通过。
    record.TimeZone = "+0800".to_string();
    assert!(record.Validate().is_ok());
    record.TimeZone = "Asia/Shanghai".to_string();
    assert!(record.Validate().is_ok());
    record.TimeZone.clear();
    assert!(record.Validate().is_ok());
}

#[test]
fn time_zone_parser_matches_tidb_system_colon_and_bounds() {
    for valid in ["SYSTEM", "system", "+02:00", "-6:00", "+14:00", "-12:59"] {
        assert!(ValidateTimeZone(valid).is_ok(), "{valid} should be valid");
    }
    for invalid in ["+14:01", "-13:00"] {
        assert!(
            ValidateTimeZone(invalid).is_err(),
            "{invalid} should be rejected"
        );
    }
}

/// 校验 INTERVAL 下次触发时间、Location 换算、禁用与非法表达式行为。
#[test]
fn test_timer_next_event_time() {
    let now = Utc::now().fixed_offset();
    let mut record = TimerRecord {
        TimerSpec: TimerSpec {
            SchedPolicyType: SchedEventInterval.to_string(),
            SchedPolicyExpr: "1h".to_string(),
            Watermark: Some(now),
            Enable: true,
            ..TimerSpec::default()
        },
        ..TimerRecord::default()
    };

    let (next, ok) = record.NextEventTime().unwrap();
    assert!(ok);
    assert_eq!(next, Some(now + Duration::hours(1)));

    // Fixed Location 应将结果换算到该偏移。
    let loc = FixedOffset::east_opt(60 * 60).unwrap();
    record.Location = Some(TimerLocation::Fixed(loc));
    let (next, ok) = record.NextEventTime().unwrap();
    assert!(ok);
    assert_eq!(next, Some((now + Duration::hours(1)).with_timezone(&loc)));

    // 禁用后不再产出下次时间。
    record.Enable = false;
    assert_eq!(record.NextEventTime().unwrap(), (None, false));

    record.SchedPolicyExpr = "abcde".to_string();
    assert_eq!(record.NextEventTime().unwrap(), (None, false));

    // 重新启用后非法表达式应返回错误。
    record.Enable = true;
    let error = record.NextEventTime().unwrap_err();
    assert!(error.to_string().contains("invalid schedule event expr"));

    // 不可能存在的 Cron（2 月 30 日）应得到 (None, false)。
    record.SchedPolicyType = SchedEventCron.to_string();
    record.SchedPolicyExpr = "0 0 30 2 *".to_string();
    assert_eq!(record.NextEventTime().unwrap(), (None, false));
}
