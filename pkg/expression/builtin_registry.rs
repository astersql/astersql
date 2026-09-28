// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 内建函数注册表的轻量快照与查找封装。
//
// 对应 Go 全局 `funcs` map：提供按名字插入/查询，以及有序、独立拥有的
// 函数名列表快照，供 SHOW / 调试与跨边界枚举使用。

use std::collections::HashMap;

/// Returns an owned, sorted snapshot of builtin function names.  Taking the
/// registry by reference makes the package-global Go map explicit at the Rust
/// module boundary while preserving its snapshot and ordering semantics.
/// 返回内建函数名的有序拥有快照，不修改源 `HashMap`。
///
/// 对应 Go 包级全局 map 的快照语义：排序保证枚举稳定。
pub fn registered_builtin_function_names<V>(funcs: &HashMap<String, V>) -> Vec<String> {
    let mut names = Vec::with_capacity(funcs.len());
    names.extend(funcs.keys().cloned());
    names.sort_unstable();
    names
}

#[derive(Clone, Debug, Default)]
/// 泛型内建函数注册表：名字 → 函数实现。
pub struct BuiltinRegistry<V> {
    funcs: HashMap<String, V>,
}

impl<V> BuiltinRegistry<V> {
    /// 注册或覆盖同名函数；若已存在则返回旧值。
    pub fn insert(&mut self, name: impl Into<String>, function: V) -> Option<V> {
        self.funcs.insert(name.into(), function)
    }

    /// 返回当前注册表中函数名的有序快照。
    pub fn registered_builtin_function_names(&self) -> Vec<String> {
        registered_builtin_function_names(&self.funcs)
    }

    /// 按名字查找已注册的内建函数。
    pub fn get(&self, name: &str) -> Option<&V> {
        self.funcs.get(name)
    }
}
