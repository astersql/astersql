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

// 直方图核心行为单元测试：定位桶、行数估计、Proto 编解码、合并与驱逐状态。

use crate::*;

/// 构造含两桶、带空值计数的示例直方图。
fn histogram() -> Histogram {
    let field_type = types::NewFieldType(types::mysql::TypeLonglong);
    let mut histogram = NewHistogram(3, 6, 2, 9, &field_type, 2, 64);
    histogram.AppendBucketWithNDV(&types::NewIntDatum(1), &types::NewIntDatum(3), 3, 1, 3);
    histogram.AppendBucketWithNDV(&types::NewIntDatum(4), &types::NewIntDatum(6), 6, 1, 3);
    histogram
}

#[test]
/// 空直方图内存占用为 0；追加桶后应大于 0。
fn empty_histogram_has_no_tracking_memory() {
    let field_type = types::NewFieldType(types::mysql::TypeBlob);
    let mut empty = NewHistogram(1, 0, 0, 0, &field_type, 8, 0);
    assert_eq!(empty.MemoryUsage(), 0);

    empty.AppendBucket(
        &types::NewBytesDatum(vec![1]),
        &types::NewBytesDatum(vec![2]),
        1,
        1,
    );
    assert!(empty.MemoryUsage() > 0);
}

#[test]
/// 覆盖 LocateBucket / equalRowCount / LessRowCount / OutOfRange。
fn histogram_locates_and_estimates_rows() {
    let histogram = histogram();
    assert_eq!(
        histogram.LocateBucket(&types::NewIntDatum(3)),
        (false, 0, true, true)
    );
    assert_eq!(
        histogram.EqualRowCount(&types::NewIntDatum(3), true),
        (1.0, true)
    );
    assert_eq!(histogram.LessRowCount(&types::NewIntDatum(4)), 3.0);
    assert_eq!(histogram.TotalRowCount(), 8.0);
    assert!(histogram.OutOfRange(&types::NewIntDatum(8)));
}

#[test]
/// Proto 往返保持桶信息；MergeHistograms 累计计数与 NDV。
fn histogram_proto_and_merge_keep_cumulative_counts() {
    let source = histogram();
    let proto = HistogramToProto(&source);
    let decoded = HistogramFromProto(&proto);
    assert_eq!(decoded.NDV, source.NDV);
    assert_eq!(decoded.Buckets, source.Buckets);

    let field_type = types::NewFieldType(types::mysql::TypeLonglong);
    let mut right = NewHistogram(3, 2, 0, 9, &field_type, 1, 0);
    right.AppendBucketWithNDV(&types::NewIntDatum(7), &types::NewIntDatum(8), 2, 1, 2);
    let merged = MergeHistograms(source, right, 3, Version2);
    assert_eq!(merged.NotNullCount(), 8.0);
    assert_eq!(merged.NDV, 8);
}

#[test]
/// 全量加载与全部驱逐状态标志互斥且可查询。
fn loaded_status_matches_eviction_lifecycle() {
    let full = NewStatsFullLoadStatus();
    assert!(full.IsFullLoad());
    assert!(!full.IsLoadNeeded());
    let evicted = NewStatsAllEvictedStatus();
    assert!(evicted.IsAllEvicted());
    assert!(evicted.IsLoadNeeded());
}

// Go TestTruncateHistogram.
#[test]
/// TruncateHistogram 只保留前缀桶并相应截断边界。
fn truncate_histogram_keeps_requested_prefix() {
    let truncated = histogram().TruncateHistogram(1);
    assert_eq!(truncated.Len(), 1);
    assert_eq!(truncated.NotNullCount(), 3.0);
    assert_eq!(truncated.GetUpper(0).GetInt64(), 3);
}

// Go TestValueToString4InvalidKey.
#[test]
/// ValueToString 可处理任意字节 Datum。
fn value_to_string_handles_arbitrary_bytes() {
    let value = types::NewBytesDatum(vec![0xff, 0x00, 0x80]);
    assert!(ValueToString(&value).is_ok());
}

// Go TestMergePartitionLevelHist.
#[test]
/// 分区级直方图合并后非空行数求和且桶数受限。
fn merge_partition_level_histogram_keeps_total_count() {
    let partitions = vec![histogram(), histogram()];
    let global = MergePartitionHist2GlobalHist(&partitions, &[], 2, true, Version2)
        .unwrap()
        .unwrap();
    assert_eq!(global.NotNullCount(), 12.0);
    assert!(global.Len() <= 2);
}

