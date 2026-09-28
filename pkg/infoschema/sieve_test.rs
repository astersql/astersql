// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// SIEVE 缓存单元测试。
//
// 对应 `sieve_test.go`：覆盖 Set/Get、Remove、淘汰策略、Contains、Size 与 Purge。
// SIEVE：带第二次机会位的缓存淘汰算法。

// 对应 pkg/infoschema/sieve_test.go。

use astersql_util_size::MB;

use crate::sieve::{Sieve, newSieve};

/// 批量写入后按键读回，验证基本存取。
#[test]
fn test_get_and_set() {
    let items = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    let cache = newSieve::<i32, i32>(10 * MB);

    for v in items {
        cache.Set(v, v * 10);
    }
    for v in items {
        let val = cache.Get(&v).expect("key should be present");
        assert_eq!(v * 10, val);
    }
    cache.Close();
}

/// 删除存在键返回 true；删除缺失键不 panic 且返回 false。
#[test]
fn test_remove() {
    let cache = newSieve::<i32, i32>(10 * MB);
    cache.Set(1, 10);

    let val = cache.Get(&1).expect("key should be present");
    assert_eq!(10, val);

    assert!(cache.Remove(&1));
    assert!(cache.Get(&1).is_none());

    // Removing a missing key must not panic and returns false.
    assert!(!cache.Remove(&-1));
    cache.Close();
}

/// 验证热数据（访问过）在容量挤占时优先存活。
#[test]
fn test_sieve_policy() {
    let entry_size = Sieve::<i32, i32>::entry_size();
    let cache = newSieve::<i32, i32>(10 * entry_size);
    let one_hit_wonders = [1, 2, 3, 4, 5];
    let popular_objects = [6, 7, 8, 9, 10];

    for v in one_hit_wonders {
        cache.Set(v, v);
    }
    for v in popular_objects {
        cache.Set(v, v);
    }
    // 访问热数据，置 visited，使其在后续插入驱逐中获得第二次机会。
    for v in popular_objects {
        assert!(cache.Get(&v).is_some());
    }
    for v in one_hit_wonders {
        cache.Set(v * 10, v * 10);
    }
    for v in popular_objects {
        assert!(
            cache.Get(&v).is_some(),
            "popular object {v} should survive eviction"
        );
    }
    cache.Close();
}

/// Contains 在插入前后反映键存在性。
#[test]
fn test_contains() {
    let cache = newSieve::<String, String>(10 * MB);
    assert!(!cache.Contains(&"hello".to_string()));

    cache.Set("hello".to_string(), "world".to_string());
    assert!(cache.Contains(&"hello".to_string()));
    cache.Close();
}

/// Size 按条目字节累加；重复 Set 同键不增加占用。
#[test]
fn test_cache_size() {
    let sz = Sieve::<i32, i32>::entry_size();
    let cache = newSieve::<i32, i32>(10 * MB);
    assert_eq!(0_u64, cache.Size());

    cache.Set(1, 1);
    assert_eq!(1 * sz, cache.Size());

    // Duplicated key only updates recent-ness / value.
    // 重复键只更新值/visited，不增加 Size。
    cache.Set(1, 1);
    assert_eq!(1 * sz, cache.Size());

    cache.Set(2, 2);
    assert_eq!(2 * sz, cache.Size());
    cache.Close();
}

/// Purge 清空后 Len 为 0。
#[test]
fn test_purge() {
    let cache = newSieve::<i32, i32>(10 * MB);
    cache.Set(1, 1);
    cache.Set(2, 2);
    assert_eq!(2, cache.Len());

    cache.Purge();
    assert_eq!(0, cache.Len());
    cache.Close();
}

/// Go `Close` cancels background work but does not permanently disable `Set`.
#[test]
fn test_set_after_close() {
    let cache = newSieve::<i32, i32>(10 * MB);
    cache.Set(1, 1);
    cache.Close();

    cache.Set(2, 2);
    assert_eq!(Some(2), cache.Get(&2));
}
