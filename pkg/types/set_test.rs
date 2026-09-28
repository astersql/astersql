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

// MySQL SET 类型解析的单元测试，对齐 Go `types` 包 `TestSet`。
//
// 覆盖按名称/数值解析、校对规则大小写折叠，以及非法成员与越界掩码错误。

use crate::file_group::set::{ParseSet, ParseSetValue};

/// 表驱动校验 ParseSet / ParseSetValue 的成功与失败路径。
#[test]
#[allow(non_snake_case)]
fn TestSet() {
    let elems = ["a", "b", "c", "d"].map(String::from);

    // (输入名, 期望位掩码数值, 规范化后的 Name)
    let parse_set_cases = [
        ("a", 1, "a"),
        ("a,b,a", 3, "a,b"),
        ("b,a", 3, "a,b"),
        ("a,b,c,d", 15, "a,b,c,d"),
        ("d", 8, "d"),
        ("", 0, ""),
        ("0", 0, ""),
    ];
    // mysql.DefaultCollationName is utf8mb4_bin.
    // 二进制与 unicode_ci 校对下，精确匹配成员名均应成功
    for collation in ["utf8mb4_bin", "utf8_unicode_ci"] {
        for (name, expected_value, expected_name) in parse_set_cases {
            let set = ParseSet(&elems, name, collation).unwrap();
            assert_eq!(set.ToNumber(), expected_value as f64);
            assert_eq!(set.String(), expected_name);
        }
    }

    // utf8_general_ci：忽略大小写与尾随空格后匹配
    let parse_set_ci_cases = [("A ", 1, "a"), ("a,B,a", 3, "a,b")];
    for (name, expected_value, expected_name) in parse_set_ci_cases {
        let set = ParseSet(&elems, name, "utf8_general_ci").unwrap();
        assert_eq!(set.ToNumber(), expected_value as f64);
        assert_eq!(set.String(), expected_name);
    }

    // 按位掩码数值解析：每位对应 elems 下标
    let parse_set_value_cases = [(0, ""), (1, "a"), (3, "a,b"), (9, "a,d")];
    for (number, expected_name) in parse_set_value_cases {
        let set = ParseSetValue(&elems, number).unwrap();
        assert_eq!(set.ToNumber(), number as f64);
        assert_eq!(set.String(), expected_name);
    }

    // 含未知成员名应失败
    for name in ["a.e", "e.f"] {
        assert!(ParseSet(&elems, name, "utf8mb4_bin").is_err());
    }

    // 超出 elems 位宽的掩码应失败
    for number in [100, 16, 64] {
        assert!(ParseSetValue(&elems, number).is_err());
    }
}
