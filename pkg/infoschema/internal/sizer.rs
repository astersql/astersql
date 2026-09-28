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

// 内存占用估算（Sizeof / MemoryUsage）。
//
// 对应 Go infoschema 侧基于 reflect 的递归 sizeof：按类型累加字段与堆分配，
// 并通过 `SizeCache` 对指针地址去重，避免共享与环引用被重复计入。
// 负数结果表示估算失败（与 Go 约定一致）。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::Hash;
use std::mem::{size_of, size_of_val};
use std::rc::Rc;
use std::sync::Arc;

/// 遍历过程中已访问过的堆地址集合，用于指针去重与环检测。
#[derive(Default)]
pub struct SizeCache {
    visited: HashSet<usize>,
}

impl SizeCache {
    /// 首次访问该地址返回 true；空指针（0）始终视为首次。
    fn first_visit(&mut self, address: usize) -> bool {
        address == 0 || self.visited.insert(address)
    }
}

/// Rust has no runtime field reflection. This trait is the typed equivalent of
/// Go's reflect walk and lets domain structs account for every field while the
/// standard implementations below preserve pointer de-duplication and cycles.
///
/// 类型化的内存占用接口：等价于 Go 的 reflect 遍历，由领域结构体自行累加各字段。
pub trait MemoryUsage {
    /// 返回自身（含嵌套）估算字节数；失败时返回负数。
    fn memory_usage(&self, cache: &mut SizeCache) -> isize;
}

/// 用新的 SizeCache 估算 `value` 的内存占用。
pub fn sizeof<T: MemoryUsage + ?Sized>(value: &T) -> isize {
    value.memory_usage(&mut SizeCache::default())
}

/// Go 风格命名的 Sizeof 入口，行为与 `sizeof` 相同。
pub fn Sizeof<T: MemoryUsage + ?Sized>(value: &T) -> isize {
    sizeof(value)
}

/// 为标量类型生成仅返回 `size_of::<Self>()` 的 MemoryUsage 实现。
macro_rules! scalar_usage {
    ($($type:ty),* $(,)?) => {$ (
        impl MemoryUsage for $type {
            fn memory_usage(&self, _cache: &mut SizeCache) -> isize {
                size_of::<Self>() as isize
            }
        }
    )* };
}

scalar_usage!(
    (),
    bool,
    u8,
    u16,
    u32,
    u64,
    u128,
    usize,
    i8,
    i16,
    i32,
    i64,
    i128,
    isize,
    f32,
    f64,
    char
);

impl MemoryUsage for str {
    fn memory_usage(&self, cache: &mut SizeCache) -> isize {
        // Go string header is ptr+len (16 on 64-bit), matching Rust &str.
        // Go string 头为 ptr+len（64 位上 16 字节），与 Rust &str 一致。
        let header = size_of::<&str>() as isize;
        if self.is_empty() || cache.first_visit(self.as_ptr() as usize) {
            header + self.len() as isize
        } else {
            header
        }
    }
}

// &T delegates to the pointee. For &str this matches Go's string kind (header+bytes),
// which is what containers of string values need — not Go's Ptr kind.
// &T 委托给被引用对象；对 &str 按 Go string（头+字节）计，而非 Ptr 种类。
impl<T: MemoryUsage + ?Sized> MemoryUsage for &T {
    fn memory_usage(&self, cache: &mut SizeCache) -> isize {
        (**self).memory_usage(cache)
    }
}

impl MemoryUsage for String {
    fn memory_usage(&self, cache: &mut SizeCache) -> isize {
        let header = size_of::<String>() as isize;
        // Go string 的底层存储不可扩容，按实际长度而不是 Rust capacity 计入；
        // 同一缓冲地址只计一次。
        if self.capacity() == 0 || cache.first_visit(self.as_ptr() as usize) {
            header + self.len() as isize
        } else {
            header
        }
    }
}

impl<T: MemoryUsage> MemoryUsage for Vec<T> {
    fn memory_usage(&self, cache: &mut SizeCache) -> isize {
        // 已访问过的缓冲只返回 0，避免共享 Vec 重复累计。
        if self.capacity() > 0 && !cache.first_visit(self.as_ptr() as usize) {
            return 0;
        }
        let mut sum = size_of::<Vec<T>>() as isize;
        for value in self {
            let value_size = value.memory_usage(cache);
            if value_size < 0 {
                return -1;
            }
            sum += value_size;
        }
        // 未使用的 capacity 槽位按元素大小计入。
        sum + (self.capacity() - self.len()) as isize * size_of::<T>() as isize
    }
}

impl<T: MemoryUsage, const N: usize> MemoryUsage for [T; N] {
    fn memory_usage(&self, cache: &mut SizeCache) -> isize {
        let mut sum = 0;
        for value in self {
            let size = value.memory_usage(cache);
            if size < 0 {
                return -1;
            }
            sum += size;
        }
        // 补上数组类型自身相对元素布局的额外字节（若有）。
        let fields = N * size_of::<T>();
        sum + size_of::<Self>().saturating_sub(fields) as isize
    }
}

