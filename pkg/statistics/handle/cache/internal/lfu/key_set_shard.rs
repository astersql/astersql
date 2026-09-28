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

// 分片版 `keySet`（对应 Go `keySetShard`）。
//
// 将表 ID 按取模路由到固定数量的分片，降低单把读写锁上的竞争；
// 对外 API 与单分片 `keySet` 对齐，供 LFU 二级键集合使用。

use statistics::Table;
use std::sync::Arc;

use super::key_set::keySet;

// keySetCnt 对应 Go 常量；固定分片数用于降低单个锁上的竞争。
/// 分片数量；固定为 256，用于降低单个锁上的竞争。
pub const keySetCnt: usize = 256;

// keySetShard 对应 Go 的 keySetShard，每个分片内部自行维护读写锁。
/// 由 `keySetCnt` 个独立 `keySet` 组成的分片集合。
pub struct keySetShard {
    resultKeySet: [keySet; keySetCnt],
}

impl keySetShard {
    // newKeySetShard 对应 Go 构造函数，为每个分片建立独立的空 map。
    /// 构造全空分片数组，每个分片各自持有独立读写锁。
    pub fn newKeySetShard() -> Self {
        let result = std::array::from_fn(|_| keySet::default());
        Self {
            resultKeySet: result,
        }
    }

    // shardIndex 保留 Go 的 key%keySetCnt 路由；负数 key 同样在数组索引时 panic。
    /// 按 `key % keySetCnt`（截断取余）选择分片下标。
    fn shardIndex(key: i64) -> usize {
        (key % keySetCnt as i64) as usize
    }

    // Get 对应 Go 的 Get，将查询转发给 key 所在分片。
    /// 将查询转发给 key 所在分片。
    pub fn Get(&self, key: i64) -> Option<Arc<Table>> {
        self.resultKeySet[Self::shardIndex(key)].Get(key)
    }

    // AddKeyValue 对应 Go 的 AddKeyValue。
    /// 将写入路由到对应分片。
    pub fn AddKeyValue(&self, key: i64, table: Arc<Table>) {
        self.resultKeySet[Self::shardIndex(key)].AddKeyValue(key, table);
    }

    // Remove 对应 Go 的 Remove；Go 会忽略 keySet.Remove 的内存成本返回值。
    /// 从对应分片删除；忽略底层返回的内存成本（与 Go 一致）。
    pub fn Remove(&self, key: i64) {
        self.resultKeySet[Self::shardIndex(key)].Remove(key);
    }

    // Keys 汇总所有分片的 key；分片之间没有全局排序，保持 Go map 的无序语义。
    /// 汇总全部分片的键；无全局排序，保持 map 无序语义。
    pub fn Keys(&self) -> Vec<i64> {
        let mut result = Vec::new();
        for key_set in &self.resultKeySet {
            result.extend(key_set.Keys());
        }
        result
    }

    // Len 汇总所有分片长度。
    /// 汇总所有分片的条目数之和。
    pub fn Len(&self) -> usize {
        self.resultKeySet.iter().map(keySet::Len).sum()
    }

    // Clear 逐个清空分片，保留分片数组和锁本身。
    /// 逐个清空分片内容，保留分片数组与锁本身。
    pub fn Clear(&self) {
        for key_set in &self.resultKeySet {
            key_set.Clear();
        }
    }
}
