// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 实例级计划缓存（`InstancePlanCache`）单元测试。
//
// 覆盖基本容量/重复键拒绝、按 LRU 部分淘汰与全部清空、多 stats_hash
// 键匹配，以及交错读写下的正确性。

use crate::{InstancePlanCache, NewInstancePlanCache, NewPlanCacheValueForTest, PlanCacheValue};
use std::sync::{Arc, Barrier};
use std::thread;
/// 由测试键与统计哈希拼出缓存键。
fn key(test_key: usize, stats_hash: usize) -> String {
    format!("{test_key}-{stats_hash}")
}

/// 构造带指定 ParseValues 与计划内存占用的测试缓存值。
fn value(test_key: usize, plan_memory_usage: i64) -> PlanCacheValue {
    let mut value = NewPlanCacheValueForTest(plan_memory_usage);
    value.ParseValues = test_key.to_string();
    value
}

/// 向缓存 Put 一条由键与内存占用描述的测试计划。
fn put(
    cache: &InstancePlanCache,
    test_key: usize,
    stats_hash: usize,
    plan_memory_usage: i64,
) -> bool {
    cache.Put(
        key(test_key, stats_hash),
        value(test_key, plan_memory_usage),
    )
}

/// 断言缓存命中且 ParseValues 与 test_key 一致。
fn hit(cache: &InstancePlanCache, test_key: usize, stats_hash: usize) {
    let value = cache
        .Get(&key(test_key, stats_hash), &[])
        .expect("expected instance-plan-cache hit");
    assert_eq!(value.ParseValues, test_key.to_string());
}

/// 断言缓存未命中。
fn miss(cache: &InstancePlanCache, test_key: usize, stats_hash: usize) {
    assert!(cache.Get(&key(test_key, stats_hash), &[]).is_none());
}

/// 单条测试值的实际 MemoryUsage（含结构体固定开销）。
fn one_value_memory() -> i64 {
    value(0, 1).MemoryUsage()
}

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn plan_cache_value_and_instance_cache_are_send_sync() {
    assert_send_sync::<PlanCacheValue>();
    assert_send_sync::<InstancePlanCache>();
}

/// 基本 Put/Get、硬上限拒绝重复写入，以及 soft 水位下的 LRU 淘汰。
#[test]
fn test_instance_plan_cache_basic_limits_duplicates_and_eviction() {
    let memory = one_value_memory();
    let cache = NewInstancePlanCache(memory * 10, memory * 10);
    assert!(put(&cache, 1, 0, 1));
    assert!(put(&cache, 2, 0, 1));
    assert!(put(&cache, 3, 0, 1));
    assert_eq!(cache.MemUsage(), memory * 3);
    hit(&cache, 1, 0);
    hit(&cache, 2, 0);
    hit(&cache, 3, 0);

    let hard_limited = NewInstancePlanCache(memory * 2, memory * 2);
    assert!(put(&hard_limited, 1, 0, 1));
    assert!(put(&hard_limited, 2, 0, 1));
    assert!(!put(&hard_limited, 3, 0, 1));
    assert!(!put(&hard_limited, 1, 0, 1));
    assert_eq!(hard_limited.Size(), 2);

    let evicting = NewInstancePlanCache(memory * 3, memory * 5);
    for test_key in 1..=5 {
        assert!(put(&evicting, test_key, 0, 1));
    }
    // 刷新 1/2/3 的 last_used，使 4/5 成为最旧并被淘汰。
    hit(&evicting, 1, 0);
    hit(&evicting, 2, 0);
    hit(&evicting, 3, 0);
    assert_eq!(evicting.Evict(false), 2);
    assert_eq!(evicting.Size(), 3);
    hit(&evicting, 1, 0);
    hit(&evicting, 2, 0);
    hit(&evicting, 3, 0);
    miss(&evicting, 4, 0);
    miss(&evicting, 5, 0);
}

/// 同 test_key 不同 stats_hash 可并存；满容量拒绝新键；Evict(true) 清空并校验限额 API。
#[test]
fn test_instance_plan_cache_match_keys_limits_and_evict_all() {
    let memory = one_value_memory();
    let cache = NewInstancePlanCache(memory * 3, memory * 3);
    for stats_hash in 1..=3 {
        assert!(put(&cache, 1, stats_hash, 1));
    }
    for stats_hash in 1..=3 {
        hit(&cache, 1, stats_hash);
    }
    miss(&cache, 1, 4);
    miss(&cache, 2, 1);
    assert!(!put(&cache, 2, 1, 1));

    assert_eq!(cache.Evict(true), 3);
    assert_eq!(cache.Size(), 0);
    assert_eq!(cache.MemUsage(), 0);
    assert!(cache.All().is_empty());

    cache.SetLimits(memory, memory * 2);
    assert_eq!(cache.GetLimits(), (memory, memory * 2));
}

/// 多轮交错 Get/Put 后缓存规模应等于软容量相关的预期条数。
#[test]
fn test_instance_plan_cache_interleaved_read_write() {
    let memory = one_value_memory();
    let cache = NewInstancePlanCache(memory * 128, memory * 256);
    for test_key in 0..64 {
        assert!(put(&cache, test_key, 0, 1));
    }

    for worker in 0..4 {
        for test_key in 0..64 {
            hit(&cache, test_key, 0);
        }
        for offset in 0..16 {
            let test_key = 64 + worker * 16 + offset;
            assert!(put(&cache, test_key, 0, 1));
        }
        for test_key in 0..64 {
            hit(&cache, test_key, 0);
        }
    }
    assert_eq!(cache.Size(), 128);
}

/// 缓存必须能在线程间共享，并承受真实的并发读取与写入。
#[test]
fn test_instance_plan_cache_concurrent_read_write() {
    let memory = one_value_memory();
    let cache = Arc::new(NewInstancePlanCache(memory * 256, memory * 512));
    for test_key in 0..64 {
        assert!(put(&cache, test_key, 0, 1));
    }

    let workers = 8;
    let barrier = Arc::new(Barrier::new(workers));
    let handles = (0..workers)
        .map(|worker| {
            let cache = Arc::clone(&cache);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                for round in 0..32 {
                    for test_key in 0..64 {
                        hit(&cache, test_key, 0);
                    }
                    let test_key = 64 + worker * 32 + round;
                    assert!(put(&cache, test_key, 0, 1));
                }
            })
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle.join().expect("instance-plan-cache worker panicked");
    }

    assert_eq!(cache.Size(), 320);
    for test_key in 0..320 {
        hit(&cache, test_key, 0);
    }
}
