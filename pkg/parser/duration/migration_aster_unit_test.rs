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

// `ParseDuration` 与 Go 用例表的迁移对照测试。
//
// 用与 Go 测试表完全一致的输入/期望对验证解析结果，并额外覆盖缺数字、
// 未知单位、畸形浮点，以及 Unicode 十进制数字的扫描-再解析行为。

use std::time::Duration;

use super::ParseDuration;

/// 按 Go 测试表逐条断言合法输入的精确 Duration。
#[test]
fn parses_go_table_cases_exactly() {
    let cases = [
        ("", Duration::ZERO),
        ("0", Duration::ZERO),
        ("1h", Duration::from_secs(60 * 60)),
        ("1h100m", Duration::from_secs(60 * 60 + 100 * 60)),
        ("1d10000m", Duration::from_secs(24 * 60 * 60 + 10_000 * 60)),
        ("1d100h", Duration::from_secs((24 + 100) * 60 * 60)),
        ("1.5d", Duration::from_secs(36 * 60 * 60)),
        ("1d1.5h", Duration::from_secs((24 * 60 + 90) * 60)),
        (
            "1d3.555h",
            Duration::from_nanos(
                24 * 60 * 60 * 1_000_000_000 + (3.555_f64 * 60.0 * 60.0 * 1_000_000_000.0) as u64,
            ),
        ),
    ];

    for (input, expected) in cases {
        assert_eq!(ParseDuration(input).unwrap(), expected, "input {input}");
    }
}

/// 缺数字、缺单位、未知单位与畸形浮点均应返回错误。
#[test]
fn rejects_missing_numbers_units_and_malformed_floats() {
    for input in ["1", "h", ".h", "1s", "1..2h"] {
        assert!(ParseDuration(input).is_err(), "input {input}");
    }
}

/// Unicode 十进制数字先被扫入数值 token，再在 f64 解析阶段报 invalid float。
#[test]
fn unicode_decimal_digits_follow_go_scanning_before_float_parsing() {
    let error = ParseDuration("1٢h").unwrap_err();
    assert!(
        error.contains("invalid float"),
        "Go scans the Unicode digit into the numeric token; got: {error}"
    );
}
