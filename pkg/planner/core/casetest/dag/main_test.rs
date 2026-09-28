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

// DAG 计划用例 crate 的 TestMain 对照。
//
// 在 `dag_test.rs` 中直连 parser / mock infoschema / hint。本文件保留

// 本文件对应 pkg/planner/core/casetest/dag/main_test.go 的 TestMain：Go 版本做 common

#[test]
fn test_main_matches_go_common_test_setup() {
    // 公共测试环境初始化（对应 Go testsetup.SetupForCommonTest）。
    astersql_testkit_testsetup::SetupForCommonTest();
}

/// Go `TestMain` 的 `LoadTestSuiteData("testdata", "plan_suite", true)` 必须成功，且
/// 每套标准/级联输入都能按 Go 的用例名访问。
#[test]
fn test_main_loads_plan_suite_for_standard_and_cascades_runs() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
    let suite = astersql_testkit::testdata::LoadTestSuiteDataWithCascades(
        directory
            .to_str()
            .expect("testdata path must be valid UTF-8"),
        "plan_suite",
        true,
    )
    .expect("Go plan_suite input/output/xut must load");

    for name in [
        "TestDAGPlanBuilderSimpleCase",
        "TestDAGPlanBuilderSimpleCaseForNextGen",
        "TestDAGPlanBuilderJoin",
        "TestDAGPlanBuilderSubquery",
        "TestDAGPlanTopN",
        "TestDAGPlanBuilderBasePhysicalPlan",
        "TestDAGPlanBuilderUnion",
        "TestDAGPlanBuilderUnionScan",
        "TestDAGPlanBuilderAgg",
        "TestDAGPlanBuilderWindow",
        "TestDAGPlanBuilderWindowParallel",
    ] {
        let (input, standard) = suite
            .LoadTestCasesByName(name, false)
            .unwrap_or_else(|error| panic!("standard {name}: {error}"));
        let (input_xut, cascades) = suite
            .LoadTestCasesByName(name, true)
            .unwrap_or_else(|error| panic!("cascades {name}: {error}"));
        let input_len = input
            .as_array()
            .unwrap_or_else(|| panic!("{name} input must be an array"))
            .len();
        assert_eq!(
            input_len,
            input_xut
                .as_array()
                .unwrap_or_else(|| panic!("{name} cascades input must be an array"))
                .len()
        );
        assert_eq!(
            input_len,
            standard
                .as_array()
                .unwrap_or_else(|| panic!("{name} standard output must be an array"))
                .len()
        );
        assert_eq!(
            input_len,
            cascades
                .as_array()
                .unwrap_or_else(|| panic!("{name} cascades output must be an array"))
                .len()
        );
    }
}
