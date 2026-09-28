// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 物理计划 casetest 的 suite 级归一化冒烟测试。
//
// 对应 Go physicalplantest 中依赖稳定 SQL 归一化的前置假设：字面量不同但结构相同的
// SQL 应得到相同的规范化文本与 plan digest（计划指纹，用于 plan cache / 绑定匹配）。

use std::path::Path;

use astersql_testkit::testdata::{LoadTestSuiteDataWithCascades, TestData};

/// 但这些常驻后台任务仍是该 casetest 的生命周期契约的一部分。

/// Go `physicalplantest` 的完整计划套件名称；顺序与 `plan_suite_in.json` 一致。
pub(crate) const PLAN_SUITE_CASES: [&str; 35] = [
    "TestMPPHints",
    "TestMPPHintsScope",
    "TestMPPBCJModel",
    "TestMPPPreferBCJ",
    "TestMPPBCJModelOneTiFlash",
    "TestMPPRightSemiJoin",
    "TestMPPRightOuterJoin",
    "TestIssue37520",
    "TestHintScope",
    "TestIndexHint",
    "TestIndexMergeHint",
    "TestRefine",
    "TestAggEliminator",
    "TestRuleColumnPruningLogicalApply",
    "TestUnmatchedTableInHint",
    "TestJoinHints",
    "TestAggregationHints",
    "TestQueryBlockHint",
    "TestSemiJoinToInner",
    "TestIndexJoinHint",
    "TestAggToCopHint",
    "TestGroupConcatOrderby",
    "TestInlineProjection",
    "TestHintFromDiffDatabase",
    "TestMPPSinglePartitionType",
    "TestSemiJoinRewriteHints",
    "TestHJBuildAndProbeHint4DynamicPartitionTable",
    "TestHJBuildAndProbeHint4TiFlash",
    "TestCountStarForTiFlash",
    "TestHashAggPushdownToTiFlashCompute",
    "TestIssues49377Plan",
    "TestPointgetIndexChoosen",
    "TestAlwaysTruePredicateWithSubquery",
    "TestExplainExpand",
    "TestLimitPushdown",
];

fn testdata_directory() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata")
}

/// 对应 Go 的 `testDataMap["plan_suite"]`，每次测试独立加载避免共享可变录制状态。
pub(crate) fn get_plan_suite_data() -> TestData {
    LoadTestSuiteDataWithCascades(
        testdata_directory()
            .to_str()
            .expect("physicalplantest testdata path must be UTF-8"),
        "plan_suite",
        true,
    )
    .unwrap_or_else(|error| panic!("load plan_suite: {error}"))
}

fn assert_suite_case_shape(suite: &TestData, name: &str, cascades: bool) -> usize {
    let (input, output) = suite
        .LoadTestCasesByName(name, cascades)
        .unwrap_or_else(|error| panic!("load {name} ({cascades}): {error}"));
    let input = input
        .as_array()
        .unwrap_or_else(|| panic!("{name} input must be an array"));
    let output = output
        .as_array()
        .unwrap_or_else(|| panic!("{name} output must be an array"));
    assert_eq!(input.len(), output.len(), "{name} input/output length");

    for (index, (input_case, expected)) in input.iter().zip(output).enumerate() {
        let sql = input_case
            .as_str()
            .or_else(|| input_case.get("SQL").and_then(|value| value.as_str()))
            .unwrap_or_else(|| panic!("{name}[{index}] input SQL must be text"));
        let expected_sql = expected
            .get("SQL")
            .and_then(|value| value.as_str())
            .unwrap_or_else(|| panic!("{name}[{index}] output SQL must be text"));
        assert_eq!(sql, expected_sql, "{name}[{index}] SQL drift");

        // Go records a plan as either []string or a compact string depending on
        // the test. Validate both shapes, while allowing UPDATE/SET cases whose
        // recorded Plan is null.
        if let Some(plan) = expected.get("Plan") {
            if !plan.is_null() {
                assert!(
                    plan.is_array() || plan.is_string(),
                    "{name}[{index}] Plan must be []string, string, or null"
                );
            }
        }
    }
    input.len()
}

#[test]
fn test_main_loads_go_testmain_state() {
    astersql_testkit_testsetup::SetupForCommonTest();

    let suite = get_plan_suite_data();
    let mut total = 0;
    for name in PLAN_SUITE_CASES {
        total += assert_suite_case_shape(&suite, name, false);
        assert_eq!(
            assert_suite_case_shape(&suite, name, true),
            assert_suite_case_shape(&suite, name, false),
            "{name} standard/cascades input count"
        );
    }
    assert_eq!(PLAN_SUITE_CASES.len(), 35);
    assert_eq!(total, 378, "plan_suite must retain every Go golden input");

    let cascades_template = LoadTestSuiteDataWithCascades(
        testdata_directory()
            .to_str()
            .expect("physicalplantest testdata path must be UTF-8"),
        "cascades_template",
        true,
    )
    .unwrap_or_else(|error| panic!("load cascades_template: {error}"));
    for name in ["TestRuleAggElimination4Join", "TestIssue62331"] {
        let count = assert_suite_case_shape(&cascades_template, name, true);
        assert_eq!(
            count,
            assert_suite_case_shape(&cascades_template, name, false)
        );
    }
}

/// 验证 NormalizeDigest 忽略字面量差异并保持语句形状一致。
#[test]
fn physical_plan_suite_uses_stable_sql_normalization() {
    // 大小写与字面量不同，但谓词结构相同，digest 应一致。
    let (left, left_digest) =
        astersql_parser::NormalizeDigest("SELECT * FROM t WHERE a = 1 AND b = 'x'");
    let (right, right_digest) =
        astersql_parser::NormalizeDigest("select * from t where a = 9 and b = 'y'");
    assert_eq!(left, right);
    assert_eq!(left_digest, right_digest);
}
