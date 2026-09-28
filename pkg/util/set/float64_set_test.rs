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

// Float64Set 单测：重复插入去重、存在性查询与未成员否定。
//
// 对应 Go `TestFloat64Set`：同一 float64 连续 Insert 多次后 Count 仍等于唯一值个数。

use super::*;

// TestFloat64Set 对应 Go 的同名测试：重复插入相同 float64 后集合大小仍等于唯一值数量。
/// 验证去重插入、全部成员 Exist，以及对未插入值返回 false。
#[test]
fn TestFloat64Set() {
    let mut set = NewFloat64Set(&[]);
    let vals = vec![1.1_f64, 1.2, 1.3, 1.4, 1.5, 1.6, 1.7, 1.8, 1.9, 2.0];

    for val in &vals {
        // Go 测试连续 Insert 五次同一个值；这里逐句保留重复写入，验证 map/set 去重语义。
        set.Insert(*val);
        set.Insert(*val);
        set.Insert(*val);
        set.Insert(*val);
        set.Insert(*val);
    }

    assert_eq!(vals.len(), set.Count());

    // Go 的 `len(set)` 直接读取 map 长度；Rust 通过公开 Count API 检查同一语义。
    assert_eq!(vals.len(), set.Count());
    for val in &vals {
        assert!(set.Exist(*val));
    }

    // 查询未插入的 3.0，应返回 false。
    assert!(!set.Exist(3.0));
}
