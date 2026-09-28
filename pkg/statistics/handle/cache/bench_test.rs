// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 统计缓存（StatsCache）基准与冒烟测试。
//
// 覆盖 CopyAndUpdate / PutGet / Get 三类热路径，并以 `TestBenchDaily`
// 注册到 benchdaily；另含 UpdateStatsCache 对 Get 可见性的正确性断言。

use crate::{NewStatsCacheImplForTest, StatisticsTable, StatsCacheImpl};
use astersql_util_benchdaily::{Benchmark, Run};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Execute one benchmark operation per logical iteration.
fn run_work(iterations: u64, operation: impl Fn(usize) + Sync) {
    std::thread::scope(|scope| {
        let operation = &operation;
        for i in 0..iterations {
            scope.spawn(move || operation(i as usize));
        }
    });
}

/// Execute two benchmark workloads in the same scope, matching a shared Go
/// wait group whose goroutines may overlap across both workloads.
fn run_paired_work(iterations: u64, first: impl Fn(usize) + Sync, second: impl Fn(usize) + Sync) {
    std::thread::scope(|scope| {
        let first = &first;
        let second = &second;
        for i in 0..iterations {
            scope.spawn(move || first(i as usize));
        }
        for i in 0..iterations {
            scope.spawn(move || second(i as usize));
        }
    });
}

/// 基于当前纳秒时间戳生成近似唯一的物理表 ID。
fn next_id() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(1)
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1)
}

/// 使用真实列载荷构造基准表，成本由 Table::MemoryUsage 计算。
fn mock_table(physical_id: i64) -> Arc<StatisticsTable> {
    let mut table = cache_testutil::NewMockStatisticsTable(1, 0, true, false, false);
    table.PhysicalID = physical_id;
    table.Version = physical_id as u64;
    Arc::new(table)
}

/// 基准：反复 UpdateStatsCache（拷贝更新路径）。
fn bench_copy_and_update(b: &mut Benchmark, cache: &StatsCacheImpl) {
    run_work(b.iterations(), |_| {
        cache.UpdateStatsCache(&[mock_table(next_id())], &[], false);
    });
}

/// 基准：交替 Put（Update）与 Get。
fn bench_put_get(b: &mut Benchmark, cache: &StatsCacheImpl) {
    run_paired_work(
        b.iterations(),
        |_| cache.UpdateStatsCache(&[mock_table(next_id())], &[], false),
        |i| {
            let _ = cache.Get(i as i64);
        },
    );
}

/// 基准：先预热写入，再测量纯 Get。
fn bench_get(b: &mut Benchmark, cache: &StatsCacheImpl) {
    run_work(b.iterations(), |_| {
        cache.UpdateStatsCache(&[mock_table(next_id())], &[], false);
    });
    run_work(b.iterations(), |i| {
        let _ = cache.Get(i as i64);
    });
}

/// LFU 缓存 CopyAndUpdate 基准入口。
fn BenchmarkStatsCacheLFUCopyAndUpdate(b: &mut Benchmark) {
    let cache = NewStatsCacheImplForTest().unwrap();
    bench_copy_and_update(b, &cache);
}

/// Map 缓存 CopyAndUpdate 基准入口。
fn BenchmarkStatsCacheMapCacheCopyAndUpdate(b: &mut Benchmark) {
    let cache = StatsCacheImpl::new(None, Some((false, 0))).unwrap();
    bench_copy_and_update(b, &cache);
}

/// LFU 缓存 PutGet 基准入口。
fn BenchmarkLFUCachePutGet(b: &mut Benchmark) {
    let cache = NewStatsCacheImplForTest().unwrap();
    bench_put_get(b, &cache);
}

/// Map 缓存 PutGet 基准入口。
fn BenchmarkMapCachePutGet(b: &mut Benchmark) {
    let cache = StatsCacheImpl::new(None, Some((false, 0))).unwrap();
    bench_put_get(b, &cache);
}

/// LFU 缓存纯 Get 基准入口。
fn BenchmarkLFUCacheGet(b: &mut Benchmark) {
    let cache = NewStatsCacheImplForTest().unwrap();
    bench_get(b, &cache);
}

/// Map 缓存纯 Get 基准入口。
fn BenchmarkMapCacheGet(b: &mut Benchmark) {
    let cache = StatsCacheImpl::new(None, Some((false, 0))).unwrap();
    bench_get(b, &cache);
}

/// 注册全部基准到 benchdaily 每日运行。
#[test]
fn TestBenchDaily() {
    Run(vec![
        BenchmarkStatsCacheLFUCopyAndUpdate,
        BenchmarkStatsCacheMapCacheCopyAndUpdate,
        BenchmarkLFUCachePutGet,
        BenchmarkMapCachePutGet,
        BenchmarkLFUCacheGet,
        BenchmarkMapCacheGet,
    ]);
}

/// 校验 UpdateStatsCache 写入后 Get 可见，删除后不可见。
#[test]
fn UpdateStatsCache_is_visible_to_get() {
    let cache = NewStatsCacheImplForTest().unwrap();
    cache.UpdateStatsCache(&[mock_table(42)], &[], false);
    assert!(cache.Get(42).is_some());
    // 第三个参数为删除列表：移除 id=42。
    cache.UpdateStatsCache(&[], &[42], false);
    assert!(cache.Get(42).is_none());
}

/// Go launches benchmark operations in goroutines; ensure the Rust harness does
/// not accidentally serialize them again.
#[test]
fn benchmark_work_overlaps() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    let active = AtomicUsize::new(0);
    let maximum = AtomicUsize::new(0);
    run_work(8, |_| {
        let now = active.fetch_add(1, Ordering::SeqCst) + 1;
        maximum.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(10));
        active.fetch_sub(1, Ordering::SeqCst);
    });

    assert!(maximum.load(Ordering::SeqCst) > 1);
}
