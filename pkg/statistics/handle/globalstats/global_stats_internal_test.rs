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

// 全局统计合并与写回逻辑的内部单元测试。
//
// 通过内存中的分区统计和记录型写入器，验证计数、草图与 TopN 的聚合结果，
// 以及缺失统计、协作取消和部分写入失败等边界语义。

use crate::{
    CmSketch, FmSketch, GlobalStats, GlobalStatsWriter, Histogram, MergeOptions,
    PartitionItemStats, PartitionStats, PartitionStatsProvider, TopN, TopNMeta,
    merge_partition_stats_to_global, write_global_stats,
};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;

#[derive(Clone)]
/// 为测试固定返回表 42 的分区统计，隔离真实存储访问。
struct FixtureProvider {
    partitions: Vec<PartitionStats>,
}

impl PartitionStatsProvider for FixtureProvider {
    fn partitions(&self, table_id: i64) -> Result<Vec<PartitionStats>, String> {
        if table_id != 42 {
            return Err(format!("unknown table {table_id}"));
        }
        Ok(self.partitions.clone())
    }
}

/// 构造同时包含直方图、CM/FM Sketch 和可选 TopN 的已分析统计项。
fn item(name: &[u8], top_count: u64, ndv: i64, exact: &[(&[u8], f64)]) -> PartitionItemStats {
    PartitionItemStats {
        histogram: Histogram {
            id: 7,
            ndv,
            buckets: vec![crate::Bucket { count: ndv, ndv: 1 }],
            exact_counts: exact
                .iter()
                .map(|(key, count)| (key.to_vec(), *count))
                .collect(),
        },
        cmsketch: CmSketch {
            counters: HashMap::from([(name.to_vec(), top_count)]),
        },
        top_n: TopN {
            values: if top_count == 0 {
                Vec::new()
            } else {
                vec![TopNMeta {
                    encoded: name.to_vec(),
                    count: top_count,
                }]
            },
        },
        fm_sketch: FmSketch { ndv },
        analyzed: true,
    }
}

fn fixture() -> FixtureProvider {
    // 两个分区刻意复用同一个 TopN 键，以检查合并时频次相加而非重复计数。
    FixtureProvider {
        partitions: vec![
            PartitionStats {
                name: "p0".into(),
                count: 8,
                modify_count: 2,
                items: vec![Some(item(b"a", 3, 5, &[(b"b", 1.0)]))],
            },
            PartitionStats {
                name: "p1".into(),
                count: 10,
                modify_count: 4,
                items: vec![Some(item(b"a", 4, 7, &[(b"b", 2.0)]))],
            },
        ],
    }
}

fn options(concurrency: usize, skip_missing: bool) -> MergeOptions {
    // 固定其余合并参数，使各用例只改变并发度和缺失统计策略。
    MergeOptions {
        top_n_size: 1,
        bucket_count: 8,
        version: 2,
        concurrency,
        skip_missing,
    }
}

#[test]
fn global_stats_merge_accumulates_partition_counts_once() {
    let result = merge_partition_stats_to_global(
        &fixture(),
        42,
        1,
        options(1, false),
        &AtomicBool::new(false),
    )
    .unwrap();
    // 表级计数来自分区求和；NDV 取合并后的 FM Sketch 结果，共享 TopN 键的频次为 3+4。
    assert_eq!((result.count, result.modify_count), (18, 6));
    assert_eq!(result.histograms[0].as_ref().unwrap().ndv, 7);
    assert_eq!(
        result.cmsketches[0].as_ref().unwrap().counters[b"a".as_slice()],
        7
    );
    assert_eq!(result.top_ns[0].as_ref().unwrap().total_count(), 7);
}

#[test]
fn global_stats_merge_reports_missing_partition_column_stats() {
    // 分区已有数据且标记为已分析，但直方图和 TopN 都为空时，属于列统计缺失。
    let provider = FixtureProvider {
        partitions: vec![PartitionStats {
            name: "p0".into(),
            count: 1,
            items: vec![Some(PartitionItemStats {
                analyzed: true,
                ..PartitionItemStats::default()
            })],
            ..PartitionStats::default()
        }],
    };
    let error = merge_partition_stats_to_global(
        &provider,
        42,
        1,
        options(1, false),
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(error.contains("partition column stats missing"));

    let result = merge_partition_stats_to_global(
        &provider,
        42,
        1,
        options(1, true),
        &AtomicBool::new(false),
    )
    .unwrap();
    // 跳过模式不终止合并，而是把缺失项留给调用方统一处理。
    assert_eq!(result.missing_partition_stats.len(), 1);
}

#[test]
fn global_stats_merge_honors_cancellation_before_loading() {
    // 预先取消必须在向 provider 拉取分区前就返回稳定的中断错误。
    let cancelled = AtomicBool::new(true);
    let error = merge_partition_stats_to_global(&fixture(), 42, 1, options(1, false), &cancelled)
        .unwrap_err();
    assert_eq!(error, "query interrupted");
}

/// 记录实际写入的统计项，并按下标注入写入错误。
struct RecordingWriter {
    errors: Vec<Option<String>>,
    calls: std::cell::RefCell<Vec<usize>>,
}

impl GlobalStatsWriter for RecordingWriter {
    fn save(&self, _table_id: i64, item_index: usize, _stats: &GlobalStats) -> Result<(), String> {
        self.calls.borrow_mut().push(item_index);
        self.errors
            .get(item_index)
            .cloned()
            .flatten()
            .map_or(Ok(()), Err)
    }
}

#[test]
fn write_global_stats_writes_present_items_and_returns_last_error() {
    let stats = GlobalStats {
        histograms: vec![Some(Histogram::default()), None, Some(Histogram::default())],
        ..GlobalStats::default()
    };
    let writer = RecordingWriter {
        errors: vec![Some("first".into()), None, Some("last".into())],
        calls: std::cell::RefCell::new(Vec::new()),
    };
    // 空直方图项不写回；其余项即使先失败也继续处理，最终返回最后一次错误。
    assert_eq!(write_global_stats(&writer, 42, &stats), Err("last".into()));
    assert_eq!(&*writer.calls.borrow(), &[0, 2]);
}
