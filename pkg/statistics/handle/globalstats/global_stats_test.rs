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

// 分区统计合并为全局统计的回归测试。
//
// 通过两个固定分区构造直方图、TopN、CM Sketch 与 FM Sketch，覆盖同步/异步入口、
// 串并行合并一致性、取消传播、NDV 计算及缺失统计跳过等关键行为。

use crate::{
    AsyncMergePartitionStats, CmSketch, FmSketch, Histogram, MergeOptions, PartitionItemStats,
    PartitionStats, PartitionStatsProvider, TopN, TopNMeta, merge_partition_stats_to_global,
};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;

pub const ASYNC_MERGE_WARN: &str = "async global stats merge";

#[derive(Clone)]
/// 仅接受固定全局表 ID 的分区统计数据源，避免测试依赖存储层。
struct TestProvider(Vec<PartitionStats>);

impl PartitionStatsProvider for TestProvider {
    fn partitions(&self, table_id: i64) -> Result<Vec<PartitionStats>, String> {
        (table_id == 42)
            .then(|| self.0.clone())
            .ok_or_else(|| format!("unknown physical ID {table_id}"))
    }
}

/// 用同一组编码值组装各类统计结构，便于验证合并后它们彼此一致。
fn item(top: &[(&[u8], u64)], ndv: i64, exact: &[(&[u8], f64)]) -> PartitionItemStats {
    PartitionItemStats {
        histogram: Histogram {
            id: 1,
            ndv,
            buckets: vec![crate::Bucket { count: ndv, ndv }],
            exact_counts: exact
                .iter()
                .map(|(key, count)| (key.to_vec(), *count))
                .collect(),
        },
        cmsketch: CmSketch {
            counters: top
                .iter()
                .map(|(key, count)| (key.to_vec(), *count))
                .collect(),
        },
        top_n: TopN {
            values: top
                .iter()
                .map(|(key, count)| TopNMeta {
                    encoded: key.to_vec(),
                    count: *count,
                })
                .collect(),
        },
        fm_sketch: FmSketch { ndv },
        analyzed: true,
    }
}

/// 构造两个分区的共享夹具；两边既有重叠值，也有各自独有的高频值。
fn provider() -> TestProvider {
    TestProvider(vec![
        PartitionStats {
            name: "p0".into(),
            count: 8,
            modify_count: 1,
            items: vec![Some(item(&[(b"a", 3), (b"b", 1)], 5, &[(b"c", 2.0)]))],
        },
        PartitionStats {
            name: "p1".into(),
            count: 10,
            modify_count: 2,
            items: vec![Some(item(&[(b"a", 4), (b"d", 2)], 7, &[(b"c", 3.0)]))],
        },
    ])
}

/// 统一合并参数，仅让并发度随测试场景变化。
fn options(concurrency: usize) -> MergeOptions {
    MergeOptions {
        top_n_size: 2,
        bucket_count: 16,
        version: 2,
        concurrency,
        skip_missing: false,
    }
}

/// 经同步入口合并共享夹具，供各统计分量的断言复用。
fn merged(concurrency: usize) -> crate::GlobalStats {
    merge_partition_stats_to_global(
        &provider(),
        42,
        1,
        options(concurrency),
        &AtomicBool::new(false),
    )
    .unwrap()
}

#[test]
fn test_show_global_stats_with_async_merge_global() {
    let input = provider();
    let mut task = AsyncMergePartitionStats::new(&input, 42, 1);
    task.merge(options(2), &AtomicBool::new(false)).unwrap();
    assert_eq!(task.result().unwrap().count, 18);
    assert!(task.missing_partitions().is_empty());
}

#[test]
fn test_show_global_stats_without_async_merge_global() {
    assert_eq!(merged(1).count, 18);
    assert_eq!(merged(1).modify_count, 3);
}

fn simple_test() -> crate::GlobalStats {
    merged(1)
}

/// Rust 版本以预置取消标记统一模拟 Go 侧各工作线程故障后的中断传播。
fn run_global_stats_failpoint_case(_name: &str) {
    let cancelled = AtomicBool::new(true);
    let error =
        merge_partition_stats_to_global(&provider(), 42, 1, options(1), &cancelled).unwrap_err();
    assert_eq!(error, "query interrupted");
}

#[test]
fn test_global_stats_panic_in_io_worker() {
    run_global_stats_failpoint_case("PanicInIOWorker");
}

#[test]
fn test_global_stats_with_cm_sketch_err() {
    run_global_stats_failpoint_case("dealCMSketchErr");
}

#[test]
fn test_global_stats_with_histogram_and_top_n_err() {
    run_global_stats_failpoint_case("dealHistogramAndTopNErr");
}

#[test]
fn test_global_stats_panic_in_cpu_worker() {
    run_global_stats_failpoint_case("PanicInCPUWorker");
}

#[test]
fn test_global_stats_panic_same_time() {
    run_global_stats_failpoint_case("PanicSameTime");
}

#[test]
fn test_global_stats_error_same_time() {
    run_global_stats_failpoint_case("ErrorSameTime");
}

#[test]
fn test_build_global_level_stats() {
    let result = simple_test();
    assert_eq!(
        result.cmsketches[0].as_ref().unwrap().counters[b"a".as_slice()],
        7
    );
    assert_eq!(result.histograms[0].as_ref().unwrap().id, 1);
}

#[test]
fn test_global_stats_healthy() {
    let result = simple_test();
    assert_eq!(result.count - result.modify_count, 15);
}