// Go TestMergeBucketNDV.
#[test]
/// mergeBucketNDV：相同区间取 max NDV，不相交累加 disjoint，重叠按比例合并。
fn merge_bucket_ndv_covers_equal_disjoint_and_overlap() {
    let left = bucket4Merging {
        lower: types::NewIntDatum(1),
        upper: types::NewIntDatum(5),
        Bucket: Bucket {
            Count: 5,
            Repeat: 1,
            NDV: 5,
        },
        disjointNDV: 0,
    };
    let same = bucket4Merging {
        Bucket: Bucket {
            NDV: 7,
            ..left.Bucket
        },
        ..left.clone()
    };
    assert_eq!(mergeBucketNDV(&left, &same).unwrap().Bucket.NDV, 7);
    let disjoint = bucket4Merging {
        lower: types::NewIntDatum(6),
        upper: types::NewIntDatum(9),
        Bucket: Bucket {
            Count: 4,
            Repeat: 1,
            NDV: 4,
        },
        disjointNDV: 0,
    };
    assert_eq!(mergeBucketNDV(&left, &disjoint).unwrap().disjointNDV, 4);
    let overlap = bucket4Merging {
        lower: types::NewIntDatum(4),
        upper: types::NewIntDatum(8),
        Bucket: Bucket {
            Count: 5,
            Repeat: 1,
            NDV: 5,
        },
        disjointNDV: 0,
    };
    assert!(mergeBucketNDV(&left, &overlap).unwrap().Bucket.NDV >= 5);
}

// Go TestIndexQueryBytes.
#[test]
/// QueryBytes 优先 TopN，其次 CMSketch。
fn index_query_bytes_prefers_topn_then_cms() {
    let mut top_n = NewTopN(1);
    top_n.AppendTopN(b"hot".to_vec(), 20);
    top_n.Sort();
    let mut cms = NewCMSketch(5, 128);
    cms.InsertBytesByCount(b"cold", 3);
    let index = Index {
        CMSketch: Some(cms),
        TopN: Some(top_n),
        FMSketch: None,
        Info: None,
        Histogram: histogram(),
        StatsLoadedStatus: NewStatsFullLoadStatus(),
        PhysicalID: 1,
        StatsVer: Version2 as i64,
    };
    assert_eq!(index.QueryBytes(b"hot"), 20);
    assert_eq!(index.QueryBytes(b"cold"), 3);
}

// Go TestStandardizeForV2AnalyzeIndex.
#[test]
/// V2 索引直方图标准化：去掉空桶并将桶 NDV 清零。
fn standardize_v2_index_removes_empty_bucket_and_ndv() {
    let field_type = types::NewFieldType(types::mysql::TypeBlob);
    let mut histogram = NewHistogram(1, 2, 0, 1, &field_type, 2, 0);
    histogram.AppendBucketWithNDV(
        &types::NewBytesDatum(vec![1]),
        &types::NewBytesDatum(vec![1]),
        0,
        0,
        1,
    );
    histogram.AppendBucketWithNDV(
        &types::NewBytesDatum(vec![2]),
        &types::NewBytesDatum(vec![2]),
        3,
        3,
        1,
    );
    histogram.StandardizeForV2AnalyzeIndex();
    assert_eq!(histogram.Len(), 1);
    assert_eq!(histogram.Buckets[0].NDV, 0);
}

// Go TestNewPseudoHistogramReuseChunk.
#[test]
/// 伪直方图边界为空，不同 ID 互不影响。
fn pseudo_histograms_share_immutable_empty_semantics() {
    let field_type = types::NewFieldType(types::mysql::TypeLonglong);
    let first = NewPseudoHistogram(1, &field_type);
    let second = NewPseudoHistogram(2, &field_type);
    assert!(first.Bounds.is_empty());
    assert!(second.Bounds.is_empty());
    assert_ne!(first.ID, second.ID);
}

// Go GetIndexPrefixLens parses the complete encoded key; numCols is only a
// capacity hint and must not truncate a valid multi-column key.
#[test]
fn index_prefix_lens_does_not_truncate_to_capacity_hint() {
    let encoded = codec::EncodeKey(
        codec::time::UTC,
        Vec::new(),
        vec![types::NewIntDatum(1), types::NewIntDatum(2)],
    )
    .unwrap();

    let prefix_lens = GetIndexPrefixLens(&encoded, 1).unwrap();
    assert_eq!(prefix_lens.len(), 2);
    assert_eq!(prefix_lens.last().copied(), Some(encoded.len()));
}

