// Copyright 2017 PingCAP, Inc.
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

// 简单 LRU（Least Recently Used）缓存实现。
//
// 对应 Go `pkg/util/kvcache/simple_lru.go`。用双向链表维护近因顺序，HashMap
// 按键哈希定位节点；可选配额（quota）与守卫比例（guard）在实例内存过高时淘汰条目。
// 本实现非线程安全，调用方需自行串行化访问。

use crate::memory;
use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// Key is the interface that every key in LRU Cache should implement.
/// 缓存键接口：须可跨线程共享，并提供用于索引的字节哈希。
pub trait Key: Send + Sync {
    /// 返回键的哈希字节；作为 `elements` 映射的查找键。
    fn Hash(&self) -> Vec<u8>;
}

/// 键的引用计数句柄。
pub type KeyRef = Arc<dyn Key>;
/// 值的类型擦除句柄（任意 `Send + Sync` 对象）。
pub type Value = Arc<dyn Any + Send + Sync>;

/// 链表节点：持有键值及前后索引（索引进 `entries` 槽位）。
struct CacheEntry {
    key: KeyRef,
    value: Value,
    previous: Option<usize>,
    next: Option<usize>,
}

/// GlobalLRUMemUsageTracker tracks all the memory usage of SimpleLRUCache.
/// 全局 LRU 内存用量 Tracker；`init` 首次调用时惰性初始化。
pub static GlobalLRUMemUsageTracker: OnceLock<Box<memory::Tracker>> = OnceLock::new();

/// ProfileName is the function name in heap profile.
/// 堆分析（heap profile）中对应 Put 路径的函数名字符串，对齐 Go 侧命名。
pub const ProfileName: &str = "github.com/pingcap/tidb/pkg/util/kvcache.(*SimpleLRUCache).Put";

/// Initializes the global tracker. Repeated calls retain the first tracker.
/// 初始化全局 Tracker；重复调用保留首次创建的实例。
pub fn init() {
    GlobalLRUMemUsageTracker
        .get_or_init(|| memory::NewTracker(memory::LabelForGlobalSimpleLRUCache, -1));
}

/// SimpleLRUCache is a simple least recently used cache. It is not thread-safe.
/// 简单 LRU 缓存：`front` 为最近使用端，`back` 为最久未使用端。
pub struct SimpleLRUCache {
    /// 哈希字节 → `entries` 下标。
    elements: HashMap<Vec<u8>, usize>,
    /// 淘汰时可选回调（键、值）。
    onEvict: Option<Box<dyn FnMut(KeyRef, Value) + Send>>,
    /// 节点槽位；空闲下标回收到 `free`。
    entries: Vec<Option<CacheEntry>>,
    free: Vec<usize>,
    front: Option<usize>,
    back: Option<usize>,
    pub capacity: usize,
    pub size: usize,
    /// 内存配额上限（字节）；0 表示仅按 capacity 淘汰。
    quota: u64,
    /// 守卫比例：有效阈值为 `quota * (1 - guard)`，用于预留安全余量。
    guard: f64,
}

/// NewSimpleLRUCache creates a cache whose capacity must be positive.
/// 创建容量至少为 1 的 LRU 缓存；`guard`/`quota` 控制内存守卫行为。
pub fn NewSimpleLRUCache(capacity: usize, guard: f64, quota: u64) -> Box<SimpleLRUCache> {
    assert!(capacity >= 1, "capacity of LRU Cache should be at least 1.");
    init();
    Box::new(SimpleLRUCache {
        elements: HashMap::new(),
        onEvict: None,
        entries: Vec::new(),
        free: Vec::new(),
        front: None,
        back: None,
        capacity,
        size: 0,
        quota,
        guard,
    })
}

impl SimpleLRUCache {
    /// 设置淘汰回调；驱逐条目时若启用则调用。
    pub fn SetOnEvict(&mut self, onEvict: Box<dyn FnMut(KeyRef, Value) + Send>) {
        self.onEvict = Some(onEvict);
    }

    /// 按键查找；命中则移到 front（标记为最近使用）并返回值。
    pub fn Get(&mut self, key: &dyn Key) -> (Option<Value>, bool) {
        let Some(&index) = self.elements.get(&key.Hash()) else {
            return (None, false);
        };
        self.move_to_front(index);
        let value = Arc::clone(&self.entry(index).value);
        (Some(value), true)
    }

    /// 插入或更新；同键更新值并刷新近因性。超出容量或内存阈值时从 back 淘汰。
    pub fn Put(&mut self, key: KeyRef, value: Value) {
        let hash = key.Hash();
        // 已存在：原地更新并移到 front。
        if let Some(&index) = self.elements.get(&hash) {
            self.entry_mut(index).value = value;
            self.move_to_front(index);
            return;
        }

        let index = self.insert_front(CacheEntry {
            key,
            value,
            previous: None,
            next: None,
        });
        self.elements.insert(hash, index);
        self.size += 1;

        // quota==0：仅按条目数 capacity 淘汰。
        if self.quota == 0 {
            if self.size > self.capacity {
                self.evict_oldest(true);
            }
            return;
        }

        // 有配额：结合实例内存与 capacity 双重约束。
        let mut memUsed = match memory::InstanceMemUsed() {
            Ok(memUsed) => memUsed,
            Err(_) => {
                self.DeleteAll();
                return;
            }
        };
        let threshold = go_float64_to_uint64(self.quota as f64 * (1.0 - self.guard));
        while memUsed > threshold || self.size > self.capacity {
            if self.back.is_none() {
                break;
            }
            self.evict_oldest(true);
            if memUsed > threshold {
                memUsed = match memory::InstanceMemUsed() {
                    Ok(memUsed) => memUsed,
                    Err(_) => {
                        self.DeleteAll();
                        return;
                    }
                };
            }
        }
    }

