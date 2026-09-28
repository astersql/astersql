// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 泛型并发映射 `SyncMap`：对 `HashMap` 加读写锁，对齐 Go `sync.RWMutex` + `map`。
//
// 由 `pkg/util/generic/sync_map.go` 迁移；提供 Store/Load/Delete/Keys，供内核侧需要
// 线程安全键值表的场景使用（如元数据缓存、会话侧登记表等）。

// 本文件由 pkg/util/generic/sync_map.go 迁移而来，保留 Go 实现结构与行为。
//
// Rust 实现使用 RwLock<HashMap<_, _>> 表达 sync.RWMutex + map 的组合。

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::RwLock;

/// 泛型并发映射：内部用 `RwLock<HashMap<K, V>>` 保护键值。
// SyncMap is the generic version of the sync.Map.
// SyncMap 对应 Go 的泛型结构体，item 保存键值，mu 的 Lock/RLock 由 RwLock guard 生命周期表达。
pub struct SyncMap<K, V>
where
    K: Eq + Hash,
{
    /// 受读写锁保护的底层哈希表。
    item: RwLock<HashMap<K, V>>,
}

/// 创建带初始容量的 `SyncMap`；`capacity < 0` 时 panic，对齐 Go。
// NewSyncMap returns a new SyncMap.
// NewSyncMap 保留 Go 的 capacity 参数，用 HashMap::with_capacity 预分配底层桶。
pub fn NewSyncMap<K, V>(capacity: isize) -> SyncMap<K, V>
where
    K: Eq + Hash,
{
    if capacity < 0 {
        panic!("capacity cannot be negative");
    }

    SyncMap {
        item: RwLock::new(HashMap::with_capacity(capacity as usize)),
    }
}

impl<K, V> SyncMap<K, V>
where
    K: Eq + Hash,
{
    /// 写入（或覆盖）键值对；持写锁。
    // Store stores a value.
    pub fn Store(&self, key: K, value: V) {
        // Go 在写入前 Lock、写入后 Unlock；Rust 写锁 guard 离开作用域时自动释放。
        let mut item = self.item.write().expect("SyncMap write lock poisoned");
        item.insert(key, value);
    }

    /// 按键读取；返回 `(Some(值), true)` 或 `(None, false)`，值需可克隆。
    // Load loads a key value.
    pub fn Load(&self, key: &K) -> (Option<V>, bool)
    where
        V: Clone,
    {
        // Go 返回 (零值, false)；Rust 实现用 Option<V> 明确表达不存在时没有可复制零值。
        let item = self.item.read().expect("SyncMap read lock poisoned");
        match item.get(key) {
            Some(val) => (Some(val.clone()), true),
            None => (None, false),
        }
    }

    /// 删除键并返回旧值是否存在；持写锁。
    // Delete deletes the value for a key, returning the previous value if any.
    // The exist result reports whether the key was present.
    pub fn Delete(&self, key: &K) -> (Option<V>, bool) {
        // 删除需要写锁；remove 同时完成 Go 中先取旧值、再 delete 的效果。
        let mut item = self.item.write().expect("SyncMap write lock poisoned");
        match item.remove(key) {
            Some(val) => (Some(val), true),
            None => (None, false),
        }
    }

    /// 在读锁下收集全部键的快照（无序）；键需可克隆。
    // Keys returns all the keys in the map.
    pub fn Keys(&self) -> Vec<K>
    where
        K: Clone,
    {
        // Go 先按 len(item) 预分配 slice，再在读锁内遍历 map；这里同样在读锁内收集键。
        let item = self.item.read().expect("SyncMap read lock poisoned");
        let mut ret = Vec::with_capacity(item.len());
        for k in item.keys() {
            ret.push(k.clone());
        }
        ret
    }
}
