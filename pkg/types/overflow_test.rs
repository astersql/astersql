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
// overflow 模块完整表驱动测试：加减乘除及混合符号边界，对齐 Go `overflow_test.go`。

use std::fmt::Debug;

use crate::file_group::overflow::{
    AddDuration, AddInt64, AddInteger, AddUint64, DivInt64, DivIntWithUint, DivUintWithInt,
    MulInt64, MulInteger, MulUint64, OverflowError, SubInt64, SubIntWithUint, SubUint64,
    SubUintWithInt,
};

/// 按期望是否溢出断言 Result：溢出则 `is_err`，否则相等。
fn assert_result<T>(result: Result<T, OverflowError>, expected: T, overflow: bool)
where
    T: Debug + Eq,
{
    if overflow {
        assert!(result.is_err(), "expected overflow, got {result:?}");
    } else {
        assert_eq!(result.unwrap(), expected);
    }
}

#[test]
#[allow(non_snake_case)]
/// 覆盖 AddUint64 / AddInt64 / AddDuration / AddInteger 用例表。
fn TestAdd() {
    // (左, 右, 期望值, 是否溢出)
    let uint64_cases = [
        (u64::MAX, 1, 0, true),
        (u64::MAX, 0, u64::MAX, false),
        (1, 1, 2, false),
    ];
    for (lhs, rhs, expected, overflow) in uint64_cases {
        assert_result(AddUint64(lhs, rhs), expected, overflow);
    }

    let int64_cases = [
        (i64::MAX, 1, 0, true),
        (i64::MAX, 0, i64::MAX, false),
        (0, i64::MIN, i64::MIN, false),
        (-1, i64::MIN, 0, true),
        (i64::MAX, i64::MIN, -1, false),
        (1, 1, 2, false),
        (1, -1, 0, false),
    ];
    for (lhs, rhs, expected, overflow) in int64_cases {
        assert_result(AddInt64(lhs, rhs), expected, overflow);
        assert_result(AddDuration(lhs, rhs), expected, overflow);
    }

    // uint64 + int64：负加数转为无符号减法
    let mixed_cases = [
        (
            u64::MAX,
            i64::MIN,
            u64::MAX - i64::MIN.unsigned_abs(),
            false,
        ),
        (i64::MAX as u64, i64::MIN, 0, true),
        (0, -1, 0, true),
        (1, -1, 0, false),
        (0, 1, 1, false),
        (1, 1, 2, false),
    ];
    for (lhs, rhs, expected, overflow) in mixed_cases {
        assert_result(AddInteger(lhs, rhs), expected, overflow);
    }
}

#[test]
#[allow(non_snake_case)]
/// 覆盖 SubUint64 / SubInt64 / SubUintWithInt / SubIntWithUint 用例表。
fn TestSub() {
    let uint64_cases = [
        (u64::MAX, 1, u64::MAX - 1, false),
        (u64::MAX, 0, u64::MAX, false),
        (0, u64::MAX, 0, true),
        (0, 1, 0, true),
        (1, u64::MAX, 0, true),
        (1, 1, 0, false),
    ];
    for (lhs, rhs, expected, overflow) in uint64_cases {
        assert_result(SubUint64(lhs, rhs), expected, overflow);
    }

    let int64_cases = [
        (i64::MIN, 0, i64::MIN, false),
        (i64::MIN, 1, 0, true),
        (i64::MAX, -1, 0, true),
        (0, i64::MIN, 0, true),
        (-1, i64::MIN, i64::MAX, false),
        (i64::MIN, i64::MAX, 0, true),
        (i64::MIN, i64::MIN, 0, false),
        (i64::MIN, -i64::MAX, -1, false),
        (1, 1, 0, false),
    ];
    for (lhs, rhs, expected, overflow) in int64_cases {
        assert_result(SubInt64(lhs, rhs), expected, overflow);
    }

    // uint64 - int64
    let uint_int_cases = [
        (0, i64::MIN, i64::MIN.unsigned_abs(), false),
        (0, 1, 0, true),
        (u64::MAX, i64::MIN, 0, true),
        (i64::MAX as u64, i64::MIN, u64::MAX, false),
        (u64::MAX, -1, 0, true),
        (0, -1, 1, false),
        (1, 1, 0, false),
    ];
    for (lhs, rhs, expected, overflow) in uint_int_cases {
        assert_result(SubUintWithInt(lhs, rhs), expected, overflow);
    }

    // int64 - uint64，结果为无符号
    let int_uint_cases = [
        (i64::MIN, 0, 0, true),
        (i64::MAX, 0, i64::MAX as u64, false),
        (i64::MAX, u64::MAX, 0, true),
        (i64::MAX, i64::MIN.unsigned_abs(), 0, true),
        (-1, 0, 0, true),
        (1, 1, 0, false),
    ];
    for (lhs, rhs, expected, overflow) in int_uint_cases {
        assert_result(SubIntWithUint(lhs, rhs), expected, overflow);
    }
}

