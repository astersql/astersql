// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// Schema casetest 的 TestMain 语义对照。
//
// TestMain，改为可执行初始化测试；主体回归见 `cannot_find_column_test.rs`。
//

// 本文件对应 pkg/planner/core/casetest/schema/main_test.go。Rust 的 cargo test 不需要
// 数据。主体回归见 cannot_find_column_test.rs。

#![allow(non_snake_case)]

use astersql_testkit::testdata::{LoadTestSuiteDataWithCascades, TestData};

/// 对应 Go `GetSchemaSuiteData`，真实加载普通与 Cascades 两套 golden 输出。
fn GetSchemaSuiteData() -> TestData {
    LoadTestSuiteDataWithCascades(
        concat!(env!("CARGO_MANIFEST_DIR"), "/testdata"),
        "cannot_find_column_suite",
        true,
    )
    .unwrap_or_else(|error| panic!("load cannot_find_column_suite: {error}"))
}

#[test]
fn TestMain() {
    // 对齐 Go TestMain 的公共测试环境初始化。
    astersql_testkit_testsetup::SetupForCommonTest();

    // Go TestMain 在测试前装载 suite，并在 callback 中生成/刷新录制输出。
    let mut suite = GetSchemaSuiteData();
    suite
        .flush()
        .unwrap_or_else(|error| panic!("flush cannot_find_column_suite: {error}"));
}

/// Go `LoadTestSuiteData("testdata", "cannot_find_column_suite", true)` / getter parity.
#[test]
fn test_main_loads_and_exposes_schema_suite() {
    let suite: TestData = GetSchemaSuiteData();
    for name in ["TestSchemaCannotFindColumnRegression"] {
        let (input, standard) = suite
            .LoadTestCasesByName(name, false)
            .unwrap_or_else(|error| panic!("load standard {name}: {error}"));
        let (cascades_input, cascades) = suite
            .LoadTestCasesByName(name, true)
            .unwrap_or_else(|error| panic!("load Cascades {name}: {error}"));
        assert_eq!(input, cascades_input, "input drift for {name}");
        assert_eq!(input.as_array().map(Vec::len), Some(2));
        assert_eq!(standard.as_array().map(Vec::len), Some(2));
        assert_eq!(cascades.as_array().map(Vec::len), Some(2));
    }
}
