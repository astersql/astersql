// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// STDDEV_POP（总体标准差）聚合测试。

use crate::func_stddevpop::{stddev_population, stddev_population_distinct};
use crate::func_varpop::{DistinctVariance, VarianceState};

fn assert_float_eq(actual: Option<f64>, expected: Option<f64>) {
    match (actual, expected) {
        (Some(actual), Some(expected)) => {
            assert!(
                (actual - expected).abs() <= f64::EPSILON,
                "{actual} != {expected}"
            );
        }
        (actual, expected) => assert_eq!(actual, expected),
    }
}

/// 对应 Go `TestMergePartialResult4Stddevpop`：先累计 0..5，再合并 2..5。
#[test]
fn merge_partial_result_matches_go_fixture() {
    let mut destination = VarianceState::default();
    destination.update((0..5).map(|value| Some(value as f64)));
    assert_float_eq(stddev_population(&destination), Some(1.4142135623730951));

    let mut source = VarianceState::default();
    source.update((2..5).map(|value| Some(value as f64)));
    assert_float_eq(stddev_population(&source), Some(0.816496580927726));

    destination.merge(&source);
    assert_float_eq(stddev_population(&destination), Some(1.3169567191065923));
}

/// 对应 Go `TestStddevpop`：NULL 不参与累计，全 NULL 输入返回 NULL。
#[test]
fn stddev_population_skips_null_and_returns_null_for_empty_input() {
    let mut state = VarianceState::default();
    state.update([None, Some(0.0), Some(1.0), Some(2.0), Some(3.0), Some(4.0)]);
    assert_float_eq(stddev_population(&state), Some(1.4142135623730951));

    let mut empty = VarianceState::default();
    empty.update([None, None]);
    assert_eq!(stddev_population(&empty), None);
}

/// Go 的 original/partial DISTINCT 最终函数共享同一计算语义。
#[test]
fn distinct_stddev_deduplicates_across_partial_results() {
    let mut destination = DistinctVariance::default();
    destination.update([Some(1.0), None, Some(1.0)]);
    let mut source = DistinctVariance::default();
    source.update([Some(1.0), Some(3.0)]);
    destination.merge(&source);

    assert_eq!(stddev_population_distinct(&destination), Some(1.0));
    assert_eq!(
        stddev_population_distinct(&DistinctVariance::default()),
        None
    );
}

/// 总体标准差应为总体方差的平方根。
///
/// 输入 1..4：总体方差为 1.25，开方即 STDDEV_POP。
#[test]
fn population_stddev_is_square_root_of_population_variance() {
    let mut state = VarianceState::default();
    state.update([Some(1.0), Some(2.0), Some(3.0), Some(4.0)]);
    assert_eq!(stddev_population(&state), Some(1.25_f64.sqrt()));
}