// Go ExtractTopN selects boundary candidates from histogram estimates, then
// consults CMS only for the real count written to TopN.
#[test]
fn extract_topn_uses_histogram_frequency_for_candidate_selection() {
    let encoded =
        codec::EncodeKey(codec::time::UTC, Vec::new(), vec![types::NewIntDatum(7)]).unwrap();
    let field_type = types::NewFieldType(types::mysql::TypeBlob);
    let mut histogram = NewHistogram(1, 1, 0, 1, &field_type, 1, 0);
    histogram.AppendBucketWithNDV(
        &types::NewBytesDatum(encoded.clone()),
        &types::NewBytesDatum(encoded.clone()),
        100,
        100,
        1,
    );
    let mut cms = NewCMSketch(5, 128);
    let mut top_n = NewTopN(1);

    histogram.ExtractTopN(&mut cms, &mut top_n, 1, 1).unwrap();

    assert_eq!(top_n.TopN.len(), 1);
    assert_eq!(top_n.TopN[0].Encoded, encoded);
    assert_eq!(top_n.TopN[0].Count, 0);
}

#[test]
fn check_kind_only_uses_the_first_non_sentinel_value() {
    assert!(checkKind(
        &[
            types::Datum::default(),
            types::NewIntDatum(1),
            types::NewBytesDatum(vec![1]),
        ],
        types::KindInt64,
    ));
}

#[test]
fn histogram_equal_matches_go_string_contract() {
    let left = histogram();
    let mut right = left.clone();
    right.NullCount += 10;
    right.LastUpdateVersion += 1;
    right.Correlation = 0.75;

    assert!(HistogramEqual(&left, &right, false));
}

#[test]
fn out_of_range_skew_preserves_base_minimum() {
    let tp = types::NewFieldType(types::mysql::TypeLonglong);
    let mut h = NewHistogram(1, 100, 0, 0, &tp, 1, 0);
    h.AppendBucket(&types::NewIntDatum(10), &types::NewIntDatum(20), 1000, 1);
    assert_eq!(
        h.OutOfRangeRowCount(
            &types::NewIntDatum(20),
            &types::NewIntDatum(30),
            1100,
            100,
            100,
            true,
            0.5
        ),
        RowEstimate {
            Est: 62.5,
            MinEst: 25.0,
            MaxEst: 100.0
        }
    );
}

fn out_of_range_histogram(ndv: i64) -> Histogram {
    let tp = types::NewFieldType(types::mysql::TypeLonglong);
    let mut h = NewHistogram(1, ndv, 0, 0, &tp, 1, 0);
    h.AppendBucket(&types::NewIntDatum(10), &types::NewIntDatum(20), 1000, 1);
    h
}

#[test]
fn out_of_range_shape_geometry_and_cached_scaling() {
    let h = out_of_range_histogram(100);
    for (l, r, percent, maximum) in [
        (0, 10, 0.5, 1.0),
        (20, 30, 0.5, 1.0),
        (0, 30, 1.0, 1.0),
        (10, 20, 0.0, 0.0),
        (20, 25, 0.375, 0.75),
        (40, 50, 0.0, 0.0),
        (25, 25, 0.0, 0.0),
    ] {
        let lower = types::NewIntDatum(l);
        let upper = types::NewIntDatum(r);
        let shape = h.OutOfRangeShape(&lower, &upper, 100);
        assert_eq!(shape.TotalPercent, percent);
        assert_eq!(shape.MaxTotalPercent, maximum);
        assert_eq!(shape.OneValue, 10.0);
        for realtime in [0, 900, 1000, 1100, 2000] {
            for modify in [0, 100] {
                for skew in [0.0, 0.5, 1.0] {
                    assert_eq!(
                        h.ScaleOutOfRangeShape(shape, realtime, modify, true, skew),
                        h.OutOfRangeRowCount(&lower, &upper, realtime, modify, 100, true, skew)
                    );
                }
            }
        }
    }
    let shape = h.OutOfRangeShape(&types::NewIntDatum(20), &types::NewIntDatum(30), 100);
    assert_eq!(
        h.ScaleOutOfRangeShape(shape, 1100, 100, true, 0.0),
        RowEstimate {
            Est: 25.0,
            MinEst: 10.0,
            MaxEst: 100.0
        }
    );
    assert_eq!(
        h.ScaleOutOfRangeShape(shape, 900, 100, true, 0.0),
        RowEstimate {
            Est: 25.0,
            MinEst: 10.0,
            MaxEst: 100.0
        }
    );
    assert_eq!(
        h.ScaleOutOfRangeShape(shape, 1000, 0, true, 0.0),
        RowEstimate {
            Est: 10.0,
            MinEst: 0.0,
            MaxEst: 10.0
        }
    );
    assert_eq!(
        h.ScaleOutOfRangeShape(shape, 0, 0, true, 0.0),
        RowEstimate {
            Est: 250.0,
            MinEst: 10.0,
            MaxEst: 1000.0
        }
    );
    // Scaling only needs the cached geometry and histogram counts.
    let mut without_bounds = h.clone();
    without_bounds.Bounds.clear();
    assert_eq!(
        without_bounds
            .ScaleOutOfRangeShape(shape, 1100, 100, true, 0.0)
            .Est,
        25.0
    );
}

