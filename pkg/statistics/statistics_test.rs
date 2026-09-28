// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 统计包通用单元测试：ANALYZE 表 ID、进度、直方图合并与 TopN 裁剪。

use crate::*;

/// 分区表应使用 PartitionID 作为统计 ID；Equals 比较含分区语义。
#[test]
fn analyze_table_id_selects_partition_and_compares_values() {
    let partition = AnalyzeTableID {
        TableID: 3,
        PartitionID: 7,
    };
    let same = AnalyzeTableID {
        TableID: 3,
        PartitionID: 7,
    };
    let table = AnalyzeTableID {
        TableID: 3,
        PartitionID: NonPartitionTableID,
    };
    assert_eq!(partition.GetStatisticsID(), 7);
    assert!(partition.IsPartitionTable());
    assert!(AnalyzeTableID::Equals(Some(&partition), Some(&same)));
    assert_eq!(table.GetStatisticsID(), 3);
    assert!(!AnalyzeTableID::Equals(Some(&partition), Some(&table)));
}

/// AnalyzeProgress 应累计增量行数，设置 dump 时间不重置 delta。
#[test]
fn analyze_progress_tracks_delta_and_persist_threshold() {
    let progress = AnalyzeProgress::default();
    progress.Update(10);
    progress.Update(15);
    assert_eq!(progress.GetDeltaCount(), 25);
    progress.SetLastDumpTime(std::time::SystemTime::now());
    assert_eq!(progress.GetDeltaCount(), 25);
}

// Go TestMergeHistogram.
fn mock_histogram(lower: i64, num: i64) -> Histogram {
    let field_type = types::NewFieldType(types::mysql::TypeLonglong);
    let mut histogram = NewHistogram(0, num, 0, 0, &field_type, num as usize, 0);
    for offset in 0..num {
        let value = types::NewIntDatum(lower + offset);
        histogram.AppendBucket(&value, &value, offset + 1, 1);
    }
    histogram
}

/// 对应 Go TestMergeHistogram：覆盖空左侧、相邻边界和共享边界的完整表驱动契约。
#[test]
fn merge_histogram_handles_shared_boundary_and_bucket_limit() {
    let cases = [
        (0, 0, 0, 1, 1, 1),
        (0, 200, 200, 200, 200, 400),
        (0, 200, 199, 200, 200, 399),
    ];
    for (left_lower, left_num, right_lower, right_num, bucket_num, ndv) in cases {
        let merged = MergeHistograms(
            mock_histogram(left_lower, left_num),
            mock_histogram(right_lower, right_num),
            256,
            Version2,
        );
        assert_eq!(merged.NDV, ndv);
        assert_eq!(merged.Len(), bucket_num);
        assert_eq!(merged.TotalRowCount(), (left_num + right_num) as f64);
        assert_eq!(merged.GetLower(0).GetInt64(), left_lower);
        assert_eq!(
            merged.GetUpper(merged.Len() - 1).GetInt64(),
            right_lower + right_num - 1
        );
    }
}

// Go TestPruneTopN.
fn top_n_with_counts(counts: &[u64]) -> Vec<TopNWithRange> {
    counts
        .iter()
        .enumerate()
        .map(|(index, &count)| TopNWithRange {
            TopNMeta: TopNMeta {
                Encoded: vec![index as u8],
                Count: count,
            },
            startIdx: 0,
            endIdx: 0,
        })
        .collect()
}

/// 对应 Go TestPruneTopN：完整覆盖保留高频项、大 NDV 与小表、裁掉低频值。
#[test]
fn prune_topn_removes_values_not_significantly_above_average() {
    let single = top_n_with_counts(&[100_000]);
    assert_eq!(
        pruneTopNItem(single.clone(), 2, 0, 100_010, 500_050),
        single
    );

    let mixed = top_n_with_counts(&[30_000, 30_000, 20_000, 20_000]);
    assert_eq!(
        pruneTopNItem(mixed.clone(), 5, 0, 100_000, 10_000_000),
        mixed
    );

    let ten_equal = top_n_with_counts(&[10_000; 10]);
    assert_eq!(
        pruneTopNItem(ten_equal.clone(), 100, 0, 100_000, 10_000_000),
        ten_equal
    );

    let small_table = top_n_with_counts(&[3_000, 3_000]);
    assert_eq!(
        pruneTopNItem(small_table.clone(), 4_002, 0, 10_000, 10_000),
        small_table
    );

    let retained = top_n_with_counts(&[90; 10]);
    let mut with_low_frequency = retained.clone();
    for encoded in 90_u8..150 {
        with_low_frequency.push(TopNWithRange {
            TopNMeta: TopNMeta {
                Encoded: vec![encoded],
                Count: 1,
            },
            startIdx: 0,
            endIdx: 0,
        });
    }
    assert_eq!(
        pruneTopNItem(with_low_frequency, 150, 0, 1_500, 1_500),
        retained
    );
}
