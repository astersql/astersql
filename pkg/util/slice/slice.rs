// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 切片通用工具：全量谓词、int64 转字符串、基于 Clone 的深拷贝。
//
// 对应 Go `pkg/util/slice`。`AllOf` 在遇反例时短路；`DeepClone` 用 `Option`
// 区分 Go 的 nil 与空切片。

#![allow(non_snake_case)]

/// 判断切片中所有元素是否都满足谓词；空切片为真，遇反例短路。
// AllOf returns true if all elements in the slice match the predict func.
// AllOf 判断切片中的所有元素是否都满足谓词函数。
// Go 版本通过 `!slices.ContainsFunc(s, !p)` 表达短路逻辑；这里保持“找不到反例”的控制流。
pub fn AllOf<T, F>(s: &[T], mut p: F) -> bool
where
    F: FnMut(&T) -> bool,
{
    !s.iter().any(|x| !p(x))
}

/// 将 int64 切片按十进制逐项转为字符串切片，保持输入顺序。
// Int64sToStrings converts a slice of int64 to a slice of string.
// Int64sToStrings 将 int64 切片逐项转换为十进制字符串切片。
pub fn Int64sToStrings(ints: &[i64]) -> Vec<String> {
    // 对应 Go 中 make([]string, len(ints)) 后按索引写入；
    // collect 会分配目标 Vec，并保持输入顺序不变。
    ints.iter().map(|v| v.to_string()).collect()
}

/// 对应 Go 约束 `interface{ Clone() T }`；标准 `Clone` 类型有默认实现。
// DeepCloneItem 对应 Go 约束 `interface{ Clone() T }`。
// 保留这个 trait 以兼容显式迁移的 Go Clone 方法，同时让标准 Rust Clone 类型直接可用。
pub trait DeepCloneItem: Sized {
    /// 返回元素的独立副本（对应 Go 的 `Clone()` 方法）。
    fn Clone(&self) -> Self;
}

impl<T> DeepCloneItem for T
where
    T: Clone,
{
    fn Clone(&self) -> Self {
        self.clone()
    }
}

/// 用元素的 `Clone()` 深拷贝切片；`None` 表示 Go nil，空切片仍返回空 `Vec`。
// DeepClone uses Clone() to clone a slice.
// The elements in the slice must implement func (T) Clone() T.
// DeepClone 使用元素自己的 Clone() 方法深拷贝切片。
pub fn DeepClone<T>(s: Option<&[T]>) -> Option<Vec<T>>
where
    T: DeepCloneItem,
{
    // Go 代码显式区分 nil slice 和空 slice：nil 输入返回 nil。
    // Rust 切片引用本身不能为 nil，因此这段逻辑用 Option 表达原 Go 的 nil 分支。
    let slice = match s {
        Some(slice) => slice,
        None => return None,
    };

    // 对应 Go 中 make([]T, 0, len(s))，先预留容量，再按原顺序追加每个元素的 Clone() 结果。
    let mut cloned = Vec::with_capacity(slice.len());
    for item in slice {
        cloned.push(item.Clone());
    }
    Some(cloned)
}
