// Copyright 2024 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// Deep-clone 静态测试助手的单元测试（对应 Go `statictesthelper` 测试）。
//
// 覆盖 `AssertRecursivelyNotEqual` / `AssertDeepClonedEqual` 对标量、nil、结构体、
// 切片、map、interface、函数的行为，以及 `WithPointerComparePath` / `WithIgnorePath` 选项。

use std::panic::{AssertUnwindSafe, catch_unwind};

use super::{
    AssertDeepClonedEqual, AssertRecursivelyNotEqual, DeepValue, WithIgnorePath,
    WithPointerComparePath,
};

/// 期望闭包内断言失败（panic）；失败信息固定为 `"test should have failed"`。
fn should_fail(f: impl FnOnce()) {
    assert!(
        catch_unwind(AssertUnwindSafe(f)).is_err(),
        "test should have failed"
    );
}

/// 构造测试结构体 `testStructA`（字段 `a`/`b`）。
fn pair(a: i64, b: i64) -> DeepValue {
    DeepValue::structure("testStructA", [("a", a), ("b", b)])
}

/// 构造带显式地址的 `i64` 切片 `DeepValue`。
fn int_slice(address: usize, values: &[i64]) -> DeepValue {
    DeepValue::slice(address, values.iter().copied())
}

/// 构造带显式地址的字符串键 `i64` map `DeepValue`。
fn int_map(address: usize, entries: &[(&str, i64)]) -> DeepValue {
    DeepValue::map(address, entries.iter().copied())
}

/// 构造 interface：外层类型名固定，内层为指向空结构体的指针。
fn test_interface(implementation: &str, address: usize) -> DeepValue {
    DeepValue::interface(
        "testInterface",
        DeepValue::pointer(
            implementation,
            address,
            DeepValue::structure(implementation, [("_", 0_i64)]),
        ),
    )
}

// TestAssertRecursivelyNotEqual covers scalar, nil, struct, slice, map,
// interface, and function behavior from the Go test.
/// 覆盖标量、nil、结构体、切片、map、interface、函数的递归不等断言（对齐 Go）。
#[test]
fn test_assert_recursively_not_equal() {
    should_fail(|| AssertRecursivelyNotEqual(1_i64, 1_i64, []));
    AssertRecursivelyNotEqual(1_i64, 2_i64, []);

    // Nil values are considered equal and therefore fail this assertion.
    // nil / Invalid 视为相等，因此“递归不等”断言应失败。
    should_fail(|| AssertRecursivelyNotEqual(DeepValue::Invalid, DeepValue::Invalid, []));

    // Different dynamic types are considered recursively not equal.
    // 动态类型不同则视为递归不等。
    AssertRecursivelyNotEqual(DeepValue::from(1_f64), DeepValue::from(1_i64), []);

    // Every common struct field is compared.
    // 结构体公共字段逐一比较：仅一侧字段不同时公共部分仍可能“相等”导致失败。
    AssertRecursivelyNotEqual(pair(1, 2), pair(2, 3), []);
    should_fail(|| AssertRecursivelyNotEqual(pair(1, 2), pair(1, 3), []));

    // The common portion of slices is compared.
    // 切片比较公共前缀元素。
    AssertRecursivelyNotEqual(int_slice(10, &[1, 2, 3]), int_slice(20, &[2, 3, 4]), []);
    should_fail(|| {
        AssertRecursivelyNotEqual(int_slice(10, &[1, 2, 3]), int_slice(20, &[1, 2, 4]), [])
    });

    // Common map keys are compared.
    // map 仅比较公共键对应的值。
    AssertRecursivelyNotEqual(
        int_map(10, &[("1", 2), ("2", 3)]),
        int_map(20, &[("2", 4), ("3", 4)]),
        [],
    );
    should_fail(|| {
        AssertRecursivelyNotEqual(
            int_map(10, &[("1", 2), ("2", 3)]),
            int_map(20, &[("1", 2), ("3", 4)]),
            [],
        )
    });

    AssertRecursivelyNotEqual(
        test_interface("testInterfaceImplA", 10),
        test_interface("testInterfaceImplB", 20),
        [],
    );

    // Functions must be compared by pointer or ignored.
    // 函数默认不可直接比内容，需指针比较或忽略路径，否则断言失败。
    should_fail(|| AssertRecursivelyNotEqual(DeepValue::function(10), DeepValue::function(20), []));
}

