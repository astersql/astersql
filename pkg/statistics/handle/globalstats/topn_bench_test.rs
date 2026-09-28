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

// 全局统计 TopN 合并的基准测试支撑代码。
//
// 构造多分区 TopN 与直方图数据，分别覆盖串行合并和固定并发度的合并路径；
// 数据准备在计时开始前完成，避免编码与样本构造成本干扰合并性能。

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;

/// 最小化的基准上下文，仅保留当前移植代码需要的迭代次数。
pub struct Benchmark {
    iterations: usize,
}
/// 基准使用的时区占位类型。
pub struct Location;
/// 基准使用的 SQL 终止器占位类型。
pub struct SQLKiller;
/// 并发合并所需的工作池配置。
pub struct WorkerPool {
    /// 工作线程数，必须与调用方声明的合并并发度一致。
    pub concurrency: usize,
}

/// 单批最多合并的分区数，与生产合并路径的上限保持一致。
pub const MAX_PARTITION_MERGE_BATCH_SIZE: usize = 256;
/// 各子基准使用的分区规模。
pub const BENCHMARK_SIZES: &[usize] = &[100, 1000, 2000, 5000, 10000];

/// 为指定数量的分区构造 TopN 样本和对应的完整直方图。
///
/// 偶数编号分区会跳过偶数键，以模拟分区间 TopN 不完全重合的情况；直方图仍保留
/// 全部键的精确频数，使合并逻辑能够为未进入局部 TopN 的值补回统计信息。
pub fn prepare_top_ns_and_hists(
    _b: &mut Benchmark,
    partitions: usize,
    _tz: &Location,
) -> (Vec<crate::TopN>, Vec<crate::Histogram>) {
    let mut top_ns = Vec::with_capacity(partitions);
    let mut histograms = Vec::with_capacity(partitions);
    for partition in 0..partitions {
        let values = (1_i64..=500)
            .filter(|key| !(partition % 2 == 0 && key % 2 == 0))
            .map(|key| crate::TopNMeta {
                encoded: key.to_be_bytes().to_vec(),
                count: ((partition as i64 + key) % 1000) as u64,
            })
            .collect();
        top_ns.push(crate::TopN { values });
        let mut exact_counts = HashMap::new();
        for key in 1_i64..=500 {
            exact_counts.insert(key.to_be_bytes().to_vec(), (10 + key * 10) as f64);
        }
        histograms.push(crate::Histogram {
            id: 1,
            ndv: 500,
            buckets: (1..=500)
                .map(|key| crate::Bucket {
                    count: 10 + key * 10,
                    ndv: 10,
                })
                .collect(),
            exact_counts,
        });
    }
    (top_ns, histograms)
}

/// 调用串行实现，将各分区 TopN 合并为全局 TopN。
pub fn merge_part_topn_2_global_topn(
    _loc: &Location,
    version: i32,
    top_ns: &[crate::TopN],
    n: u32,
    hists: &mut [crate::Histogram],
    _is_index: bool,
    _killer: &SQLKiller,
) -> Result<(Option<crate::TopN>, Vec<crate::TopNMeta>), String> {
    crate::merge_partition_top_n(
        top_ns,
        n as usize,
        hists,
        version as i64,
        &AtomicBool::new(false),
    )
}

/// 调用并发实现合并全局 TopN，并校验工作池并发度与请求配置一致。
///
/// 直方图在调用前克隆，避免一次合并对后续基准迭代的输入产生累积影响。
pub fn merge_global_stats_topn_by_concurrency(
    pool: &WorkerPool,
    merge_concurrency: usize,
    batch_size: usize,
    wrapper: &crate::StatsWrapper,
    _loc: &Location,
    version: i32,
    n: u32,
    _is_index: bool,
    _killer: &SQLKiller,
) -> Result<(Option<crate::TopN>, Vec<crate::TopNMeta>), String> {
    if pool.concurrency != merge_concurrency {
        return Err("worker pool concurrency mismatch".into());
    }
    let mut histograms = wrapper.histograms.clone();
    crate::merge_global_top_n_by_concurrency(
        &wrapper.top_ns,
        n as usize,
        &mut histograms,
        version as i64,
        merge_concurrency,
        batch_size,
        &AtomicBool::new(false),
    )
}

