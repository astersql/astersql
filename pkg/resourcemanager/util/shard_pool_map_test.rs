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

// ShardPoolMap 的单元测试，对齐 Go 侧 `TestShardPoolMap`。

#![allow(non_snake_case)]

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use crate::mock_gpool::NewMockGPool;
use crate::shard_pool_map::NewShardPoolMap;
use crate::util::{DDL, PoolContainer};

/// 构造 DDL 组件下的 Mock 池容器。
fn pool_container(id: String) -> PoolContainer {
    PoolContainer {
        Pool: Arc::new(NewMockGPool(id, 10)),
        Component: DDL,
    }
}

// Covers the Add/Iter/Del behavior and the duplicate Add error branch from
// TestShardPoolMap in shard_pool_map_test.go.
/// 覆盖 Add/Iter/Del 路径以及重复 Add 返回错误的分支。
#[test]
pub fn TestShardPoolMap() {
    let rc = 10;
    let pm = NewShardPoolMap();
    // 依次注册 rc 个不同 key 的池。
    for i in 0..rc {
        let id = i.to_string();
        assert!(pm.Add(id.clone(), pool_container(id)).is_ok());
    }
    // 重复 key "1" 应失败。
    assert!(
        pm.Add("1".to_owned(), pool_container("1".to_owned()))
            .is_err()
    );

    let cnt = AtomicI32::new(0);
    pm.Iter(|_| {
        cnt.fetch_add(1, Ordering::SeqCst);
    });
    assert_eq!(rc, cnt.load(Ordering::SeqCst));

    // 全部删除后 Iter 计数应为 0。
    for i in 0..rc {
        pm.Del(i.to_string());
    }
    cnt.store(0, Ordering::SeqCst);
    pm.Iter(|_| {
        cnt.fetch_add(1, Ordering::SeqCst);
    });
    assert_eq!(0, cnt.load(Ordering::SeqCst));
    // 对已删除 key 再 Del 一次，确认幂等。
    pm.Del("0");
}
