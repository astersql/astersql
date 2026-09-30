// Copyright 2025 PingCAP, Inc.
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

// 对应 Go `tidb_vars` 测试：NextGen MDL 行为与慢日志限流并发基准辅助。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use astersql_sessionctx_vardef::*;

#[test]
fn go_merge_47_analyze_defaults_are_atomic_and_have_go_bounds() {
    assert_eq!(DefTiDBAnalyzeDefaultNumBuckets, 256);
    assert_eq!(DefTiDBAnalyzeDefaultNumTopN, 100);
    assert_eq!(MinTiDBAnalyzeDefaultNumBuckets, 1);
    assert_eq!(MaxTiDBAnalyzeDefaultNumBuckets, 100_000);
    assert_eq!(MinTiDBAnalyzeDefaultNumTopN, 0);
    assert_eq!(MaxTiDBAnalyzeDefaultNumTopN, 100_000);
    let previous_buckets = AnalyzeDefaultNumBuckets.Load();
    let previous_top_n = AnalyzeDefaultNumTopN.Load();
    AnalyzeDefaultNumBuckets.Store(123);
    AnalyzeDefaultNumTopN.Store(45);
    assert_eq!(AnalyzeDefaultNumBuckets.Load(), 123);
    assert_eq!(AnalyzeDefaultNumTopN.Load(), 45);
    AnalyzeDefaultNumBuckets.Store(previous_buckets);
    AnalyzeDefaultNumTopN.Store(previous_top_n);
}

/// 测试结束时将 `enableMDL` 复位为 false 的 RAII 守卫。
struct EnableMdlRestore;

/// Drop 时恢复 Classic 下 MDL 包级开关的初始状态。
impl Drop for EnableMdlRestore {
    fn drop(&mut self) {
        // The native harness is an isolated process and enableMDL starts false.
        SetEnableMDL(false);
    }
}

// Mirrors TestIsMDLEnabledInNextGen, including restoration of the private
// package-level flag after the assertions.
#[test]
/// 镜像 `TestIsMDLEnabledInNextGen`：NextGen 下无论 SetEnableMDL 如何均为 true。
fn test_is_mdl_enabled_in_next_gen() {
    // Classic 无此强制行为，直接返回。
    if kerneltype::IsClassic() {
        return;
    }

    let _restore = EnableMdlRestore;

    SetEnableMDL(false);
    assert!(IsMDLEnabled());
    SetEnableMDL(true);
    assert!(IsMDLEnabled());
}

// Mirrors runConcurrentTest: all workers wait for the same start signal and
// invoke Allow floor(b.N/goroutines) times. As in Go, Allow's result is ignored.
/// 镜像 Go `runConcurrentTest`：多线程同步启动后各调用 Allow floor(N/workers) 次。
fn run_concurrent_test(bench_n: usize, goroutines: usize) -> usize {
    assert!(goroutines > 0);
    let calls_per_worker = bench_n / goroutines;
    // Barrier 含主线程：所有 worker 就位后再一起压测。
    let start = Arc::new(Barrier::new(goroutines + 1));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut workers = Vec::with_capacity(goroutines);

    for _ in 0..goroutines {
        let start = Arc::clone(&start);
        let calls = Arc::clone(&calls);
        workers.push(thread::spawn(move || {
            start.wait();
            for _ in 0..calls_per_worker {
                let _ = GlobalSlowLogRateLimiter.Allow();
                calls.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    start.wait();
    for worker in workers {
        worker.join().expect("rate limiter worker panicked");
    }
    calls.load(Ordering::Relaxed)
}

// Mirrors BenchmarkRateLimiterSimple.
/// 单线程限流基准入口（对应 BenchmarkRateLimiterSimple）。
pub fn benchmark_rate_limiter_simple(bench_n: usize) -> usize {
    run_concurrent_test(bench_n, 1)
}

// Mirrors BenchmarkRateLimiterCurrency100 (name retained from Go).
/// 100 并发限流基准（名称保留 Go 拼写 currency）。
pub fn benchmark_rate_limiter_currency_100(bench_n: usize) -> usize {
    run_concurrent_test(bench_n, 100)
}

// Mirrors BenchmarkRateLimiterCurrency1000 (name retained from Go).
/// 1000 并发限流基准。
pub fn benchmark_rate_limiter_currency_1000(bench_n: usize) -> usize {
    run_concurrent_test(bench_n, 1_000)
}

#[test]
/// 校验并发调用次数为 floor(bench_n / goroutines) * goroutines。
fn test_run_concurrent_test_matches_go_floor_division() {
    assert_eq!(benchmark_rate_limiter_simple(17), 17);
    assert_eq!(run_concurrent_test(103, 10), 100);
}
