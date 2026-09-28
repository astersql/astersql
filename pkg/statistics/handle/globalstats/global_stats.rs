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

// 分区表全局统计合并。
//
// 将各分区的直方图（Histogram）、CMSketch、TopN、FMSketch 合并为全局统计
// （global stats），供优化器在动态裁剪（dynamic prune）模式下估计整表基数。
// 对应 Go `statistics/handle/globalstats` 的同步合并路径。

use crate::{
    CmSketch, FmSketch, Histogram, TopN, merge_global_top_n_by_concurrency, merge_partition_top_n,
};
use std::sync::atomic::{AtomicBool, Ordering};

/// 并发合并 TopN 时每个 worker 批次大小上限。
pub const MAX_PARTITION_MERGE_BATCH_SIZE: usize = 256;

/// 单个分区的统计快照：行数、修改计数与各列/索引项。
#[derive(Clone, Debug, Default)]
pub struct PartitionStats {
    /// 分区名（用于缺失诊断文案）。
    pub name: String,
    /// 分区行数。
    pub count: i64,
    /// 自上次 ANALYZE 以来的修改行数估计。
    pub modify_count: i64,
    /// 按列/索引下标对齐的项统计；`None` 表示该项缺失。
    pub items: Vec<Option<PartitionItemStats>>,
}

/// 分区内某一列或索引的统计组件集合。
#[derive(Clone, Debug, Default)]
pub struct PartitionItemStats {
    /// 等深直方图。
    pub histogram: Histogram,
    /// Count-Min Sketch，用于近似频率。
    pub cmsketch: CmSketch,
    /// 高频值 TopN。
    pub top_n: TopN,
    /// FMSketch，用于 NDV（不同值个数）估计。
    pub fm_sketch: FmSketch,
    /// 该项是否已完成 ANALYZE。
    pub analyzed: bool,
}

/// 合并后的全局统计：按项下标对齐的各组件，以及缺失分区列表与汇总行数。
#[derive(Clone, Debug, Default)]
pub struct GlobalStats {
    pub histograms: Vec<Option<Histogram>>,
    pub cmsketches: Vec<Option<CmSketch>>,
    pub top_ns: Vec<Option<TopN>>,
    pub fm_sketches: Vec<Option<FmSketch>>,
    /// 在 `skip_missing` 模式下记录的缺失分区项描述。
    pub missing_partition_stats: Vec<String>,
    pub count: i64,
    pub modify_count: i64,
}

/// 合并参数：TopN 大小、桶数、统计版本、并发度与是否跳过缺失分区。
#[derive(Clone, Copy, Debug)]
pub struct MergeOptions {
    pub top_n_size: usize,
    pub bucket_count: usize,
    pub version: i64,
    pub concurrency: usize,
    pub skip_missing: bool,
}

/// 按表 ID 提供各分区统计的数据源。
pub trait PartitionStatsProvider {
    fn partitions(&self, table_id: i64) -> Result<Vec<PartitionStats>, String>;
}

/// 将合并结果按项写回存储的接口。
pub trait GlobalStatsWriter {
    fn save(&self, table_id: i64, item_index: usize, stats: &GlobalStats) -> Result<(), String>;
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Acquire) {
        Err("query interrupted".into())
    } else {
        Ok(())
    }
}

fn missing_item_error(partition: &PartitionStats, index: usize, column_stats: bool) -> String {
    let kind = if column_stats {
        "partition column stats missing"
    } else {
        "partition stats missing"
    };
    format!("{kind}: partition `{}` item {index}", partition.name)
}

