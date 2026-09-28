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

// use std::collections::HashMap;
// use std::sync::RwLock;
//
// ShardCount controls the shard maps within the concurrent map
// ShardCount 对应 Go 常量：把 map 切成固定数量的 shard 以降低写锁竞争。
// pub const ShardCount: usize = 320;
//
// A "thread" safe map of type string:Anything.
// To avoid lock bottlenecks this map is dived to several (ShardCount) map shards.
// concurrentMap 在 Go 中是 []*concurrentMapShared；这里用 Vec 保留 shard 数组语义。
// pub type concurrentMap = Vec<concurrentMapShared>;
//
// A "thread" safe string to anything map.
// Go 的 syncutil.RWMutex 保护内部 MemAwareMap；用 RwLock 包住 map 表达同一并发边界。
// pub struct concurrentMapShared {
//     pub items: RwLock<hack::MemAwareMap<u64, *mut entry>>,
// }
//
// newConcurrentMap creates a new concurrent map.
// newConcurrentMap 对应 Go 构造函数：初始化所有 shard，并在测试模式下固定 hash seed。
// pub fn newConcurrentMap() -> concurrentMap {
//     let mut m = Vec::with_capacity(ShardCount);
//     for _ in 0..ShardCount {
//         let mut items = hack::MemAwareMap::<u64, *mut entry>::new(HashMap::new());
//         if intest::InTest {
// Go 测试中 MockSeedForTest 保证迭代/哈希行为可重复；这里仅保留调用点。
//             items.MockSeedForTest();
//         }
//         m.push(concurrentMapShared {
//             items: RwLock::new(items),
//         });
//     }
//     m
// }
//
// getShard returns shard under given key
// getShard 按 hash key 对 shard 数取模，保持 Go 的 uint64(ShardCount) 取模规则。
// pub fn getShard(m: &concurrentMap, hashKey: u64) -> &concurrentMapShared {
//     &m[(hashKey % ShardCount as u64) as usize]
// }
//
// Insert inserts a value in a shard safely
// Insert 对应 Go 的加写锁插入；value.Next 链接旧 entry，以保留 hash join 冲突链。
// pub fn Insert(m: &concurrentMap, key: u64, value: *mut entry) -> i64 {
//     let shard = getShard(m, key);
//     let mut items = shard.items.write().expect("rwlock poisoned");
//     let oldValue = items.M.get(&key).copied().unwrap_or(std::ptr::null_mut());
//     unsafe {
// Go 直接写 value.Next；用裸指针标出该外部 entry 所有权尚未迁移。
//         (*value).Next = oldValue;
//     }
//     items.Set(key, value)
// }
//
// Get retrieves an element from map under given key.
// Note that in hash joins, reading proceeds after all writes, so we ignore RLock() here.
// Otherwise, we should use RLock() for concurrent reads and writes.
// Get 保留 Go “写完再读，因此省略 RLock” 的假设；仍借 RwLock 读取以表达共享访问。
// pub fn Get(m: &concurrentMap, key: u64) -> (*mut entry, bool) {
//     let shard = getShard(m, key);
//     let items = shard.items.read().expect("rwlock poisoned");
//     match items.M.get(&key).copied() {
//         Some(val) => (val, true),
//         None => (std::ptr::null_mut(), false),
//     }
// }
//
// IterCb :Iterator callback,called for every key,value found in
// maps. RLock is held for all calls for a given shard
// therefore callback sess consistent view of a shard,
// but not across the shards
// IterCb 对应 Go 回调类型；调用期间只保证单个 shard 内视图一致。
// pub type IterCb = fn(key: u64, e: *mut entry);
//
// IterCb iterates the map using a callback, cheapest way to read
// all elements in a map.
// IterCb 逐 shard 持有读锁并调用回调，保留 Go 中跨 shard 不提供全局快照的语义。
// pub fn IterCb_fn(m: &concurrentMap, f: IterCb) {
//     for shard in m {
//         let items = shard.items.read().expect("rwlock poisoned");
//         for (key, value) in items.M.iter() {
//             f(*key, *value);
//         }
//     }
// }
// */
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
