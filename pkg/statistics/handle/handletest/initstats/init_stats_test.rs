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

// 统计初始化（InitStats）行为测试。
//
// 用可执行内存模型覆盖 Go 侧 InitStats/InitStatsLite 的表 ID 选择、
// 分区/删表过滤、缓存内存策略、按需加载与 skip-init-stats 生命周期。

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

use super::StatsInitializer;

#[derive(Debug)]
struct TestConfig {
    // Go time.Duration is a signed nanosecond count; the tests use -1 to
    // disable automatic initialization.
    stats_lease: i64,
}

const THREE_SECONDS: i64 = 3_000_000_000;

fn with_stats_lease<F>(config: &mut TestConfig, lease: i64, body: F)
where
    F: FnOnce(&mut TestConfig),
{
    let original = config.stats_lease;
    config.stats_lease = lease;
    let result = catch_unwind(AssertUnwindSafe(|| body(config)));
    config.stats_lease = original;
    if let Err(payload) = result {
        resume_unwind(payload);
    }
}

fn with_is_full_cache_func<F>(initializer: &mut StatsInitializer, is_full: bool, body: F)
where
    F: FnOnce(&mut StatsInitializer),
{
    initializer.set_is_full_cache_override(is_full);
    let result = catch_unwind(AssertUnwindSafe(|| body(initializer)));
    initializer.restore_is_full_cache_override();
    if let Err(payload) = result {
        resume_unwind(payload);
    }
}

fn analyzed_fixture(partitioned: bool) -> StatsInitializer {
    let mut initializer = StatsInitializer::new();
    initializer.register_table(1, "test", false, 3, 1);
    initializer.register_table(2, "test", false, 3, 1);
    initializer.register_table(3, "test", false, 3, 1);
    initializer.register_table(4, "test", false, 3, 1);
    initializer.register_table(5, "test", partitioned, 3, 1);
    initializer.analyze([1, 2, 3, 4, 5], 5);
    initializer
}

#[test]
fn lite_init_stats_loads_only_requested_live_table() {
    let mut initializer = StatsInitializer::new();
    initializer.register_table(1, "test", false, 2, 1);
    initializer.register_table(2, "test", false, 2, 1);
    initializer.analyze([1, 2], 5);

    initializer.clear();
    initializer
        .init_stats_lite(&[1])
        .expect("selected lite initialization should succeed");

    assert!(initializer.stats(1).is_some());
    assert!(initializer.stats(2).is_none());
    assert!(initializer.stats(1).unwrap().all_evicted());
}

/// Go TestLiteInitStatsWithTableIDs：选择性加载可重复执行，空列表加载所有存活物理表。
#[test]
fn test_lite_init_stats_with_table_ids() {
    let mut initializer = analyzed_fixture(true);
    initializer.drop_table(4);
    initializer.clear();

    initializer
        .init_stats_lite(&[1])
        .expect("lite init for one table");
    assert!(initializer.stats(1).is_some());
    assert!(initializer.stats(2).is_none());
    assert!(initializer.stats(3).is_none());

    initializer
        .init_stats_lite(&[1, 2])
        .expect("lite init is repeatable");
    assert!(initializer.stats(1).is_some());
    assert!(initializer.stats(2).is_some());
    assert!(initializer.stats(3).is_none());

    initializer
        .init_stats_lite(&[])
        .expect("lite init for all tables");
    for physical_id in [1, 2, 3, 5, 501, 502] {
        assert!(
            initializer.stats(physical_id).is_some(),
            "missing {physical_id}"
        );
        assert!(initializer.stats(physical_id).unwrap().all_evicted());
    }
    assert!(
        initializer.stats(4).is_none(),
        "dropped table must be skipped"
    );
    assert!(initializer.has_persisted_stats(4));
}

/// Go TestNonLiteInitStatsWithTableIDs：完整初始化加载列/索引 payload 且可重复执行。
#[test]
fn test_non_lite_init_stats_with_table_ids() {
    let mut initializer = analyzed_fixture(false);
    initializer.clear();

    initializer
        .init_stats(&[1])
        .expect("full init for one table");
    assert!(initializer.stats(1).unwrap().all_full_load());
    assert!(initializer.stats(2).is_none());
    assert!(initializer.stats(3).is_none());

    initializer
        .init_stats(&[1, 2])
        .expect("full init is repeatable");
    assert!(initializer.stats(1).unwrap().all_full_load());
    assert!(initializer.stats(2).unwrap().all_full_load());
    assert!(initializer.stats(3).is_none());

    initializer
        .init_stats(&[])
        .expect("full init for all tables");
    for table_id in 1..=5 {
        assert!(initializer.stats(table_id).unwrap().all_full_load());
    }
}

fn concurrent_fixture() -> StatsInitializer {
    let mut initializer = StatsInitializer::new();
    for table_id in 1..=9 {
        initializer.register_table(table_id, "test", false, 3, 0);
    }
    initializer.analyze(1..=9, 6);
    initializer
}

