// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// cost crate 迁移期单元测试。
//
// 断言标量因子、小扫描阈值与聚合函数成本权重表与 Go 版条目一致。

use astersql_planner_core_cost::ast;
use astersql_planner_core_cost::factors_thresholds::{
    AggFuncFactor, DistinctFactor, SelectionFactor, SmallScanThreshold, ToleranceFactor,
};

/// 校验 Selection / Distinct / Tolerance / SmallScan 与 Go 常量数值一致。
#[test]
fn scalar_factors_and_threshold_match_go() {
    assert_eq!(SelectionFactor, 0.8);
    assert_eq!(DistinctFactor, 0.8);
    assert_eq!(ToleranceFactor, 0.00001);
    assert_eq!(SmallScanThreshold, 10_000);
}

/// 校验 `AggFuncFactor` 条目数量、各函数权重及未知键返回 None。
#[test]
fn aggregation_factors_match_go_entries() {
    let expected = [
        (ast::AggFuncCount, 1.0),
        (ast::AggFuncSum, 1.0),
        (ast::AggFuncSumInt, 1.0),
        (ast::AggFuncAvg, 2.0),
        (ast::AggFuncFirstRow, 0.1),
        (ast::AggFuncMax, 1.0),
        (ast::AggFuncMin, 1.0),
        (ast::AggFuncGroupConcat, 1.0),
        (ast::AggFuncBitOr, 0.9),
        (ast::AggFuncBitXor, 0.9),
        (ast::AggFuncBitAnd, 0.9),
        (ast::AggFuncVarPop, 3.0),
        (ast::AggFuncVarSamp, 3.0),
        (ast::AggFuncStddevPop, 3.0),
        (ast::AggFuncStddevSamp, 3.0),
        ("default", 1.5),
    ];

    assert_eq!(AggFuncFactor.len(), expected.len());
    for (name, factor) in expected {
        assert_eq!(AggFuncFactor.get(name), Some(&factor), "factor for {name}");
    }
    assert_eq!(AggFuncFactor.get("unknown_aggregate"), None);
}

#[test]
fn count_extrema_cost_matches_max_min() {
    for name in [crate::ast::AggFuncMaxCount, crate::ast::AggFuncMinCount] {
        assert_eq!(
            crate::factors_thresholds::AggFuncFactor.get(name),
            Some(&1.0)
        );
    }
}
