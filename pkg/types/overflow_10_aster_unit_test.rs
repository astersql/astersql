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
// Aster 侧 overflow / SET / 字符串包装单元测试，对照 Go 行为表验证边界与解析。
//
// SET 是 MySQL 枚举式多选类型：按位掩码存成员组合；collation 为字符串校对规则。

use super::overflow::*;
use super::set::{ParseSet, ParseSetName, ParseSetValue, Set};
use super::string::{HackedStr, PlainStr, String as StringValue};

/// 断言 i64 运算结果或溢出。
fn assert_i64(result: Result<i64, OverflowError>, expected: i64, overflow: bool) {
    if overflow {
        assert!(result.is_err());
    } else {
        assert_eq!(result.unwrap(), expected);
    }
}

/// 断言 u64 运算结果或溢出。
fn assert_u64(result: Result<u64, OverflowError>, expected: u64, overflow: bool) {
    if overflow {
        assert!(result.is_err());
    } else {
        assert_eq!(result.unwrap(), expected);
    }
}

#[test]
/// 加减（含 Duration、混合符号）与 Go 用例表一致。
fn overflow_add_and_sub_match_go_tables() {
    for (a, b, expected, overflow) in [
        (u64::MAX, 1, 0, true),
        (u64::MAX, 0, u64::MAX, false),
        (1, 1, 2, false),
    ] {
        assert_u64(AddUint64(a, b), expected, overflow);
    }
    assert_eq!(
        AddUint64(u64::MAX, 1).unwrap_err().to_string(),
        format!(
            "BIGINT UNSIGNED value is out of range in '({}, 1)'",
            u64::MAX
        )
    );

    for (a, b, expected, overflow) in [
        (u64::MAX, 1, u64::MAX - 1, false),
        (u64::MAX, 0, u64::MAX, false),
        (0, u64::MAX, 0, true),
        (0, 1, 0, true),
        (1, u64::MAX, 0, true),
        (1, 1, 0, false),
    ] {
        assert_u64(SubUint64(a, b), expected, overflow);
    }

    for (a, b, expected, overflow) in [
        (i64::MAX, 1, 0, true),
        (i64::MAX, 0, i64::MAX, false),
        (0, i64::MIN, i64::MIN, false),
        (-1, i64::MIN, 0, true),
        (i64::MAX, i64::MIN, -1, false),
        (1, 1, 2, false),
        (1, -1, 0, false),
    ] {
        assert_i64(AddInt64(a, b), expected, overflow);
        assert_i64(AddDuration(a, b), expected, overflow);
    }

    for (a, b, expected, overflow) in [
        (u64::MAX, i64::MIN, u64::MAX - (1_u64 << 63), false),
        (i64::MAX as u64, i64::MIN, 0, true),
        (0, -1, 0, true),
        (1, -1, 0, false),
        (0, 1, 1, false),
        (1, 1, 2, false),
    ] {
        assert_u64(AddInteger(a, b), expected, overflow);
    }

    for (a, b, expected, overflow) in [
        (i64::MIN, 0, i64::MIN, false),
        (i64::MIN, 1, 0, true),
        (i64::MAX, -1, 0, true),
        (0, i64::MIN, 0, true),
        (-1, i64::MIN, i64::MAX, false),
        (i64::MIN, i64::MAX, 0, true),
        (i64::MIN, i64::MIN, 0, false),
        (i64::MIN, -i64::MAX, -1, false),
        (1, 1, 0, false),
    ] {
        assert_i64(SubInt64(a, b), expected, overflow);
        assert_i64(SubDuration(a, b), expected, overflow);
    }

    for (a, b, expected, overflow) in [
        (0, i64::MIN, 1_u64 << 63, false),
        (0, 1, 0, true),
        (u64::MAX, i64::MIN, 0, true),
        (i64::MAX as u64, i64::MIN, u64::MAX, false),
        (u64::MAX, -1, 0, true),
        (0, -1, 1, false),
        (1, 1, 0, false),
    ] {
        assert_u64(SubUintWithInt(a, b), expected, overflow);
    }

    for (a, b, expected, overflow) in [
        (i64::MIN, 0, 0, true),
        (i64::MAX, 0, i64::MAX as u64, false),
        (i64::MAX, u64::MAX, 0, true),
        (i64::MAX, 1_u64 << 63, 0, true),
        (-1, 0, 0, true),
        (1, 1, 0, false),
    ] {
        assert_u64(SubIntWithUint(a, b), expected, overflow);
    }
}

