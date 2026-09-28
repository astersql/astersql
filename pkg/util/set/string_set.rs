// Copyright 2018 PingCAP, Inc.
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

// 字符串集合 `StringSet`。
//
// 对应 Go 的 `map[string]struct{}`：只关心 key 是否存在。提供构造、Exist/Insert、
// 交集（含大小写折叠查询）、Count/Empty/Clear 以及回调遍历。

use std::collections::HashSet;

/// Apply Unicode simple uppercase mappings, matching Go's `strings.ToUpper`.
/// Rust exposes full mappings, so characters whose uppercase form expands to
/// multiple scalars retain their original scalar, as Go's rune mapping does.
fn go_simple_uppercase(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            let mut uppercase = ch.to_uppercase();
            let first = uppercase.next().expect("case mapping is never empty");
            if uppercase.next().is_none() {
                first
            } else {
                ch
            }
        })
        .collect()
}

/// Apply Unicode simple lowercase mappings, matching Go's `strings.ToLower`.
/// The only unconditional Unicode lowercase expansion starts with the scalar
/// used by its simple mapping, so discard the combining suffix Rust emits.
fn go_simple_lowercase(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            ch.to_lowercase()
                .next()
                .expect("case mapping is never empty")
        })
        .collect()
}

// StringSet is a string set.
// StringSet 对应 Go 的 `type StringSet map[string]struct{}`。
// Go map 的 value 是空结构体，只表达“key 是否存在”；这里用 HashSet<String> 保留集合语义。
/// 字符串集合，内部以 `HashSet<String>` 存储成员。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StringSet {
    // Rust 用私有 HashSet 字段维护成员不变量。
    inner: HashSet<String>,
}

// NewStringSet builds a string set.
// NewStringSet 对应 Go 构造函数 `NewStringSet(ss ...string) StringSet`。
// Rust 没有 Go 式可变参数；这里用字符串切片表达 `ss`。
/// 由字符串切片构造集合；空切片得到空集。
pub fn NewStringSet(ss: &[&str]) -> StringSet {
    // 对应 Go 的 `make(StringSet, len(ss))`，按输入数量预分配容量只影响性能，不改变语义。
    let mut set = StringSet {
        inner: HashSet::with_capacity(ss.len()),
    };
    // 保持 Go 的 range 控制流：逐项调用 Insert，重复字符串会被集合自然去重。
    for s in ss {
        set.Insert((*s).to_owned());
    }
    set
}

impl StringSet {
    // Exist checks whether `val` exists in `s`.
    // Exist 对应 Go 方法 `func (s StringSet) Exist(val string) bool`。
    // Go 通过 `_, ok := s[val]` 判断 key 是否存在；Rust 用 contains 表达同一成员查询。
    /// 判断字符串是否为集合成员。
    pub fn Exist(&self, val: &str) -> bool {
        self.inner.contains(val)
    }

    // Insert inserts `val` into `s`.
    // Insert 对应 Go 方法 `func (s StringSet) Insert(val string)`。
    // Go 写入 `struct{}{}` 不保存额外值；HashSet::insert 只保留字符串 key。
    /// 插入字符串；重复插入不影响集合大小。
    pub fn Insert(&mut self, val: String) {
        self.inner.insert(val);
    }

    // Intersection returns the intersection of two sets
    // Intersection 对应 Go 方法 `func (s StringSet) Intersection(rhs StringSet) StringSet`。
    // Go 遍历左侧集合并查询右侧集合；命中时把原字符串插入新集合。
    /// 返回与 `rhs` 的交集（遍历左侧，保留左侧原始字符串）。
    pub fn Intersection(&self, rhs: &StringSet) -> StringSet {
        let mut newSet = NewStringSet(&[]);
        // Go map 和 Rust HashSet 都不承诺遍历顺序。
        for elt in &self.inner {
            if rhs.Exist(elt) {
                newSet.Insert(elt.clone());
            }
        }
        newSet
    }

    // IntersectionWithLower returns the intersection of two sets with different case of string.
    // IntersectionWithLower 对应 Go 方法 `func (s StringSet) IntersectionWithLower(rhs StringSet, toLower bool) StringSet`。
    // 它遍历 rhs，将 rhs 元素转成小写或大写后去 s 中查询，命中时保留 rhs 的原始字符串。
    /// 大小写折叠后的交集：遍历 rhs，用 lower/upper 形式在 self 中查询，命中则保留 rhs 原文。
    pub fn IntersectionWithLower(&self, rhs: &StringSet, toLower: bool) -> StringSet {
        let mut newSet = NewStringSet(&[]);
        // 这里保持 Go 的“遍历 rhs，而不是遍历 s”的方向，因为返回值插入的是 rhs 原始元素 origElt。
        for origElt in &rhs.inner {
            let elt = if toLower {
                // Go 使用逐 rune 的简单映射，不采用 Rust 的多字符完整映射。
                go_simple_lowercase(origElt)
            } else {
                // Go 使用逐 rune 的简单映射，不采用 Rust 的多字符完整映射。
                go_simple_uppercase(origElt)
            };
            if self.Exist(&elt) {
                newSet.Insert(origElt.clone());
            }
        }
        newSet
    }

    // Count returns the number in Set s.
    // Count 对应 Go 方法 `func (s StringSet) Count() int`，返回集合当前成员数。
    // Rust collection lengths use usize.
    /// 返回当前成员个数。
    pub fn Count(&self) -> usize {
        self.inner.len()
    }

    // Empty returns whether s is empty.
    // Empty 对应 Go 方法 `func (s StringSet) Empty() bool`，等价于判断 len(s) == 0。
    /// 判断集合是否为空。
    pub fn Empty(&self) -> bool {
        self.inner.is_empty()
    }

    // Clear clears the set.
    // Clear 对应 Go 方法 `func (s StringSet) Clear()`。
    // Go 的 clear(map) 会删除所有 key；HashSet::clear 表达同样的原地清空语义。
    /// 清空全部成员。
    pub fn Clear(&mut self) {
        self.inner.clear();
    }

    // IterateWith iterate items in StringSet and pass it to `fn`.
    // IterateWith 对应 Go 方法 `func (s StringSet) IterateWith(fn func(string))`。
    // Go 回调接收 string 值；Rust 克隆 key 后交给 FnMut，避免暴露内部引用生命周期。
    /// 遍历集合并对每个成员调用回调（顺序未定义）。
    pub fn IterateWith<F>(&self, mut fn_: F)
    where
        F: FnMut(String),
    {
        // Go map range 顺序未定义；HashSet 遍历也不提供稳定顺序，调用方不应依赖顺序。
        for k in &self.inner {
            fn_(k.clone());
        }
    }
}
