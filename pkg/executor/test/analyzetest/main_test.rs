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

// ANALYZE 测试包级冒烟：区分全量加载与已驱逐的统计状态。
//
// 对应 Go `pkg/executor/test/analyzetest/main_test.go`。统计缓存可按内存
// 配额驱逐列/索引直方图；全量加载状态与「全部已驱逐」状态必须可区分，
// 以免优化器误用空统计。

/// 校验 `NewStatsFullLoadStatus` 与 `NewStatsAllEvictedStatus` 不相等。
#[test]
fn analyze_loaded_status_distinguishes_full_and_evicted_stats() {
    // 全量加载：直方图/TopN 等均在内存中。
    let full = astersql_statistics::NewStatsFullLoadStatus();
    // 全部驱逐：统计条目仍在但载荷已按配额清出。
    let evicted = astersql_statistics::NewStatsAllEvictedStatus();
    assert_ne!(full, evicted);
}

/// Go `TestMain` enables the statistics-cache memory quota before running the
/// package tests. Keep that package-level setup observable in Rust as well.
#[test]
fn analyzetest_main_enables_stats_cache_mem_quota() {
    let restore = astersql_config::restore_func();
    astersql_config::update_global(|config| {
        config.performance.enable_stats_cache_mem_quota = true;
    });
    assert!(
        astersql_config::get_global_config()
            .performance
            .enable_stats_cache_mem_quota
    );
    restore();
}
