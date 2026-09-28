// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// AVG 聚合函数单元测试。
//
// 块注释内保留从 Go 机械迁移的 merge / 执行 / 内存 / benchmark 草稿；
// 可执行部分覆盖 Float AVG 的 partial 合并、NULL 忽略，以及 DISTINCT AVG。

/*
// 描述 AVG 聚合函数的合并、执行、内存估算和 benchmark 用例。
// test_merge_partial_result_4_avg 对应 Go 的 TestMergePartialResult4Avg。
// Go 里逐个构造 aggTest 并交给 testMergePartialResult 校验两段 AVG partial result 的合并结果。
#[test]
fn test_merge_partial_result_4_avg() {
    let tests = vec![
        buildAggTester(
            ast::AggFuncAvg,
            mysql::TypeNewDecimal,
            0,
            5,
            2.0,
            3.0,
            2.375,
        ),
        buildAggTester(ast::AggFuncAvg, mysql::TypeDouble, 0, 5, 2.0, 3.0, 2.375),
    ];

    for test in tests {
        // Go 测试框架把 t 传入辅助函数；保留调用形状，测试上下文由后续接线补齐。
        testMergePartialResult(test);
    }
}

// test_avg 对应 Go 的 TestAvg，覆盖 decimal 与 double 两种 AVG 输入类型。
#[test]
fn test_avg() {
    let tests = vec![
        buildAggTester(
            ast::AggFuncAvg,
            mysql::TypeNewDecimal,
            0,
            5,
            None::<f64>,
            2.0,
        ),
        buildAggTester(ast::AggFuncAvg, mysql::TypeDouble, 0, 5, None::<f64>, 2.0),
    ];

    for test in tests {
        // nil 期望值在这里用 None 表达，表示 Go 用例里的空输入/空初始值语义。
        testAggFunc(test);
    }
}

// test_mem_avg 对应 Go 的 TestMemAvg，保留普通 AVG 与 DISTINCT AVG 的内存增量用例。
#[test]
fn test_mem_avg() {
    let tests = vec![
        buildAggMemTester(
            ast::AggFuncAvg,
            mysql::TypeNewDecimal,
            0,
            5,
            aggfuncs::DefPartialResult4AvgDecimalSize,
            defaultUpdateMemDeltaGens,
            false,
        ),
        buildAggMemTester(
            ast::AggFuncAvg,
            mysql::TypeNewDecimal,
            mysql::TypeNewDecimal,
            5,
            aggfuncs::DefPartialResult4AvgDistinctDecimalSize
                + hack::DefBucketMemoryUsageForSetString,
            distinctUpdateMemDeltaGens,
            true,
        ),
        buildAggMemTester(
            ast::AggFuncAvg,
            mysql::TypeDouble,
            0,
            5,
            aggfuncs::DefPartialResult4AvgFloat64Size,
            defaultUpdateMemDeltaGens,
            false,
        ),
        buildAggMemTester(
            ast::AggFuncAvg,
            mysql::TypeDouble,
            0,
            5,
            aggfuncs::DefPartialResult4AvgDistinctFloat64Size
                + hack::DefBucketMemoryUsageForSetFloat64,
            distinctUpdateMemDeltaGens,
            true,
        ),
    ];

    for test in tests {
        // 这里的 updateMemDeltaGens 是 Go 侧内存变化生成器，暂时只保留引用关系。
        testAggMemFunc(test);
    }
}

// benchmark_avg 对应 Go 的 BenchmarkAvg。
// Benchmark 在本计划中不接入 Rust bench harness，仅保留 mock context、rowNum 和用例顺序。
pub fn benchmark_avg() {
    let ctx = mock::NewContext();
    let row_num = 50_000;
    let tests = vec![
        buildAggTester(
            ast::AggFuncAvg,
            mysql::TypeNewDecimal,
            0,
            row_num,
            None::<f64>,
            2.0,
        ),
        buildAggTester(
            ast::AggFuncAvg,
            mysql::TypeDouble,
            0,
            row_num,
            None::<f64>,
            2.0,
        ),
    ];

    for test in tests {
        // Go 的 benchmarkAggFunc 接收 *testing.B；这里不执行性能测试，只记录原始调用意图。
        benchmarkAggFunc(&ctx, test);
    }
}
*/

use crate::func_avg::{DecimalAvg, DistinctFloatAvg, FloatAvg};
use crate::func_sum::Decimal;

/// 校验 Float AVG：合并 `(count, sum)` partial、跳过 NULL，以及 DISTINCT 去重平均。
#[test]
fn float_avg_merges_partials_and_ignores_nulls() {
    // (2,4.0)+(2,12.0) => sum=16, count=4 => avg=4.0；中间 None 被忽略。
    let mut avg = FloatAvg::default();
    avg.update_partial([Some((2, 4.0)), None, Some((2, 12.0))]);
    assert_eq!(avg.result(), Some(4.0));

    // DISTINCT：{1.0, 3.0} 平均为 2.0；重复 1.0 与 None 不计入。
    let mut distinct = DistinctFloatAvg::default();
    distinct.update([Some(1.0), Some(1.0), Some(3.0), None]);
    assert_eq!(distinct.result(), Some(2.0));
}

/// Go 的普通与 HighPrecision Float AVG 都按输入顺序直接执行 `sum += value`。
#[test]
fn float_avg_preserves_go_sequential_addition() {
    let mut avg = FloatAvg::default();
    avg.update([Some(1.0e16), Some(1.0), Some(-1.0e16)]);
    assert_eq!(avg.result(), Some(0.0));
}

/// Go partial 更新先累加 sum，再累加 count；零 count 不会丢弃非零 sum。
#[test]
fn float_avg_preserves_zero_count_partial_sum() {
    let mut avg = FloatAvg::default();
    avg.update_partial([Some((0, 5.0))]);
    avg.update([Some(1.0)]);
    assert_eq!(avg.partial_result(), (1, 6.0));
    assert_eq!(avg.result(), Some(6.0));
}

#[test]
fn decimal_avg_preserves_zero_count_partial_sum() {
    let mut avg = DecimalAvg::default();
    avg.update_partial([Some((0, Decimal::new(50, 1)))])
        .unwrap();
    avg.update([Some(Decimal::new(10, 1))]).unwrap();
    assert_eq!(avg.partial_result(), (1, Decimal::new(60, 1)));
    assert_eq!(avg.result(1).unwrap(), Some(Decimal::new(60, 1)));
}