#[test]
/// 乘除（含混合符号）与 Go 用例表一致。
fn overflow_mul_and_div_match_go_tables() {
    for (a, b, expected, overflow) in [
        (u64::MAX, 1, u64::MAX, false),
        (u64::MAX, 0, 0, false),
        (u64::MAX, 2, 0, true),
        (1, 1, 1, false),
    ] {
        assert_u64(MulUint64(a, b), expected, overflow);
    }

    for (a, b, expected, overflow) in [
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
    ] {
        assert_i64(MulInt64(a, b), expected, overflow);
    }

    for (a, b, expected, overflow) in [
        (u64::MAX, 0, 0, false),
        (0, -1, 0, false),
        (1, -1, 0, true),
        (u64::MAX, -1, 0, true),
        (u64::MAX, 10, 0, true),
        (1, 1, 1, false),
    ] {
        assert_u64(MulInteger(a, b), expected, overflow);
    }

    for (a, b, expected, overflow) in [
        (i64::MAX, 1, i64::MAX, false),
        (i64::MIN, 1, i64::MIN, false),
        (i64::MIN, -1, 0, true),
        (i64::MAX, -1, -i64::MAX, false),
        (1, -1, -1, false),
        (-1, 1, -1, false),
        (-1, 2, 0, false),
        (i64::MIN, 2, i64::MIN / 2, false),
    ] {
        assert_i64(DivInt64(a, b), expected, overflow);
    }

    for (a, b, expected, overflow) in [
        (0, -1, 0, false),
        (1, -1, 0, true),
        (i64::MAX as u64, i64::MIN, 0, false),
        (i64::MAX as u64, -1, 0, true),
        (100, 20, 5, false),
    ] {
        assert_u64(DivUintWithInt(a, b), expected, overflow);
    }

    for (a, b, expected, overflow) in [
        (i64::MIN, i64::MAX as u64, 0, true),
        (0, 1, 0, false),
        (-1, i64::MAX as u64, 0, false),
    ] {
        assert_u64(DivIntWithUint(a, b), expected, overflow);
    }
}

#[test]
/// SET 按名/按值解析、校对规则折叠与 Copy 独立性。
fn set_parsing_matches_go_name_value_and_collation_behavior() {
    // 成员按出现位置对应 bit0..bit3；组合值等于位或
    let elems = ["a", "b", "c", "d"].map(str::to_owned);
    // bin 与 unicode_ci 校对下，按名解析结果应一致
    for collation in ["utf8mb4_bin", "utf8_unicode_ci"] {
        for (input, value, name) in [
            ("a", 1, "a"),
            ("a,b,a", 3, "a,b"),
            ("b,a", 3, "a,b"),
            ("a,b,c,d", 15, "a,b,c,d"),
            ("d", 8, "d"),
            ("", 0, ""),
            ("0", 0, ""),
        ] {
            let parsed = ParseSet(&elems, input, collation).unwrap();
            assert_eq!((parsed.Value, parsed.String()), (value, name.to_owned()));
            assert_eq!(parsed.ToNumber(), value as f64);
        }
    }

    // general_ci 忽略大小写与尾部空格
    for (input, value, name) in [("A ", 1, "a"), ("a,B,a", 3, "a,b")] {
        let parsed = ParseSet(&elems, input, "utf8_general_ci").unwrap();
        assert_eq!((parsed.Value, parsed.String()), (value, name.to_owned()));
    }

    // 按位掩码数值反查成员名
    for (number, name) in [(0, ""), (1, "a"), (3, "a,b"), (9, "a,d")] {
        let parsed = ParseSetValue(&elems, number).unwrap();
        assert_eq!((parsed.Value, parsed.String()), (number, name.to_owned()));
    }

    assert!(ParseSetName(&elems, "a.e", "utf8mb4_bin").is_err());
    assert!(ParseSet(&elems, "e.f", "utf8mb4_bin").is_err());
    for number in [100, 16, 64] {
        assert!(ParseSetValue(&elems, number).is_err());
    }

    // 十六进制/八进制字面量按数值解析；非法八进制 08 失败
    assert_eq!(ParseSet(&elems, "0xf", "utf8mb4_bin").unwrap().Value, 15);
    assert_eq!(ParseSet(&elems, "017", "utf8mb4_bin").unwrap().Value, 15);
    assert!(ParseSet(&elems, "08", "utf8mb4_bin").is_err());

    // Copy 应深拷贝 Name，指针不同
    let original = Set {
        Name: "a,b".to_owned(),
        Value: 3,
    };
    let copied = original.Copy();
    assert_eq!((copied.Name.as_str(), copied.Value), ("a,b", 3));
    assert_ne!(copied.Name.as_ptr(), original.Name.as_ptr());
}

#[test]
/// PlainStr / HackedStr 的 String 与 FreezeStr 契约（冻结后缓冲独立）。
fn string_wrappers_match_go_string_and_freeze_contracts() {
    let plain = PlainStr("stable".to_owned());
    assert_eq!(plain.String(), "stable");

    let hacked = HackedStr("buffer".to_owned());
    let frozen = hacked.FreezeStr();
    assert_eq!(hacked.String(), "buffer");
    assert_eq!(frozen, "buffer");
    assert_ne!(frozen.as_ptr(), hacked.0.as_ptr());
}
