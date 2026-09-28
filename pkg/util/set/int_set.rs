// Copyright 2019 PingCAP, Inc.
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

// 整数集合：`IntSet`（`isize`）与 `Int64Set`（`i64`）。
//
// 对应 Go `map[int]struct{}` / `map[int64]struct{}`：仅用 key 存在性表示成员，
// 无额外 value。Rust 侧用 `HashSet` 表达同等去重与查询语义。

use std::collections::HashSet;

// IntSet is a int set.
// IntSet 对应 Go 的 `type IntSet map[int]struct{}`。
// Go map 只使用 key 存在性表达集合成员，struct{} 值不携带数据；这里用 HashSet<isize> 保留同样意图。
/// 平台位宽整数集合；Go `int` 映射为指针宽度 `isize`。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IntSet {
    // Go `int` maps to pointer-sized `isize`.
    inner: HashSet<isize>,
}

// NewIntSet builds a IntSet.
// NewIntSet 对应 Go 构造函数 `NewIntSet(is ...int) IntSet`。
// Go variadic arguments are represented by a slice; pass an empty slice for no values.
/// 由切片构造 `IntSet`；空切片对应无初始成员。
pub fn NewIntSet(is: &[isize]) -> IntSet {
    // 对应 Go 的 `make(IntSet, len(is))`，预分配容量只影响性能，不改变集合语义。
    let mut set = IntSet {
        inner: HashSet::with_capacity(is.len()),
    };
    // 保持 Go 的循环顺序；重复元素会被 HashSet 去重，等价于多次写入同一个 map key。
    for &x in is {
        set.Insert(x);
    }
    set
}

impl IntSet {
    // Exist checks whether `val` exists in `s`.
    // Exist 对应 Go 方法 `func (s IntSet) Exist(val int) bool`。
    // Go 中通过 `_, ok := s[val]` 判断 key 是否存在；Rust 用 contains 表达同一成员查询。
    /// 判断 `val` 是否为集合成员。
    pub fn Exist(&self, val: isize) -> bool {
        self.inner.contains(&val)
    }

    // Insert inserts `val` into `s`.
    // Insert 对应 Go 方法 `func (s IntSet) Insert(val int)`。
    // Go 写入 `struct{}{}` 不保存额外值；HashSet::insert 只保留 key，重复插入保持集合大小不变。
    /// 插入 `val`；已存在则集合大小不变。
    pub fn Insert(&mut self, val: isize) {
        self.inner.insert(val);
    }

    // Count returns the number in Set s.
    // Count 对应 Go 方法 `func (s IntSet) Count() int`，返回集合当前成员数。
    // Rust collection lengths use usize.
    /// 返回当前成员个数。
    pub fn Count(&self) -> usize {
        self.inner.len()
    }
}

// Int64Set is a int64 set.
// Int64Set 对应 Go 的 `type Int64Set map[int64]struct{}`。
// 这里同样用 HashSet，只把元素类型换成 i64 来对应 Go int64。
/// 固定 64 位整数集合，对应 Go `Int64Set`。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Int64Set {
    // Go map 的 value 是空结构体，仅表示“存在”；Rust HashSet 不需要额外 value。
    inner: HashSet<i64>,
}

// NewInt64Set builds a Int64Set.
// NewInt64Set 对应 Go 构造函数 `NewInt64Set(xs ...int64) Int64Set`。
// Go 变参 `xs` 迁移为切片，保留“按输入序列逐个 Insert”的控制流。
/// 由切片构造 `Int64Set`，按输入顺序逐个 Insert。
pub fn NewInt64Set(xs: &[i64]) -> Int64Set {
    // 对应 Go 的 `make(Int64Set, len(xs))`，使用输入长度作为初始容量。
    let mut set = Int64Set {
        inner: HashSet::with_capacity(xs.len()),
    };
    // 逐项插入保留原 Go 函数的顺序和去重行为。
    for &x in xs {
        set.Insert(x);
    }
    set
}

impl Int64Set {
    // Exist checks whether `val` exists in `s`.
    // Exist 对应 Go 方法 `func (s Int64Set) Exist(val int64) bool`。
    // 原 Go 代码只读取 map，不修改集合；Rust 接收者因此使用不可变引用。
    /// 判断 `val` 是否为集合成员。
    pub fn Exist(&self, val: i64) -> bool {
        self.inner.contains(&val)
    }

    // Insert inserts `val` into `s`.
    // Insert 对应 Go 方法 `func (s Int64Set) Insert(val int64)`。
    // 原 Go map 写入会修改接收者底层 map；Rust 用 &mut self 表达同样的可变语义。
    /// 插入 `val`；已存在则集合大小不变。
    pub fn Insert(&mut self, val: i64) {
        self.inner.insert(val);
    }

    // Count returns the number in Set s.
    // Count 对应 Go 方法 `func (s Int64Set) Count() int`。
    // Rust collection lengths use usize.
    /// 返回当前成员个数。
    pub fn Count(&self) -> usize {
        self.inner.len()
    }
}