fn assert_deferred_then_loaded(initializer: &mut StatsInitializer, is_full: bool) {
    with_is_full_cache_func(initializer, is_full, |initializer| {
        initializer.clear();
        assert_eq!(initializer.cache_len(), 0);
        initializer.init_stats(&[]).expect("deferred full init");
        for table_id in 1..=9 {
            assert!(initializer.stats(table_id).unwrap().all_evicted());
        }
        for table_id in 1..=9 {
            initializer.load_needed_histograms(table_id);
        }
        for table_id in 1..=9 {
            let stats = initializer.stats(table_id).unwrap();
            assert!(stats.all_full_load());
            assert_eq!(stats.row_count, 6);
        }
        assert_eq!(initializer.max_physical_table_id(), 9);
    });
}

/// Go TestConcurrentlyInitStatsWithMemoryLimit：满缓存时先 all-evicted，再按需加载。
#[test]
fn test_concurrently_init_stats_with_memory_limit() {
    let mut initializer = concurrent_fixture();
    assert_deferred_then_loaded(&mut initializer, true);
}

/// Go TestConcurrentlyInitStatsWithoutMemoryLimit：无满缓存限制也保持按需加载契约。
#[test]
fn test_concurrently_init_stats_without_memory_limit() {
    let mut initializer = concurrent_fixture();
    assert_deferred_then_loaded(&mut initializer, false);
}

fn assert_drop_before_init(initializer: &mut StatsInitializer) {
    initializer.drop_table(4);
    initializer.clear();
    initializer.init_stats(&[]).expect("full init after drop");
    assert!(initializer.stats(4).is_none());
    assert!(initializer.has_persisted_stats(4));
    for physical_id in [5, 501, 502] {
        assert!(initializer.stats(physical_id).unwrap().all_full_load());
    }
}

/// Go TestDropTableBeforeConcurrentlyInitStats：删除表的残留 stats_meta 不得进入缓存。
#[test]
fn test_drop_table_before_concurrently_init_stats() {
    let mut initializer = analyzed_fixture(true);
    assert_drop_before_init(&mut initializer);
}

/// Go TestDropTableBeforeNonLiteInitStats：完整路径同样过滤删除表和分区。
#[test]
fn test_drop_table_before_non_lite_init_stats() {
    let mut initializer = analyzed_fixture(true);
    assert_drop_before_init(&mut initializer);
}

/// Go TestSkipStatsInitWithSkipInitStats：跳过初始化时不得创建任何缓存条目。
#[test]
fn test_skip_stats_init_with_skip_init_stats() {
    let mut initializer = StatsInitializer::new();
    initializer.register_table(1, "test", false, 3, 1);
    initializer.analyze([1], 5);
    initializer.set_skip_init_stats(true);
    initializer
        .init_stats(&[])
        .expect("skip-init-stats is not an error");
    assert_eq!(initializer.cache_len(), 0);
    initializer
        .init_stats_lite(&[])
        .expect("skip-init-stats applies to lite init too");
    assert_eq!(initializer.cache_len(), 0);
    assert!(initializer.init_stats_done());
}

/// Go TestNonLiteInitStatsAndCheckTheLastTableStats：默认完整路径保留最后表的完整统计。
#[test]
fn test_non_lite_init_stats_and_check_the_last_table_stats() {
    let mut initializer = StatsInitializer::new();
    for table_id in 1..=3 {
        initializer.register_table(table_id, "test", false, 3, 1);
    }
    initializer.analyze(1..=3, 5);
    assert_eq!(initializer.cache_len(), 0);
    initializer.init_stats(&[]).expect("full initialization");

    for table_id in 1..=3 {
        assert!(initializer.stats(table_id).unwrap().all_full_load());
    }
    assert_eq!(initializer.max_physical_table_id(), 3);
}

/// withStatsLease 必须在测试体结束后恢复全局配置。
#[test]
fn test_stats_lease_is_restored_after_body() {
    let mut config = TestConfig {
        stats_lease: THREE_SECONDS,
    };
    with_stats_lease(&mut config, 0, |config| {
        assert_eq!(config.stats_lease, 0);
    });
    assert_eq!(config.stats_lease, THREE_SECONDS);
}

#[test]
fn test_stats_lease_supports_disabled_sentinel_and_restores() {
    let mut config = TestConfig {
        stats_lease: THREE_SECONDS,
    };
    with_stats_lease(&mut config, -1, |config| {
        assert_eq!(config.stats_lease, -1);
    });
    assert_eq!(config.stats_lease, THREE_SECONDS);
}

#[test]
fn test_stats_lease_is_restored_after_panic() {
    let mut config = TestConfig {
        stats_lease: THREE_SECONDS,
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        with_stats_lease(&mut config, -1, |_| panic!("body panic"));
    }));

    assert!(result.is_err());
    assert_eq!(config.stats_lease, THREE_SECONDS);
}

#[test]
fn test_is_full_cache_override_is_restored_after_panic() {
    let mut initializer = analyzed_fixture(false);
    let result = catch_unwind(AssertUnwindSafe(|| {
        with_is_full_cache_func(&mut initializer, true, |_| panic!("body panic"));
    }));

    assert!(result.is_err());
    initializer.clear();
    initializer.init_stats(&[]).expect("full initialization");
    assert!(initializer.stats(1).unwrap().all_full_load());
}
