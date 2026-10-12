// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// Hash Join 构建侧并发哈希表（分片 map）。
//
// 将键空间切成固定数量的 shard（分片），每个 shard 独立持有读写锁，
// 以降低并发插入时的锁竞争。同一 hash key 下的多行通过 `Entry` 链表链接，
// 用于处理哈希冲突（collision chain）。对应 Go 的 `concurrentMap`。

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// 分片数量：把 map 切成固定数量的 shard 以降低写锁竞争。
pub const SHARD_COUNT: usize = 320;

/// 哈希冲突链表节点：同一 key 下多行按插入顺序链接。
#[derive(Debug)]
pub struct Entry<V> {
    /// 存放的值（如构建侧行指针）。
    pub value: V,
    /// 指向同 key 的下一条冲突链节点。
    pub next: Option<Arc<Entry<V>>>,
}

/// 线程安全的分片哈希表：`u64` 键映射到冲突链头节点。
pub struct ConcurrentMap<V> {
    /// 各分片及其读写锁保护的内部 HashMap。
    shards: Vec<RwLock<HashMap<u64, Arc<Entry<V>>>>>,
}

impl<V> Default for ConcurrentMap<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V> ConcurrentMap<V> {
    #[inline]
    const fn map_slot_bytes() -> usize {
        std::mem::size_of::<u64>() + std::mem::size_of::<Arc<Entry<V>>>()
    }

    /// 创建空的并发分片 map，初始化全部 shard。
    pub fn new() -> Self {
        Self {
            shards: (0..SHARD_COUNT)
                .map(|_| RwLock::new(HashMap::new()))
                .collect(),
        }
    }

    /// 按 hash key 对分片数取模，定位所属 shard。
    fn shard(&self, key: u64) -> &RwLock<HashMap<u64, Arc<Entry<V>>>> {
        &self.shards[(key % SHARD_COUNT as u64) as usize]
    }

    /// 安全插入：新节点 `next` 指向旧头，形成冲突链；返回因扩容估计的内存增量字节数。
    pub fn insert(&self, key: u64, value: V) -> i64 {
        let mut shard = self.shard(key).write().expect("concurrent map poisoned");
        // 将新 entry 链接到原有冲突链头部，保留同 key 的多行。
        let old = shard.get(&key).cloned();
        let entry = Arc::new(Entry { value, next: old });
        let before = shard.capacity();
        shard.insert(key, entry);
        // 仅在 capacity 增长时估算键与 Arc 头指针占用的增量。
        ((shard.capacity() - before) * Self::map_slot_bytes()) as i64
    }

    /// 按 key 取冲突链头；Hash Join 通常在全部写完成后读，因此省略读锁竞争优化假设与 Go 一致。
    pub fn get(&self, key: u64) -> Option<Arc<Entry<V>>> {
        self.shard(key)
            .read()
            .expect("concurrent map poisoned")
            .get(&key)
            .cloned()
    }

    /// 逐 shard 持读锁回调遍历；仅保证单 shard 内视图一致，不提供跨 shard 全局快照。
    pub fn for_each(&self, mut visitor: impl FnMut(u64, &Arc<Entry<V>>)) {
        for shard in &self.shards {
            for (key, value) in shard.read().expect("concurrent map poisoned").iter() {
                visitor(*key, value);
            }
        }
    }

    /// 返回所有 shard 当前为键和链头指针保留的容量字节数。
    ///
    /// 与 [`Self::insert`] 返回的增量使用同一口径，可用于校验调用方累计的
    /// map 内存增量；冲突链节点本身由调用方按 entry 大小另行统计。
    pub fn real_memory_bytes(&self) -> i64 {
        self.shards
            .iter()
            .map(|shard| {
                let capacity = shard.read().expect("concurrent map poisoned").capacity();
                (capacity * Self::map_slot_bytes()) as i64
            })
            .sum()
    }
}
