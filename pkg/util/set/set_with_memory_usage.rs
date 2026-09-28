// Copyright 2021 PingCAP, Inc.
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

// 带内存用量追踪的集合/映射封装。
//
// 在 `MemAwareMap` 之上包装字符串映射、十进制映射以及 string/float64/int64 集合，
// 可选绑定 `memory::Tracker`：插入产生的字节增量可立即 `Consume`，否则作为未追踪
// delta 返回给调用方（便于上层统一记账）。float64 集合通过 `FloatKey` 处理 NaN/-0.0。

use crate::float64_set::FloatKey;
use crate::{hack, memory, types};
use std::hash::Hash;
use std::sync::Arc;

/// 按容量构造内存感知哈希表（解引用 `NewMemAwareMap` 的装箱结果）。
fn new_mem_aware_map<K: Eq + Hash, V>(capacity: usize) -> hack::MemAwareMap<K, V> {
    *hack::NewMemAwareMap(capacity)
}

// StringToStringMapWithMemoryUsage is a string-string map with memory usage.
/// 字符串到字符串的映射，并统计占用字节；可选绑定内存 Tracker。
pub struct StringToStringMapWithMemoryUsage {
    /// 底层带字节计数的哈希表。
    pub mem_aware_map: hack::MemAwareMap<String, String>,
    /// 若存在，非零增量会立刻 Consume，Insert 对外返回 0。
    tracker: Option<Arc<memory::Tracker>>,
}

impl Default for StringToStringMapWithMemoryUsage {
    fn default() -> Self {
        NewStringToStringMapWithMemoryUsage().0
    }
}

// NewStringToStringMapWithMemoryUsage builds a string map.
/// 构造空的字符串映射，并返回初始 Bytes 作为未追踪 delta。
pub fn NewStringToStringMapWithMemoryUsage() -> (StringToStringMapWithMemoryUsage, i64) {
    let mem_aware_map = new_mem_aware_map(0);
    let delta = mem_aware_map.Bytes as i64;
    (
        StringToStringMapWithMemoryUsage {
            mem_aware_map,
            tracker: None,
        },
        delta,
    )
}

impl StringToStringMapWithMemoryUsage {
    // Insert inserts `val` into `s` and returns the untracked delta.
    /// 插入键值；有 Tracker 时消费增量并返回 0，否则返回未追踪 delta。
    pub fn Insert(&mut self, key: String, val: String) -> i64 {
        let delta = self.mem_aware_map.Set(key, val);
        // 非零增量且已绑定 Tracker：立刻记账，调用方无需再累加。
        if delta != 0 {
            if let Some(tracker) = &self.tracker {
                tracker.Consume(delta);
                return 0;
            }
        }
        delta
    }

    // SetTracker makes future non-zero deltas consume immediately.
    /// 绑定或清除内存 Tracker，影响后续 Insert 的返回语义。
    pub fn SetTracker(&mut self, tracker: Option<Arc<memory::Tracker>>) {
        self.tracker = tracker;
    }

    /// 判断 key 是否存在。
    pub fn Exist<Q: AsRef<str>>(&self, key: Q) -> bool {
        self.mem_aware_map.M.contains_key(key.as_ref())
    }

    /// 返回元素个数（与底层 Count 一致）。
    pub fn Count(&self) -> usize {
        self.mem_aware_map.Count()
    }

    /// 返回长度（与底层 Len 一致）。
    pub fn Len(&self) -> usize {
        self.mem_aware_map.Len()
    }

    /// 判断是否为空。
    pub fn Empty(&self) -> bool {
        self.mem_aware_map.Empty()
    }

    /// 当前估计占用字节数。
    pub fn Bytes(&self) -> u64 {
        self.mem_aware_map.Bytes
    }
}

// StringToDecimalMapWithMemoryUsage is a string-decimal map with memory usage.
/// 字符串到 MyDecimal 指针的映射，并统计占用；可选绑定 Tracker。
pub struct StringToDecimalMapWithMemoryUsage {
    /// 底层带字节计数的哈希表；值为原始指针以对齐 Go 侧语义。
    pub mem_aware_map: hack::MemAwareMap<String, *mut types::MyDecimal>,
    /// 若存在，非零增量会立刻 Consume。
    tracker: Option<Arc<memory::Tracker>>,
}

