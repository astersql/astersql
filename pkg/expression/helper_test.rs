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

// `helper.rs` 默认时间解析的单元测试。
//
// 覆盖 `get_time_value`、`is_valid_current_timestamp_expr`，
// 以及 statement 时间缓存与会话时区对 `CURRENT_TIMESTAMP` 显示的影响。

use chrono::{TimeZone, Utc};

use crate::helper_kernel::*;

#[test]
/// 文本/整型/函数/NULL 输入解析，以及非法值拒绝。
fn test_get_time_value() {
    let context = TimeContext::new(Utc.timestamp_opt(1_234, 0).unwrap(), chrono_tz::UTC);
    // 重复求值应稳定（同一 statement 时间）。
    for _ in 0..4 {
        assert_eq!(
            get_time_value(
                &context,
                TimeInput::Text("2012-12-12 00:00:00".into()),
                TimeType::Timestamp,
                0,
                None,
            )
            .unwrap()
            .to_string(),
            "2012-12-12 00:00:00",
        );
    }

    // statement 时间冻结：两次 current_timestamp 结果相同。
    let first = get_time_current_timestamp(&context, TimeType::Timestamp, 0).unwrap();
    let second = get_time_current_timestamp(&context, TimeType::Timestamp, 0).unwrap();
    assert_eq!(first, second, "statement timestamp must be cached/stable");

    // epoch 1234s → UTC 1970-01-01 00:20:34。
    let cases = [
        (
            TimeInput::Text("2012-12-12 00:00:00".into()),
            "2012-12-12 00:00:00",
        ),
        (
            TimeInput::Text("current_timestamp".into()),
            "1970-01-01 00:20:34",
        ),
        (
            TimeInput::Text("0000-00-00 00:00:00".into()),
            "0000-00-00 00:00:00",
        ),
        (TimeInput::Integer(0), "0000-00-00 00:00:00"),
        (TimeInput::Integer(121_212), "2012-12-12 00:00:00"),
        (TimeInput::Integer(2012_1212), "2012-12-12 00:00:00"),
        (TimeInput::Integer(121_212_123_456), "2012-12-12 12:34:56"),
    ];
    for (input, expected) in cases {
        assert_eq!(
            get_time_value(&context, input, TimeType::Timestamp, 0, None)
                .unwrap()
                .to_string(),
            expected,
        );
    }
    assert_eq!(
        get_time_value(&context, TimeInput::Null, TimeType::Timestamp, 0, None).unwrap(),
        TimeValue::Null,
    );
    // 函数节点应延迟求值，保留大写函数名。
    assert_eq!(
        get_time_value(
            &context,
            TimeInput::Function("current_timestamp".into()),
            TimeType::Timestamp,
            0,
            None,
        )
        .unwrap(),
        TimeValue::DeferredFunction("CURRENT_TIMESTAMP".into()),
    );
    assert_eq!(
        get_time_value(
            &context,
            TimeInput::Integer(0),
            TimeType::Timestamp,
            3,
            None,
        )
        .unwrap()
        .to_string(),
        "0000-00-00 00:00:00.000",
    );
    assert_eq!(
        get_time_value(&context, TimeInput::Integer(0), TimeType::Date, 0, None)
            .unwrap()
            .to_string(),
        "0000-00-00",
    );

    // 非法月份、非法短整型、未知函数、一元非整零均应失败。
    for invalid in [
        TimeInput::Text("2012-13-12 00:00:00".into()),
        TimeInput::Integer(1),
        TimeInput::Function("xxx".into()),
        TimeInput::UnaryInteger(1),
    ] {
        assert!(get_time_value(&context, invalid, TimeType::Timestamp, 0, None).is_err());
    }
}

#[test]
/// `CURRENT_TIMESTAMP` 默认值合法性：参数与字段 fsp 一致性。
fn test_is_current_timestamp_expr() {
    assert!(!is_valid_current_timestamp_expr(&TimeExpr::Value, None));
    assert!(is_valid_current_timestamp_expr(
        &TimeExpr::current_timestamp(vec![]),
        None,
    ));
    let fsp_three = TimeFieldType { decimal: 3 };
    assert!(is_valid_current_timestamp_expr(
        &TimeExpr::current_timestamp(vec![3]),
        Some(&fsp_three),
    ));
    // Go checks only fn.Args[0]; extra arguments do not change this helper's result.
    assert!(is_valid_current_timestamp_expr(
        &TimeExpr::current_timestamp(vec![3, 6]),
        Some(&fsp_three),
    ));
    // 参数与字段 decimal 不一致 → 非法。
    assert!(!is_valid_current_timestamp_expr(
        &TimeExpr::current_timestamp(vec![1]),
        Some(&fsp_three),
    ));
    assert!(!is_valid_current_timestamp_expr(
        &TimeExpr::current_timestamp(vec![]),
        Some(&fsp_three),
    ));
    let fsp_zero = TimeFieldType { decimal: 0 };
    assert!(!is_valid_current_timestamp_expr(
        &TimeExpr::current_timestamp(vec![2]),
        Some(&fsp_zero),
    ));
    assert!(!is_valid_current_timestamp_expr(
        &TimeExpr::current_timestamp(vec![2]),
        None,
    ));
}

#[test]
/// 同一 UTC 时刻在不同会话时区下显示不同墙钟时间。
fn test_current_timestamp_time_zone() {
    let instant = Utc.timestamp_opt(1_234, 0).unwrap();
    let utc = TimeContext::new(instant, chrono_tz::UTC);
    assert_eq!(
        get_time_value(
            &utc,
            TimeInput::Text("current_timestamp".into()),
            TimeType::Timestamp,
            0,
            None,
        )
        .unwrap()
        .to_string(),
        "1970-01-01 00:20:34",
    );

    // Asia/Shanghai = UTC+8，墙钟应加 8 小时。
    let shanghai = TimeContext::new(instant, "Asia/Shanghai".parse().unwrap());
    assert_eq!(
        get_time_value(
            &shanghai,
            TimeInput::Text("current_timestamp".into()),
            TimeType::Timestamp,
            0,
            None,
        )
        .unwrap()
        .to_string(),
        "1970-01-01 08:20:34",
    );
}
