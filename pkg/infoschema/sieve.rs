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

// SIEVE 缓存：InfoSchema V2 使用的线程安全淘汰算法。
//
// SIEVE 比 LRU 更简单：命中只置 visited 位，淘汰时从 hand 反向扫描，
// visited 条目获第二次机会。InfoSchema：库表等元数据的内存视图。

#![allow(non_camel_case_types, non_snake_case)]

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::mem::size_of;
use std::sync::{Arc, Mutex};

/// Metrics/status callbacks used by the Go SIEVE implementation.
/// SIEVE 指标/状态回调，对应 Go sieveStatusHook。
pub trait SieveStatusHook: Send + Sync {
    /// 缓存命中。
    fn on_hit(&self) {}
    /// 缓存未命中。
    fn on_miss(&self) {}
    /// 条目被淘汰。
    fn on_evict(&self) {}
    /// 占用字节数或条目数变化。
    fn on_update(&self, _size: u64, _count: u64) {}
    /// 容量上限变化。
    fn on_update_limit(&self, _limit: u64) {}
}

/// 默认空钩子，避免未绑定指标时空指针。
#[derive(Default)]
pub struct EmptySieveStatusHook;
impl SieveStatusHook for EmptySieveStatusHook {}

/// 单条缓存条目：值、访问位与字节大小。
#[derive(Clone, Debug)]
struct Entry<V> {
    value: V,
    visited: bool,
    size: u64,
}

/// 受 Mutex 保护的内部状态。
struct State<K, V> {
    count: u64,
    size: u64,
    capacity: u64,
    items: HashMap<K, Entry<V>>,
    /// 队头为最新插入，队尾为 hand 起始扫描位置。
    /// Front is newest insertion, back is the first hand position.
    order: VecDeque<K>,
    hand: Option<K>,
}

/// InfoSchema V2 使用的线程安全 SIEVE 缓存。
///
/// 与 Go 一致：更新不改变插入顺序；命中只置 visited；
/// 淘汰从后往前扫描，visited 条目获第二次机会。
/// Thread-safe implementation of the SIEVE cache used by InfoSchema v2.
///
/// As in Go, updates do not change insertion order. A hit only sets the visited
/// bit; eviction walks backwards and gives visited entries one second chance.
pub struct Sieve<K, V> {
    state: Mutex<State<K, V>>,
    hook: Mutex<Arc<dyn SieveStatusHook>>,
}

