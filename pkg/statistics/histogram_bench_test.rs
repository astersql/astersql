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

// 直方图查找稳定性与分区直方图合并为全局直方图的场景基准/契约测试。

use crate::*;

const HISTOGRAM_LEN: usize = 100;
const POPPED_TOP_N_LEN: usize = 100;
const EXPECTED_BUCKET_NUMBER: usize = 100;

/// Go `genBucket4TestData`/`genHist4Bench` 的确定性等价输入。
///
/// 原 benchmark 使用进程级随机数避免只测一种桶宽；契约测试使用递增桶宽，
/// 既保持连续、非重叠的编码键边界，也保证每次测试结果可复现。
fn histogram_for_merge(partition: usize) -> Histogram {
    let field_type = types::NewFieldType(types::mysql::TypeBlob);
    let mut histogram = NewHistogram(
        partition as i64,
        HISTOGRAM_LEN as i64,
        0,
        0,
        &field_type,
        HISTOGRAM_LEN,
        HISTOGRAM_LEN as i64,
    );
    let mut lower = (partition * 20_000) as i64;
    for bucket in 0..HISTOGRAM_LEN {
        let upper = lower + bucket as i64 + 1;
        let encoded_lower = codec::EncodeKey(
            codec::time::UTC,
            Vec::new(),
            vec![types::NewIntDatum(lower)],
        )
        .unwrap();
        let encoded_upper = codec::EncodeKey(
            codec::time::UTC,
            Vec::new(),
            vec![types::NewIntDatum(upper)],
        )
        .unwrap();
        histogram.AppendBucketWithNDV(
            &types::NewBytesDatum(encoded_lower),
            &types::NewBytesDatum(encoded_upper),
            (bucket + 1) as i64,
            1,
            (bucket + 2) as i64,
        );
        lower = upper + 1;
    }
    histogram
}

fn popped_top_n_for_merge() -> Vec<TopNMeta> {
    (0..POPPED_TOP_N_LEN)
        .map(|value| TopNMeta {
            Encoded: codec::EncodeKey(
                codec::time::UTC,
                Vec::new(),
                vec![types::NewIntDatum(value as i64 * 137)],
            )
            .unwrap(),
            Count: (value + 1) as u64,
        })
        .collect()
}

#[test]
/// 反复等值查找应稳定返回桶内 Repeat 计数。
fn repeated_histogram_lookup_is_stable() {
    let field_type = types::NewFieldType(types::mysql::TypeLonglong);
    let mut histogram = NewHistogram(1, 100, 0, 1, &field_type, 100, 0);
    for value in 0..100 {
        histogram.AppendBucketWithNDV(
            &types::NewIntDatum(value),
            &types::NewIntDatum(value),
            value + 1,
            1,
            1,
        );
    }
    for value in 0..100 {
        assert_eq!(
            histogram.EqualRowCount(&types::NewIntDatum(value), true),
            (1.0, true)
        );
    }
}

// Go benchmarkMergePartitionHist2GlobalHist / BenchmarkMergePartitionHist2GlobalHist.
#[test]
/// 覆盖 Go benchmark 的编码键直方图、弹出 TopN 与多分区规模变化。
fn benchmark_merge_partition_hist_to_global_scenario() {
    // Go 的 1K/10K/100K 是仅由 `go test -bench` 触发的性能规模；这里按相同
    // 小/中/大比例运行单元契约，避免常规 `cargo test` 构造一千万个桶。
    for partition_count in [1, 4, 16] {
        let histograms = (0..partition_count)
            .map(histogram_for_merge)
            .collect::<Vec<_>>();
        let popped = popped_top_n_for_merge();
        let global = MergePartitionHist2GlobalHist(
            &histograms,
            &popped,
            EXPECTED_BUCKET_NUMBER,
            true,
            Version2,
        )
        .unwrap()
        .unwrap();

        assert!(global.Len() <= EXPECTED_BUCKET_NUMBER);
        assert_eq!(
            global.NotNullCount(),
            (partition_count * HISTOGRAM_LEN + (1..=POPPED_TOP_N_LEN).sum::<usize>()) as f64
        );
        assert_eq!(global.TotColSize, (partition_count * HISTOGRAM_LEN) as i64);
    }
}
