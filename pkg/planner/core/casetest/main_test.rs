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

// `casetest` 根包 TestMain / BookKeeper 语义与 suite fixture 接线。
//
// Go 入口通过 `testdata.BookKeeper` 装载 plan_normalized / stats / integration /
// json_plan 四个 suite，并在 `testmain.WrapTestingM` 收尾时生成 golden；下方大段
// 注释代码保留该结构对照；可执行测试通过同一组 in/out/xut fixture。

// TestData 是 Go testdata.TestData 的占位别名；只保留 suite 名称流转关系。
// pub struct TestData {
//     pub suite_name: &'static str,
// }
//
// TestDataMap 对应 Go 的 testDataMap := make(testdata.BookKeeper)。
// 原 Go BookKeeper 负责读取/生成测试数据文件；这里不会触发任何磁盘写入。
// pub struct TestDataMap {
//     suites: &'static [&'static str],
// }
//
// impl TestDataMap {
// load_test_suite_data 对应 BookKeeper.LoadTestSuiteData("testdata", name, true)。
// 第三个参数在 Go 中表示允许录制输出；仅记录该语义。
//     pub fn load_test_suite_data(&self, dir: &str, suite: &str, record: bool) {
//         let _ = (self.suites, dir, suite, record);
//     }
//
// generate_output_if_needed 对应 TestMain callback 中的 GenerateOutputIfNeeded。
// 真实 Go 逻辑会在测试结束前刷新 golden 输出；这里不做 IO。
//     pub fn generate_output_if_needed(&self) {}
//
// get 对应 Go map 下标 testDataMap["suite"]。
//     pub fn get(&self, suite: &'static str) -> TestData {
//         TestData { suite_name: suite }
//     }
// }
//
// Go 全局变量 testDataMap。Rust 用静态 suite 名称表示 BookKeeper 的键空间。
// pub static TEST_DATA_MAP: TestDataMap = TestDataMap {
//     suites: &[
//         "plan_normalized_suite",
//         "stats_suite",
//         "integration_suite",
//         "json_plan_suite",
//     ],
// };
//
// test_main 对应 Go 的 TestMain(m *testing.M)。
// Go 入口先做通用测试初始化和 flag.Parse，再装载四个 suite，最后通过 testmain.WrapTestingM 注入收尾 callback。
// pub fn test_main() {
// testsetup.SetupForCommonTest() 会初始化 TiDB 测试环境；仅保留调用顺序。
//     setup_for_common_test();
//     parse_flags();
//
//     TEST_DATA_MAP.load_test_suite_data("testdata", "plan_normalized_suite", true);
//     TEST_DATA_MAP.load_test_suite_data("testdata", "stats_suite", true);
//     TEST_DATA_MAP.load_test_suite_data("testdata", "integration_suite", true);
//     TEST_DATA_MAP.load_test_suite_data("testdata", "json_plan_suite", true);
//
//     let callback = |i: i32| -> i32 {
// Go callback 在 testing.M 结束路径生成 golden 输出；这里不访问文件系统。
//         TEST_DATA_MAP.generate_output_if_needed();
//         i
//     };
//
// }
//
// setup_for_common_test 对应 testsetup.SetupForCommonTest。
// fn setup_for_common_test() {}
//
// parse_flags 对应 flag.Parse。
// fn parse_flags() {}
//
// get_plan_normalized_suite_data 对应 Go 的 GetPlanNormalizedSuiteData。
// pub fn get_plan_normalized_suite_data() -> TestData {
//     TEST_DATA_MAP.get("plan_normalized_suite")
// }
//
// get_stats_suite_data 对应 Go 的 GetStatsSuiteData。
// pub fn get_stats_suite_data() -> TestData {
//     TEST_DATA_MAP.get("stats_suite")
// }
//
// get_integration_suite_data 对应 Go 的 GetIntegrationSuiteData。
// pub fn get_integration_suite_data() -> TestData {
//     TEST_DATA_MAP.get("integration_suite")
// }
//
// get_json_plan_suite_data 对应 Go 的 GetJSONPlanSuiteData。
// pub fn get_json_plan_suite_data() -> TestData {
//     TEST_DATA_MAP.get("json_plan_suite")
// }
// */
use astersql_parser::Parser;
use astersql_testkit::testdata::{LoadTestSuiteDataWithCascades, TestData};

pub(crate) fn load_suite(suite: &str, cascades: bool) -> TestData {
    LoadTestSuiteDataWithCascades(
        concat!(env!("CARGO_MANIFEST_DIR"), "/testdata"),
        suite,
        cascades,
    )
    .unwrap_or_else(|error| panic!("load {suite} fixture: {error}"))
}

pub(crate) fn get_plan_normalized_suite_data() -> TestData {
    load_suite("plan_normalized_suite", true)
}