// TestAssertRecursivelyNotEqualAndComparePointer verifies the pointer option
// for functions, pointers, slices, and maps.
/// 验证指针比较 / 忽略路径选项作用于函数、指针、切片与 map。
#[test]
fn test_assert_recursively_not_equal_and_compare_pointer() {
    AssertRecursivelyNotEqual(
        DeepValue::function(10),
        DeepValue::function(20),
        [WithPointerComparePath(["$"])],
    );
    AssertRecursivelyNotEqual(
        DeepValue::function(10),
        DeepValue::function(10),
        [WithIgnorePath(["$"])],
    );
    should_fail(|| {
        AssertRecursivelyNotEqual(
            DeepValue::function(10),
            DeepValue::function(10),
            [WithPointerComparePath(["$"])],
        )
    });

    AssertRecursivelyNotEqual(
        DeepValue::pointer("structA", 10, pair(1, 0)),
        DeepValue::pointer("structA", 20, pair(1, 0)),
        [WithPointerComparePath(["$"])],
    );
    AssertRecursivelyNotEqual(
        int_slice(10, &[1, 2, 3]),
        int_slice(20, &[1, 2, 3]),
        [WithPointerComparePath(["$"])],
    );
    AssertRecursivelyNotEqual(
        int_map(10, &[("1", 2), ("2", 3)]),
        int_map(20, &[("1", 2), ("2", 3)]),
        [WithPointerComparePath(["$"])],
    );
}

// TestAssertDeepClonedEqual covers deep equality and allocation identity for
// structs, pointers, slices, maps, interfaces, and functions.
/// 覆盖深拷贝相等：内容一致且分配身份（地址）不同；同地址或内容不等应失败。
#[test]
fn test_assert_deep_cloned_equal() {
    AssertDeepClonedEqual(pair(1, 2), pair(1, 2), []);

    AssertDeepClonedEqual(
        DeepValue::pointer("structA", 10, pair(1, 2)),
        DeepValue::pointer("structA", 20, pair(1, 2)),
        [],
    );
    should_fail(|| {
        AssertDeepClonedEqual(
            DeepValue::pointer("structA", 10, pair(1, 2)),
            DeepValue::pointer("structA", 10, pair(1, 2)),
            [],
        )
    });
    AssertDeepClonedEqual(
        DeepValue::pointer("structA", 10, pair(1, 2)),
        DeepValue::pointer("structA", 10, pair(9, 9)),
        [WithPointerComparePath(["$"])],
    );

    AssertDeepClonedEqual(DeepValue::nil_slice("i64"), DeepValue::nil_slice("i64"), []);
    AssertDeepClonedEqual(int_slice(10, &[1, 2, 3]), int_slice(20, &[1, 2, 3]), []);
    should_fail(|| AssertDeepClonedEqual(int_slice(10, &[1, 2, 3]), int_slice(20, &[1, 2, 4]), []));
    should_fail(|| {
        AssertDeepClonedEqual(int_slice(10, &[1, 2, 3]), int_slice(20, &[1, 2, 3, 4]), [])
    });
    should_fail(|| AssertDeepClonedEqual(int_slice(10, &[1, 2, 3]), int_slice(20, &[1, 2]), []));
    should_fail(|| {
        AssertDeepClonedEqual(
            int_slice(10, &[1, 2, 3]),
            int_slice(20, &[1, 2, 3]),
            [WithPointerComparePath(["$"])],
        )
    });
    AssertDeepClonedEqual(
        int_slice(10, &[1, 2]),
        int_slice(10, &[1, 2]),
        [WithPointerComparePath(["$"])],
    );
    should_fail(|| {
        AssertDeepClonedEqual(
            int_slice(10, &[1, 2]),
            int_slice(10, &[1, 2, 3]),
            [WithPointerComparePath(["$"])],
        )
    });

    AssertDeepClonedEqual(DeepValue::nil_map("map"), DeepValue::nil_map("map"), []);
    AssertDeepClonedEqual(
        int_map(10, &[("1", 2), ("2", 3)]),
        int_map(20, &[("1", 2), ("2", 3)]),
        [],
    );
    should_fail(|| {
        AssertDeepClonedEqual(
            int_map(10, &[("1", 2), ("2", 3)]),
            int_map(20, &[("1", 2), ("3", 4)]),
            [],
        )
    });
    AssertDeepClonedEqual(
        int_map(10, &[("1", 2), ("2", 3)]),
        int_map(10, &[("1", 9), ("2", 8)]),
        [WithPointerComparePath(["$"])],
    );
    should_fail(|| {
        AssertDeepClonedEqual(
            int_map(10, &[("1", 2), ("2", 3)]),
            int_map(20, &[("1", 2), ("2", 3)]),
            [WithPointerComparePath(["$"])],
        )
    });

    AssertDeepClonedEqual(
        test_interface("testInterfaceImplA", 10),
        test_interface("testInterfaceImplA", 20),
        [],
    );
    should_fail(|| {
        AssertDeepClonedEqual(
            test_interface("testInterfaceImplA", 10),
            test_interface("testInterfaceImplA", 10),
            [],
        )
    });
    AssertDeepClonedEqual(
        test_interface("testInterfaceImplA", 10),
        test_interface("testInterfaceImplA", 10),
        [WithPointerComparePath(["$"])],
    );

    AssertDeepClonedEqual(DeepValue::nil_function(), DeepValue::nil_function(), []);
    AssertDeepClonedEqual(
        DeepValue::function(10),
        DeepValue::function(10),
        [WithPointerComparePath(["$"])],
    );
    should_fail(|| AssertDeepClonedEqual(DeepValue::function(10), DeepValue::function(20), []));
}
