// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// KV 工具函数单元测试：IncInt64 / GetInt64 以及用户/系统 keyspace 判定。

use crate::test_fixtures::{MockMap, MockStorage};
use kv::Mutator;
use kv_dependency as kv;

/// 验证自增：初值写入、累加、非整数失败，以及接近 u32::MAX 时的 wrapping 行为。
#[test]
fn test_inc_int64() {
    let mut map = MockMap::default();
    let key = kv::Key(b"key".to_vec());
    assert_eq!(1, kv::IncInt64(&mut map, &key, 1).unwrap());
    assert_eq!(11, kv::IncInt64(&mut map, &key, 10).unwrap());

    map.Set(key.clone(), b"not int".to_vec()).unwrap();
    assert!(kv::IncInt64(&mut map, &key, 1).is_err());

    let max_u32 = u32::MAX as i64;
    map.Set(key.clone(), max_u32.to_string().into_bytes())
        .unwrap();
    assert_eq!(max_u32 + 1, kv::IncInt64(&mut map, &key, 1).unwrap());
}

/// 验证 GetInt64：缺失键返回 0，写入后读回正确值。
#[test]
fn test_get_int64() {
    let mut map = MockMap::default();
    let key = kv::Key(b"key".to_vec());
    let ctx = kv::Context::todo();
    assert_eq!(0, kv::GetInt64(&ctx, &map, &key).unwrap());
    kv::IncInt64(&mut map, &key, 15).unwrap();
    assert_eq!(15, kv::GetInt64(&ctx, &map, &key).unwrap());
}

/// 验证 IsUserKS：classic / user keyspace 随 IsNextGen 变化，SYSTEM 恒为 false。
#[test]
fn test_is_user_ks() {
    let classic = MockStorage::default();
    assert_eq!(kerneltype::IsNextGen(), kv::IsUserKS(&classic));

    let user = MockStorage::with_keyspace("user");
    assert_eq!(kerneltype::IsNextGen(), kv::IsUserKS(&user));
    let system = MockStorage::with_keyspace(keyspace::System);
    assert!(!kv::IsUserKS(&system));
}

/// 验证 IsSystemKS：仅 SYSTEM keyspace 在 nextgen 下为 true。
#[test]
fn test_is_system_ks() {
    let user = MockStorage::with_keyspace("user");
    assert!(!kv::IsSystemKS(&user));
    let system = MockStorage::with_keyspace(keyspace::System);
    assert_eq!(kerneltype::IsNextGen(), kv::IsSystemKS(&system));
}
