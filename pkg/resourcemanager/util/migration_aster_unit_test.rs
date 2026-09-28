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

// 资源管理器 util 包的迁移对齐单元测试。
//
// 验证 Mock 池调谐语义、调度常量与 Go 侧一致，以及 ShardPoolMap
// 的增删遍历与空 key 的 panic 行为。

use std::sync::Arc;
use std::time::Duration;

use crate::{Component, DDL, NewMockGPool, NewShardPoolMap, PoolContainer, util};

/// 构造带 DDL 组件标签的池容器，便于写入 ShardPoolMap。
fn container(name: &str) -> PoolContainer {
    PoolContainer {
        Pool: Arc::new(NewMockGPool(name.to_owned(), 10)),
        Component: DDL,
    }
}

/// 覆盖 Mock 池名称、容量、原始并发度与 Tune 后 LastTunerTs 的行为。
#[test]
fn mock_pool_preserves_name_origin_and_tuning_behavior() {
    let pool = NewMockGPool("ddl".to_owned(), 10);

    assert_eq!(pool.Name(), "ddl");
    assert_eq!(pool.Cap(), 10);
    assert_eq!(pool.GetOriginConcurrency(), 10);
    pool.Tune(7);
    assert_eq!(pool.Cap(), 7);
    assert_eq!(pool.GetOriginConcurrency(), 10);
    // LastTunerTs 固定返回「当前时间减 10 秒」，故 elapsed 至少约 9 秒。
    assert!(pool.LastTunerTs().elapsed().unwrap() >= Duration::from_secs(9));
}

/// 确认调度间隔、超频上限与 Component 常量取值与 Go 版本对齐。
#[test]
fn resource_manager_constants_match_go_values() {
    assert_eq!(
        util::MinSchedulerInterval.Load(),
        Duration::from_millis(200)
    );
    assert_eq!(util::MaxOverclockCount, 1);
    assert_eq!(util::UNKNOWN, Component::UNKNOWN);
    assert_eq!(util::DDL, Component::DDL);
    assert_eq!(util::DistTask, Component::DistTask);
    assert_eq!(util::CheckTable, Component::CheckTable);
    assert_eq!(util::ImportInto, Component::ImportInto);
}

/// 覆盖 ShardPoolMap 的 Add/Iter/Del，以及重复 Add 返回错误。
#[test]
fn shard_pool_map_adds_iterates_rejects_duplicates_and_deletes() {
    let pools = NewShardPoolMap();
    for i in 0..10 {
        pools.Add(i.to_string(), container(&i.to_string())).unwrap();
    }

    // 同名 key 再次 Add 应失败，错误文案与 Go 一致。
    let duplicate = pools.Add("1".to_owned(), container("replacement"));
    assert_eq!(duplicate.unwrap_err().to_string(), "pool is already exist");

    let mut names = Vec::new();
    pools.Iter(|pool| {
        assert_eq!(pool.Component, DDL);
        names.push(pool.Pool.Name().to_owned());
    });
    names.sort();
    assert_eq!(names, (0..10).map(|i| i.to_string()).collect::<Vec<_>>());

    for i in 0..10 {
        pools.Del(&i.to_string());
    }
    // 重复删除不存在的 key 应为空操作。
    pools.Del("0");
    let mut count = 0;
    pools.Iter(|_| count += 1);
    assert_eq!(count, 0);
}

/// 空字符串 key 会在 hash 时越界，保持与 Go 侧索引失败语义一致。
#[test]
fn empty_keys_keep_the_go_index_failure_semantics() {
    let pools = NewShardPoolMap();
    let result = std::panic::catch_unwind(|| pools.Add(String::new(), container("empty")));
    assert!(result.is_err());
}