/// 拉取全部分区统计并合并为 `GlobalStats`。
///
/// 流程：累加行数 → 收集每项已分析的分区样本 → 合并 FM/CM/TopN/直方图；
/// `cancelled` 允许协作取消。
pub fn merge_partition_stats_to_global(
    provider: &dyn PartitionStatsProvider,
    table_id: i64,
    item_count: usize,
    options: MergeOptions,
    cancelled: &AtomicBool,
) -> Result<GlobalStats, String> {
    check_cancelled(cancelled)?;
    let partitions = provider.partitions(table_id)?;
    let mut output = GlobalStats {
        histograms: vec![None; item_count],
        cmsketches: vec![None; item_count],
        top_ns: vec![None; item_count],
        fm_sketches: vec![None; item_count],
        ..GlobalStats::default()
    };
    let mut all_items = vec![Vec::new(); item_count];
    for partition in partitions {
        check_cancelled(cancelled)?;
        output.count += partition.count;
        output.modify_count += partition.modify_count;
        for index in 0..item_count {
            let item = partition.items.get(index).and_then(Option::as_ref);
            let mut skip_partition = false;
            if !item.is_some_and(|item| item.analyzed) {
                let missing = missing_item_error(&partition, index, false);
                if !options.skip_missing {
                    return Err(missing);
                }
                output.missing_partition_stats.push(missing);
                skip_partition = true;
            }
            // Go evaluates this independently from the analyzed check, so an
            // unanalyzed row-bearing item can produce both diagnostics.
            let column_stats_missing = partition.count > 0
                && item.is_none_or(|item| {
                    item.histogram.buckets.is_empty() && item.top_n.total_count() == 0
                });
            if column_stats_missing {
                let missing = missing_item_error(&partition, index, true);
                if !options.skip_missing {
                    return Err(missing);
                }
                output.missing_partition_stats.push(missing);
                skip_partition = true;
            }
            if !skip_partition && let Some(item) = item {
                all_items[index].push(item.clone());
            }
        }
    }
    for (index, items) in all_items.into_iter().enumerate() {
        check_cancelled(cancelled)?;
        if items.is_empty() {
            continue;
        }
        // 先合并 FMSketch / CMSketch。
        let mut fms = items[0].fm_sketch.clone();
        let mut cms = items[0].cmsketch.clone();
        for item in items.iter().skip(1) {
            fms.merge(&item.fm_sketch);
            cms.merge(&item.cmsketch);
        }
        let mut histograms = items
            .iter()
            .map(|item| item.histogram.clone())
            .collect::<Vec<_>>();
        let top_ns = items
            .iter()
            .map(|item| item.top_n.clone())
            .collect::<Vec<_>>();
        // 统计版本 1 或并发度 < 2 走串行 TopN 合并，否则按并发批次合并。
        let (top, popped) = if options.concurrency < 2 || options.version == 1 {
            merge_partition_top_n(
                &top_ns,
                options.top_n_size,
                &mut histograms,
                options.version,
                cancelled,
            )?
        } else {
            let batch =
                (top_ns.len() / options.concurrency).clamp(1, MAX_PARTITION_MERGE_BATCH_SIZE);
            merge_global_top_n_by_concurrency(
                &top_ns,
                options.top_n_size,
                &mut histograms,
                options.version,
                options.concurrency,
                batch,
                cancelled,
            )?
        };
        let mut merged = Histogram {
            id: histograms[0].id,
            // NDV 不超过全局行数。
            ndv: fms.ndv.min(output.count),
            ..Histogram::default()
        };
        for histogram in histograms {
            merged.buckets.extend(histogram.buckets);
            for (key, value) in histogram.exact_counts {
                *merged.exact_counts.entry(key).or_default() += value;
            }
        }
        // TopN 溢出值写回直方图 exact_counts。
        for value in popped {
            *merged.exact_counts.entry(value.encoded).or_default() += value.count as f64;
        }
        // Histogram buckets are ordered by their value bounds in Go.  This
        // model does not carry bounds, so retain the source order rather than
        // sorting by row count (which changes the histogram's meaning).
        if merged.buckets.len() > options.bucket_count && options.bucket_count > 0 {
            merged.buckets.truncate(options.bucket_count);
        }
        for bucket in &mut merged.buckets {
            bucket.ndv = 0;
        }
        output.histograms[index] = Some(merged);
        output.cmsketches[index] = Some(cms);
        output.top_ns[index] = top;
        // 合并后 FMSketch 不再保留（NDV 已写入直方图）。
        output.fm_sketches[index] = None;
    }
    Ok(output)
}

/// 将全局统计按有直方图的项逐个写回；保留最后一次写失败错误。
pub fn write_global_stats(
    writer: &dyn GlobalStatsWriter,
    table_id: i64,
    stats: &GlobalStats,
) -> Result<(), String> {
    let mut last = None;
    for index in 0..stats.histograms.len() {
        if stats.histograms[index].is_none() {
            continue;
        }
        if let Err(error) = writer.save(table_id, index, stats) {
            last = Some(error);
        }
    }
    last.map_or(Ok(()), Err)
}
