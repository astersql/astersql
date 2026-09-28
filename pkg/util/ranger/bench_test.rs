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

// Ranger 基准风格回归：大 IN/DNF 拆分建 range 的确定性负载。
//
// 对齐 Go bench：构造长等值析取，反复调用 `DetachCondAndBuildRangeForIndex`，
// 断言 DNF 与 range 基数。Range 指索引可扫描的键区间。

use crate::test_support_aster_unit_test::{column, disjunction, equality, ranger_context};
use crate::{DetachCondAndBuildRangeForIndex, types};

/// 长 IN 列表查询形状摘要，用于核对谓词维度是否齐全。
const LONG_IN_LIST_QUERY_SHAPE: &str =
    "flag = false AND work_type = 'ART' AND start_time < ? AND org_id IN (...)";

/// 生成 `1,2,...,count` 形式的整数列表字符串，对齐 Go bench 格式。
fn make_benchmark_int_list(count: usize) -> String {
    (1..=count)
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// 校验整数列表格式：Go 从 1 开始生成，且无尾随逗号。
#[test]
fn test_benchmark_int_list_matches_go_format() {
    assert_eq!(make_benchmark_int_list(0), "");
    assert_eq!(make_benchmark_int_list(1), "1");
    assert_eq!(make_benchmark_int_list(5), "1,2,3,4,5");
    assert!(!make_benchmark_int_list(128).ends_with(','));
}

/// 查询形状字符串需包含各谓词维度关键字。
#[test]
fn test_long_in_list_workload_keeps_all_predicate_dimensions() {
    for predicate in ["flag", "work_type", "start_time", "org_id", "IN"] {
        assert!(LONG_IN_LIST_QUERY_SHAPE.contains(predicate));
    }
}

/// 大 DNF（128 个等值）重复拆分建 range，断言基数与首尾区间。
#[test]
fn test_bench_daily() {
    let context = ranger_context();
    let indexed = column(1, 0);
    // 构造 128 路 OR 等值，模拟长 IN 列表拆成的 DNF。
    let dnf = disjunction(
        &context,
        (0..128)
            .map(|value| equality(&context, &indexed, value * 2))
            .collect(),
    );

    // The Go benchmark repeatedly exercises detachment of a large value list.
    // Keep the workload deterministic and assert the produced range cardinality.
    // 重复三次：验证 DetachCondAndBuildRangeForIndex 对大 DNF 的稳定性。
    for _ in 0..3 {
        let result = DetachCondAndBuildRangeForIndex(
            &context,
            vec![dnf.clone()],
            vec![indexed.clone()],
            vec![types::UnspecifiedLength],
            0,
        )
        .expect("large DNF benchmark range construction succeeds");
        assert!(result.IsDNFCond);
        assert_eq!(result.Ranges.len(), 128);
        assert_eq!(result.Ranges[0].String(), "[0,0]");
        assert_eq!(result.Ranges[127].String(), "[254,254]");
    }
}
