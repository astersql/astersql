// Copyright 2026 AsterSQL.

// Copyright 2024 PingCAP, Inc.
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

// 并查集迁移补充单元测试。
//
// 覆盖稠密 `SimpleIntSet` 与稀疏 `Set` 的 Union/Find/Clear/Grow 行为，
// 以及深链合并后路径压缩不因栈溢出失败（对齐 Go 大链场景）。

use super::{NewIntSet, NewSet};

/// 验证整数并查集合并连通性、清空后 `GrowNewIntSet` 重建独立元素。
#[test]
fn simple_int_set_matches_go_union_clear_and_grow_behavior() {
    let mut set = NewIntSet(10);
    set.Union(0, 1);
    set.Union(1, 3);
    set.Union(4, 2);
    set.Union(2, 6);
    set.Union(3, 5);
    set.Union(7, 8);
    set.Union(9, 6);

    assert_eq!(set.FindRoot(0), set.FindRoot(5));
    assert_eq!(set.FindRoot(4), set.FindRoot(9));
    assert_eq!(set.FindRoot(7), set.FindRoot(8));
    assert_ne!(set.FindRoot(0), set.FindRoot(4));

    set.Clear();
    set.GrowNewIntSet(4);
    for index in 0..4 {
        assert_eq!(index, set.FindRoot(index));
    }
}

/// 验证稀疏 `Set`：惰性注册、Union 后同组、`FindRoot`/`FindVal` 根值语义。
#[test]
fn sparse_set_matches_go_registration_union_and_root_value_behavior() {
    let mut set = NewSet::<&str>(2);

    assert!(!set.InSameGroup("a", "b"));
    let a = set.FindRoot("a");
    let b = set.FindRoot("b");
    assert_ne!(a, b);

    set.Union("a", "b");
    set.Union("b", "c");
    assert!(set.InSameGroup("a", "c"));
    assert_eq!(set.FindRoot("a"), a);
    assert_eq!(set.FindVal(b), ("a", true));

    let d = set.FindRoot("d");
    assert_eq!(set.FindVal(d), ("d", true));
    assert!(!set.InSameGroup("a", "d"));
}

/// 十万级链式 Union 后 `FindRoot` 应压缩到链尾根，且不栈溢出。
#[test]
fn simple_int_set_compresses_a_deep_go_style_chain_without_stack_overflow() {
    const ELEMENTS: usize = 100_000;
    let mut set = NewIntSet(ELEMENTS);
    for index in 0..ELEMENTS - 1 {
        set.Union(index, index + 1);
    }

    assert_eq!(set.FindRoot(0), ELEMENTS - 1);
    assert_eq!(set.FindRoot(ELEMENTS / 2), ELEMENTS - 1);
}

/// 稀疏集深链：以较小下标为根合并，最终 `FindVal(0)` 应映射到末元素原值。
#[test]
fn sparse_set_compresses_a_deep_go_style_chain_without_stack_overflow() {
    const ELEMENTS: usize = 100_000;
    let mut set = NewSet::<usize>(ELEMENTS);
    for index in 0..ELEMENTS - 1 {
        set.Union(index + 1, index);
    }

    let first = set.FindRoot(0);
    assert_eq!(first, set.FindRoot(ELEMENTS - 1));
    assert_eq!(set.FindVal(0), (ELEMENTS - 1, true));
}