impl Default for StringToDecimalMapWithMemoryUsage {
    fn default() -> Self {
        NewStringToDecimalMapWithMemoryUsage().0
    }
}

// NewStringToDecimalMapWithMemoryUsage builds a string map.
/// 构造空的字符串→Decimal 映射，并返回初始 Bytes 作为未追踪 delta。
pub fn NewStringToDecimalMapWithMemoryUsage() -> (StringToDecimalMapWithMemoryUsage, i64) {
    let mem_aware_map = new_mem_aware_map(0);
    let delta = mem_aware_map.Bytes as i64;
    (
        StringToDecimalMapWithMemoryUsage {
            mem_aware_map,
            tracker: None,
        },
        delta,
    )
}

impl StringToDecimalMapWithMemoryUsage {
    // Insert inserts `val` into `s` and returns the untracked delta.
    /// 插入键与 Decimal 指针；有 Tracker 时消费增量并返回 0。
    pub fn Insert(&mut self, key: String, val: *mut types::MyDecimal) -> i64 {
        let delta = self.mem_aware_map.Set(key, val);
        if delta != 0 {
            if let Some(tracker) = &self.tracker {
                tracker.Consume(delta);
                return 0;
            }
        }
        delta
    }

    /// 绑定或清除内存 Tracker。
    pub fn SetTracker(&mut self, tracker: Option<Arc<memory::Tracker>>) {
        self.tracker = tracker;
    }

    /// 判断 key 是否存在。
    pub fn Exist<Q: AsRef<str>>(&self, key: Q) -> bool {
        self.mem_aware_map.M.contains_key(key.as_ref())
    }

    /// 返回元素个数。
    pub fn Count(&self) -> usize {
        self.mem_aware_map.Count()
    }

    /// 返回长度。
    pub fn Len(&self) -> usize {
        self.mem_aware_map.Len()
    }

    /// 判断是否为空。
    pub fn Empty(&self) -> bool {
        self.mem_aware_map.Empty()
    }

    /// 当前估计占用字节数。
    pub fn Bytes(&self) -> u64 {
        self.mem_aware_map.Bytes
    }
}

// StringSetWithMemoryUsage is a string set with memory usage.
/// 字符串集合（value 为单元类型），并统计占用；可选绑定 Tracker。
pub struct StringSetWithMemoryUsage {
    /// 底层带字节计数的集合映射。
    pub mem_aware_map: hack::MemAwareMap<String, ()>,
    tracker: Option<Arc<memory::Tracker>>,
}

impl Default for StringSetWithMemoryUsage {
    fn default() -> Self {
        NewStringSetWithMemoryUsage(&[]).0
    }
}

// NewStringSetWithMemoryUsage builds a string set.
/// 由初始字符串切片构造集合，返回集合与当前 Bytes（作为初始 delta）。
pub fn NewStringSetWithMemoryUsage(ss: &[String]) -> (StringSetWithMemoryUsage, i64) {
    let mem_aware_map = new_mem_aware_map(ss.len());
    let mut set = StringSetWithMemoryUsage {
        mem_aware_map,
        tracker: None,
    };
    // 逐项 Insert，重复 key 由底层去重；最终 Bytes 作为构造期未追踪增量。
    for value in ss {
        set.Insert(value.clone());
    }
    let bytes = set.mem_aware_map.Bytes as i64;
    (set, bytes)
}

impl StringSetWithMemoryUsage {
    // Insert inserts `val` into `s` and returns the untracked delta.
    /// 插入字符串；有 Tracker 时消费增量并返回 0，否则返回未追踪 delta。
    pub fn Insert<S: Into<String>>(&mut self, val: S) -> i64 {
        let delta = self.mem_aware_map.Set(val.into(), ());
        if delta != 0 {
            if let Some(tracker) = &self.tracker {
                tracker.Consume(delta);
                return 0;
            }
        }
        delta
    }

    /// 绑定或清除内存 Tracker。
    pub fn SetTracker(&mut self, tracker: Option<Arc<memory::Tracker>>) {
        self.tracker = tracker;
    }

    /// 判断成员是否存在。
    pub fn Exist<Q: AsRef<str>>(&self, val: Q) -> bool {
        self.mem_aware_map.M.contains_key(val.as_ref())
    }

    /// 返回元素个数。
    pub fn Count(&self) -> usize {
        self.mem_aware_map.Count()
    }

