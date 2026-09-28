// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 类型辅助函数的单元测试。
//
// 对照 Go `pkg/types/helper_test.go`，覆盖 `strToInt` 溢出边界、
// `Truncate` 与 `TruncateFloatToString` 的截断格式化。

// 对照 pkg/types/helper_test.go，覆盖整数解析、浮点截断和格式化截断。
//

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_variables
)]

use crate::field::{ErrBadNumber, ErrOverflow, ErrTruncated, errors};

// 直接编译生产实现，使测试能覆盖包内私有 strToInt，不复制生产逻辑。
include!("helper.rs");

/// i64 上下界附近的解析：溢出时钳位并返回 ErrBadNumber。
#[test]
fn TestStrToInt() {
    for (input, expected, expected_error) in [
        ("9223372036854775806", "9223372036854775806", false),
        ("9223372036854775807", "9223372036854775807", false),
        ("9223372036854775808", "9223372036854775807", true),
        ("-9223372036854775807", "-9223372036854775807", false),
        ("-9223372036854775808", "-9223372036854775808", false),
        ("-9223372036854775809", "-9223372036854775808", true),
    ] {
        let (output, error) = strToInt(input);
        assert_eq!(expected, output.to_string(), "input {input}");
        assert_eq!(expected_error, error.is_some(), "input {input}");
        if let Some(error) = error {
            let expected = errors::SharedError::new((**ErrBadNumber).clone());
            assert!(errors::ErrorEqual(Some(&error), Some(&expected)));
        }
    }
}

/// 按小数位向零截断，含极大/极小 dec 边界。
#[test]
fn TestTruncate() {
    for (value, decimal, expected) in [
        (123.45, 0, 123.0),
        (123.45, 1, 123.4),
        (123.45, 2, 123.45),
        (123.45, 3, 123.450),
        (123.45, -400, 0.0),
        (123.45, 400, 123.45),
    ] {
        assert_eq!(expected, Truncate(value, decimal));
    }
}

/// 截断后再格式化为字符串的若干典型值。
#[test]
fn TestTruncateFloatToString() {
    for (value, decimal, expected) in [
        (12.13, -1, "10"),
        (13.15, 0, "13"),
        (0.0, 2, "0"),
        (0.001, 2, "0"),
        (0.539, 2, "0.53"),
        (0.9951, 2, "0.99"),
        (1.0, 2, "1"),
        (-0.456, 2, "-0.45"),
    ] {
        assert_eq!(expected, TruncateFloatToString(value, decimal));
    }
}
