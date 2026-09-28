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

// 分区 TopN 合并为全局 TopN。
//
// 提供串行 `merge_partition_top_n` 与按批/并发参数驱动的
// `merge_global_top_n_by_concurrency`；合并后按频次选出 Top-N，
// 其余高频项作为 overflow 返回，供后续并入直方图。

use crate::{Histogram, StatsWrapper, TopN, TopNMergeTask, TopNMeta, TopNStatsMergeWorker};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;

/// 串行合并全部分区 TopN：一次任务覆盖全部下标，并回写被踢出值后的直方图。
pub fn merge_partition_top_n(
    top_ns: &[TopN],
    n: usize,
    histograms: &mut [Histogram],
    version: i64,
    cancelled: &AtomicBool,
) -> Result<(Option<TopN>, Vec<TopNMeta>), String> {
    if top_ns.len() != histograms.len() {
        return Err(format!(
            "topN/histogram partition count mismatch: {} != {}",
            top_ns.len(),
            histograms.len()
        ));
    }
    // 所有分区 TopN 为空时无需合并。
    if top_ns.iter().all(|top| top.total_count() == 0) {
        return Ok((None, Vec::new()));
    }
    let mut wrapper = StatsWrapper {
        histograms: histograms.to_vec(),
        top_ns: top_ns.to_vec(),
    };
    let len = wrapper.top_ns.len();
    let mut worker = TopNStatsMergeWorker {
        wrapper: &mut wrapper,
        counter: HashMap::new(),
        cancelled,
    };
    let response = worker.run_task(TopNMergeTask { start: 0, end: len }, version);
    if let Some(error) = response.error {
        return Err(error);
    }
    let counter = worker.result();
    histograms.clone_from_slice(&wrapper.histograms);
    Ok(select_top_n(counter, n))
}

/// 按 `batch_size` 分批调用合并工作器；`concurrency` 须为正（与 Go 侧参数校验一致）。
pub fn merge_global_top_n_by_concurrency(
    top_ns: &[TopN],
    n: usize,
    histograms: &mut [Histogram],
    version: i64,
    concurrency: usize,
    batch_size: usize,
    cancelled: &AtomicBool,
) -> Result<(Option<TopN>, Vec<TopNMeta>), String> {
    if concurrency == 0 {
        return Err("merge concurrency must be positive".into());
    }
    if top_ns.len() != histograms.len() {
        return Err(format!(
            "topN/histogram partition count mismatch: {} != {}",
            top_ns.len(),
            histograms.len()
        ));
    }
    // 无分区输入时直接返回空结果。
    if top_ns.is_empty() {
        return Ok((None, Vec::new()));
    }
    let mut wrapper = StatsWrapper {
        histograms: histograms.to_vec(),
        top_ns: top_ns.to_vec(),
    };
    let mut counter = HashMap::new();
    // 批大小钳制在 [1, 256]，与 MAX_PARTITION_MERGE_BATCH_SIZE 对齐。
    let step = batch_size.max(1).min(256);
    let mut start = 0;
    while start < top_ns.len() {
        let end = (start + step).min(top_ns.len());
        let mut worker = TopNStatsMergeWorker {
            wrapper: &mut wrapper,
            counter,
            cancelled,
        };
        let response = worker.run_task(TopNMergeTask { start, end }, version);
        if let Some(error) = response.error {
            return Err(error);
        }
        counter = worker.result();
        start = end;
    }
    histograms.clone_from_slice(&wrapper.histograms);
    Ok(select_top_n(counter, n))
}

/// 按频次降序、编码升序选出前 `n` 项作为全局 TopN，其余进入 overflow。
fn select_top_n(counter: HashMap<Vec<u8>, f64>, n: usize) -> (Option<TopN>, Vec<TopNMeta>) {
    let mut values = counter
        .into_iter()
        .map(|(encoded, count)| TopNMeta {
            encoded,
            count: count.max(0.0) as u64,
        })
        .collect::<Vec<_>>();
    // 频次高者优先；频次相同时按编码稳定排序。
    values.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.encoded.cmp(&right.encoded))
    });
    // split_off(n) 将下标 n 及之后移入 overflow（left）。
    let left = if values.len() > n {
        values.split_off(n)
    } else {
        Vec::new()
    };
    // Go's GetMergedTopNFromSortedSlice always materializes a TopN when the
    // candidate set is non-empty, even when n is zero, then sorts the selected
    // values by their encoded bytes for TopN binary-search invariants.
    values.sort_by(|left, right| left.encoded.cmp(&right.encoded));
    (Some(TopN { values }), left)
}
