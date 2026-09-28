// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// `estimate.rs` 中 NDV / 全局 singleton 估算的单元测试。
//
// 覆盖 GEE 公式的上下界夹紧，以及多分区 FMSketch 合并时对跨分区重复值的处理。

use crate::*;

fn sketches_from_samples(max_size: usize, samples: &[i64]) -> (FMSketch, FMSketch) {
    use std::collections::HashMap;

    let context = stmtctx::NewStmtCtx();
    let mut ndv = NewFMSketch(max_size);
    let mut singleton = NewFMSketch(max_size);
    let mut counts = HashMap::new();
    for &value in samples {
        *counts.entry(value).or_insert(0_u64) += 1;
        ndv.InsertValue(&context, types::NewIntDatum(value))
            .unwrap();
    }
    for (&value, &count) in &counts {
        if count == 1 {
            singleton
                .InsertValue(&context, types::NewIntDatum(value))
                .unwrap();
        }
    }
    (ndv, singleton)
}

/// 验证 EstimateNDVByGEE：GEE 修正、四舍五入及上下界与 Go 表格用例一致。
#[test]
fn gee_estimate_is_clamped_to_sample_ndv_and_rows() {
    assert_eq!(EstimateNDVByGEE(5, 0, 10, 100), 5);
    assert_eq!(EstimateNDVByGEE(5, 3, 10, 100), 11);
    assert_eq!(EstimateNDVByGEE(10, 3, 20, 80), 13);
    assert_eq!(EstimateNDVByGEE(10, 7, 20, 45), 14);
    assert_eq!(EstimateNDVByGEE(10, 7, 20, 10), 10);
    assert_eq!(EstimateNDVByGEE(100, 100, 100, 100), 100);
}

#[test]
#[should_panic(expected = "sampleSize should be greater than 0")]
fn gee_estimate_rejects_zero_sample_size() {
    EstimateNDVByGEE(1, 1, 0, 1);
}

#[test]
#[should_panic(expected = "sampleNDV should be greater than 0")]
fn gee_estimate_rejects_zero_sample_ndv() {
    EstimateNDVByGEE(0, 0, 1, 1);
}

#[test]
#[should_panic(expected = "rowCount should be greater than or equal to sampleNDV")]
fn gee_estimate_rejects_row_count_below_sample_ndv() {
    EstimateNDVByGEE(10, 3, 20, 9);
}

#[test]
fn calculate_estimate_ndv_matches_go_special_cases() {
    let all_singletons = topNHelper {
        sorted: (0..4)
            .map(|value| dataCnt {
                data: vec![value],
                cnt: 1,
            })
            .collect(),
        sampleSize: 4,
        singletonItems: 4,
        sumTopN: 0,
        actualNumTop: 0,
    };
    assert_eq!(calculateEstimateNDV(&all_singletons, 40), (40, 1));

    let no_singletons = topNHelper {
        sorted: vec![
            dataCnt {
                data: vec![1],
                cnt: 2,
            },
            dataCnt {
                data: vec![2],
                cnt: 2,
            },
        ],
        sampleSize: 4,
        singletonItems: 0,
        sumTopN: 0,
        actualNumTop: 0,
    };
    assert_eq!(calculateEstimateNDV(&no_singletons, 40), (2, 10));
}

/// 验证 EstimateGlobalSingletonBySketches：分区 A={1,2}、B={2,3}，
/// singleton 分别为 {1} 与 {3} 时，全局 singleton 为 2（值 2 跨分区重复，不计入）。
#[test]
fn singleton_sketch_estimate_handles_cross_partition_duplicates() {
    let context = stmtctx::NewStmtCtx();
    let mut ndv_a = NewFMSketch(128);
    let mut ndv_b = NewFMSketch(128);
    let mut singleton_a = NewFMSketch(128);
    let mut singleton_b = NewFMSketch(128);
    for value in [1, 2] {
        ndv_a
            .InsertValue(&context, types::NewIntDatum(value))
            .unwrap();
    }
    for value in [2, 3] {
        ndv_b
            .InsertValue(&context, types::NewIntDatum(value))
            .unwrap();
    }
    singleton_a
        .InsertValue(&context, types::NewIntDatum(1))
        .unwrap();
    singleton_b
        .InsertValue(&context, types::NewIntDatum(3))
        .unwrap();
    let estimate =
        EstimateGlobalSingletonBySketches(&[&ndv_a, &ndv_b], &[&singleton_a, &singleton_b]);
    assert_eq!(estimate, 2);
}

#[test]
fn singleton_sketch_estimate_matches_go_table_cases() {
    let (ndv_a, singleton_a) = sketches_from_samples(1_000, &[1, 2, 3]);
    assert_eq!(
        EstimateGlobalSingletonBySketches(&[&ndv_a], &[&singleton_a]),
        3
    );

    let (ndv_a, singleton_a) = sketches_from_samples(1_000, &[1, 2]);
    let (ndv_b, singleton_b) = sketches_from_samples(1_000, &[3, 4]);
    let (ndv_c, singleton_c) = sketches_from_samples(1_000, &[5, 6]);
    assert_eq!(
        EstimateGlobalSingletonBySketches(
            &[&ndv_a, &ndv_b, &ndv_c],
            &[&singleton_a, &singleton_b, &singleton_c],
        ),
        6
    );

    let (ndv_a, singleton_a) = sketches_from_samples(1_000, &[1, 2]);
    let (ndv_b, singleton_b) = sketches_from_samples(1_000, &[1, 2]);
    assert_eq!(
        EstimateGlobalSingletonBySketches(&[&ndv_a, &ndv_b], &[&singleton_a, &singleton_b]),
        0
    );

    let (ndv_a, singleton_a) = sketches_from_samples(3, &[0]);
    let (ndv_b, singleton_b) = sketches_from_samples(3, &[0, 0, 0, 1, 1, 4, 7]);
    assert_eq!(
        EstimateGlobalSingletonBySketches(&[&ndv_a, &ndv_b], &[&singleton_a, &singleton_b]),
        2
    );
}

#[test]
#[should_panic(expected = "ndvSketches shouldn't be empty")]
fn singleton_sketch_estimate_rejects_empty_input() {
    EstimateGlobalSingletonBySketches(&[], &[]);
}

#[test]
#[should_panic(expected = "sketch lengths must match")]
fn singleton_sketch_estimate_rejects_mismatched_lengths() {
    let (ndv, singleton) = sketches_from_samples(1_000, &[1]);
    EstimateGlobalSingletonBySketches(&[&ndv], &[&singleton, &singleton]);
}
