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

// ApplyCache 的单元测试。
//
// 覆盖配额不足时的 LRU 淘汰，以及双线程交替 Get/Set 的并发正确性。

#![allow(non_snake_case)]

use std::sync::Arc;

use applycache::{ApplyCacheContext, ApplyCacheKey, NewApplyCache, applyCacheKVMem};

#[path = "main_test.rs"]
mod main_test;

/// 测试用会话上下文，仅提供 MemQuotaApplyCache。
struct ApplyCacheTestContext {
    mem_quota_apply_cache: i64,
}

impl ApplyCacheTestContext {
    fn new(mem_quota_apply_cache: i64) -> Self {
        Self {
            mem_quota_apply_cache,
        }
    }
}

impl ApplyCacheContext for ApplyCacheTestContext {
    fn MemQuotaApplyCache(&self) -> i64 {
        self.mem_quota_apply_cache
    }
}

/// 构造仅含一个 INT64 单元格的 List，作为缓存值。
fn value(index: i64) -> Arc<tidb_chunk::List> {
    let fields = vec![*tidb_chunk::types::NewFieldType(
        tidb_chunk::mysql::TypeLonglong,
    )];
    let mut value = tidb_chunk::list::NewList(fields.clone(), 1, 1);
    let mut src_chunk = tidb_chunk::NewChunkWithCapacity(fields, 1);
    src_chunk.AppendInt64(0, index);
    value.AppendRow(src_chunk.GetRow(0));
    Arc::from(value)
}

/// 构造约 100 字节的键，便于精确控制内存占用。
fn key(index: usize) -> ApplyCacheKey {
    index.to_string().repeat(100).into_bytes().into()
}

#[test]
/// 验证容量为 100 时只能保留最近写入的条目（前两项被淘汰）。
pub fn TestApplyCache() {
    main_test::setup_for_applycache_tests();
    let ctx = ApplyCacheTestContext::new(100);
    let apply_cache = NewApplyCache(&ctx).unwrap();
    let values = [value(0), value(1), value(2)];
    let keys = [key(0), key(1), key(2)];

    for index in 0..3 {
        assert_eq!(applyCacheKVMem(&keys[index], &values[index]), 100);
    }

    assert!(
        apply_cache
            .Set(keys[0].clone(), Arc::clone(&values[0]))
            .unwrap()
    );
    assert!(apply_cache.Get(keys[0].clone()).unwrap().is_some());

    assert!(
        apply_cache
            .Set(keys[1].clone(), Arc::clone(&values[1]))
            .unwrap()
    );
    assert!(apply_cache.Get(keys[1].clone()).unwrap().is_some());

    assert!(
        apply_cache
            .Set(keys[2].clone(), Arc::clone(&values[2]))
            .unwrap()
    );
    assert!(apply_cache.Get(keys[2].clone()).unwrap().is_some());

    assert!(apply_cache.Get(keys[0].clone()).unwrap().is_none());
    assert!(apply_cache.Get(keys[1].clone()).unwrap().is_none());
}

#[test]
/// 两线程交替“读到对方键则写回己方键”，验证互斥下无数据竞争。
pub fn TestApplyCacheConcurrent() {
    main_test::setup_for_applycache_tests();
    let ctx = ApplyCacheTestContext::new(100);
    let apply_cache = Arc::new(NewApplyCache(&ctx).unwrap());
    let values = [value(0), value(1)];
    let keys = [key(0), key(1)];

    let _ = apply_cache
        .Set(keys[0].clone(), Arc::clone(&values[0]))
        .unwrap();

    let first_cache = Arc::clone(&apply_cache);
    let first_keys = keys.clone();
    let first_values = values.clone();
    let first = std::thread::spawn(move || {
        for _ in 0..100 {
            loop {
                if first_cache.Get(first_keys[0].clone()).unwrap().is_some() {
                    let _ = first_cache
                        .Set(first_keys[1].clone(), Arc::clone(&first_values[1]))
                        .unwrap();
                    break;
                }
                std::thread::yield_now();
            }
        }
    });

    let second_cache = Arc::clone(&apply_cache);
    let second_keys = keys.clone();
    let second_values = values.clone();
    let second = std::thread::spawn(move || {
        for _ in 0..100 {
            loop {
                if second_cache.Get(second_keys[1].clone()).unwrap().is_some() {
                    let _ = second_cache
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
    assert!(apply_cache.Get(keys[0].clone()).unwrap().is_some());
}
