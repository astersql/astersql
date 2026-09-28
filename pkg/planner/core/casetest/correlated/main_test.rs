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

// 相关子查询用例 crate 的 TestMain 对照。
//
// 主体回归见 `correlated_test.rs`。

// 本文件对应 pkg/planner/core/casetest/correlated/main_test.go。Rust 的 cargo test 不需要
// 数据。主体回归见 correlated_test.rs。

#![allow(non_snake_case)]

/// 加载与 Go `testDataMap["correlated_subquery_suite"]` 相同的 golden 套件。
pub(crate) fn GetCorrelatedSubquerySuiteData() -> astersql_testkit::testdata::TestData {
    astersql_testkit::testdata::LoadTestSuiteDataWithCascades(
        concat!(env!("CARGO_MANIFEST_DIR"), "/testdata"),
        "correlated_subquery_suite",
        true,
    )
    .expect("load correlated_subquery_suite")
}

#[test]
fn TestMain() {
    // 与 Go testsetup.SetupForCommonTest 对齐的公共测试环境初始化。
    astersql_testkit_testsetup::SetupForCommonTest();

    let suite = GetCorrelatedSubquerySuiteData();
    for cascades in [false, true] {
        let (input, output) = suite
            .LoadTestCasesByName("TestNaturalJoinWithCorrelatedSubquery", cascades)
            .expect("load natural join cases");
        assert_eq!(input.as_array().map(Vec::len), Some(3));
        assert_eq!(output.as_array().map(Vec::len), Some(3));
    }
}
