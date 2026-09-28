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

// 通用并查集：支持任意可哈希类型与稀疏元素。
//
// 对应 Go `Set`：将原值映射为内部整数下标后再跑核心并查集算法；连续整数场景
// 应优先用 `SimpleIntSet` 以避免 HashMap 开销。`Union` 保留 `a` 侧根为新集合根。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::hash::Hash;

// Set is the universal implementation of a disjoint set.
// It's designed for sparse cases or non-integer types.
// If you are dealing with continuous integers, you should use SimpleIntSet to avoid the cost of a hash map.
// We hash the original value to an integer index and then apply the core disjoint set algorithm.
// Time complexity: the union operation has an inverse Ackermann function time complexity, which is very close to O(1).
/// 通用并查集：值↔下标双向映射 + `parent` 数组。
pub struct Set<T>
where
    T: Eq + Hash + Clone,
{
    /// 内部下标的父指针数组。
    pub(super) parent: Vec<usize>,
    /// 原值到内部下标的映射。
    val2Idx: HashMap<T, usize>,
    /// 内部下标到原值的映射（根下标对应集合代表原值）。
    idx2Val: HashMap<usize, T>,
    /// 下一个可分配的内部下标。
    tailIdx: usize,
}

// NewSet creates a disjoint set.
/// 创建预留容量约 `size` 的空并查集（元素按首次访问惰性注册）。
pub fn NewSet<T>(size: usize) -> Set<T>
where
    T: Eq + Hash + Clone,
{
    Set {
        parent: Vec::with_capacity(size),
        val2Idx: HashMap::with_capacity(size),
        idx2Val: HashMap::with_capacity(size),
        tailIdx: 0,
    }
}

impl<T> Set<T>
where
    T: Eq + Hash + Clone,
{
    /// 查找原值 `a` 所属集合根下标；若尚未注册则分配新下标并自成一类。
    fn findRootOriginalVal(&mut self, a: T) -> usize {
        if let Some(&idx) = self.val2Idx.get(&a) {
            return self.findRootInternal(idx);
        }

        let idx = self.tailIdx;
        self.parent.push(idx);
        self.val2Idx.insert(a.clone(), idx);
        self.idx2Val.insert(idx, a);
        self.tailIdx += 1;
        idx
    }

    // findRootInternal is an internal implementation. Call it inside findRootOriginalVal.
    /// 按内部下标查找根，并做路径压缩。
    fn findRootInternal(&mut self, a: usize) -> usize {
        let mut root = a;
        while root != self.parent[root] {
            root = self.parent[root];
        }

        // Path compression, which leads the time complexity to the inverse Ackermann function.
        // 路径压缩：沿途节点直接挂到根上。
        let mut current = a;
        while current != root {
            let parent = self.parent[current];
            self.parent[current] = root;
            current = parent;
        }
        root
    }

    // InSameGroup checks whether a and b are in the same group.
    /// 判断 `a` 与 `b` 是否属于同一连通分量（必要时惰性注册）。
    pub fn InSameGroup(&mut self, a: T, b: T) -> bool {
        self.findRootOriginalVal(a) == self.findRootOriginalVal(b)
    }

    // Union joins two sets in the disjoint set.
    /// 合并两集合：将 `b` 的根挂到 `a` 的根下（保留 `a` 侧代表元）。
    pub fn Union(&mut self, a: T, b: T) {
        let root_a = self.findRootOriginalVal(a);
        let root_b = self.findRootOriginalVal(b);
        // take b as successor, respect the rootA as the root of the new set.
        // 以 a 的根为新集合根，b 的根成为其子节点。
        if root_a != root_b {
            self.parent[root_b] = root_a;
        }
    }

    // FindRoot finds the root of the set that contains a.
    /// 返回含 `a` 的集合根下标；若 `a` 不在集合中则先注册再返回。
    pub fn FindRoot(&mut self, a: T) -> usize {
        // if a is not in the set, assign a new index to it.
        // 未注册时分配新内部下标。
        self.findRootOriginalVal(a)
    }

    // FindVal finds the value of the set corresponding to the index.
    /// 由内部下标找到其集合代表原值；有效下标恒返回 `(值, true)`。
    pub fn FindVal(&mut self, idx: usize) -> (T, bool) {
        let root = self.findRootInternal(idx);
        // Every valid internal index is inserted into idx2Val together with parent.
        // 合法下标与 parent 同步写入 idx2Val。
        (self.idx2Val[&root].clone(), true)
    }
}
