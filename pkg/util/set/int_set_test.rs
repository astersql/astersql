// Copyright 2019 PingCAP, Inc.
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

// IntSet / Int64Set 单测：重复插入去重、Exist 与变参构造。
//
// 对应 Go `TestIntSet`、`TestInt64Set`。

use super::*;

// TestIntSet 对应 Go 的同名测试：重复插入 int 后集合计数保持唯一值数量。
/// 验证 IntSet 去重、成员查询与未成员否定。
#[test]
fn TestIntSet() {
    let mut set = NewIntSet(&[]);
    let vals: Vec<isize> = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

    for val in &vals {
        // 连续五次插入同一个值，保留 Go map 写入同一 key 不增加长度的断言意图。
        set.Insert(*val);
        set.Insert(*val);
        set.Insert(*val);
        set.Insert(*val);
        set.Insert(*val);
    }

    assert_eq!(vals.len(), set.Count());

    // Go 的 `len(set)` 是 map 长度；Rust 通过公开 Count API 检查同一语义。
    assert_eq!(vals.len(), set.Count());
    for val in &vals {
        assert!(set.Exist(*val));
    }

    assert!(!set.Exist(11));
}

// TestInt64Set 对应 Go 的同名测试：覆盖 int64 集合的去重插入、存在性查询和变参构造。
/// 验证 Int64Set 去重、Exist，以及切片构造初始化。
#[test]
fn TestInt64Set() {
    let mut set = NewInt64Set(&[]);
    let vals: Vec<i64> = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

    for val in &vals {
        // 与 IntSet 相同，重复插入只应保留一个成员。
        set.Insert(*val);
        set.Insert(*val);
        set.Insert(*val);
        set.Insert(*val);
        set.Insert(*val);
    }

    assert_eq!(vals.len(), set.Count());
    for val in &vals {
        assert!(set.Exist(*val));
    }

    assert!(!set.Exist(11));

    // Go 的 NewInt64Set(1, 2, 3, 4, 5, 6) 迁移为切片参数，保留变参初始化语义。
    set = NewInt64Set(&[1, 2, 3, 4, 5, 6]);
    for i in 1..7 {
        assert!(set.Exist(i as i64));
    }
    assert!(!set.Exist(7));
}
