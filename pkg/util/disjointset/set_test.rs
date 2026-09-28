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

// 通用并查集 `Set` 的单元测试。
//
// 对应 Go `TestDisjointSet`：用字符串键验证懒创建节点、`Union`/`InSameGroup`/
// `FindRoot`/`FindVal` 的连通性与根不变性。

use super::*;

// TestDisjointSet 对应 Go 的同名测试，验证字符串键的稀疏并查集行为。
#[test]
fn TestDisjointSet() {
    // NewSet[string](10) 在 Go 中只预分配 map/slice 容量；实际节点仍在首次查询或合并时懒创建。
    let mut set = NewSet::<String>(10);

    // InSameGroup("a", "b") 会为两个新值分配索引，因此第一次查询后 parent 长度变为 2。
    assert!(!set.InSameGroup("a".to_string(), "b".to_string()));
    assert_eq!(set.parent.len(), 2);

    // Union 把 b 所在根挂到 a 所在根；后续查询 a/b 应在同一组。
    set.Union("a".to_string(), "b".to_string());
    assert!(set.InSameGroup("a".to_string(), "b".to_string()));

    // 查询新值 c 时会新增第三个节点，但不会自动并入 a/b。
    assert!(!set.InSameGroup("a".to_string(), "c".to_string()));
    assert_eq!(set.parent.len(), 3);
    assert!(!set.InSameGroup("b".to_string(), "c".to_string()));
    assert_eq!(set.parent.len(), 3);

    // 将 b 与 c 合并后，a/b/c 通过根节点传递性连通。
    set.Union("b".to_string(), "c".to_string());
    assert!(set.InSameGroup("a".to_string(), "c".to_string()));
    assert!(set.InSameGroup("b".to_string(), "c".to_string()));

    // 另建 d/e/f/g 链，检查独立集合不会影响 a/b/c 的连通性。
    set.Union("d".to_string(), "e".to_string());
    set.Union("e".to_string(), "f".to_string());
    set.Union("f".to_string(), "g".to_string());
    assert_eq!(set.parent.len(), 7);
    assert!(!set.InSameGroup("a".to_string(), "d".to_string()));
    assert!(set.InSameGroup("d".to_string(), "g".to_string()));
    assert!(!set.InSameGroup("c".to_string(), "g".to_string()));

    // 最后把两个集合接起来，原测试逐一确认所有代表值已经跨集合连通。
    set.Union("a".to_string(), "g".to_string());
    assert!(set.InSameGroup("a".to_string(), "d".to_string()));
    assert!(set.InSameGroup("b".to_string(), "g".to_string()));
    assert!(set.InSameGroup("c".to_string(), "f".to_string()));
    assert!(set.InSameGroup("a".to_string(), "e".to_string()));
    assert!(set.InSameGroup("b".to_string(), "c".to_string()));

    let root = set.FindRoot("a".to_string());
    set.Union("a".to_string(), "a".to_string());
    set.Union("b".to_string(), "a".to_string());
    assert_eq!(set.FindRoot("a".to_string()), root);
    let (value, ok) = set.FindVal(root);
    assert!(ok);
    assert_eq!(value, "a");
}