    /// 按键删除；不触发 onEvict。
    pub fn Delete(&mut self, key: &dyn Key) {
        if let Some(index) = self.elements.remove(&key.Hash()) {
            self.remove(index);
            self.size -= 1;
        }
    }

    /// 清空全部条目；不触发 onEvict。
    pub fn DeleteAll(&mut self) {
        while let Some(index) = self.back {
            let entry = self.remove(index);
            self.elements.remove(&entry.key.Hash());
            self.size -= 1;
        }
    }

    /// 当前条目数。
    pub fn Size(&self) -> isize {
        self.size as isize
    }

    /// Values returns values from most recently used to least recently used.
    /// 按最近使用到最久未使用顺序返回值列表。
    pub fn Values(&self) -> Vec<Value> {
        let mut values = Vec::with_capacity(self.size as usize);
        let mut current = self.front;
        while let Some(index) = current {
            let entry = self.entry(index);
            values.push(Arc::clone(&entry.value));
            current = entry.next;
        }
        values
    }

    /// Keys returns keys from most recently used to least recently used.
    /// 按最近使用到最久未使用顺序返回键列表。
    pub fn Keys(&self) -> Vec<KeyRef> {
        let mut keys = Vec::with_capacity(self.size as usize);
        let mut current = self.front;
        while let Some(index) = current {
            let entry = self.entry(index);
            keys.push(Arc::clone(&entry.key));
            current = entry.next;
        }
        keys
    }

    /// 调整容量；若新容量更小则从最旧端淘汰直至满足，且不调用 onEvict。
    pub fn SetCapacity(&mut self, capacity: usize) -> Result<(), String> {
        if capacity < 1 {
            return Err("capacity of lru cache should be at least 1".to_owned());
        }
        self.capacity = capacity;
        while self.size > self.capacity {
            self.evict_oldest(false);
        }
        Ok(())
    }

    /// 移除并返回最久未使用的键值；空缓存返回 false。不触发 onEvict。
    pub fn RemoveOldest(&mut self) -> (Option<KeyRef>, Option<Value>, bool) {
        let Some(index) = self.back else {
            return (None, None, false);
        };
        let entry = self.remove(index);
        self.elements.remove(&entry.key.Hash());
        self.size -= 1;
        (Some(entry.key), Some(entry.value), true)
    }

    /// 取只读节点引用。
    fn entry(&self, index: usize) -> &CacheEntry {
        self.entries[index].as_ref().expect("live LRU entry")
    }

    /// 取可变节点引用。
    fn entry_mut(&mut self, index: usize) -> &mut CacheEntry {
        self.entries[index].as_mut().expect("live LRU entry")
    }

    /// 在 front 插入新节点；优先复用 `free` 槽位。
    fn insert_front(&mut self, mut entry: CacheEntry) -> usize {
        entry.next = self.front;
        let index = if let Some(index) = self.free.pop() {
            self.entries[index] = Some(entry);
            index
        } else {
            self.entries.push(Some(entry));
            self.entries.len() - 1
        };
        if let Some(front) = self.front {
            self.entry_mut(front).previous = Some(index);
        } else {
            self.back = Some(index);
        }
        self.front = Some(index);
        index
    }

    /// 将已有节点移到 front（最近使用端）。
    fn move_to_front(&mut self, index: usize) {
        if self.front == Some(index) {
            return;
        }
        let (previous, next) = {
            let entry = self.entry(index);
            (entry.previous, entry.next)
        };
        // 先从原位置断开。
        if let Some(previous) = previous {
            self.entry_mut(previous).next = next;
        }
        if let Some(next) = next {
            self.entry_mut(next).previous = previous;
        } else {
            self.back = previous;
        }
        // 再接到 front。
        let old_front = self.front;
        {
            let entry = self.entry_mut(index);
            entry.previous = None;
            entry.next = old_front;
        }
        if let Some(front) = old_front {
            self.entry_mut(front).previous = Some(index);
        }
        self.front = Some(index);
    }

    /// 从链表摘除节点并回收槽位到 `free`；返回节点内容。
    fn remove(&mut self, index: usize) -> CacheEntry {
        let (previous, next) = {
            let entry = self.entry(index);
            (entry.previous, entry.next)
        };
        if let Some(previous) = previous {
            self.entry_mut(previous).next = next;
        } else {
            self.front = next;
        }
        if let Some(next) = next {
            self.entry_mut(next).previous = previous;
        } else {
            self.back = previous;
        }
        let entry = self.entries[index].take().expect("live LRU entry");
        self.free.push(index);
        entry
    }

    /// 淘汰最旧条目；`callOnEvict` 为真时调用 onEvict 回调。
    fn evict_oldest(&mut self, callOnEvict: bool) {
        let Some(index) = self.back else {
            return;
        };
        let entry = self.remove(index);
        self.elements.remove(&entry.key.Hash());
        self.size -= 1;
        if callOnEvict {
            if let Some(onEvict) = self.onEvict.as_mut() {
                onEvict(entry.key, entry.value);
            }
        }
    }
}

/// 对齐 Go/amd64 的运行时 float64→uint64 转换，尤其保留负值的补码结果。
fn go_float64_to_uint64(value: f64) -> u64 {
    if value.is_sign_negative() {
        (value as i64) as u64
    } else {
        value as u64
    }
}
