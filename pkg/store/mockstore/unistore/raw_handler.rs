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

// Raw KV 内存处理器：与 MVCC 路径隔离的无序键值读写。
//
// 对应 Go 侧 `rawHandler`；用 `BTreeMap` 提供与 lockstore.MemStore 相同的
// 有序 seek/scan 语义，供 unistore RPC 的 Raw* 命令使用。

use std::collections::BTreeMap;
use std::ops::Bound::{Excluded, Included, Unbounded};
use std::sync::RwLock;

/// 原始键值对，用于批量 put/get/scan 结果。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KvPair {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

/// RawGet 响应：值与是否未找到标记。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RawGetResponse {
    pub value: Vec<u8>,
    pub not_found: bool,
}

/// Raw KV intentionally remains separate from MVCC, like Go's rawHandler. A
/// BTreeMap provides the same ordered seek semantics as lockstore.MemStore.
///
/// RawHandler：线程安全的内存 Raw 存储；与 MVCC（多版本并发控制）完全分离。
#[derive(Default)]
pub struct RawHandler {
    /// 有序键值表；读写通过 RwLock 保护。
    store: RwLock<BTreeMap<Vec<u8>, Vec<u8>>>,
}

impl RawHandler {
    /// 创建空的 RawHandler。
    pub fn new() -> Self {
        Self::default()
    }

    /// 按键读取；空值视为 not_found（与 Go 侧约定一致）。
    pub fn raw_get(&self, key: &[u8]) -> RawGetResponse {
        let value = self
            .store
            .read()
            .expect("raw-store lock poisoned")
            .get(key)
            .cloned()
            .unwrap_or_default();
        RawGetResponse {
            not_found: value.is_empty(),
            value,
        }
    }

    /// 批量按键读取；缺失键返回空 value。
    pub fn raw_batch_get(&self, keys: &[Vec<u8>]) -> Vec<KvPair> {
        let store = self.store.read().expect("raw-store lock poisoned");
        keys.iter()
            .map(|key| KvPair {
                key: key.clone(),
                value: store.get(key).cloned().unwrap_or_default(),
            })
            .collect()
    }

    /// 写入或覆盖单个键值。
    pub fn raw_put(&self, key: Vec<u8>, value: Vec<u8>) {
        self.store
            .write()
            .expect("raw-store lock poisoned")
            .insert(key, value);
    }

    /// 批量写入键值对。
    pub fn raw_batch_put(&self, pairs: &[KvPair]) {
        let mut store = self.store.write().expect("raw-store lock poisoned");
        for pair in pairs {
            store.insert(pair.key.clone(), pair.value.clone());
        }
    }

    /// 删除单个键。
    pub fn raw_delete(&self, key: &[u8]) {
        self.store
            .write()
            .expect("raw-store lock poisoned")
            .remove(key);
    }

    /// 批量删除键。
    pub fn raw_batch_delete(&self, keys: &[Vec<u8>]) {
        let mut store = self.store.write().expect("raw-store lock poisoned");
        for key in keys {
            store.remove(key);
        }
    }

    /// 删除半开区间 `[start, end)` 内的全部键。
    pub fn raw_delete_range(&self, start: &[u8], end: &[u8]) {
        // Go seeks to start and immediately stops when the first key is at or
        // beyond end. Preserve that empty-range behavior instead of passing an
        // inverted range to BTreeMap, which would panic.
        if start >= end {
            return;
        }
        let mut store = self.store.write().expect("raw-store lock poisoned");
        // 先收集再删，避免边迭代边修改 BTreeMap。
        let keys = store
            .range::<[u8], _>((Included(start), Excluded(end)))
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in keys {
            store.remove(&key);
        }
    }

    /// 有序扫描：正向为 `[start, end)`，反向时 start 排他、end 包容（对齐 Go）。
    pub fn raw_scan(&self, start: &[u8], end: &[u8], limit: usize, reverse: bool) -> Vec<KvPair> {
        let store = self.store.read().expect("raw-store lock poisoned");
        if !reverse {
            if !end.is_empty() && start >= end {
                return Vec::new();
            }
            // end 为空表示上界无限制。
            let upper = if end.is_empty() {
                Unbounded
            } else {
                Excluded(end)
            };
            store
                .range::<[u8], _>((Included(start), upper))
                .take(limit)
                .map(copy_pair)
                .collect()
        } else {
            if start <= end {
                return Vec::new();
            }
            // SeekForPrev(start) followed by Go's explicit equality skip means
            // the start key is exclusive; end is inclusive in reverse scans.
            let lower = if end.is_empty() {
                Unbounded
            } else {
                Included(end)
            };
            store
                .range::<[u8], _>((lower, Excluded(start)))
                .rev()
                .take(limit)
                .map(copy_pair)
                .collect()
        }
    }

    /// 返回当前全部键的有序列表（测试/调试用）。
    pub fn keys(&self) -> Vec<Vec<u8>> {
        self.store
            .read()
            .expect("raw-store lock poisoned")
            .keys()
            .cloned()
            .collect()
    }
}

/// 将 BTree 条目拷贝为独立的 KvPair，避免暴露内部引用。
fn copy_pair((key, value): (&Vec<u8>, &Vec<u8>)) -> KvPair {
    KvPair {
        key: safeCopy(key),
        value: safeCopy(value),
    }
}

/// Go 风格工厂：创建新的 RawHandler。
pub fn newRawHandler() -> RawHandler {
    RawHandler::new()
}

/// 字节切片安全拷贝（对齐 Go 侧 safeCopy）。
pub fn safeCopy(value: &[u8]) -> Vec<u8> {
    value.to_vec()
}