/// 在给定分区规模下测量串行 TopN 合并；样本准备不计入耗时。
pub fn benchmark_merge_part_topn_2_global_topn_with_hists(partitions: usize, b: &mut Benchmark) {
    let loc = Location;
    let killer = SQLKiller;
    let (top_ns, mut histograms) = prepare_top_ns_and_hists(b, partitions, &loc);
    reset_timer(b);
    for _ in benchmark_iterations(b) {
        merge_part_topn_2_global_topn(&loc, 1, &top_ns, 100, &mut histograms, false, &killer)
            .expect("serial TopN benchmark merge");
    }
}

/// 按预设分区规模注册串行合并子基准。
pub fn benchmark_merge_part_topn_2_global_topn_with_hists_entry(b: &mut Benchmark) {
    for size in BENCHMARK_SIZES {
        run_sub_benchmark(b, &format!("Size{size}"), |sub_b| {
            benchmark_merge_part_topn_2_global_topn_with_hists(*size, sub_b);
        });
    }
}

/// 在给定分区规模下测量四路并发 TopN 合并；样本准备不计入耗时。
pub fn benchmark_merge_global_stats_topn_by_concurrency_with_hists(
    partitions: usize,
    b: &mut Benchmark,
) {
    let loc = Location;
    let killer = SQLKiller;
    let (top_ns, histograms) = prepare_top_ns_and_hists(b, partitions, &loc);
    let wrapper = crate::StatsWrapper { histograms, top_ns };
    let pool = WorkerPool { concurrency: 4 };
    // 让每批分区数随输入规模增长，同时遵守生产路径的上下界约束。
    let batch_size =
        (wrapper.top_ns.len() / pool.concurrency).clamp(1, MAX_PARTITION_MERGE_BATCH_SIZE);
    reset_timer(b);
    for _ in benchmark_iterations(b) {
        merge_global_stats_topn_by_concurrency(
            &pool,
            pool.concurrency,
            batch_size,
            &wrapper,
            &loc,
            1,
            100,
            false,
            &killer,
        )
        .expect("concurrent TopN benchmark merge");
    }
    close_worker_pool(pool);
}

/// 按预设分区规模注册并发合并子基准。
pub fn benchmark_merge_global_stats_topn_by_concurrency_with_hists_entry(b: &mut Benchmark) {
    for size in BENCHMARK_SIZES {
        run_sub_benchmark(b, &format!("Size{size}"), |sub_b| {
            benchmark_merge_global_stats_topn_by_concurrency_with_hists(*size, sub_b);
        });
    }
}

/// 结束准备阶段并初始化本地基准迭代次数。
pub fn reset_timer(b: &mut Benchmark) {
    b.iterations = 1;
}

/// 返回基准迭代区间，并保证至少执行一次合并。
pub fn benchmark_iterations(b: &Benchmark) -> std::ops::Range<usize> {
    0..b.iterations.max(1)
}

/// 创建独立的子基准上下文并执行其测试体。
pub fn run_sub_benchmark<F: FnOnce(&mut Benchmark)>(_b: &mut Benchmark, _name: &str, f: F) {
    let mut sub = Benchmark { iterations: 1 };
    f(&mut sub);
}

/// 关闭工作池占位对象，并检查其配置有效。
pub fn close_worker_pool(pool: WorkerPool) {
    assert!(pool.concurrency > 0);
}

#[test]
fn benchmark_helpers_execute_real_serial_and_concurrent_merges() {
    let mut benchmark = Benchmark { iterations: 1 };
    benchmark_merge_part_topn_2_global_topn_with_hists(2, &mut benchmark);
    benchmark_merge_global_stats_topn_by_concurrency_with_hists(2, &mut benchmark);
}
