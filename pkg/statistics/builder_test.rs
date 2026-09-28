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

// 直方图/TopN 构建器单元测试：SortedBuilder、SequentialRangeChecker 与 BuildHistAndTopN。
//
// 直方图把有序值域切成桶以估计分布；TopN 单独记录高频值，避免桶内被少数热点污染。

use crate::*;

#[test]
/// SortedBuilder 合并桶时不得把相同取值拆进不同桶。
fn sorted_builder_merges_buckets_without_splitting_duplicates() {
    let field_type = types::NewFieldType(types::mysql::TypeLonglong);
    let mut builder = NewSortedBuilder(2, 7, &field_type, Version2);
    for value in [1, 1, 2, 3, 4, 5] {
        builder.Iterate(types::NewIntDatum(value)).unwrap();
    }
    let histogram = builder.IntoHist();
    assert!(histogram.Len() <= 2);
    assert_eq!(histogram.TotalRowCount(), 6.0);
    assert_eq!(histogram.NDV, 5);
}

#[test]
/// SequentialRangeChecker 按序推进，判断采样下标是否落在某个 TopN 区间内。
fn sequential_range_checker_advances_once() {
    let ranges = vec![
        TopNWithRange {
            TopNMeta: TopNMeta {
                Encoded: vec![2],
                Count: 2,
            },
            startIdx: 2,
            endIdx: 4,
        },
        TopNWithRange {
            TopNMeta: TopNMeta {
                Encoded: vec![8],
                Count: 1,
            },
            startIdx: 8,
            endIdx: 8,
        },
    ];
    let mut checker = NewSequentialRangeChecker(ranges);
    assert!(!checker.IsIndexInTopNRange(1));
    assert!(checker.IsIndexInTopNRange(2));
    assert!(checker.IsIndexInTopNRange(4));
    assert!(!checker.IsIndexInTopNRange(5));
    assert!(checker.IsIndexInTopNRange(8));
}

// Go BenchmarkBuildHistAndTopN.
/// 对应 Go BenchmarkBuildHistAndTopN 的场景断言。
#[test]
fn benchmark_build_hist_and_top_n_scenario() {
    let context = stmtctx::NewStmtCtx();
    let mut collector = SampleCollector::New(1_000, 1_024);
    for value in 0_i64..1_000 {
        collector
            .Collect(&context, types::NewBytesDatum(value.to_be_bytes().to_vec()))
            .unwrap();
    }
    let field_type = types::NewFieldType(types::mysql::TypeBlob);
    let (histogram, top_n) =
        BuildHistAndTopN(&context, 64, 20, 1, &mut collector, &field_type, false).unwrap();
    assert!(histogram.Len() <= 64);
    assert!(top_n.Num() <= 20);
}

// Go BenchmarkBuildHistAndTopNWithLowNDV.
/// 对应 Go BenchmarkBuildHistAndTopNWithLowNDV：低 NDV 场景下 TopN 条数受限。
#[test]
fn benchmark_build_hist_and_top_n_with_low_ndv_scenario() {
    let context = stmtctx::NewStmtCtx();
    let mut collector = SampleCollector::New(500, 128);
    for value in 0..500 {
        collector
            .Collect(&context, types::NewBytesDatum(vec![(value % 5) as u8]))
            .unwrap();
    }
    let field_type = types::NewFieldType(types::mysql::TypeBlob);
    let (histogram, top_n) = BuildHistAndTopN(
        &context,
        DefaultHistogramBuckets,
        DefaultTopNValue,
        1,
        &mut collector,
        &field_type,
        false,
    )
    .unwrap();
    assert!(histogram.Len() <= DefaultHistogramBuckets);
    assert!(top_n.Num() <= 5);
}

#[test]
/// Go parity: sampled data must leave a histogram value when the estimated NDV
/// is larger than the NDV represented by the sample.
fn sampled_top_n_does_not_consume_every_sampled_value() {
    let context = stmtctx::NewStmtCtx();
    let mut collector = SampleCollector::New(100, 16);
    for value in [1_i64, 2] {
        for _ in 0..5 {
            collector
                .Collect(&context, types::NewIntDatum(value))
                .unwrap();
        }
    }
    for value in 3_i64..=10 {
        collector
            .FMSketch
            .InsertValue(&context, types::NewIntDatum(value))
            .unwrap();
    }
    collector.Count = 100;

    let field_type = types::NewFieldType(types::mysql::TypeLonglong);
    let (histogram, top_n) = BuildHistAndTopN(
        &context,
        DefaultHistogramBuckets,
        DefaultTopNValue,
        1,
        &mut collector,
        &field_type,
        true,
    )
    .unwrap();

    assert_eq!(top_n.Num(), 1);
    assert!(histogram.Len() > 0);
}
