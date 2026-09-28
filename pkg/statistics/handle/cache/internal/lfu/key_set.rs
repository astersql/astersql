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

// LFU 统计缓存的二级键集合（对应 Go `keySet`）。
//
// 在主缓存（TinyLFU/Moka）之外保留表 ID → `Table` 的映射，
// 用读写锁保护并发访问；淘汰后仍可通过本集合读到“壳表”（已丢弃重统计数据）。

use statistics::Table;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

// keySet 对应 Go 的 keySet：保存表对象，并用 RWMutex 保护 map 的并发访问。
/// 受 `RwLock` 保护的表统计集合；键为物理表 ID。
#[derive(Default)]
pub(crate) struct keySet {
    set: RwLock<HashMap<i64, Arc<Table>>>,
}

impl keySet {
    // Remove 对应 Go 的 Remove；返回被删除表仍占用的跟踪内存。
    /// 删除指定键，并返回该表的跟踪内存用量（用于外层扣减 cost）。
    pub fn Remove(&self, key: i64) -> i64 {
        let mut set = self.set.write().unwrap();
        let mut cost = 0;
        if let Some(table) = set.remove(&key) {
            cost = table.MemoryUsage().TotalTrackingMemUsage();
        }
        cost
    }

    // Keys 对应 Go 的 Keys；复制 map 的 key 后立即释放读锁，避免把锁带出方法。
    /// 在读锁内复制全部键后立即释放锁，避免把锁带出方法。
    pub fn Keys(&self) -> Vec<i64> {
        let set = self.set.read().unwrap();
        set.keys().copied().collect()
    }

    // Len 对应 Go 的 Len，读取集合当前大小。
    /// 返回当前集合中的条目数。
    pub fn Len(&self) -> usize {
        self.set.read().unwrap().len()
    }

    // AddKeyValue 对应 Go 的 AddKeyValue；写锁覆盖整个插入过程。
    /// 插入或覆盖表统计；写锁覆盖整个插入过程。
    pub fn AddKeyValue(&self, key: i64, value: Arc<Table>) {
        self.set.write().unwrap().insert(key, value);
    }

    // Get 对应 Go 的 Get，返回表指针和是否命中；指针生命周期由外部缓存管理。
    /// 按表 ID 查找；命中时克隆 `Arc`，生命周期由调用方与缓存共同管理。
    pub fn Get(&self, key: i64) -> Option<Arc<Table>> {
        self.set.read().unwrap().get(&key).cloned()
    }

    // Clear 对应 Go 的 Clear；用全新的空 map 替换旧 map，保持原子地切换集合内容。
    /// 用空 map 整体替换旧内容，实现原子清空。
    pub fn Clear(&self) {
        *self.set.write().unwrap() = HashMap::new();
    }
}
