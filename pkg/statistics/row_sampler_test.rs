// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

use crate::*;

#[test]
fn collect_columns_does_not_own_row_count_and_excludes_flag_byte() {
    let context = stmtctx::NewStmtCtx();
    let mut collector = NewReservoirRowSampleCollector(1, 1, 128);

    collector
        .BaseMut()
        .CollectColumns(&context, &[types::NewBytesDatum(vec![1, 2, 3])])
        .unwrap();

    assert_eq!(collector.Base().Count, 0);
    assert_eq!(collector.Base().TotalSizes, vec![2]);
}

#[test]
fn builder_reuses_single_column_group_statistics() {
    let context = stmtctx::NewStmtCtx();
    let builder = RowSampleBuilder {
        ColGroups: vec![vec![0]],
        MaxSampleSize: 2,
        SampleRate: 0.0,
        MaxFMSketchSize: 128,
    };
    let rows = vec![
        vec![types::NewBytesDatum(vec![1, 2, 3])],
        vec![types::Datum::default()],
    ];

    let collector = builder.Collect(&context, rows).unwrap().unwrap();
    let base = match &collector {
        RowSampleCollectorKind::Reservoir(value) => value.Base(),
        RowSampleCollectorKind::Bernoulli(value) => value.Base(),
    };
    assert_eq!(base.Count, 2);
    assert_eq!(base.NullCount, vec![1, 1]);
    assert_eq!(base.TotalSizes, vec![2, 2]);
    assert_eq!(
        base.FMSketches[0].hash_values(),
        base.FMSketches[1].hash_values()
    );
}

#[test]
fn row_samples_proto_encodes_null_with_nil_flag() {
    let samples = WeightedRowSampleHeap(vec![ReservoirRowSampleItem {
        Handle: 0,
        Columns: vec![types::Datum::default()],
        Weight: 7,
    }]);

    let proto = RowSamplesToProto(&samples);
    assert_eq!(proto[0].get_row()[0], vec![codec::NilFlag]);
}

#[test]
fn proto_restore_accounts_for_sample_memory() {
    let mut collector = NewReservoirRowSampleCollector(1, 1, 128);
    collector.SampleRow(vec![types::NewBytesDatum(vec![1, 2, 3])], 7);

    let restored = baseCollector::FromProto(&collector.Base().ToProto());
    assert_eq!(restored.MemSize, restored.Samples[0].MemUsage());
}

#[test]
fn destroy_only_releases_fm_sketches() {
    let mut collector = NewReservoirRowSampleCollector(1, 1, 128);
    collector.BaseMut().Count = 4;
    collector.SampleRow(vec![types::NewBytesDatum(vec![1])], 7);

    collector.DestroyAndPutToPool();

    assert_eq!(collector.Base().Count, 4);
    assert_eq!(collector.Base().Samples.len(), 1);
    assert!(collector.Base().FMSketches.is_empty());
}
