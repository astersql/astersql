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

// 采样相关单元测试：蓄水池采样、加权行采样、合并、NDV 与 protobuf 往返。

use crate::*;

/// 验证 SampleCollector 蓄水池容量与 ToProto/FromProto 往返一致性。
#[test]
fn sample_collector_reservoir_and_proto_round_trip() {
    let context = stmtctx::NewStmtCtx();
    let mut collector = SampleCollector::New(3, 128);
    collector.CMSketch = Some(NewCMSketch(5, 128));
    for value in 0..10 {
        collector
            .Collect(&context, types::NewBytesDatum(vec![value]))
            .unwrap();
    }
    assert_eq!(collector.Count, 10);
    assert_eq!(collector.Samples.len(), 3);
    assert!(collector.FMSketch.NDV() >= 1);
    let proto = SampleCollectorToProto(&collector);
    let decoded = SampleCollectorFromProto(&proto);
    assert_eq!(decoded.Count, collector.Count);
    assert_eq!(decoded.Samples.len(), collector.Samples.len());
    assert!(decoded.CMSketch.is_some());
}

/// 加权蓄水池应保留权重最高的若干行，且 Base 可与 FromProto 对齐。
#[test]
fn row_reservoir_keeps_largest_weights() {
    let mut collector = NewReservoirRowSampleCollector(2, 1, 128);
    collector.SampleRow(vec![types::NewIntDatum(1)], 1);
    collector.SampleRow(vec![types::NewIntDatum(2)], 5);
    collector.SampleRow(vec![types::NewIntDatum(3)], 3);
    let weights = collector
        .Base()
        .Samples
        .iter()
        .map(|sample| sample.Weight)
        .collect::<Vec<_>>();
    assert_eq!(weights, vec![3, 5]);
    let proto = collector.Base().ToProto();
    let decoded = baseCollector::FromProto(&proto);
    assert_eq!(decoded.Samples.len(), 2);
}

// Go TestWeightedSampling.
/// 对应 Go TestWeightedSampling：全局最高优先级权重应保留在样本中。
#[test]
fn weighted_sampling_keeps_highest_priority_rows() {
    let mut collector = NewReservoirRowSampleCollector(3, 1, 128);
    for weight in [8, 2, 9, 1, 7, 3] {
        collector.SampleRow(vec![types::NewIntDatum(weight)], weight);
    }
    assert_eq!(
        collector
            .Base()
            .Samples
            .iter()
            .map(|sample| sample.Weight)
            .collect::<Vec<_>>(),
        vec![7, 8, 9]
    );
}

// Go TestDistributedWeightedSampling.
/// 对应 Go TestDistributedWeightedSampling：两路合并后应等于全局 Top 权重。
#[test]
fn distributed_weighted_sampling_merge_matches_global_top_weights() {
    let mut left = NewReservoirRowSampleCollector(2, 1, 128);
    let mut right = NewReservoirRowSampleCollector(2, 1, 128);
    left.SampleRow(vec![types::NewIntDatum(1)], 1);
    left.SampleRow(vec![types::NewIntDatum(8)], 8);
    right.SampleRow(vec![types::NewIntDatum(4)], 4);
    right.SampleRow(vec![types::NewIntDatum(9)], 9);
    left.MergeCollector(&right);
    assert_eq!(
        left.Base()
            .Samples
            .iter()
            .map(|sample| sample.Weight)
            .collect::<Vec<_>>(),
        vec![8, 9]
    );
}

// Go TestBuildStatsOnRowSample.
/// 对应 Go TestBuildStatsOnRowSample：行采样构建应跟踪列与列组 FM Sketch。
#[test]
fn build_stats_on_row_sample_tracks_columns_and_groups() {
    let context = stmtctx::NewStmtCtx();
    let builder = RowSampleBuilder {
        ColGroups: vec![vec![0, 1]],
        MaxSampleSize: 4,
        SampleRate: 0.0,
        MaxFMSketchSize: 128,
    };
    let rows = (0..8)
        .map(|value| vec![types::NewIntDatum(value), types::NewIntDatum(value % 2)])
        .collect();
    let collector = builder.Collect(&context, rows).unwrap().unwrap();
    let base = match collector {
        RowSampleCollectorKind::Reservoir(value) => value.baseCollector,
        RowSampleCollectorKind::Bernoulli(value) => value.baseCollector,
    };
    assert_eq!(base.Count, 8);
    assert_eq!(base.FMSketches.len(), 3);
    assert_eq!(base.Samples.len(), 4);
}