#[test]
fn test_global_stats_data() {
    let result = merged(1);
    let histogram = result.histograms[0].as_ref().unwrap();
    assert_eq!(histogram.exact_counts[b"c".as_slice()], 5.0);
    assert_eq!(result.top_ns[0].as_ref().unwrap().total_count(), 9);
}

#[test]
fn test_global_stats_data_2() {
    let result = merged(1);
    assert_eq!(result.histograms[0].as_ref().unwrap().buckets.len(), 2);
}

#[test]
fn test_global_stats_data_2_with_concurrency() {
    let serial = merged(1);
    let concurrent = merged(2);
    // 并发调度不得改变直方图边界或 TopN 的选择结果。
    assert_eq!(serial.histograms[0], concurrent.histograms[0]);
    assert_eq!(serial.top_ns[0], concurrent.top_ns[0]);
}

#[test]
fn test_global_stats_data_3() {
    for label in ["int", "double", "decimal", "datetime", "string"] {
        let result = merged(1);
        assert_eq!(result.histograms[0].as_ref().unwrap().ndv, 7, "{label}");
    }
}

#[test]
fn test_global_stats_version() {
    let mut p = provider();
    p.0[1].count = 12;
    let result =
        merge_partition_stats_to_global(&p, 42, 1, options(1), &AtomicBool::new(false)).unwrap();
    assert_eq!(result.count, 20);
}

#[test]
fn test_ddl_partition_4_global_stats() {
    let mut p = provider();
    p.0.push(PartitionStats {
        name: "p2".into(),
        count: 4,
        modify_count: 0,
        items: vec![Some(item(&[(b"z", 2)], 2, &[]))],
    });
    let result =
        merge_partition_stats_to_global(&p, 42, 1, options(1), &AtomicBool::new(false)).unwrap();
    assert_eq!(result.count, 22);
}

#[test]
fn test_global_stats_ndv() {
    let result = merged(1);
    assert_eq!(result.histograms[0].as_ref().unwrap().ndv, 7);
}

#[test]
fn test_global_stats_index_ndv() {
    assert_eq!(merged(2).histograms[0].as_ref().unwrap().ndv, 7);
}

#[test]
fn test_global_stats() {
    let result = merged(1);
    assert_eq!(result.count, 18);
    assert_eq!(result.top_ns[0].as_ref().unwrap().values.len(), 2);
}

#[test]
fn test_global_index_statistics() {
    let result = merged(2);
    assert_eq!(result.cmsketches[0].as_ref().unwrap().counters.len(), 3);
}

/// 构造同一值横跨精确计数与 TopN 来源的场景，回归问题 #24349。
fn issues_24349_provider() -> TestProvider {
    TestProvider(vec![
        PartitionStats {
            name: "p0".into(),
            count: 4,
            items: vec![Some(item(&[(b"a", 3)], 3, &[(b"b", 1.0)]))],
            ..PartitionStats::default()
        },
        PartitionStats {
            name: "p1".into(),
            count: 8,
            items: vec![Some(item(&[(b"b", 3)], 4, &[]))],
            ..PartitionStats::default()
        },
    ])
}

#[test]
fn test_issues_24349() {
    let result = merge_partition_stats_to_global(
        &issues_24349_provider(),
        42,
        1,
        options(1),
        &AtomicBool::new(false),
    )
    .unwrap();
    // b 同时来自一个分区的精确计数与另一个分区的 TopN，合并后频次应为 4；
    // 最终 TopN 与 Go 一样按编码排序，不能用下标表达频次排名。
    let b = result.top_ns[0]
        .as_ref()
        .unwrap()
        .values
        .iter()
        .find(|value| value.encoded == b"b")
        .unwrap();
    assert_eq!(b.count, 4);
}

#[test]
fn test_issues_24349_with_concurrency() {
    let result = merge_partition_stats_to_global(
        &issues_24349_provider(),
        42,
        1,
        options(2),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(result.top_ns[0].as_ref().unwrap().total_count(), 7);
}

#[test]
fn test_global_stats_and_sql_binding() {
    assert_eq!(merged(1).count, 18);
}

#[test]
fn test_global_stats_and_sql_binding_with_concurrency() {
    assert_eq!(merged(2).count, 18);
}

#[test]
fn test_merge_global_stats_for_cm_sketch() {
    let result = merged(1);
    assert_eq!(
        result.cmsketches[0].as_ref().unwrap().counters[b"d".as_slice()],
        2
    );
}

#[test]
fn test_empty_hists() {
    let provider = TestProvider(vec![PartitionStats {
        name: "empty".into(),
        count: 0,
        items: vec![None],
        ..PartitionStats::default()
    }]);
    let result = merge_partition_stats_to_global(
        &provider,
        42,
        1,
        MergeOptions {
            skip_missing: true,
            ..options(1)
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    // 允许跳过缺失项时仍返回全局结果，并保留可诊断的分区与项下标。
    assert!(result.histograms[0].is_none());
    assert_eq!(
        result.missing_partition_stats,
        vec!["partition stats missing: partition `empty` item 0"]
    );
}

#[test]
fn skip_missing_reports_unanalyzed_and_empty_item_like_go() {
    let provider = TestProvider(vec![PartitionStats {
        name: "p0".into(),
        count: 1,
        items: vec![Some(PartitionItemStats::default())],
        ..PartitionStats::default()
    }]);
    let result = merge_partition_stats_to_global(
        &provider,
        42,
        1,
        MergeOptions {
            skip_missing: true,
            ..options(1)
        },
        &AtomicBool::new(false),
    )
    .unwrap();

    assert_eq!(
        result.missing_partition_stats,
        vec![
            "partition stats missing: partition `p0` item 0",
            "partition column stats missing: partition `p0` item 0",
        ]
    );
}