pub(crate) fn get_stats_suite_data() -> TestData {
    load_suite("stats_suite", true)
}

pub(crate) fn get_integration_suite_data() -> TestData {
    load_suite("integration_suite", true)
}

pub(crate) fn get_json_plan_suite_data() -> TestData {
    load_suite("json_plan_suite", true)
}

/// Load every named case used by a casetest module and return its SQL inputs.
///
/// Go's `BookKeeper` binds each test function to both the input and expected
/// output entries.  Keeping that lookup here makes the Rust modules exercise
/// the same fixture boundary instead of testing only handwritten SQL samples.
pub(crate) fn load_named_sql_cases(suite: &str, names: &[&str], cascades: bool) -> Vec<String> {
    let data = load_suite(suite, cascades);
    names
        .iter()
        .flat_map(|name| {
            let (input, output) = data
                .LoadTestCasesByName(name, cascades)
                .unwrap_or_else(|error| panic!("load {suite}/{name}: {error}"));
            let input = input
                .as_array()
                .unwrap_or_else(|| panic!("{suite}/{name} input is not an array"));
            let output = output
                .as_array()
                .unwrap_or_else(|| panic!("{suite}/{name} output is not an array"));
            assert_eq!(
                input.len(),
                output.len(),
                "{suite}/{name} input/output mismatch"
            );
            input
                .iter()
                .map(|sql| {
                    sql.as_str()
                        .unwrap_or_else(|| panic!("{suite}/{name} input is not SQL: {sql}"))
                        .to_owned()
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// 回归：三类典型 suite 输入 SQL 各自独立 ParseOneStmt 成功，互不共享解析状态。
#[test]
fn casetest_suite_inputs_are_independent_parser_runs() {
    // 分别覆盖过滤、JSON 计划、聚合三类 casetest 常见语句形态。
    let suites = [
        "select * from t where a in (1,2,3)",
        "explain format='json' select * from t",
        "select count(*) from t group by a",
    ];
    for sql in suites {
        Parser::default().ParseOneStmt(sql, "", "").unwrap();
    }
}

/// Go TestMain loads all four BookKeeper suites before any package test runs.
/// Verify the Rust getters read real input/output pairs rather than returning
/// the former placeholder names.
#[test]
fn testmain_loads_all_casetest_suites_and_named_cases() {
    let suites = [
        (
            "plan_normalized_suite",
            [
                "TestNormalizedPlan",
                "TestNormalizedPlanForNextGen",
                "TestPreferRangeScan",
                "TestNormalizedPlanForDiffStore",
                "TestTiFlashLateMaterialization",
                "TestInvertedIndex",
            ]
            .as_slice(),
        ),
        (
            "stats_suite",
            ["TestGroupNDVs", "TestNDVGroupCols"].as_slice(),
        ),
        (
            "integration_suite",
            [
                "TestVerboseExplain",
                "TestIsolationReadDoNotFilterSystemDB",
                "TestIsolationReadTiFlashNotChoosePointGet",
                "TestMergeContinuousSelections",
                "TestPushDownGroupConcatToTiFlash",
                "TestTiFlashPartitionTableScan",
                "TestTiFlashFineGrainedShuffle",
                "TestTiFlashExtraColumnPrune",
            ]
            .as_slice(),
        ),
        (
            "json_plan_suite",
            ["TestJSONPlanInExplain", "TestJSONPlanInExplainForNextGen"].as_slice(),
        ),
    ];
    for (suite, names) in suites {
        let standard = load_named_sql_cases(suite, names, false);
        let cascades = load_named_sql_cases(suite, names, true);
        assert!(!standard.is_empty(), "{suite} has no fixture SQL");
        assert_eq!(
            standard.len(),
            cascades.len(),
            "{suite} standard/cascades mismatch"
        );
        for sql in standard.into_iter().chain(cascades) {
            Parser::default()
                .Parse(&sql, "", "")
                .unwrap_or_else(|error| panic!("parse {suite} fixture {sql:?}: {error}"));
        }
    }

    // Keep the public getter wiring exercised as well; these are the Go
    // package-level accessors used by the individual casetest files.
    assert!(
        !get_plan_normalized_suite_data()
            .LoadTestCasesByName("TestNormalizedPlan", true)
            .unwrap()
            .0
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        !get_stats_suite_data()
            .LoadTestCasesByName("TestGroupNDVs", true)
            .unwrap()
            .0
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        !get_integration_suite_data()
            .LoadTestCasesByName("TestVerboseExplain", true)
            .unwrap()
            .0
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        !get_json_plan_suite_data()
            .LoadTestCasesByName("TestJSONPlanInExplain", true)
            .unwrap()
            .0
            .as_array()
            .unwrap()
            .is_empty()
    );
}