// Go TestBuildSampleFullNDV.
/// 对应 Go TestBuildSampleFullNDV：样本未截断时 FM Sketch NDV 等于不同值个数。
#[test]
fn full_sample_ndv_equals_distinct_input() {
    let context = stmtctx::NewStmtCtx();
    let mut collector = SampleCollector::New(10, 128);
    for value in 0..10 {
        collector
            .Collect(&context, types::NewIntDatum(value))
            .unwrap();
    }
    assert_eq!(collector.Samples.len(), 10);
    assert_eq!(collector.FMSketch.NDV(), 10);
}

// Go TestSampleSerial.
/// 对应 Go TestSampleSerial：序列化应保留空值计数、非空计数与样本字节。
#[test]
fn sample_serial_preserves_null_count_size_and_values() {
    let context = stmtctx::NewStmtCtx();
    let mut collector = SampleCollector::New(4, 128);
    collector
        .Collect(&context, types::Datum::default())
        .unwrap();
    collector
        .Collect(&context, types::NewBytesDatum(vec![1, 2]))
        .unwrap();
    let decoded = SampleCollectorFromProto(&SampleCollectorToProto(&collector));
    assert_eq!(decoded.NullCount, 1);
    assert_eq!(decoded.Count, 1);
    assert_eq!(decoded.Samples[0].Value.GetBytes(), vec![1, 2]);
}

// Go SampleCollector.ExtractTopN.
/// ExtractTopN must move selected frequencies out of the CMSketch, while a zero limit is a no-op.
#[test]
fn extract_top_n_subtracts_selected_counts_and_zero_is_noop() {
    let mut collector = SampleCollector::New(8, 128);
    let mut cms = NewCMSketch(5, 128);
    for _ in 0..3 {
        cms.InsertBytes(b"frequent");
    }
    cms.InsertBytes(b"rare");
    collector.CMSketch = Some(cms);
    collector.Samples = vec![
        SampleItem {
            Value: types::NewBytesDatum(b"frequent".to_vec()),
            Handle: 0,
            Ordinal: 0,
        },
        SampleItem {
            Value: types::NewBytesDatum(b"frequent".to_vec()),
            Handle: 0,
            Ordinal: 1,
        },
        SampleItem {
            Value: types::NewBytesDatum(b"rare".to_vec()),
            Handle: 0,
            Ordinal: 2,
        },
    ];

    collector.ExtractTopN(1);
    assert_eq!(collector.TopN.as_ref().unwrap().TotalCount(), 3);
    assert_eq!(collector.CMSketch.as_ref().unwrap().TotalCount(), 1);

    let previous_top_n = collector.TopN.clone();
    collector.ExtractTopN(0);
    assert_eq!(
        collector.TopN.as_ref().unwrap().TotalCount(),
        previous_top_n.as_ref().unwrap().TotalCount()
    );
    assert_eq!(collector.CMSketch.as_ref().unwrap().TotalCount(), 1);
}

// Go SampleCollector.Destroy and SampleCollectorFromProto.
/// Lifecycle and decoded transient state must match Go: sketches are cleared and reservoir state is not synthesized.
#[test]
fn destroy_and_proto_restore_reset_transient_collector_state() {
    let context = stmtctx::NewStmtCtx();
    let mut collector = SampleCollector::New(2, 128);
    collector.Collect(&context, types::NewIntDatum(1)).unwrap();
    collector.Destroy();
    assert_eq!(collector.FMSketch.NDV(), 0);

    let mut proto = tipb::SampleCollector::new();
    proto.set_samples(protobuf::RepeatedField::from_vec(vec![vec![1], vec![2]]));
    let decoded = SampleCollectorFromProto(&proto);
    assert_eq!(decoded.MaxSampleSize, 0);
    assert!(decoded.Samples.iter().all(|sample| sample.Ordinal == 0));
}
