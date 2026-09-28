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

// LRU 计划缓存（`LRUPlanCache`）单元测试。
//
// 覆盖默认容量、同键多参数类型桶、Get 提升后再淘汰、Delete/DeleteAll、
// SetCapacity 与内存记账，以及 memory guard 强制腾空。

use crate::{NewLRUPlanCache, NewPlanCacheValueForTest, PlanCacheValue};
use types_dependency::metadata::{FieldType, mysql};

/// 将 MySQL 类型字节序列转为 `FieldType` 参数类型列表。
fn parameter_types(types: &[u8]) -> Vec<FieldType> {
    types
        .iter()
        .map(|kind| {
            let mut field_type = FieldType::default();
            field_type.SetType(*kind);
            field_type
        })
        .collect()
}

/// 构造带 ParseValues、参数类型与计划内存占用的测试缓存值。
fn cache_value(id: &str, types: &[u8], plan_memory_usage: i64) -> PlanCacheValue {
    let mut value = NewPlanCacheValueForTest(plan_memory_usage);
    value.ParseValues = id.to_owned();
    value.ParamTypes = parameter_types(types);
    value
}

/// 预定义的多组参数类型，用于同键多桶测试。
const PARAMETER_KINDS: [[u8; 2]; 5] = [
    [mysql::TypeFloat, mysql::TypeDouble],
    [mysql::TypeFloat, mysql::TypeEnum],
    [mysql::TypeFloat, mysql::TypeDate],
    [mysql::TypeFloat, mysql::TypeLong],
    [mysql::TypeFloat, mysql::TypeInt24],
];

/// 默认容量 100；同键多参数类型桶受容量限制，最旧桶被驱逐。
#[test]
fn test_lru_put_preserves_parameter_type_buckets_and_capacity() {
    let default_cache = NewLRUPlanCache(0, 0.0, 0);
    for index in 0..101 {
        default_cache.Put(
            format!("default-{index}"),
            cache_value(&index.to_string(), &PARAMETER_KINDS[0], 64),
        );
    }
    assert_eq!(default_cache.Size(), 100);

    let cache = NewLRUPlanCache(3, 0.0, 0);
    for (index, types) in PARAMETER_KINDS.iter().enumerate() {
        cache.Put(
            "key-1".to_owned(),
            cache_value(&index.to_string(), types, 64),
        );
    }
    assert_eq!(cache.Size(), 3);
    for types in &PARAMETER_KINDS[..2] {
        assert!(cache.Get("key-1", &parameter_types(types)).is_none());
    }
    for (index, types) in PARAMETER_KINDS.iter().enumerate().skip(2) {
        let value = cache
            .Get("key-1", &parameter_types(types))
            .expect("newest parameter buckets remain cached");
        assert_eq!(value.ParseValues, index.to_string());
    }
}

/// Get 命中会将条目提升为最近使用，下一轮淘汰应优先驱逐未提升的旧项。
#[test]
fn test_lru_get_promotes_entry_before_next_eviction() {
    let cache = NewLRUPlanCache(3, 0.0, 0);
    for index in 0..3 {
        cache.Put(
            format!("key-{index}"),
            cache_value(&index.to_string(), &PARAMETER_KINDS[index], 64),
        );
    }
    assert!(
        cache
            .Get("key-0", &parameter_types(&PARAMETER_KINDS[0]))
            .is_some()
    );
    cache.Put(
        "key-3".to_owned(),
        cache_value("3", &PARAMETER_KINDS[3], 64),
    );

    assert!(
        cache
            .Get("key-1", &parameter_types(&PARAMETER_KINDS[1]))
            .is_none()
    );
    assert!(
        cache
            .Get("key-0", &parameter_types(&PARAMETER_KINDS[0]))
            .is_some()
    );
}

/// Delete 移除同键全部参数桶；DeleteAll 清空缓存与内存计数。
#[test]
fn test_lru_delete_and_delete_all_remove_real_entries() {
    let cache = NewLRUPlanCache(5, 0.0, 0);
    cache.Put(
        "shared".to_owned(),
        cache_value("float", &PARAMETER_KINDS[0], 64),
    );
    cache.Put(
        "shared".to_owned(),
        cache_value("enum", &PARAMETER_KINDS[1], 64),
    );
    cache.Put(
        "other".to_owned(),
        cache_value("other", &PARAMETER_KINDS[2], 64),
    );
    cache.Delete("shared");
    assert_eq!(cache.Size(), 1);
    assert!(
        cache
            .Get("shared", &parameter_types(&PARAMETER_KINDS[0]))
            .is_none()
    );
    assert!(
        cache
            .Get("shared", &parameter_types(&PARAMETER_KINDS[1]))
            .is_none()
    );

    cache.DeleteAll();
    assert_eq!(cache.Size(), 0);
    assert_eq!(cache.MemoryUsage(), 0);
}

/// SetCapacity 缩小容量会驱逐；容量 0 报错；Delete/Close 正确扣减内存。
#[test]
fn test_lru_set_capacity_and_memory_accounting() {
    let cache = NewLRUPlanCache(5, 0.0, 0);
    for index in 0..5 {
        cache.Put(
            format!("key-{index}"),
            cache_value(&index.to_string(), &PARAMETER_KINDS[index], 64),
        );
    }
    assert!(cache.MemoryUsage() > 0);
    cache.SetCapacity(3).expect("capacity three is valid");
    assert_eq!(cache.Size(), 3);
    assert_eq!(
        cache.SetCapacity(0).unwrap_err(),
        "capacity of LRU cache should be at least 1"
    );

    let before_delete = cache.MemoryUsage();
    cache.Delete("key-4");
    assert!(cache.MemoryUsage() < before_delete);
    cache.Close();
    assert_eq!(cache.Size(), 0);
    assert_eq!(cache.MemoryUsage(), 0);
}

/// 极小 quota + 非零 guard 时 Put 后应被 memoryControl 腾空且不 panic。
#[test]
fn test_lru_memory_guard_evicts_without_panicking() {
    let cache = NewLRUPlanCache(3, 0.1, 1);
    cache.Put(
        "key-1".to_owned(),
        cache_value("guarded", &PARAMETER_KINDS[0], 64),
    );
    assert_eq!(cache.Size(), 0);
    assert_eq!(cache.MemoryUsage(), 0);
}

/// 与 Go `Put` 一致：兼容参数桶的替换在更新并提升条目后立即返回，
/// 不应再次运行只针对新增条目的 memory guard 驱逐。
#[test]
fn test_lru_compatible_bucket_replacement_skips_memory_guard() {
    let initial = cache_value("initial", &PARAMETER_KINDS[0], 0);
    let quota = ("key-1".len() as i64 + initial.MemoryUsage()) as u64 * 2;
    let cache = NewLRUPlanCache(3, 0.1, quota);
    cache.Put("key-1".to_owned(), initial);
    assert_eq!(cache.Size(), 1);

    cache.Put(
        "key-1".to_owned(),
        cache_value("replacement", &PARAMETER_KINDS[0], quota as i64 * 2),
    );
    assert_eq!(cache.Size(), 1);
    assert_eq!(
        cache
            .Get("key-1", &parameter_types(&PARAMETER_KINDS[0]))
            .expect("replacement remains cached")
            .ParseValues,
        "replacement"
    );
}
