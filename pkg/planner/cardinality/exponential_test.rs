// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// `ApplyExponentialBackoff` 的单元测试：覆盖 NDV、选择率与边界裁剪场景，
// 并确认超过 `MaxExponentialBackoffCols` 的输入列被忽略。

// testExponentialBackoffHelper 对应 Go 的测试辅助函数。
// 它验证返回值接近期望值，并同时检查结果没有越过调用方传入的上下界。
fn test_exponential_backoff_helper(
    name: &str,
    values: Vec<f64>,
    lower_bound: f64,
    upper_bound: f64,
    expected_result: f64,
    tolerance: f64,
) {
    let result = ApplyExponentialBackoff(&values, lower_bound, upper_bound);
    assert!(
        (result - expected_result).abs() <= tolerance,
        "Test case: {}; expected {}, got {}",
        name,
        expected_result,
        result
    );
    assert!(
        result >= lower_bound,
        "Result should respect lower bound for: {}",
        name
    );
    assert!(
        result <= upper_bound,
        "Result should respect upper bound for: {}",
        name
    );
}

// TestApplyExponentialBackoff 对应 Go 的同名测试，覆盖 NDV、选择率和边界裁剪三组场景。
use crate::ApplyExponentialBackoff;

#[test]
fn test_apply_exponential_backoff() {
    crate::main_test::setup_for_cardinality_test();
    // NDV 场景：输入值大于 1，指数退避按 v0 * sqrt(v1) * sqrt(sqrt(v2)) ... 组合。
    test_exponential_backoff_helper("Single NDV", vec![100.0], 10.0, 10000.0, 100.0, 0.1);

    let expected2 = 1000.0 * 500.0_f64.sqrt();
    test_exponential_backoff_helper(
        "Two NDVs",
        vec![1000.0, 500.0],
        100.0,
        100000.0,
        expected2,
        0.1,
    );

    let expected3 = 1000.0 * 500.0_f64.sqrt() * 100.0_f64.sqrt().sqrt();
    test_exponential_backoff_helper(
        "Three NDVs",
        vec![1000.0, 500.0, 100.0],
        100.0,
        100000.0,
        expected3,
        0.1,
    );

    let expected4 =
        1000.0 * 500.0_f64.sqrt() * 100.0_f64.sqrt().sqrt() * 10.0_f64.sqrt().sqrt().sqrt();
    test_exponential_backoff_helper(
        "Four NDVs",
        vec![1000.0, 500.0, 100.0, 10.0],
        10.0,
        100000.0,
        expected4,
        0.1,
    );
    // Go 测试确认第五个值被 MaxExponentialBackoffCols 上限忽略。
    test_exponential_backoff_helper(
        "Five NDVs (cap at 4)",
        vec![1000.0, 500.0, 100.0, 10.0, 5.0],
        5.0,
        100000.0,
        expected4,
        0.1,
    );

    // 选择率场景：小于 1 的值同样应用指数退避，但合法边界为 [lower, 1]。
    test_exponential_backoff_helper("Single selectivity", vec![0.1], 0.001, 1.0, 0.1, 0.001);

    let expected2 = 0.01 * 0.02_f64.sqrt();
    test_exponential_backoff_helper(
        "Two selectivities",
        vec![0.01, 0.02],
        0.001,
        1.0,
        expected2,
        0.001,
    );

    let expected3 = 0.01 * 0.02_f64.sqrt() * 0.05_f64.sqrt().sqrt();
    test_exponential_backoff_helper(
        "Three selectivities",
        vec![0.01, 0.02, 0.05],
        0.001,
        1.0,
        expected3,
        0.001,
    );

    let expected4 = 0.01 * 0.02_f64.sqrt() * 0.05_f64.sqrt().sqrt() * 0.1_f64.sqrt().sqrt().sqrt();
    test_exponential_backoff_helper(
        "Four selectivities",
        vec![0.01, 0.02, 0.05, 0.1],
        0.001,
        1.0,
        expected4,
        0.001,
    );

    // 边界裁剪场景：下溢时返回 lower bound，上溢时返回 upper bound，空输入返回 lower bound。
    test_exponential_backoff_helper(
        "Below lower bound",
        vec![0.001, 0.0005],
        0.01,
        1.0,
        0.01,
        0.001,
    );
    test_exponential_backoff_helper("Above upper bound", vec![100.0, 50.0], 1.0, 10.0, 10.0, 0.1);
    test_exponential_backoff_helper("Empty input", vec![], 5.0, 100.0, 5.0, 0.1);
}