impl<K, V> Sieve<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    /// 按字节容量构造空缓存。
    pub fn new(capacity: u64) -> Self {
        Self {
            state: Mutex::new(State {
                count: 0,
                size: 0,
                capacity,
                items: HashMap::new(),
                order: VecDeque::new(),
                hand: None,
            }),
            hook: Mutex::new(Arc::new(EmptySieveStatusHook)),
        }
    }

    /// 替换状态钩子（命中/未命中/淘汰等回调）。
    pub fn SetStatusHook(&self, hook: Arc<dyn SieveStatusHook>) {
        *self.hook.lock().expect("sieve hook lock poisoned") = hook;
    }

    /// 更新容量上限并通知钩子；不立即驱逐。
    pub fn SetCapacity(&self, capacity: u64) {
        self.state.lock().expect("sieve lock poisoned").capacity = capacity;
        self.hook
            .lock()
            .expect("sieve hook lock poisoned")
            .on_update_limit(capacity);
    }

    /// 降低容量后循环驱逐，每轮最多 10 次，复刻 Go 批次节奏。
    pub fn SetCapacityAndWaitEvict(&self, capacity: u64) {
        self.SetCapacity(capacity);
        let hook = self.hook.lock().expect("sieve hook lock poisoned").clone();
        let mut state = self.state.lock().expect("sieve lock poisoned");
        while state.size > state.capacity && !state.items.is_empty() {
            for _ in 0..10 {
                if state.size <= state.capacity || state.items.is_empty() {
                    break;
                }
                Self::evict(&mut state, hook.as_ref());
            }
        }
    }

    /// 返回当前容量上限（字节）。
    pub fn Capacity(&self) -> u64 {
        self.state.lock().expect("sieve lock poisoned").capacity
    }

    /// 插入或更新：已有键只改值并置 visited，不重排链表。
    pub fn Set(&self, key: K, value: V) {
        let hook = self.hook.lock().expect("sieve hook lock poisoned").clone();
        let mut state = self.state.lock().expect("sieve lock poisoned");
        if let Some(entry) = state.items.get_mut(&key) {
            entry.value = value;
            entry.visited = true;
            return;
        }
        // Preserve Go's pre-insertion capacity check. This deliberately allows
        // one entry to push the cache above the byte limit until the next Set.
        // 插入前最多驱逐 10 次；仍可能暂时略超容量，与 Go 一致。
        for _ in 0..10 {
            if state.size <= state.capacity || state.items.is_empty() {
                break;
            }
            Self::evict(&mut state, hook.as_ref());
        }
        let entry_size = (size_of::<K>() + size_of::<V>() + size_of::<Entry<V>>()) as u64;
        state.size = state.size.saturating_add(entry_size);
        state.count += 1;
        state.order.push_front(key.clone());
        state.items.insert(
            key,
            Entry {
                value,
                visited: false,
                size: entry_size,
            },
        );
        hook.on_update(state.size, state.count);
    }

    /// 查找：命中置 visited 并回调 on_hit。
    pub fn Get(&self, key: &K) -> Option<V> {
        let hook = self.hook.lock().expect("sieve hook lock poisoned").clone();
        let mut state = self.state.lock().expect("sieve lock poisoned");
        if let Some(entry) = state.items.get_mut(key) {
            entry.visited = true;
            hook.on_hit();
            Some(entry.value.clone())
        } else {
            hook.on_miss();
            None
        }
    }

    /// 删除条目；若 hand 指向该键则先移到前驱。
    pub fn Remove(&self, key: &K) -> bool {
        let hook = self.hook.lock().expect("sieve hook lock poisoned").clone();
        let mut state = self.state.lock().expect("sieve lock poisoned");
        if !state.items.contains_key(key) {
            return false;
        }
        if state.hand.as_ref() == Some(key) {
            state.hand = Self::previous_key(&state.order, key);
        }
        Self::remove_entry(&mut state, key, hook.as_ref());
        true
    }

    /// 仅判断键是否存在，不改 visited、不触发指标。
    pub fn Contains(&self, key: &K) -> bool {
        self.state
            .lock()
            .expect("sieve lock poisoned")
            .items
            .contains_key(key)
    }

    /// 窥视值：不改 visited，不触发命中/未命中。
    pub fn Peek(&self, key: &K) -> Option<V> {
        self.state
            .lock()
            .expect("sieve lock poisoned")
            .items
            .get(key)
            .map(|entry| entry.value.clone())
    }

    /// 当前占用字节数。
    pub fn Size(&self) -> u64 {
        self.state.lock().expect("sieve lock poisoned").size
    }

    /// 当前条目个数。
    pub fn Len(&self) -> usize {
        self.state.lock().expect("sieve lock poisoned").order.len()
    }

    /// 清空全部条目，逐项走 removeEntry 以保持指标一致。
    pub fn Purge(&self) {
        let hook = self.hook.lock().expect("sieve hook lock poisoned").clone();
        let mut state = self.state.lock().expect("sieve lock poisoned");
        let keys: Vec<K> = state.order.iter().cloned().collect();
        for key in keys {
            Self::remove_entry(&mut state, &key, hook.as_ref());
        }
        state.order.clear();
        state.hand = None;
    }

    /// 清空缓存；与 Go 一致，不永久禁用后续写入。
    pub fn Close(&self) {
        self.Purge();
    }

    /// 给定键值类型下单条缓存条目的字节大小，供测试按条数设容量。
    /// Byte size of one cache entry for the given key/value types.
    /// Used by tests that mirror Go `entry[K,V].Size()`.
    pub fn entry_size() -> u64 {
        (size_of::<K>() + size_of::<V>() + size_of::<Entry<V>>()) as u64
    }

    /// 在插入顺序上取“更旧”的前驱键（对应 Go list.Element.Prev）。
    fn previous_key(order: &VecDeque<K>, key: &K) -> Option<K> {
        order
            .iter()
            .position(|candidate| candidate == key)
            .and_then(|position| order.get(position + 1))
            .cloned()
    }

    /// 从 map/链表删除并同步 size/count，触发 on_update。
    fn remove_entry(state: &mut State<K, V>, key: &K, hook: &dyn SieveStatusHook) {
        if let Some(entry) = state.items.remove(key) {
            state.order.retain(|candidate| candidate != key);
            state.size = state.size.saturating_sub(entry.size);
            state.count = state.count.saturating_sub(1);
            hook.on_update(state.size, state.count);
        }
    }

    /// SIEVE hand 扫描：visited 清位并继续，首个未访问条目被驱逐。
    fn evict(state: &mut State<K, V>, hook: &dyn SieveStatusHook) {
        let Some(mut current) = state.hand.clone().or_else(|| state.order.back().cloned()) else {
            return;
        };
        loop {
            let entry = state
                .items
                .get_mut(&current)
                .expect("sieve: evicting non-existent element");
            if !entry.visited {
                break;
            }
            // 第二次机会：清 visited，hand 移到前驱（或绕回队尾）。
            entry.visited = false;
            current = Self::previous_key(&state.order, &current)
                .or_else(|| state.order.back().cloned())
                .expect("sieve: empty list while evicting");
        }
        state.hand = Self::previous_key(&state.order, &current);
        Self::remove_entry(state, &current, hook);
        hook.on_evict();
    }
}

/// Go 风格构造函数别名。
pub fn newSieve<K, V>(capacity: u64) -> Sieve<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    Sieve::new(capacity)
}
