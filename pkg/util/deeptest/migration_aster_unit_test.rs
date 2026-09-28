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

// Deep-clone 断言的迁移补充单元测试。
//
// 相对 Go 原测试，额外覆盖：递归不等断言的标量/结构体/切片/map/函数路径，
// 深拷贝相等对指针地址与内容的区分，以及 ignore/pointer 选项的 glob 与覆盖顺序
//（对齐 Go `applyOptions`：后出现的同类型选项替换先前路径）。

use std::panic::{AssertUnwindSafe, catch_unwind};

use super::{
    AssertDeepClonedEqual, AssertRecursivelyNotEqual, DeepValue, WithIgnorePath,
    WithPointerComparePath,
};

/// 期望闭包内断言失败（panic）；用 `catch_unwind` 捕获以模拟 Go 的失败用例。
fn should_fail(f: impl FnOnce()) {
    assert!(catch_unwind(AssertUnwindSafe(f)).is_err());
}

/// 构造名为 `Point`、含 `x`/`y` 字段的结构体 `DeepValue`，便于复用。
fn point(x: i64, y: i64) -> DeepValue {
    DeepValue::structure("Point", [("x", x), ("y", y)])
}

/// 验证 `AssertRecursivelyNotEqual` 与 Go 行为一致：相等应失败，字段/切片/map 公共部分比较，函数需指针或忽略路径。
#[test]
fn migration_recursively_not_equal_matches_go_behavior() {
    should_fail(|| AssertRecursivelyNotEqual(1_i64, 1_i64, []));
    AssertRecursivelyNotEqual(1_i64, 2_i64, []);
    AssertRecursivelyNotEqual(DeepValue::from(1_i64), DeepValue::from(1_u64), []);
    should_fail(|| AssertRecursivelyNotEqual(DeepValue::Invalid, DeepValue::Invalid, []));

    AssertRecursivelyNotEqual(point(1, 2), point(2, 3), []);
    should_fail(|| AssertRecursivelyNotEqual(point(1, 2), point(1, 3), []));
    AssertRecursivelyNotEqual(
        DeepValue::slice(10, [1_i64, 2, 3]),
        DeepValue::slice(20, [2_i64, 3, 4]),
        [],
    );
    should_fail(|| {
        AssertRecursivelyNotEqual(
            DeepValue::slice(10, [1_i64, 2, 3]),
            DeepValue::slice(20, [1_i64, 3, 4]),
            [],
        )
    });

    AssertRecursivelyNotEqual(
        DeepValue::map(10, [("1", 2_i64), ("2", 3)]),
        DeepValue::map(20, [("2", 4_i64), ("3", 4)]),
        [],
    );
    should_fail(|| {
        AssertRecursivelyNotEqual(
            DeepValue::map(10, [("1", 2_i64), ("2", 3)]),
            DeepValue::map(20, [("1", 2_i64), ("3", 4)]),
            [],
        )
    });

    should_fail(|| AssertRecursivelyNotEqual(DeepValue::function(10), DeepValue::function(20), []));
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
}

/// 验证 `AssertDeepClonedEqual`：内容相等且分配地址不同视为深拷贝成功；同地址或内容不等应失败。
#[test]
fn migration_deep_cloned_equal_matches_go_behavior() {
    AssertDeepClonedEqual(point(1, 2), point(1, 2), []);
    should_fail(|| AssertDeepClonedEqual(point(1, 2), point(1, 3), []));

    AssertDeepClonedEqual(
        DeepValue::pointer("Point", 10, point(1, 2)),
        DeepValue::pointer("Point", 20, point(1, 2)),
        [],
    );
    should_fail(|| {
        AssertDeepClonedEqual(
            DeepValue::pointer("Point", 10, point(1, 2)),
            DeepValue::pointer("Point", 10, point(1, 2)),
            [],
        )
    });
    AssertDeepClonedEqual(
        DeepValue::pointer("Point", 10, point(1, 2)),
        DeepValue::pointer("Point", 10, point(9, 9)),
        [WithPointerComparePath(["$"])],
    );

    AssertDeepClonedEqual(DeepValue::nil_slice("i64"), DeepValue::nil_slice("i64"), []);
    AssertDeepClonedEqual(
        DeepValue::slice(10, [1_i64, 2, 3]),
        DeepValue::slice(20, [1_i64, 2, 3]),
        [],
    );
    should_fail(|| {
        AssertDeepClonedEqual(
            DeepValue::slice(10, [1_i64, 2, 3]),
            DeepValue::slice(20, [1_i64, 2, 4]),
            [],
        )
    });
    should_fail(|| {
        AssertDeepClonedEqual(
            DeepValue::slice(10, [1_i64, 2]),
            DeepValue::slice(20, [1_i64, 2, 3]),
            [],
        )
    });

    AssertDeepClonedEqual(
        DeepValue::map(10, [("1", 2_i64), ("2", 3)]),
        DeepValue::map(20, [("1", 2_i64), ("2", 3)]),
        [],
    );
    AssertDeepClonedEqual(
        DeepValue::map(10, [("1", 9_i64)]),
        DeepValue::map(10, [("1", 2_i64)]),
        [WithPointerComparePath(["$"])],
    );
    should_fail(|| AssertDeepClonedEqual(DeepValue::function(10), DeepValue::function(10), []));
    AssertDeepClonedEqual(
        DeepValue::function(10),
        DeepValue::function(10),
        [WithPointerComparePath(["$"])],
    );
    AssertDeepClonedEqual(DeepValue::nil_function(), DeepValue::nil_function(), []);
    AssertDeepClonedEqual(DeepValue::nil_channel(), DeepValue::nil_channel(), []);
    should_fail(|| AssertDeepClonedEqual(DeepValue::channel(10), DeepValue::channel(20), []));
}

/// 验证路径 glob 忽略与同类型选项后者覆盖前者（对齐 Go `applyOptions`）。
#[test]
fn migration_glob_paths_and_option_order_match_go_behavior() {
    let left = DeepValue::structure("Outer", [("kept", point(1, 2)), ("ignored", point(3, 4))]);
    let right = DeepValue::structure("Outer", [("kept", point(1, 2)), ("ignored", point(9, 9))]);
    should_fail(|| AssertDeepClonedEqual(left.clone(), right.clone(), []));
    AssertDeepClonedEqual(left, right, [WithIgnorePath(["$.ignored*"])]);

    // Like Go applyOptions, a later option of the same kind replaces the earlier paths.
    // 同类型选项后写覆盖先写：只忽略 `$.y` 时 `$.x` 仍会比较并失败。
    should_fail(|| {
        AssertDeepClonedEqual(
            point(1, 2),
            point(9, 2),
            [WithIgnorePath(["$.x"]), WithIgnorePath(["$.y"])],
        )
    });
}
