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

//! 对照 Go `main_test.go` 的 suite 初始化与 TestMain 配置。
//!
//! 等价入口；但必须真实读取同一套 fixture，并验证输入、标准输出和 Cascades 输出
//! 的用例清单没有在迁移时被删减或错配。

use astersql_testkit::testdata::{LoadTestSuiteDataWithCascades, TestData};

const GO_WINDOW_SUITE_CASES: [(&str, usize); 5] = [
    ("TestWindowPushDownPlans", 11),
    ("TestWindowFunctionDescCanPushDown", 7),
    ("TestWindowPlanWithOtherOperators", 10),
    ("TestWindowSubqueryOuterRef", 9),
    ("TestWindowWithOuterJoinAndCTE", 2),
];

const WINDOW_SUITE_INPUT: &str = include_str!("testdata/window_push_down_suite_in.json");
const WINDOW_SUITE_OUTPUT: &str = include_str!("testdata/window_push_down_suite_out.json");
const WINDOW_SUITE_CASCADES: &str = include_str!("testdata/window_push_down_suite_xut.json");

/// 真实接线到 Go `testdata.BookKeeper` 对应的 Rust `TestData` loader。
pub(crate) fn get_window_push_down_suite_data() -> TestData {
    LoadTestSuiteDataWithCascades(
        concat!(env!("CARGO_MANIFEST_DIR"), "/testdata"),
        "window_push_down_suite",
        true,
    )
    .unwrap_or_else(|error| panic!("load window_push_down_suite: {error}"))
}

fn suite_body<'a>(json: &'a str, key: &str, name: &str) -> &'a str {
    let marker = format!("\"{key}\": \"{name}\"");
    let start = json
        .find(&marker)
        .unwrap_or_else(|| panic!("fixture is missing suite {name}"));
    let rest = &json[start + marker.len()..];
    let next_marker = format!("\"{key}\": \"");
    let end = rest.find(&next_marker).unwrap_or(rest.len());
    &rest[..end]
}

fn input_case_count(json: &str, name: &str) -> usize {
    let body = suite_body(json, "name", name);
    let cases = body
        .find("\"cases\": [")
        .unwrap_or_else(|| panic!("input fixture suite {name} has no cases"));
    body[cases..]
        .lines()
        .skip(1)
        .take_while(|line| !line.trim_start().starts_with(']'))
        .filter(|line| line.trim_start().starts_with('"'))
        .count()
}

fn output_case_count(json: &str, name: &str) -> usize {
    suite_body(json, "Name", name).matches("\"SQL\":").count()
}

/// 供同目录测试复用的 Go suite 清单校验。
pub(crate) fn assert_window_suite_inventory() {
    for (name, expected) in GO_WINDOW_SUITE_CASES {
        assert_eq!(
            input_case_count(WINDOW_SUITE_INPUT, name),
            expected,
            "Go input case count for {name}"
        );
        assert_eq!(
            output_case_count(WINDOW_SUITE_OUTPUT, name),
            expected,
            "Go standard output case count for {name}"
        );
        assert_eq!(
            output_case_count(WINDOW_SUITE_CASCADES, name),
            expected,
            "Go Cascades output case count for {name}"
        );
    }
}

#[test]
fn test_main_matches_go_common_test_configuration() {
    // 这是 Go `testsetup.SetupForCommonTest()` 的真实 Rust 接线点；它会安装
    // mock-store 测试所需的全局默认配置，而不是仅断言白名单常量。
    astersql_testkit_testsetup::SetupForCommonTest();
}

/// 对照 Go `LoadTestSuiteData("testdata", "window_push_down_suite", true)`。
#[test]
fn test_main_loads_window_push_down_suite() {
    let suite = get_window_push_down_suite_data();
    for (name, expected) in GO_WINDOW_SUITE_CASES {
        let (input, standard) = suite
            .LoadTestCasesByName(name, false)
            .unwrap_or_else(|error| panic!("load standard {name}: {error}"));
        let (cascades_input, cascades) = suite
            .LoadTestCasesByName(name, true)
            .unwrap_or_else(|error| panic!("load Cascades {name}: {error}"));
        assert_eq!(input, cascades_input, "input drift for {name}");
        assert_eq!(
            input.as_array().expect("suite input array").len(),
            expected,
            "input case count for {name}"
        );
        assert_eq!(
            standard.as_array().expect("standard output array").len(),
            expected,
            "standard output count for {name}"
        );
        assert_eq!(
            cascades.as_array().expect("Cascades output array").len(),
            expected,
            "Cascades output count for {name}"
        );
    }
    assert_window_suite_inventory();
}
