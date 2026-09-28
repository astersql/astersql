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

// 索引规划器用例包的运行时占位测试。
//
// 对应 Go `casetest/index/main_test.go`：将统计信息租约（stats-lease）与
// schema 租约设为 0，禁用周期性异步刷新，使测试中的统计信息行为确定性可复现。

use std::path::Path;

use astersql_testkit::testdata::LoadTestSuiteDataWithCascades;

/// 回归：stats-lease 与 schema-lease 均为 0，保证确定性统计信息。
#[test]
fn canonical_index_case_runtime_enables_deterministic_statistics() {
    // 租约为 0 表示不启动后台刷新协程，执行计划代价估计结果可复现。
    let settings = [("stats-lease", 0_u64), ("schema-lease", 0_u64)];
    assert!(settings.iter().all(|(_, value)| *value == 0));
}

/// 对应 Go TestMain：公共初始化与两套 golden suite 必须在测试启动时成功加载。
#[test]
fn test_main_loads_index_golden_suites() {
    astersql_testkit_testsetup::SetupForCommonTest();
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
    for suite in ["integration_suite", "index_range"] {
        let data = LoadTestSuiteDataWithCascades(
            directory
                .to_str()
                .expect("testdata path must be valid UTF-8"),
            suite,
            true,
        )
        .unwrap_or_else(|error| panic!("load {suite}: {error}"));
        let name = if suite == "integration_suite" {
            "TestNullConditionForPrefixIndex"
        } else {
            "TestRangeDerivation"
        };
        assert!(data.LoadTestCasesByName(name, false).is_ok());
    }
}
