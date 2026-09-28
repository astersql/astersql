// Copyright 2020 PingCAP, Inc.
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

// Apply 算子的内存 LRU 缓存。
//
// 以编码后的外层行字节为键，缓存内层计划产出的 `chunk::List`。
// 底层 `SimpleLRUCache` 非线程安全，所有访问经互斥锁串行化；
// 内存配额由会话变量 `MemQuotaApplyCache` 控制，超限时淘汰最久未用条目。

#![allow(dead_code, non_snake_case)]

use std::convert::Infallible;
use std::sync::Arc;

use crate::{chunk, kvcache, memory, syncutil};

/// ApplyCache 构造所需的窄会话接口。
///
/// Narrow session boundary needed by ApplyCache's constructor.
///
/// The migrated `sessionctx::Context` is an API trait whose session-variable
/// type is intentionally associated. Implementations adapt that type here and
/// expose the same `MemQuotaApplyCache` value read by Go.
pub trait ApplyCacheContext {
    fn MemQuotaApplyCache(&self) -> i64;
}

/// 按外层行编码值存储内层行列表。
///
/// ApplyCache stores inner-row lists by the encoded outer-row value.
/// `SimpleLRUCache` is not internally synchronized, so every operation is
/// serialized by this mutex, just like the lock in the Go struct.
pub struct ApplyCache {
    cache: syncutil::Mutex<Box<kvcache::SimpleLRUCache>>,
    mem_tracker: Box<memory::Tracker>,
    mem_capacity: i64,
}

/// 缓存键：Go 侧为命名 `[]byte` 类型，Rust 用 newtype 保留类型身份。
///
/// Go defines this as a named `[]byte`, so Rust uses a newtype rather than a
/// plain alias and can implement the cache-key contract without losing type
/// identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplyCacheKey(Vec<u8>);

impl ApplyCacheKey {
    /// 返回键的哈希字节（此处直接克隆底层字节）。
    pub fn Hash(&self) -> Vec<u8> {
        self.0.clone()
    }
}

impl From<Vec<u8>> for ApplyCacheKey {
    fn from(value: Vec<u8>) -> Self {
        Self(value)
    }
}

impl From<&[u8]> for ApplyCacheKey {
    fn from(value: &[u8]) -> Self {
        Self(value.to_vec())
    }
}

impl AsRef<[u8]> for ApplyCacheKey {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl kvcache::Key for ApplyCacheKey {
    fn Hash(&self) -> Vec<u8> {
        self.Hash()
    }
}

/// 内存占用估算：键字节数 + List Tracker 当前消耗（不含 List 结构体本身）。
///
/// Matches Go's accounting: encoded key bytes plus the list tracker's current
/// consumption. The list's struct allocation is intentionally not counted.
pub fn applyCacheKVMem(key: &ApplyCacheKey, value: &chunk::List) -> i64 {
    key.0.len() as i64 + value.GetMemTracker().BytesConsumed()
}

/// 创建 ApplyCache：底层 LRU 容量设为几乎无限，实际淘汰由本层内存配额控制。
///
/// Creates an ApplyCache whose underlying LRU capacity is effectively
/// unlimited; memory eviction is controlled by ApplyCache itself.
pub fn NewApplyCache<C: ApplyCacheContext + ?Sized>(ctx: &C) -> Result<ApplyCache, Infallible> {
    Ok(ApplyCache {
        cache: syncutil::Mutex::new(kvcache::NewSimpleLRUCache(usize::MAX, 0.1, 0)),
        mem_capacity: ctx.MemQuotaApplyCache(),
        mem_tracker: memory::NewTracker(memory::LabelForApplyCache, -1),
    })
}

impl ApplyCache {
    /// 加锁查询缓存；返回 (值, 是否命中)。
    fn get(&self, key: &ApplyCacheKey) -> (Option<kvcache::Value>, bool) {
        self.cache.lock().Get(key)
    }

    /// 加锁写入缓存条目。
    fn put(&self, key: ApplyCacheKey, value: Arc<chunk::List>) {
        let cache_key: kvcache::KeyRef = Arc::new(key);
        let cache_value: kvcache::Value = value;
        self.cache.lock().Put(cache_key, cache_value);
    }

    /// 淘汰最久未使用条目。
    fn removeOldest(&self) -> (Option<kvcache::KeyRef>, Option<kvcache::Value>, bool) {
        self.cache.lock().RemoveOldest()
    }

    /// 查询缓存项；命中时返回与 Set 时相同的共享 List。
    ///
    /// Gets a cache item. A hit returns the same shared list inserted by Set.
    pub fn Get(&self, key: ApplyCacheKey) -> Result<Option<Arc<chunk::List>>, Infallible> {
        let (value, hit) = self.get(&key);
        if !hit {
            return Ok(None);
        }
        Ok(Some(
            Arc::downcast::<chunk::List>(value.expect("cache hit must have a value"))
                .expect("ApplyCache only stores chunk::List values"),
        ))
    }

    /// 插入条目并按需淘汰 LRU，直到新项能放入配额；单条超过配额则忽略。
    ///
    /// Inserts an item and evicts least-recently-used entries until the new
    /// item fits. An individual item larger than the quota is ignored.
    pub fn Set(&self, key: ApplyCacheKey, value: Arc<chunk::List>) -> Result<bool, Infallible> {
        // 单条过大则拒绝缓存，避免无法通过淘汰腾出空间。
        let mem = applyCacheKVMem(&key, &value);
        if mem > self.mem_capacity {
            return Ok(false);
        }

        // 循环淘汰最旧项，直到当前消耗 + 新项不超过配额。
        while mem + self.mem_tracker.BytesConsumed() > self.mem_capacity {
            let (evicted_key, evicted_value, evicted) = self.removeOldest();
            if !evicted {
                return Ok(false);
            }
            let evicted_key =
                ApplyCacheKey::from(evicted_key.expect("evicted entry must have a key").Hash());
            let evicted_value = Arc::downcast::<chunk::List>(
                evicted_value.expect("evicted entry must have a value"),
            )
            .expect("ApplyCache only stores chunk::List values");
            self.mem_tracker
                .Consume(-applyCacheKVMem(&evicted_key, &evicted_value));
        }

        self.mem_tracker.Consume(mem);
        self.put(key, value);
        Ok(true)
    }

    /// 返回本缓存的内存 Tracker，供上层挂接父子追踪关系。
    pub fn GetMemTracker(&self) -> &memory::Tracker {
        &self.mem_tracker
    }
}
