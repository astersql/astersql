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

// 稠密整数并查集单元测试（对应 Go `int_set` 测试）。
//
// 验证初始自环、`Union` 形成连通分量、`FindRoot` 一致性、幂等合并，以及 `Clear`/`GrowNewIntSet`。

use super::*;

// TestIntDisjointSet 对应 Go 的单元测试：构造 10 个元素的并查集并验证若干连通分量。
/// 构造 10 元并查集，按 Go 测试顺序合并后检查三组连通分量与清空/扩容。
#[test]
fn test_int_disjoint_set() {
    let mut set = NewIntSet(10);
    assert_eq!(set.parent.len(), 10);
    for i in 0..set.parent.len() {
        assert_eq!(i, set.parent[i]);
    }

    // 下面的 Union 顺序与 Go 测试一致，用于形成 {0,1,3,5}、{2,4,6,9}、{7,8} 三组。
    set.Union(0, 1);
    set.Union(1, 3);
    set.Union(4, 2);
    set.Union(2, 6);
    set.Union(3, 5);
    set.Union(7, 8);
    set.Union(9, 6);

    assert_eq!(set.FindRoot(0), set.FindRoot(1));
    assert_eq!(set.FindRoot(3), set.FindRoot(1));
    assert_eq!(set.FindRoot(5), set.FindRoot(1));
    assert_eq!(set.FindRoot(2), set.FindRoot(4));
    assert_eq!(set.FindRoot(6), set.FindRoot(4));
    assert_eq!(set.FindRoot(9), set.FindRoot(2));
    assert_eq!(set.FindRoot(7), set.FindRoot(8));

    // 已在同一集合内再 Union，以及自合并，代表元应保持不变。
    let root = set.FindRoot(0);
    set.Union(0, 5);
    set.Union(5, 5);
    assert_eq!(set.FindRoot(0), root);

    set.Clear();
    assert!(set.parent.is_empty());
    set.GrowNewIntSet(4);
    assert_eq!(set.parent, vec![0, 1, 2, 3]);
}
