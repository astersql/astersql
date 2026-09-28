// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// slice 迁移补充单元测试。
//
// 对齐 Go 侧 `AllOf` 空切片恒真与短路、`Int64sToStrings` 十进制边界，
// 以及 `DeepClone` 对 nil/空切片与元素指针独立性的语义。

use super::{AllOf, DeepClone, Int64sToStrings};

/// 空切片恒真；遇首个不满足谓词的元素立即短路，不再访问后续元素。
#[test]
fn migration_all_of_matches_go_vacuous_truth_and_short_circuiting() {
    assert!(AllOf::<i32, _>(&[], |_| false));

    let mut visited = Vec::new();
    let all_even = AllOf(&[2, 4, 3, 6], |value| {
        visited.push(*value);
        value % 2 == 0
    });

    assert!(!all_even);
    assert_eq!(visited, vec![2, 4, 3]);
}

/// 覆盖 i64 最小/最大值与常见整数的十进制字符串表示，顺序与输入一致。
#[test]
fn migration_int64s_to_strings_matches_go_decimal_boundaries() {
    assert_eq!(Int64sToStrings(&[]), Vec::<String>::new());
    assert_eq!(
        Int64sToStrings(&[i64::MIN, -1, 0, 7, 7, i64::MAX]),
        vec![
            "-9223372036854775808",
            "-1",
            "0",
            "7",
            "7",
            "9223372036854775807",
        ]
    );
}

/// `None` 对应 Go nil 返回 `None`；空切片与有元素切片各自深拷贝，元素堆地址不同。
#[test]
fn migration_deep_clone_accepts_standard_clone_types_and_preserves_nil() {
    assert!(DeepClone::<Box<i32>>(None).is_none());
    assert_eq!(
        DeepClone::<String>(Some(&[])).unwrap(),
        Vec::<String>::new()
    );

    let original = vec![Box::new(42), Box::new(-7)];
    let cloned = DeepClone(Some(&original)).unwrap();

    assert_eq!(cloned, original);
    assert_ne!(
        cloned[0].as_ref() as *const i32,
        original[0].as_ref() as *const i32
    );
    assert_ne!(
        cloned[1].as_ref() as *const i32,
        original[1].as_ref() as *const i32
    );
}
