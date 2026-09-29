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

// SimpleLRUCache 单元测试，对齐 Go `simple_lru_test.go`。
//
// 覆盖 Put 淘汰与 onEvict、零配额、OOM 守卫、Get 刷新近因性、Delete/DeleteAll、
// Values 顺序，以及堆 profile 函数名常量。

use super::simple_lru::{Key, KeyRef, NewSimpleLRUCache, ProfileName, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// 模拟缓存键：用 i64 生成与 Go 侧兼容的 8 字节哈希。
#[derive(Debug)]
struct MockCacheKey {
    key: i64,
}

impl Key for MockCacheKey {
    fn Hash(&self) -> Vec<u8> {
        let mut hash = vec![0_u8; 8];
        // The Go expression's unsigned underflow makes byte zero use an
        // oversized shift (and therefore zero); bytes 1..7 are little-endian.
        // 对齐 Go：第 0 字节因无符号下溢移位过大而为 0；其余为小端。
        for (i, byte) in hash.iter_mut().enumerate().skip(1) {
            *byte = ((self.key >> ((i - 1) * 8)) & 0xff) as u8;
        }
        hash
    }
}

/// 由 i64 构造模拟键。
fn new_mock_hash_key(key: i64) -> KeyRef {
    Arc::new(MockCacheKey { key })
}

/// 将 i64 装箱为缓存值。
fn value(value: i64) -> Value {
    Arc::new(value)
}

/// 从缓存值取出 i64。
fn number(value: &Value) -> i64 {
    *value.downcast_ref::<i64>().expect("int64 cache value")
}

/// 批量提取键哈希，便于断言顺序。
fn hashes(keys: &[KeyRef]) -> Vec<Vec<u8>> {
    keys.iter().map(|key| key.Hash()).collect()
}

/// 读取进程 MemTotal，用作测试配额上限。
fn mem_total() -> u64 {
    (tidb_memory::meminfo::MemTotal
        .read()
        .expect("MemTotal lock poisoned"))()
    .expect("memory total should be available")
}

/// Put 超出容量时应淘汰最旧项并触发 onEvict；有配额与零配额路径行为一致。
#[test]
fn TestPut() {
    let mut lru_max_mem = NewSimpleLRUCache(3, 0.0, mem_total());
    let mut lru_zero_quota = NewSimpleLRUCache(3, 0.0, 0);
    assert_eq!(3, lru_max_mem.capacity);
    assert_eq!(3, lru_zero_quota.capacity);

    let max_mem_dropped = Arc::new(Mutex::new(HashMap::<Vec<u8>, i64>::new()));
    let zero_quota_dropped = Arc::new(Mutex::new(HashMap::<Vec<u8>, i64>::new()));
    let captured = Arc::clone(&max_mem_dropped);
    lru_max_mem.SetOnEvict(Box::new(move |key, value| {
        captured.lock().unwrap().insert(key.Hash(), number(&value));
    }));
    let captured = Arc::clone(&zero_quota_dropped);
    lru_zero_quota.SetOnEvict(Box::new(move |key, value| {
        captured.lock().unwrap().insert(key.Hash(), number(&value));
    }));

    let keys = (0..5).map(new_mock_hash_key).collect::<Vec<_>>();
    for (i, key) in keys.iter().enumerate() {
        lru_max_mem.Put(Arc::clone(key), value(i as i64));
        lru_zero_quota.Put(Arc::clone(key), value(i as i64));
    }

    assert_eq!(lru_max_mem.size, lru_max_mem.capacity);
    assert_eq!(lru_zero_quota.size, lru_zero_quota.capacity);
    assert_eq!(3, lru_max_mem.size);
    assert_eq!(lru_zero_quota.size, lru_max_mem.size);
    assert_eq!(2, max_mem_dropped.lock().unwrap().len());
    for (i, key) in keys.iter().enumerate().take(2) {
        assert!(!lru_max_mem.Get(key.as_ref()).1);
        assert_eq!(max_mem_dropped.lock().unwrap()[&key.Hash()], i as i64);
        assert_eq!(zero_quota_dropped.lock().unwrap()[&key.Hash()], i as i64);
    }
    assert_eq!(
        hashes(&lru_max_mem.Keys()),
        hashes(&keys[2..].iter().rev().cloned().collect::<Vec<_>>())
    );
    assert_eq!(
        lru_max_mem.Values().iter().map(number).collect::<Vec<_>>(),
        vec![4, 3, 2]
    );
}

/// 零配额下仅按 capacity 限制条目数，可填满容量。
#[test]
fn TestZeroQuota() {
    let mut lru = NewSimpleLRUCache(100, 0.0, 0);
    assert_eq!(100, lru.capacity);
    for i in 0..100 {
        lru.Put(new_mock_hash_key(i), value(i));
    }
    assert_eq!(lru.size, lru.capacity);
    assert_eq!(100, lru.size);
}

/// guard=1.0 时阈值为 0，任何 Put 都应立即清空缓存（OOM 守卫）。
#[test]
fn TestOOMGuard() {
    let mut lru = NewSimpleLRUCache(3, 1.0, mem_total());
    let keys = (0..5).map(new_mock_hash_key).collect::<Vec<_>>();
    for (i, key) in keys.iter().enumerate() {
        lru.Put(Arc::clone(key), value(i as i64));
    }
    assert_eq!(0, lru.size);
    for key in keys {
        assert!(!lru.Get(key.as_ref()).1);
    }
}

/// Get 命中应返回值并将键移到最近使用端；已淘汰键返回不存在。
#[test]
fn TestGet() {
    let mut lru = NewSimpleLRUCache(3, 0.0, mem_total());
    let keys = (0..5).map(new_mock_hash_key).collect::<Vec<_>>();
    for (i, key) in keys.iter().enumerate() {
        lru.Put(Arc::clone(key), value(i as i64));
    }
    for key in &keys[..2] {
        let (value, exists) = lru.Get(key.as_ref());
        assert!(!exists);
        assert!(value.is_none());
    }
    for (i, key) in keys.iter().enumerate().skip(2) {
        let (got, exists) = lru.Get(key.as_ref());
        assert!(exists);
        assert_eq!(i as i64, number(&got.unwrap()));
        assert_eq!(3, lru.size);
        assert_eq!(3, lru.capacity);
        assert_eq!(key.Hash(), lru.Keys()[0].Hash());
        assert_eq!(i as i64, number(&lru.Values()[0]));
    }
}

#[test]
fn go_merge_24_peek_keeps_lru_order() {
    let mut lru = NewSimpleLRUCache(3, 0.0, 0);
    let keys = (0..4).map(new_mock_hash_key).collect::<Vec<_>>();
    for (i, key) in keys.iter().take(3).enumerate() {
        lru.Put(Arc::clone(key), value(i as i64));
    }
    let before = hashes(&lru.Keys());
    let (found, exists) = lru.Peek(keys[0].as_ref());
    assert!(exists);
    assert_eq!(0, number(&found.unwrap()));
    assert_eq!(before, hashes(&lru.Keys()));
    let (missing, exists) = lru.Peek(keys[3].as_ref());
    assert!(!exists);
    assert!(missing.is_none());
    lru.Put(Arc::clone(&keys[3]), value(3));
    assert!(!lru.Get(keys[0].as_ref()).1);
}

/// Delete 移除指定键后 Size 减少，其余键仍可 Get。
#[test]
fn TestDelete() {
    let mut lru = NewSimpleLRUCache(3, 0.0, mem_total());
    let keys = (0..3).map(new_mock_hash_key).collect::<Vec<_>>();
    for (i, key) in keys.iter().enumerate() {
        lru.Put(Arc::clone(key), value(i as i64));
    }
    assert_eq!(3, lru.Size());
    lru.Delete(keys[1].as_ref());
    let (got, exists) = lru.Get(keys[1].as_ref());
    assert!(!exists);
    assert!(got.is_none());
    assert_eq!(2, lru.Size());
    assert!(lru.Get(keys[0].as_ref()).1);
    assert!(lru.Get(keys[2].as_ref()).1);
}

/// DeleteAll 清空后所有键不可见且 Size 为 0。
#[test]
fn TestDeleteAll() {
    let mut lru = NewSimpleLRUCache(3, 0.0, mem_total());
    let keys = (0..3).map(new_mock_hash_key).collect::<Vec<_>>();
    for (i, key) in keys.iter().enumerate() {
        lru.Put(Arc::clone(key), value(i as i64));
    }
    assert_eq!(3, lru.Size());
    lru.DeleteAll();
    for key in keys {
        let (got, exists) = lru.Get(key.as_ref());
        assert!(!exists);
        assert!(got.is_none());
        assert_eq!(0, lru.Size());
    }
}

/// Values 应按 MRU→LRU 顺序返回插入值。
#[test]
fn TestValues() {
    let mut lru = NewSimpleLRUCache(5, 0.0, mem_total());
    for i in 0..5 {
        lru.Put(new_mock_hash_key(i), value(i));
    }
    let values = lru.Values();
    assert_eq!(5, values.len());
    assert_eq!(
        values.iter().map(number).collect::<Vec<_>>(),
        vec![4, 3, 2, 1, 0]
    );
}

/// ProfileName 常量须与 Go 侧堆 profile 函数名字符串一致。
#[test]
fn TestPutProfileName() {
    let lru = NewSimpleLRUCache(3, 0.0, 10);
    assert_eq!(3, lru.capacity);
    let profile_name = format!(
        "{}.(*{}).{}",
        "github.com/pingcap/tidb/pkg/util/kvcache", "SimpleLRUCache", "Put"
    );
    assert_eq!(ProfileName, profile_name);
}
