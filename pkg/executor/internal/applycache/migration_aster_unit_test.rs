// Copyright 2026 AsterSQL.

// ApplyCache 迁移回归单测。
//
// ApplyCache 是 Apply 算子（相关子查询按外表行重复执行内表）用的 LRU 结果缓存：
// 以外表行编码值为键，缓存内表行列表，并按会话内存配额 `MemQuotaApplyCache` 做容量控制。
// 本文件对照 Go 侧行为验证：键内存记账、超大条目拒绝、容量淘汰、Get 刷新 LRU，以及并发交接。

#![allow(non_snake_case)]

use std::sync::Arc;

use astersql_executor_internal_applycache::{
    ApplyCacheContext, ApplyCacheKey, NewApplyCache, applyCacheKVMem, chunk,
};

/// 仅提供 ApplyCache 构造所需的内存配额，对应会话变量 `MemQuotaApplyCache`。
struct MockContext {
    quota: i64,
}

impl ApplyCacheContext for MockContext {
    fn MemQuotaApplyCache(&self) -> i64 {
        self.quota
    }
}

/// 构造空的 chunk::List，作为缓存 value（内表结果行列表）。
fn empty_list() -> Arc<chunk::List> {
    Arc::from(chunk::NewList(Vec::<chunk::types::FieldType>::new(), 1, 1))
}

/// 生成指定字节重复 `length` 次的缓存键，便于精确控制键占用内存。
fn key(byte: u8, length: usize) -> ApplyCacheKey {
    vec![byte; length].into()
}

/// 验证键哈希、`applyCacheKVMem` 记账，以及超过配额的条目无法写入。
#[test]
fn key_memory_and_oversized_entries_match_go() {
    let cache = NewApplyCache(&MockContext { quota: 100 }).unwrap();
    let value = empty_list();
    let exact = key(b'e', 100);
    let oversized = key(b'o', 101);

    assert_eq!(exact.Hash(), vec![b'e'; 100]);
    assert_eq!(applyCacheKVMem(&exact, &value), 100);
    assert_eq!(applyCacheKVMem(&oversized, &value), 101);
    // 键本身已超过配额时 Set 失败，Get 为空且 tracker 无消耗。
    assert!(!cache.Set(oversized.clone(), Arc::clone(&value)).unwrap());
    assert!(cache.Get(oversized).unwrap().is_none());
    assert_eq!(cache.GetMemTracker().BytesConsumed(), 0);
}

/// 验证配额满时按 LRU 淘汰旧条目，最终仅保留最新命中项。
#[test]
fn capacity_eviction_and_hits_match_go_test_apply_cache() {
    let cache = NewApplyCache(&MockContext { quota: 100 }).unwrap();
    let values = [empty_list(), empty_list(), empty_list()];
    let keys = [key(b'0', 100), key(b'1', 100), key(b'2', 100)];

    // 每个键占满配额，写入下一项会挤出前一项。
    for index in 0..3 {
        assert!(
            cache
                .Set(keys[index].clone(), Arc::clone(&values[index]))
                .unwrap()
        );
        assert!(cache.Get(keys[index].clone()).unwrap().is_some());
    }

    assert!(cache.Get(keys[0].clone()).unwrap().is_none());
    assert!(cache.Get(keys[1].clone()).unwrap().is_none());
    assert_eq!(cache.GetMemTracker().BytesConsumed(), 100);
}

/// 验证 Get 会刷新 LRU：访问旧键后再插入新键时，被淘汰的是最久未用项。
#[test]
fn get_refreshes_lru_before_memory_driven_eviction() {
    let cache = NewApplyCache(&MockContext { quota: 100 }).unwrap();
    let a = key(b'a', 40);
    let b = key(b'b', 40);
    let c = key(b'c', 40);

    assert!(cache.Set(a.clone(), empty_list()).unwrap());
    assert!(cache.Set(b.clone(), empty_list()).unwrap());
    // 先访问 a，使其成为较新项；再插入 c 时应淘汰 b。
    assert!(cache.Get(a.clone()).unwrap().is_some());
    assert!(cache.Set(c.clone(), empty_list()).unwrap());

    assert!(cache.Get(a).unwrap().is_some());
    assert!(cache.Get(b).unwrap().is_none());
    assert!(cache.Get(c).unwrap().is_some());
    assert_eq!(cache.GetMemTracker().BytesConsumed(), 80);
}

/// 双线程交替 Get/Set，验证并发下仍保持“交接式”互斥写入语义，最终内存记账正确。
#[test]
fn concurrent_get_and_set_preserve_go_handoff_behavior() {
    let cache = Arc::new(NewApplyCache(&MockContext { quota: 100 }).unwrap());
    let values = [empty_list(), empty_list()];
    let keys = [key(b'0', 100), key(b'1', 100)];
    assert!(cache.Set(keys[0].clone(), Arc::clone(&values[0])).unwrap());

    // 线程一：看到 keys[0] 后写入 keys[1]（挤出 keys[0]）。
    let first_cache = Arc::clone(&cache);
    let first_keys = keys.clone();
    let first_values = values.clone();
    let first = std::thread::spawn(move || {
        for _ in 0..100 {
            loop {
                if first_cache.Get(first_keys[0].clone()).unwrap().is_some() {
                    first_cache
                        .Set(first_keys[1].clone(), Arc::clone(&first_values[1]))
                        .unwrap();
                    break;
                }
                std::thread::yield_now();
            }
        }
    });

    // 线程二：看到 keys[1] 后写回 keys[0]，形成乒乓交接。
    let second_cache = Arc::clone(&cache);
    let second_keys = keys.clone();
    let second_values = values.clone();
    let second = std::thread::spawn(move || {
        for _ in 0..100 {
            loop {
                if second_cache.Get(second_keys[1].clone()).unwrap().is_some() {
                    second_cache
                        .Set(second_keys[0].clone(), Arc::clone(&second_values[0]))
                        .unwrap();
                    break;
                }
                std::thread::yield_now();
            }
        }
    });

    first.join().unwrap();
    second.join().unwrap();
    assert!(cache.Get(keys[0].clone()).unwrap().is_some());
    assert_eq!(cache.GetMemTracker().BytesConsumed(), 100);
}
