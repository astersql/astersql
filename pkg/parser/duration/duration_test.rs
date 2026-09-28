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

// `ParseDuration` 单元测试。
//
// 覆盖正常解析（含小数与多段拼接）、零值特判，以及缺单位、未知单位、
// 畸形浮点等错误路径，对齐 Go `duration_test.go` 的用例表语义。

use super::ParseDuration;
use std::time::Duration;

/// 一分钟的纳秒数。
const MINUTE_NANOS: u64 = 60 * 1_000_000_000;
/// 一小时的纳秒数。
const HOUR_NANOS: u64 = 60 * MINUTE_NANOS;

/// 单条解析用例：输入文本与期望纳秒数。
struct ParseDurationCase {
    /// 待解析的时长字符串。
    str_: &'static str,
    /// 期望得到的纳秒值。
    duration_nanos: u64,
}

// test_parse_duration 对应 Go 的 TestParseDuration。
// Go 使用 t.Run 为每个输入建立子测试；这里保留相同 case 表和逐例断言语义。
/// 校验常见合法时长文本的纳秒解析结果。
#[test]
fn test_parse_duration() {
    let cases = [
        ParseDurationCase {
            str_: "1h",
            duration_nanos: HOUR_NANOS,
        },
        ParseDurationCase {
            str_: "1h100m",
            duration_nanos: HOUR_NANOS + 100 * MINUTE_NANOS,
        },
        ParseDurationCase {
            str_: "1d10000m",
            duration_nanos: 24 * HOUR_NANOS + 10_000 * MINUTE_NANOS,
        },
        ParseDurationCase {
            str_: "1d100h",
            duration_nanos: 24 * HOUR_NANOS + 100 * HOUR_NANOS,
        },
        ParseDurationCase {
            str_: "1.5d",
            duration_nanos: 36 * HOUR_NANOS,
        },
        ParseDurationCase {
            str_: "1d1.5h",
            duration_nanos: 24 * HOUR_NANOS + HOUR_NANOS + 30 * MINUTE_NANOS,
        },
        ParseDurationCase {
            str_: "1d3.555h",
            duration_nanos: 24 * HOUR_NANOS + (3.555 * HOUR_NANOS as f64) as u64,
        },
    ];

    for c in cases {
        assert_eq!(
            ParseDuration(c.str_),
            Ok(Duration::from_nanos(c.duration_nanos)),
            "input: {}",
            c.str_
        );
    }
}

/// 校验裸 `"0"` 与空串（当前实现下空串亦为 ZERO）的零值行为。
#[test]
fn test_parse_duration_zero() {
    assert_eq!(ParseDuration("0"), Ok(Duration::ZERO));
    assert_eq!(ParseDuration(""), Ok(Duration::ZERO));
}

/// 校验缺单位、未知单位与畸形浮点的错误消息。
#[test]
fn test_parse_duration_errors() {
    for input in ["1", "x", "1h0"] {
        assert_eq!(
            ParseDuration(input).unwrap_err(),
            "fail to read an integer",
            "input: {input}"
        );
    }

    assert_eq!(ParseDuration("1x").unwrap_err(), "unknown unit x");
    assert!(ParseDuration("1.2.3h").is_err());
}

/// Go `strconv.ParseFloat` reports a range error instead of accepting infinity.
#[test]
fn test_parse_duration_rejects_float_range_errors() {
    let overflow = concat!(
        "99999999999999999999999999999999999999999999999999",
        "99999999999999999999999999999999999999999999999999",
        "99999999999999999999999999999999999999999999999999",
        "99999999999999999999999999999999999999999999999999",
        "99999999999999999999999999999999999999999999999999",
        "99999999999999999999999999999999999999999999999999",
        "99999999999999999999999999999999999999999999999999",
        "h",
    );

    assert!(
        ParseDuration(overflow).is_err(),
        "input should be out of range"
    );
}
