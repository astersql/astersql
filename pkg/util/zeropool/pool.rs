// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Package zeropool provides a zero-allocation type-safe alternative for sync.Pool, used to workaround staticheck SA6002.
// The contents of this package are brought from https://github.com/colega/zeropool because "little copying is better than little dependency".
// 类型安全且尽量避免额外指针分配的对象池（zeropool）。
//
// 对应 Go `pkg/util/zeropool`，源自 colega/zeropool：用次级 `pointers` 池回收
// `Box<T>` 外壳，使 Get/Put 热路径通常不再向堆申请新指针，绕开 staticcheck SA6002
//（对 `sync.Pool` 存切片指针的告警）。`Pool` 零值可用，空池时返回 `T::default()`。

#![allow(non_snake_case)]

use std::sync::{Arc, Mutex, MutexGuard};

// Pool is a type-safe pool of items that does not allocate pointers to items.
// That is not entirely true, it does allocate sometimes, but not most of the time,
// just like the usual sync.Pool pools items most of the time, except when they're evicted.
// It does that by storing the allocated pointers in a secondary pool instead of letting them go,
// so they can be used later to store the items again.
//
// Zero value of Pool[T] is valid, and it will return zero values of T if nothing is pooled.
/// 双池结构：`items` 存可取用对象，`pointers` 存空 `Box` 外壳以复用指针。
pub struct Pool<T> {
    // items holds pointers to the pooled items, which are valid to be used.
    /// 可直接取出使用的对象槽。
    items: Mutex<Vec<Box<T>>>,
    // pointers holds just pointers to the pooled item types.
    // The values referenced by pointers are not valid to be used (as they're used by some other caller)
    // and it is safe to overwrite these pointers.
    /// 已掏空内容、仅保留分配外壳的指针槽。
    pointers: Mutex<Vec<Box<T>>>,
    /// 可选工厂；`None` 表示零值 Pool，空时返回 `T::default()`。
    new: Option<Arc<dyn Fn() -> T + Send + Sync + 'static>>,
}

impl<T> Default for Pool<T> {
    fn default() -> Self {
        Self {
            items: Mutex::new(Vec::new()),
            pointers: Mutex::new(Vec::new()),
            new: None,
        }
    }
}

// New creates a new Pool[T] with the given function to create new items.
// A Pool must not be copied after first use.
/// 使用工厂函数创建 Pool；首次使用后不应再被按值拷贝。
pub fn New<T, F>(item: F) -> Pool<T>
where
    F: Fn() -> T + Send + Sync + 'static,
{
    Pool {
        items: Mutex::new(Vec::new()),
        pointers: Mutex::new(Vec::new()),
        new: Some(Arc::new(item)),
    }
}

impl<T: Default> Pool<T> {
    // Get returns an item from the pool, creating a new one if necessary.
    // Get may be called concurrently from multiple goroutines.
    // Get 对应 Go 的取值逻辑；零值 Pool 且 items 为空时返回 T 的零值。
    /// 从池中取出一个 `T`；池空时用工厂或 `Default` 构造。
    pub fn Get(&self) -> T {
        let mut ptr = match lock(&self.items).pop() {
            Some(ptr) => ptr,
            None => match &self.new {
                Some(new) => Box::new(new()),
                // The only way this can happen is when someone is using the zero-value of zeropool.Pool, and items pool is empty.
                // We don't have a pointer to store in p.pointers, so just return the empty value.
                // 零值 Pool 且无库存：无 Box 外壳可回收，直接返回默认值。
                None => return T::default(),
            },
        };

        let item = std::mem::take(&mut *ptr);
        // We don't want to retain the value in p.pointers.
        // If T holds a reference to something, we want that to be garbage-collected
        // if for some reason caller does less Put() calls than Get() calls.
        // 把掏空的 Box 放回 pointers，供后续 Put 复用指针外壳。
        lock(&self.pointers).push(ptr);
        item
    }

    // Put adds an item to the pool.
    // Put 对应 Go 的归还逻辑：优先复用 pointers 中的 *T，没有则 new(T)。
    /// 将对象归还池中；优先复用 `pointers` 里的 `Box` 外壳。
    pub fn Put(&self, item: T) {
        let mut ptr = match lock(&self.pointers).pop() {
            Some(ptr) => ptr,
            None => Box::new(T::default()),
        };
        *ptr = item;
        lock(&self.items).push(ptr);
    }
}

/// 获取互斥锁；中毒时取出内部数据，以贴近 Go sync.Pool 在 panic 后仍可用。
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Go's sync.Pool remains usable after a goroutine panics. Recover poisoned
    // Rust mutexes for the same operational behavior.
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
