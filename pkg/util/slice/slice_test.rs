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

// slice 单元测试：`AllOf`、`Int64sToStrings`、`DeepClone` 行为对齐。
//
// 覆盖偶数谓词表驱动、非 Clone 类型、i64 边界字符串，以及自定义
// `DeepCloneItem` 实现下的 nil/空切片与堆地址独立性。

use super::*;

/// 表驱动用例：输入切片与期望的 `AllOf(even)` 结果。
struct SliceCase {
    a: Vec<i32>,
    all_of: bool,
}

/// 验证 `AllOf` 对空切片、全偶、含奇元素等输入的结果。
#[test]
fn test_slice() {
    let tests = vec![
        SliceCase {
            a: vec![],
            all_of: true,
        },
        SliceCase {
            a: vec![1, 2, 3],
            all_of: false,
        },
        SliceCase {
            a: vec![1, 3],
            all_of: false,
        },
        SliceCase {
            a: vec![2, 2, 4],
            all_of: true,
        },
    ];

    for test in tests {
        let even = |val: &i32| -> bool { val % 2 == 0 };
        assert_eq!(test.all_of, AllOf(&test.a, even));
    }
}

/// `AllOf` 只要求谓词闭包，元素类型无需实现 `Clone`。
#[test]
fn test_all_of_does_not_require_clone() {
    struct NonClone(i32);

    let values = [NonClone(2), NonClone(4)];
    assert!(AllOf(&values, |value| value.0 % 2 == 0));
}

/// 空切片、重复值与 i64 极值的十进制字符串转换。
#[test]
fn test_int64s_to_strings() {
    let cases = [
        (vec![], vec![]),
        (vec![7, 7], vec!["7", "7"]),
        (
            vec![i64::MIN, 0, i64::MAX],
            vec!["-9223372036854775808", "0", "9223372036854775807"],
        ),
    ];

    for (input, expected) in cases {
        assert_eq!(Int64sToStrings(&input), expected);
    }
}

/// 测试用可深拷贝值：显式实现 `DeepCloneItem`，内部持有 `Box<i32>`。
struct CloneableValue(Box<i32>);

impl DeepCloneItem for CloneableValue {
    fn Clone(&self) -> Self {
        Self(Box::new(*self.0))
    }
}

/// `None`→`None`，空切片长度 0；有元素时值相等但堆指针不同。
#[test]
fn test_deep_clone() {
    assert!(DeepClone::<CloneableValue>(None).is_none());
    assert_eq!(DeepClone::<CloneableValue>(Some(&[])).unwrap().len(), 0);

    let original = [CloneableValue(Box::new(42))];
    let cloned = DeepClone(Some(&original)).unwrap();
    assert_eq!(*cloned[0].0, 42);
    assert_ne!(
        original[0].0.as_ref() as *const i32,
        cloned[0].0.as_ref() as *const i32
    );
}
