// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

fn seed_base(
    collector: &mut dyn RowSampleCollector,
    count: i64,
    nulls: i64,
    size: i64,
    value: i64,
) {
    let context = stmtctx::NewStmtCtx();
    let base = collector.BaseMut();
    base.Count = count;
    base.NullCount[0] = nulls;
    base.TotalSizes[0] = size;
    base.FMSketches[0]
        .InsertValue(&context, types::NewIntDatum(value))
        .unwrap();
}

#[test]
fn fm_sketch_merge_preserves_row_sampler_state_and_memory_ownership() {
    let mut reservoir = NewReservoirRowSampleCollector(2, 1, 128);
    let mut reservoir_part = NewReservoirRowSampleCollector(2, 1, 128);
    seed_base(&mut reservoir, 2, 1, 4, 11);
    seed_base(&mut reservoir_part, 3, 2, 6, 22);
    reservoir.SampleRow(vec![types::NewIntDatum(1)], 1);
    reservoir.SampleRow(vec![types::NewIntDatum(9)], 9);
    reservoir_part.SampleRow(vec![types::NewIntDatum(5)], 5);
    reservoir_part.SampleRow(vec![types::NewIntDatum(7)], 7);
    let reservoir_memory_before = reservoir.Base().MemSize;
    let part_memory = reservoir_part.Base().MemSize;

    reservoir.MergeCollector(&reservoir_part);

    assert_eq!(reservoir.Base().Count, 5);
    assert_eq!(reservoir.Base().NullCount, vec![3]);
    assert_eq!(reservoir.Base().TotalSizes, vec![10]);
    assert_eq!(reservoir.Base().FMSketches[0].NDV(), 2);
    assert_eq!(
        reservoir
            .Base()
            .Samples
            .iter()
            .map(|sample| sample.Weight)
            .collect::<Vec<_>>(),
        vec![7, 9]
    );
    assert_eq!(
        reservoir.Base().MemSize,
        (reservoir_memory_before + part_memory) * 2 / 4
    );

    let mut bernoulli = NewBernoulliRowSampleCollector(1.0, 1, 128);
    let mut bernoulli_part = NewBernoulliRowSampleCollector(1.0, 1, 128);
    seed_base(&mut bernoulli, 4, 2, 8, 33);
    seed_base(&mut bernoulli_part, 5, 3, 10, 44);
    bernoulli.SampleRow(vec![types::NewIntDatum(3)], 3);
    bernoulli_part.SampleRow(vec![types::NewIntDatum(4)], 4);
    bernoulli.BaseMut().MemSize = 11;
    bernoulli_part.BaseMut().MemSize = 13;

    bernoulli.MergeCollector(&bernoulli_part);

    assert_eq!(bernoulli.Base().Count, 9);
    assert_eq!(bernoulli.Base().NullCount, vec![5]);
    assert_eq!(bernoulli.Base().TotalSizes, vec![18]);
    assert_eq!(bernoulli.Base().FMSketches[0].NDV(), 2);
    assert_eq!(bernoulli.Base().Samples.len(), 2);
    assert_eq!(bernoulli.Base().MemSize, 24);
}
