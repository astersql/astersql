// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// `SyncMap` 单元测试：覆盖增删查、覆盖写、删除不存在键，以及 `Keys` 排序后断言。
//
// 对应 Go `sync_map_test.go` 的单流程测试；`Keys` 因哈希遍历无序，测试侧显式排序再比较。

use crate as generic;

/// 按 Go 原顺序覆盖 Store/Load/Delete/Keys 的基本行为。
// TestSyncMap 对应 Go 的单个表驱动式流程测试，按原顺序覆盖增删查和 Keys 排序。
#[test]
fn TestSyncMap() {
    let sm = generic::NewSyncMap::<i64, String>(10);
    sm.Store(1, "a".to_string());
    sm.Store(2, "b".to_string());

    // Load an exist key.
    // Go 的 Load 返回 (value, ok)；实现中的 value 用 Option 表达不存在时的零值差异。
    let (v, ok) = sm.Load(&1);
    assert!(ok);
    assert_eq!(Some("a".to_string()), v);

    // Load a non-exist key.
    let (v, ok) = sm.Load(&3);
    assert!(!ok);
    assert_eq!(None::<String>, v);

    // Overwrite the value.
    sm.Store(1, "c".to_string());
    let (v, ok) = sm.Load(&1);
    assert!(ok);
    assert_eq!(Some("c".to_string()), v);

    // Drop an exist key.
    // Go 版 Delete 不关心返回值；只保留删除后的 Load 断言。
    sm.Delete(&1);
    let (v, ok) = sm.Load(&1);
    assert!(!ok);
    assert_eq!(None::<String>, v);

    // Drop a non-exist key.
    sm.Delete(&3);
    assert_eq!(vec![2], sm.Keys());
    let (v, ok) = sm.Load(&3);
    assert!(!ok);
    assert_eq!(None::<String>, v);

    // Test the Keys() method.
    // Go map 遍历无序，所以测试显式 slices.Sort；同样排序后比较。
    let sm = generic::NewSyncMap::<i64, String>(10);
    sm.Store(2, "b".to_string());
    sm.Store(1, "a".to_string());
    sm.Store(3, "c".to_string());
    let mut keys = sm.Keys();
    keys.sort();
    assert_eq!(vec![1, 2, 3], keys);
}
