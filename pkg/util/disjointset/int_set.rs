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

// 稠密整数并查集（disjoint set / union-find）。
//
// 对应 Go `SimpleIntSet`：用连续下标的 `parent` 数组表示连通分量，适合元素连续的场景；
// 稀疏或非整数请用 `Set`。`Union`/`FindRoot` 配合路径压缩，摊还接近 O(1)
//（逆阿克曼函数级别）。

#![allow(non_snake_case)]

// SimpleIntSet is the int disjoint set.
// It's not designed for sparse case. You should use it when the elements are continuous.
// Time complexity: the union operation is inverse ackermann function, which is very close to O(1).
/// 整数并查集：`parent[i]` 指向所属集合代表元；初始时每个下标自成一类。
pub struct SimpleIntSet {
    /// 父指针数组：下标即元素 id，值为其父节点（根满足 `parent[root] == root`）。
    pub(super) parent: Vec<usize>,
}

// NewIntSet returns a new int disjoint set.
/// 创建容量为 `size` 的整数并查集，初始每个元素指向自身。
pub fn NewIntSet(size: usize) -> SimpleIntSet {
    SimpleIntSet {
        parent: (0..size).collect(),
    }
}

impl SimpleIntSet {
    // Union unions two sets in int disjoint set.
    /// 合并 `a` 与 `b` 所在集合：将 `a` 的根挂到 `b` 的根下。
    pub fn Union(&mut self, a: usize, b: usize) {
        let root_a = self.FindRoot(a);
        let root_b = self.FindRoot(b);
        self.parent[root_a] = root_b;
    }

    // FindRoot finds the representative element of the set that `a` belongs to.
    /// 查找 `a` 所属集合的代表元（根），并做路径压缩。
    pub fn FindRoot(&mut self, a: usize) -> usize {
        let mut root = a;
        while root != self.parent[root] {
            root = self.parent[root];
        }

        // Path compression, which leads the time complexity to the inverse Ackermann function.
        // 路径压缩：沿途节点直接挂到根上，降低后续查找深度。
        let mut current = a;
        while current != root {
            let parent = self.parent[current];
            self.parent[current] = root;
            current = parent;
        }
        root
    }

    // Clear clears the int disjoint set.
    /// 清空并查集（释放全部元素）。
    pub fn Clear(&mut self) {
        self.parent.clear();
    }

    // GrowNewIntSet grows the int disjoint set to at least `n` elements.
    /// 重置为恰好 `n` 个独立元素（先清空再按 `0..n` 重建）。
    pub fn GrowNewIntSet(&mut self, n: usize) {
        self.parent.clear();
        self.parent.reserve(n);
        self.parent.extend(0..n);
    }
}
