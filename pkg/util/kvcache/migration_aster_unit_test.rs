// Copyright 2026 AsterSQL.

// `kvcache` 迁移期单元测试：校验 SimpleLRUCache 与 Go 语义对齐。
//
// 覆盖：容量淘汰与驱逐回调、Get/更新刷新近因性、Delete/RemoveOldest/DeleteAll、
// 以及 quota+guard 触发的 OOM 守卫清空缓存。

#![allow(dead_code, non_snake_case, unused_imports)]

/// 测试用内存与 Tracker 再导出，与生产 `lib.rs` 中的 `memory` 桥接一致。
pub mod memory {
    pub use tidb_memory::meminfo::InstanceMemUsed;
    pub use tidb_memory::tracker::{LabelForGlobalSimpleLRUCache, NewTracker, Tracker};
}

#[path = "simple_lru.rs"]
mod simple_lru;

use simple_lru::{GlobalLRUMemUsageTracker, Key, KeyRef, NewSimpleLRUCache, Value};
use std::sync::{Arc, Mutex};

/// 测试键：以原始字节作为 Hash，便于断言二进制哈希不被改写。
#[derive(Debug)]
struct TestKey(Vec<u8>);

impl Key for TestKey {
    fn Hash(&self) -> Vec<u8> {
        self.0.clone()
    }
}

/// 由字节切片构造 `KeyRef`。
fn key(bytes: &[u8]) -> KeyRef {
    Arc::new(TestKey(bytes.to_vec()))
}

/// 将 i64 装箱为缓存 `Value`。
fn value(value: i64) -> Value {
    Arc::new(value)
}

/// 从 `Value` 取出 i64（测试断言辅助）。
fn number(value: &Value) -> i64 {
    *value.downcast_ref::<i64>().unwrap()
}

/// 容量为 2 时第三次 Put 应驱逐最旧项，并调用 onEvict；Keys/Values 保持 MRU→LRU。
#[test]
fn capacity_eviction_preserves_binary_hashes_and_calls_callback() {
    let evicted = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&evicted);
    let mut cache = NewSimpleLRUCache(2, 0.0, 0);
    cache.SetOnEvict(Box::new(move |key, value| {
        captured.lock().unwrap().push((key.Hash(), number(&value)));
    }));

    cache.Put(key(&[0xff]), value(1));
    cache.Put(key(&[0xfe]), value(2));
    cache.Put(key(&[0x00]), value(3));

    assert_eq!(cache.Size(), 2);
    assert_eq!(&*evicted.lock().unwrap(), &[(vec![0xff], 1)]);
    assert!(!cache.Get(key(&[0xff]).as_ref()).1);
    assert_eq!(
        cache.Keys().iter().map(|k| k.Hash()).collect::<Vec<_>>(),
        vec![vec![0x00], vec![0xfe]]
    );
    assert_eq!(
        cache.Values().iter().map(number).collect::<Vec<_>>(),
        vec![3, 2]
    );
}

/// Get/同键 Put 应刷新近因性且不增大 size；随后 Put 新键应驱逐最不常用项。
#[test]
fn get_and_update_refresh_recency_without_growing() {
    let mut cache = NewSimpleLRUCache(2, 0.0, 0);
    cache.Put(key(b"a"), value(1));
    cache.Put(key(b"b"), value(2));

    assert_eq!(number(&cache.Get(key(b"a").as_ref()).0.unwrap()), 1);
    cache.Put(key(b"a"), value(10));
    cache.Put(key(b"c"), value(3));

    assert_eq!(cache.Size(), 2);
    assert_eq!(number(&cache.Get(key(b"a").as_ref()).0.unwrap()), 10);
    assert!(!cache.Get(key(b"b").as_ref()).1);
}

/// Delete、SetCapacity、RemoveOldest、DeleteAll 行为与 Go 侧一致。
#[test]
fn delete_remove_oldest_delete_all_and_capacity_match_go() {
    let mut cache = NewSimpleLRUCache(3, 0.0, 0);
    for i in 0..3 {
        cache.Put(key(&[i]), value(i as i64));
    }
    cache.Delete(key(&[1]).as_ref());
    assert_eq!(cache.Size(), 2);
    assert!(cache.SetCapacity(0).is_err());
    cache.SetCapacity(1).unwrap();
    assert_eq!(cache.Keys()[0].Hash(), vec![2]);

    let (oldest_key, oldest_value, ok) = cache.RemoveOldest();
    assert!(ok);
    assert_eq!(oldest_key.unwrap().Hash(), vec![2]);
    assert_eq!(number(&oldest_value.unwrap()), 2);
    assert!(!cache.RemoveOldest().2);

    cache.Put(key(b"x"), value(4));
    cache.DeleteAll();
    assert_eq!(cache.Size(), 0);
    assert!(cache.Keys().is_empty());
}

/// guard=1.0 且 quota=MemTotal 时，阈值恒为 0，Put 后缓存应被清空（OOM 守卫）。
#[test]
fn quota_guard_clears_the_cache() {
    let max_memory = (tidb_memory::meminfo::MemTotal
        .read()
        .expect("MemTotal lock poisoned"))()
    .expect("memory total should be available");
    let mut guarded = NewSimpleLRUCache(3, 1.0, max_memory);
    guarded.Put(key(b"a"), value(1));
    assert_eq!(guarded.Size(), 0);
}

/// Go 的 float64→uint64 在 guard>1 时按负整数补码转换，阈值不会饱和为 0。
#[test]
fn guard_above_one_preserves_go_unsigned_threshold_conversion() {
    let mut guarded = NewSimpleLRUCache(3, 2.0, 100);
    guarded.Put(key(b"a"), value(1));
    assert_eq!(guarded.Size(), 1);
}

/// Go 包 init 自动创建全局 Tracker；首次构造缓存时 Rust 侧也必须保证其可用。
#[test]
fn cache_construction_initializes_global_tracker() {
    let _cache = NewSimpleLRUCache(1, 0.0, 0);
    assert!(GlobalLRUMemUsageTracker.get().is_some());
}

/// Go 的 uint 容量在 64 位平台不应被 Rust u32 人为截断。
#[test]
fn capacity_preserves_native_word_width() {
    let capacity = u32::MAX as usize + 1;
    let cache = NewSimpleLRUCache(capacity, 0.0, 0);
    assert_eq!(cache.capacity, capacity);
}