    /// 判断是否为空。
    pub fn Empty(&self) -> bool {
        self.mem_aware_map.Empty()
    }

    /// 当前估计占用字节数。
    pub fn Bytes(&self) -> u64 {
        self.mem_aware_map.Bytes
    }
}

// Float64SetWithMemoryUsage is a float64 set with memory usage.
/// float64 集合：用 `FloatKey` 对齐 Go 对 ±0/NaN 的哈希语义，并统计占用。
pub struct Float64SetWithMemoryUsage {
    mem_aware_map: hack::MemAwareMap<FloatKey, ()>,
    /// 为每次插入的 NaN 分配递增 payload，使多个 NaN 可并存（对齐 Go）。
    next_nan_payload: u64,
}

impl Default for Float64SetWithMemoryUsage {
    fn default() -> Self {
        NewFloat64SetWithMemoryUsage(&[]).0
    }
}

// NewFloat64SetWithMemoryUsage builds a float64 set.
/// 由初始 f64 切片构造集合，返回集合与当前 Bytes。
pub fn NewFloat64SetWithMemoryUsage(ss: &[f64]) -> (Float64SetWithMemoryUsage, i64) {
    let mem_aware_map = new_mem_aware_map(ss.len());
    let mut set = Float64SetWithMemoryUsage {
        mem_aware_map,
        next_nan_payload: 1,
    };
    for &value in ss {
        set.Insert(value);
    }
    let bytes = set.mem_aware_map.Bytes as i64;
    (set, bytes)
}

impl Float64SetWithMemoryUsage {
    // Insert inserts `val` into `s` and returns the memory delta.
    /// 插入浮点值（含 NaN 特化），返回内存字节增量。
    pub fn Insert(&mut self, val: f64) -> i64 {
        let key = FloatKey::for_insert(val, &mut self.next_nan_payload);
        self.mem_aware_map.Set(key, ())
    }

    /// 查询是否存在；NaN 的查找键为 None，视为不存在（对齐 Go 查找语义）。
    pub fn Exist(&self, val: f64) -> bool {
        FloatKey::for_lookup(val).is_some_and(|key| self.mem_aware_map.Exist(&key))
    }

    /// 返回元素个数。
    pub fn Count(&self) -> usize {
        self.mem_aware_map.Count()
    }

    /// 判断是否为空。
    pub fn Empty(&self) -> bool {
        self.mem_aware_map.Empty()
    }

    /// 当前估计占用字节数。
    pub fn Bytes(&self) -> u64 {
        self.mem_aware_map.Bytes
    }
}

// Int64SetWithMemoryUsage is an int64 set with memory usage.
/// int64 集合，并统计占用字节。
pub struct Int64SetWithMemoryUsage {
    /// 底层带字节计数的集合映射。
    pub mem_aware_map: hack::MemAwareMap<i64, ()>,
}

impl Default for Int64SetWithMemoryUsage {
    fn default() -> Self {
        NewInt64SetWithMemoryUsage(&[]).0
    }
}

// NewInt64SetWithMemoryUsage builds an int64 set.
/// 由初始 i64 切片构造集合，返回集合与当前 Bytes。
pub fn NewInt64SetWithMemoryUsage(ss: &[i64]) -> (Int64SetWithMemoryUsage, i64) {
    let mem_aware_map = new_mem_aware_map(ss.len());
    let mut set = Int64SetWithMemoryUsage { mem_aware_map };
    for &value in ss {
        set.Insert(value);
    }
    let bytes = set.mem_aware_map.Bytes as i64;
    (set, bytes)
}

impl Int64SetWithMemoryUsage {
    // Insert inserts `val` into `s` and returns the memory delta.
    /// 插入 int64，返回内存字节增量。
    pub fn Insert(&mut self, val: i64) -> i64 {
        self.mem_aware_map.Set(val, ())
    }

    /// 判断成员是否存在。
    pub fn Exist(&self, val: i64) -> bool {
        self.mem_aware_map.Exist(&val)
    }

    /// 返回元素个数。
    pub fn Count(&self) -> usize {
        self.mem_aware_map.Count()
    }

    /// 判断是否为空。
    pub fn Empty(&self) -> bool {
        self.mem_aware_map.Empty()
    }

    /// 当前估计占用字节数。
    pub fn Bytes(&self) -> u64 {
        self.mem_aware_map.Bytes
    }
}