#[test]
#[allow(non_snake_case)]
/// 覆盖 MulUint64 / MulInt64 / MulInteger 用例表。
fn TestMul() {
    let uint64_cases = [
        (u64::MAX, 1, u64::MAX, false),
        (u64::MAX, 0, 0, false),
        (u64::MAX, 2, 0, true),
        (1, 1, 1, false),
    ];
    for (lhs, rhs, expected, overflow) in uint64_cases {
        assert_result(MulUint64(lhs, rhs), expected, overflow);
    }

    let int64_cases = [
        (i64::MAX, 1, i64::MAX, false),
        (i64::MIN, 1, i64::MIN, false),
        (i64::MAX, -1, -i64::MAX, false),
        (i64::MIN, -1, 0, true),
        (i64::MIN, 0, 0, false),
        (i64::MAX, 0, 0, false),
        (i64::MAX, i64::MAX, 0, true),
        (i64::MAX, i64::MIN, 0, true),
        (i64::MIN / 10, 11, 0, true),
        (1, 1, 1, false),
    ];
    for (lhs, rhs, expected, overflow) in int64_cases {
        assert_result(MulInt64(lhs, rhs), expected, overflow);
    }

    let mixed_cases = [
        (u64::MAX, 0, 0, false),
        (0, -1, 0, false),
        (1, -1, 0, true),
        (u64::MAX, -1, 0, true),
        (u64::MAX, 10, 0, true),
        (1, 1, 1, false),
    ];
    for (lhs, rhs, expected, overflow) in mixed_cases {
        assert_result(MulInteger(lhs, rhs), expected, overflow);
    }
}

#[test]
#[allow(non_snake_case)]
/// 覆盖 DivInt64 / DivUintWithInt / DivIntWithUint，并核对错误消息文本。
fn TestDiv() {
    let int64_cases = [
        (i64::MAX, 1, i64::MAX, false),
        (i64::MIN, 1, i64::MIN, false),
        (i64::MIN, -1, 0, true),
        (i64::MAX, -1, -i64::MAX, false),
        (1, -1, -1, false),
        (-1, 1, -1, false),
        (-1, 2, 0, false),
        (i64::MIN, 2, i64::MIN / 2, false),
    ];
    for (lhs, rhs, expected, overflow) in int64_cases {
        assert_result(DivInt64(lhs, rhs), expected, overflow);
    }

    let uint_int_cases = [
        (0, -1, 0, false),
        (1, -1, 0, true),
        (i64::MAX as u64, i64::MIN, 0, false),
        (i64::MAX as u64, -1, 0, true),
        (100, 20, 5, false),
    ];
    for (lhs, rhs, expected, overflow) in uint_int_cases {
        assert_result(DivUintWithInt(lhs, rhs), expected, overflow);
    }

    let int_uint_cases = [
        (i64::MIN, i64::MAX as u64, 0, true),
        (0, 1, 0, false),
        (-1, i64::MAX as u64, 0, false),
    ];
    for (lhs, rhs, expected, overflow) in int_uint_cases {
        let result = DivIntWithUint(lhs, rhs);
        // 核对 OverflowError 的类型名、表达式与 Display 文案
        if overflow {
            let err = result.expect_err("expected signed/unsigned division overflow");
            assert_eq!(err.target_type, "BIGINT UNSIGNED");
            assert_eq!(err.expression, format!("({}, {})", lhs, rhs));
            assert_eq!(
                err.to_string(),
                "BIGINT UNSIGNED value is out of range in '(-9223372036854775808, 9223372036854775807)'"
            );
        } else {
            assert_eq!(result.unwrap(), expected);
        }
    }
}