impl<T: MemoryUsage + ?Sized> MemoryUsage for Box<T> {
    fn memory_usage(&self, cache: &mut SizeCache) -> isize {
        let pointer_size = size_of::<Self>() as isize;
        let address = (&**self as *const T as *const ()) as usize;
        if !cache.first_visit(address) {
            return pointer_size;
        }
        let nested = (**self).memory_usage(cache);
        if nested < 0 {
            -1
        } else {
            pointer_size + nested
        }
    }
}

impl<T: MemoryUsage + ?Sized> MemoryUsage for Arc<T> {
    fn memory_usage(&self, cache: &mut SizeCache) -> isize {
        let pointer_size = size_of::<Self>() as isize;
        let address = Arc::as_ptr(self) as *const () as usize;
        if !cache.first_visit(address) {
            return pointer_size;
        }
        let nested = (**self).memory_usage(cache);
        if nested < 0 {
            -1
        } else {
            // Go reflect.Ptr accounts for the pointer word and pointee only;
            // Arc's runtime counters are an implementation detail here.
            pointer_size + nested
        }
    }
}

impl<T: MemoryUsage + ?Sized> MemoryUsage for Rc<T> {
    fn memory_usage(&self, cache: &mut SizeCache) -> isize {
        let pointer_size = size_of::<Self>() as isize;
        let address = Rc::as_ptr(self) as *const () as usize;
        if !cache.first_visit(address) {
            return pointer_size;
        }
        let nested = (**self).memory_usage(cache);
        if nested < 0 {
            -1
        } else {
            // Match Go reflect.Ptr rather than charging Rc's runtime counters.
            pointer_size + nested
        }
    }
}

impl<T: MemoryUsage> MemoryUsage for Option<T> {
    fn memory_usage(&self, cache: &mut SizeCache) -> isize {
        match self {
            None => size_of::<Self>() as isize,
            Some(value) => {
                let nested = value.memory_usage(cache);
                if nested < 0 {
                    -1
                } else {
                    // 补上 Option 判别位相对 T 的额外布局开销。
                    nested + size_of::<Self>().saturating_sub(size_of::<T>()) as isize
                }
            }
        }
    }
}

// Go's map value is a single pointer (8 bytes on 64-bit). Rust HashMap is larger;
// keep Go's header size so Sizeof matches the Go infoschema accounting.
// Go map 值为单指针（64 位 8 字节）；Rust HashMap 更大，仍用 Go 头大小以对齐记账。
const GO_MAP_HEADER_SIZE: isize = 8;

impl<K, V, S> MemoryUsage for HashMap<K, V, S>
where
    K: MemoryUsage + Eq + Hash,
    V: MemoryUsage,
{
    fn memory_usage(&self, cache: &mut SizeCache) -> isize {
        let address = self as *const Self as usize;
        if !cache.first_visit(address) {
            return 0;
        }
        let mut sum = GO_MAP_HEADER_SIZE;
        for (key, value) in self {
            let key_size = key.memory_usage(cache);
            let value_size = value.memory_usage(cache);
            if key_size < 0 || value_size < 0 {
                return -1;
            }
            sum += key_size + value_size;
        }
        // Include overhead due to unused map buckets. 10.79 comes
        // from https://golang.org/src/runtime/map.go.
        // 计入未使用 bucket 开销；10.79 来自 Go runtime/map.go。
        sum + (self.len() as f64 * 10.79) as isize
    }
}

impl<K: MemoryUsage + Ord, V: MemoryUsage> MemoryUsage for BTreeMap<K, V> {
    fn memory_usage(&self, cache: &mut SizeCache) -> isize {
        let mut sum = size_of_val(self) as isize;
        for (key, value) in self {
            let key_size = key.memory_usage(cache);
            let value_size = value.memory_usage(cache);
            if key_size < 0 || value_size < 0 {
                return -1;
            }
            sum += key_size + value_size;
        }
        sum
    }
}

impl<A: MemoryUsage, B: MemoryUsage> MemoryUsage for (A, B) {
    fn memory_usage(&self, cache: &mut SizeCache) -> isize {
        let first = self.0.memory_usage(cache);
        let second = self.1.memory_usage(cache);
        if first < 0 || second < 0 {
            return -1;
        }
        first + second + size_of::<Self>().saturating_sub(size_of::<A>() + size_of::<B>()) as isize
    }
}

impl<T> MemoryUsage for *const T {
    fn memory_usage(&self, _cache: &mut SizeCache) -> isize {
        size_of::<Self>() as isize
    }
}

impl<T> MemoryUsage for *mut T {
    fn memory_usage(&self, _cache: &mut SizeCache) -> isize {
        size_of::<Self>() as isize
    }
}
