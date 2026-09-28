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

// STDDEV_SAMP（样本标准差）聚合测试。

/*
// 描述 stddev_samp 聚合的测试夹具和期望值，
// executor 依赖；aggTest、buildAggTester、testAggFunc 等调用均为 Go 语义占位。
// test_merge_partial_result4_stddevsamp 对应 Go 的 TestMergePartialResult4Stddevsamp。
// 与 stddev_pop 不同，这里验证的是样本标准差，partial 合并期望值沿用 Go 中的三个浮点常量。
#[test]
fn test_merge_partial_result4_stddevsamp() {
    let tests = vec![
        // 参数顺序保持 Go buildAggTester 调用，便于人工逐项对照原始测试。
        buildAggTester(
            ast::AggFuncStddevSamp,
            mysql::TypeDouble,
            0,
            5,
            1.5811388300841898,
            1.0,
            1.407885953173359,
        ),
    ];

    for test in tests {
        // Go 版本复用 testMergePartialResult(t, test)；这里保留聚合 partial merge 的校验入口。
        testMergePartialResult(test);
    }
}

// test_stddevsamp 对应 Go 的 TestStddevsamp。
// None 对应 Go 的 nil 初始 partial result，最终值仍是样本标准差 1.5811388300841898。
#[test]
fn test_stddevsamp() {
    let tests = vec![
        buildAggTester(
            ast::AggFuncStddevSamp,
            mysql::TypeDouble,
            0,
            5,
            None::<()>,
            1.5811388300841898,
        ),
    ];

    for test in tests {
        // testAggFunc 在 Go 中负责驱动输入行、更新 partial result 并断言最终输出。
        testAggFunc(test);
    }
}
*/

use crate::func_stddevsamp::{stddev_sample, stddev_sample_distinct};
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

/// 对应 Go `TestMergePartialResult4Stddevsamp`：先累计 0..5，再合并 2..5。
#[test]
fn merge_partial_result_matches_go_fixture() {
    let mut destination = VarianceState::default();
    destination.update((0..5).map(|value| Some(value as f64)));
    assert_float_eq(stddev_sample(&destination), Some(1.5811388300841898));

    let mut source = VarianceState::default();
    source.update((2..5).map(|value| Some(value as f64)));
    assert_float_eq(stddev_sample(&source), Some(1.0));

    destination.merge(&source);
    assert_float_eq(stddev_sample(&destination), Some(1.407885953173359));
}

/// 对应 Go `TestStddevsamp`：NULL 不参与累计，少于两个有效值返回 NULL。
#[test]
fn stddev_sample_skips_null_and_requires_two_values() {
    let mut state = VarianceState::default();
    state.update([None, Some(0.0), Some(1.0), Some(2.0), Some(3.0), Some(4.0)]);
    assert_float_eq(stddev_sample(&state), Some(1.5811388300841898));

    let mut singleton = VarianceState::default();
    singleton.update([None, Some(7.0), None]);
    assert_eq!(stddev_sample(&singleton), None);
    assert_eq!(stddev_sample(&VarianceState::default()), None);
}

/// Go 的 original/partial DISTINCT 最终函数共享同一计算语义。
#[test]
fn distinct_stddev_deduplicates_across_partial_results() {
    let mut destination = DistinctVariance::default();
    destination.update([Some(1.0), None, Some(1.0)]);
    let mut source = DistinctVariance::default();
    source.update([Some(1.0), Some(3.0)]);
    destination.merge(&source);

    assert_eq!(stddev_sample_distinct(&destination), Some(2.0_f64.sqrt()));
    assert_eq!(stddev_sample_distinct(&DistinctVariance::default()), None);
}

/// 样本标准差至少需要两行，且使用 N−1 作分母。
///
/// 单行 `[1.0]` → None；再加入 `3.0` 后方差为 `(1-2)²+(3-2)²)/(2-1)=2`，开方为 √2。
#[test]
fn sample_stddev_requires_two_rows_and_uses_n_minus_one() {
    let mut state = crate::func_varpop::VarianceState::default();
    state.update([Some(1.0)]);
    assert_eq!(stddev_sample(&state), None);
    state.update([Some(3.0)]);
    assert_eq!(stddev_sample(&state), Some(2.0_f64.sqrt()));
}
