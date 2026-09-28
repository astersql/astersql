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

// float64 集合：对齐 Go `map[float64]struct{}` 的相等与 NaN 语义。
//
// Go 中 `-0.0` 与 `0.0` 相等；NaN 与任何值（含自身）都不相等，因而每次
// 插入 NaN 都会新增成员。本模块用 `FloatKey` 包装位型，在 Rust `HashMap`
// 上复现上述规则。

use std::collections::HashMap;
/// Hash-map key preserving Go's `float64` map behavior.
///
/// Go considers `-0.0` and `0.0` equal, while NaN is unequal even to itself.
/// The explicit wrapper keeps both rules when a Rust `HashMap` is used.
/// 保留 Go float64 map 语义的哈希键：±0 归一，NaN 不可用于查找。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct FloatKey(u64);

impl FloatKey {
    /// 查询用键：NaN 返回 `None`；±0 归一为 0；其余用 `to_bits`。
    pub(crate) fn for_lookup(value: f64) -> Option<Self> {
        if value.is_nan() {
            None
        } else if value == 0.0 {
            // Go 视 +0/-0 为同一 key，统一映射为 0。
            Some(Self(0))
        } else {
            Some(Self(value.to_bits()))
        }
    }

    /// 插入用键：非 NaN 同 `for_lookup`；NaN 分配递增 payload，保证互不相等。
    pub(crate) fn for_insert(value: f64, next_nan_payload: &mut u64) -> Self {
        if let Some(key) = Self::for_lookup(value) {
            return key;
        }

        // IEEE754 安静 NaN 指数位 + 递增 payload，模拟 Go 每次写入新 NaN 条目。
        const NAN_PAYLOAD_MASK: u64 = 0x000f_ffff_ffff_ffff;
        assert!(
            (1..=NAN_PAYLOAD_MASK).contains(next_nan_payload),
            "too many NaN keys"
        );
        let key = Self(0x7ff0_0000_0000_0000 | *next_nan_payload);
        *next_nan_payload += 1;
        key
    }
}

// Float64Set is a float64 set.
/// float64 集合；内部 `HashMap<FloatKey, ()>` 对应 Go 的 `map[float64]struct{}`。
#[derive(Clone, Debug)]
pub struct Float64Set {
    /// 成员表：仅用 key 存在性表示集合成员。
    inner: HashMap<FloatKey, ()>,
    /// 下一个 NaN 插入用的 payload 计数器，从 1 起递增。
    next_nan_payload: u64,
}

impl Default for Float64Set {
    fn default() -> Self {
        NewFloat64Set(&[])
    }
}

// NewFloat64Set builds a float64 set.
/// 由切片构造集合；对应 Go 变参 `NewFloat64Set(fs ...float64)`。
pub fn NewFloat64Set(fs: &[f64]) -> Float64Set {
    let mut x = Float64Set {
        inner: HashMap::with_capacity(fs.len()),
        next_nan_payload: 1,
    };
    for &f in fs {
        x.Insert(f);
    }
    x
}

impl Float64Set {
    // Exist checks whether `val` exists in `s`.
    /// 判断成员是否存在；NaN 查询恒为 false（与 Go 一致）。
    pub fn Exist(&self, val: f64) -> bool {
        FloatKey::for_lookup(val).is_some_and(|key| self.inner.contains_key(&key))
    }

    // Insert inserts `val` into `s`.
    /// 插入成员；重复非 NaN 不增计数，每次 NaN 各占一席。
    pub fn Insert(&mut self, val: f64) {
        let key = FloatKey::for_insert(val, &mut self.next_nan_payload);
        self.inner.insert(key, ());
    }

    // Count returns the number in Set s.
    /// 返回当前成员数（含多次插入的不同 NaN）。
    pub fn Count(&self) -> usize {
        self.inner.len()
    }
}