#[test]
fn out_of_range_shape_empty_impossible_and_determinate_order() {
    let mut h = out_of_range_histogram(1);
    h.Tp.SetFlag(types::mysql::UnsignedFlag);
    for (l, r) in [(-10, -1), (-10, 0), (20, 10)] {
        let shape = h.OutOfRangeShape(&types::NewIntDatum(l), &types::NewIntDatum(r), 0);
        assert!(shape.Impossible);
        assert_eq!(shape.HistNDV, 1);
        assert_eq!(
            h.ScaleOutOfRangeShape(shape, 100, 100, true, 0.0),
            DefaultRowEst(0.0)
        );
        assert_eq!(
            h.ScaleOutOfRangeShape(shape, 100, 100, false, 0.5),
            DefaultRowEst(1000.0)
        );
    }
    assert!(
        !h.OutOfRangeShape(&types::NewIntDatum(0), &types::NewIntDatum(0), 1)
            .Impossible
    );
    let tp = types::NewFieldType(types::mysql::TypeLonglong);
    let empty = NewHistogram(1, 0, 0, 0, &tp, 0, 0);
    let shape = empty.OutOfRangeShape(&types::NewIntDatum(0), &types::NewIntDatum(1), 0);
    assert!(shape.Empty);
    assert_eq!(
        empty.ScaleOutOfRangeShape(shape, 100, 100, false, 0.5),
        DefaultRowEst(0.0)
    );
}

#[test]
fn out_of_range_shape_low_ndv_and_degenerate_width() {
    let mut h = out_of_range_histogram(1);
    let point = h.OutOfRangeShape(&types::NewIntDatum(30), &types::NewIntDatum(30), -1);
    assert_eq!(
        h.ScaleOutOfRangeShape(point, 100, 0, true, 0.0),
        RowEstimate {
            Est: 1.0,
            MinEst: 1.0,
            MaxEst: 900.0
        }
    );
    assert_eq!(
        h.ScaleOutOfRangeShape(point, 100, 0, false, 0.0),
        DefaultRowEst(1000.0)
    );
    h.Bounds = vec![
        types::NewFloat64Datum(-f64::MAX),
        types::NewFloat64Datum(f64::MAX),
    ];
    let shape = h.OutOfRangeShape(
        &types::NewFloat64Datum(-1.0),
        &types::NewFloat64Datum(1.0),
        1,
    );
    assert_eq!(shape.TotalPercent, 0.0);
    h.Bounds = vec![types::NewIntDatum(20), types::NewIntDatum(10)];
    assert_eq!(
        h.OutOfRangeShape(&types::NewIntDatum(0), &types::NewIntDatum(30), 1)
            .TotalPercent,
        0.0
    );
}

#[test]
fn out_of_range_shape_removes_common_byte_prefix() {
    let tp = types::NewFieldType(types::mysql::TypeBlob);
    let mut h = NewHistogram(1, 100, 0, 0, &tp, 1, 0);
    let datum = |n| types::NewBytesDatum([b"long-common-prefix".as_slice(), &[n]].concat());
    h.AppendBucket(&datum(10), &datum(20), 1000, 1);
    let shape = h.OutOfRangeShape(&datum(20), &datum(30), 100);
    assert_eq!(shape.TotalPercent, 0.5);
    assert_eq!(shape.MaxTotalPercent, 1.0);
    assert_eq!(
        h.ScaleOutOfRangeShape(shape, 1100, 100, true, 0.0).Est,
        25.0
    );
}
