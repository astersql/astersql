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

// CBO casetest 的 TestMain 语义。
//
// `cbo_test.rs` 的真实 DDL/统计路径替代，此处不再挂 BookKeeper。

// 本文件对应 pkg/planner/core/casetest/cbotest/main_test.go 的 TestMain：Go 版本做 common
// cbo_test.rs（见其文件头注释）已经改为不依赖 golden 文件的真实 DDL/DML/统计信息测试，因此这里
// 不再需要 BookKeeper 去加载 analyze_suite。保留同样的初始化语义：真实调用

use std::path::Path;

use astersql_testkit::testdata::LoadTestSuiteData;

#[test]
fn test_main_matches_go_common_test_setup() {
    // 对齐 Go testsetup.SetupForCommonTest() 的进程级公共初始化。
    astersql_testkit_testsetup::SetupForCommonTest();
}

/// Go TestMain 必须能加载 analyze_suite，并按名称访问全部 CBO fixtures。
#[test]
fn test_main_loads_analyze_suite() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
    let suite = LoadTestSuiteData(
        directory
            .to_str()
            .expect("analyze_suite testdata path must be UTF-8"),
        "analyze_suite",
    )
    .expect("load analyze_suite input/output");

    for (name, count) in [
        ("TestCBOWithoutAnalyze", 2),
        ("TestTableDual", 2),
        ("TestEstimation", 1),
        ("TestOutdatedAnalyze", 4),
        ("TestInconsistentEstimation", 1),
        ("TestLimitCrossEstimation", 9),
        ("TestIssue9562", 2),
        ("TestTiFlashCostModel", 4),
    ] {
        let (input, output) = suite
            .LoadTestCasesByName(name, false)
            .unwrap_or_else(|error| panic!("load {name}: {error}"));
        assert_eq!(input.as_array().expect("input cases array").len(), count);
        assert_eq!(output.as_array().expect("output cases array").len(), count);
    }
}
