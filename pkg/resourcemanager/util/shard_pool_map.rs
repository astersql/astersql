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

// 分片池映射（ShardPoolMap）。
//
// 将命名的 `PoolContainer` 按 key 首字节散列到固定数量的分片上，
// 每个分片持有独立 `RwLock`，以降低资源管理器并发注册/删除时的锁竞争。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::sync::{Arc, RwLock};

use crate::util::PoolContainer;

/// 固定分片数量，与 Go 侧实现一致。
const SHARD: usize = 8;

// hash 对应 Go 的 key 首字节取模逻辑；空字符串仍按 Go 原实现会触发索引错误。
/// 按 key 首字节对分片数取模；空 key 会 panic，对齐 Go 索引失败语义。
pub fn hash(key: &str) -> usize {
    key.as_bytes()[0] as usize % SHARD
}

// ShardPoolMap 是按固定分片数组织的池映射。
/// 按固定分片数组织的池映射，对外提供 Add/Del/Iter。
pub struct ShardPoolMap {
    pools: [PoolMap; SHARD],
}

// NewShardPoolMap 创建 8 个独立的池映射，减少不同 key 之间的锁竞争。
/// 创建含 8 个独立池映射的 ShardPoolMap。
pub fn NewShardPoolMap() -> ShardPoolMap {
    ShardPoolMap {
        pools: std::array::from_fn(|_| PoolMap::new()),
    }
}

impl ShardPoolMap {
    /// 向对应分片注册命名池；同名已存在时返回 `PoolMapError`。
    pub fn Add(&self, key: String, pool: PoolContainer) -> Result<(), PoolMapError> {
        self.pools[hash(&key)].Add(key, pool)
    }

    /// 从对应分片删除命名池；不存在时为空操作。
    pub fn Del<K>(&self, key: K)
    where
        K: AsRef<str>,
    {
        let key = key.as_ref();
        self.pools[hash(key)].Del(key);
    }

    /// 依次遍历所有分片中的池容器，对每个调用 `callback`。
    pub fn Iter<F>(&self, mut callback: F)
    where
        F: FnMut(&PoolContainer),
    {
        for pool_map in &self.pools {
            pool_map.Iter(&mut callback);
        }
    }
}

/// 池已存在时 Add 返回的错误类型，Display 文案与 Go 一致。
#[derive(Debug, Eq, PartialEq)]
pub struct PoolMapError;

impl fmt::Display for PoolMapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("pool is already exist")
    }
}

impl Error for PoolMapError {}

/// 单个分片内的池映射：`RwLock` 保护的 `HashMap`。
struct PoolMap {
    pools: RwLock<HashMap<String, Arc<PoolContainer>>>,
}

impl PoolMap {
    /// 创建空的分片映射。
    fn new() -> Self {
        Self {
            pools: RwLock::new(HashMap::new()),
        }
    }

    /// 写锁下插入；key 冲突则返回错误。
    fn Add(&self, key: String, pool: PoolContainer) -> Result<(), PoolMapError> {
        let mut pools = self.pools.write().unwrap();
        if pools.contains_key(&key) {
            return Err(PoolMapError);
        }
        pools.insert(key, Arc::new(pool));
        Ok(())
    }

    /// 写锁下按 key 删除。
    fn Del(&self, key: &str) {
        self.pools.write().unwrap().remove(key);
    }

    /// 读锁下遍历所有池并回调。
    fn Iter<F>(&self, callback: &mut F)
    where
        F: FnMut(&PoolContainer),
    {
        let pools = self.pools.read().unwrap();
        for pool in pools.values() {
            callback(pool);
        }
    }
}
