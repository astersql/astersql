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

// Hint 用例包的 TestMain 语义对照。
//
// 对应 Go `main_test.go`：common test 初始化、加载 integration_suite、更新全局配置、

// 本文件对应 pkg/planner/core/casetest/hint/main_test.go 的 TestMain：Go 版本先做
// common test 初始化，再加载 `integration_suite` testdata 黄金文件、更新全局配置，最后用
// 把测试改为直连 `astersql-parser`/`astersql-util-hint` 生产 API，不再依赖 testdata 黄金
// 文件回放，因此这里不再需要 BookKeeper/GetIntegrationSuiteData；Rust 测试框架也没有
// 名单原样保留成可断言的数据，避免这段语义被静默丢弃。

#![allow(non_snake_case)]

use astersql_testkit::testdata::{LoadTestSuiteDataWithCascades, TestData};

/// Go `hint_test.go` 中由 TestMain 预加载的 integration_suite 用例名。
pub(crate) const HINT_INTEGRATION_TESTS: [&str; 8] = [
    "TestOptimizeHintOnPartitionTable",
    "TestReadFromStorageHint",
    "TestAllViewHintType",
    "TestJoinHintCompatibility",
    "TestReadFromStorageHintAndIsolationRead",
    "TestIsolationReadTiFlashUseIndexHint",
    "TestHints",
    "TestQBHintHandlerDuplicateObjects",
];

/// 加载本 crate 自己的 Go integration_suite fixture。
pub(crate) fn load_integration_suite() -> TestData {
    LoadTestSuiteDataWithCascades(
        concat!(env!("CARGO_MANIFEST_DIR"), "/testdata"),
        "integration_suite",
        true,
    )
    .unwrap_or_else(|error| panic!("load hint integration_suite fixture: {error}"))
}

#[test]
fn TestMain() {
    // 对齐 Go TestMain 的公共测试环境初始化。
    astersql_testkit_testsetup::SetupForCommonTest();

    // 对齐 Go TestMain 的 BookKeeper.LoadTestSuiteData("testdata", "integration_suite", true)。
    let _integration_suite = load_integration_suite();

    // 对应 Go TestMain 里的 config.UpdateGlobal：稳定异步提交与统计缓存行为。
    let restore = astersql_config::restore_func();
    astersql_config::update_global(|config| {
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
        config.performance.enable_stats_cache_mem_quota = true;
    });
    let config = astersql_config::get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.performance.enable_stats_cache_mem_quota);
    restore();
}

/// 回归：TestMain 预加载的 fixture 必须覆盖 Go hint 包的全部 8 个测试入口。
#[test]
fn testmain_loads_all_hint_integration_cases() {
    let suite = load_integration_suite();
    for name in HINT_INTEGRATION_TESTS {
        let (input, output) = suite
            .LoadTestCasesByName(name, false)
            .unwrap_or_else(|error| panic!("missing hint integration case {name}: {error}"));
        let input = input
            .as_array()
            .unwrap_or_else(|| panic!("{name} input cases must be an array"));
        let output = output
            .as_array()
            .unwrap_or_else(|| panic!("{name} output cases must be an array"));
        assert!(!input.is_empty(), "{name} input fixture is empty");
        assert_eq!(input.len(), output.len(), "{name} input/output case count");

        let (input_xut, output_xut) = suite
            .LoadTestCasesByName(name, true)
            .unwrap_or_else(|error| panic!("missing cascades hint case {name}: {error}"));
        assert_eq!(
            input_xut
                .as_array()
                .unwrap_or_else(|| panic!("{name} cascades input cases must be an array"))
                .len(),
            input.len(),
            "{name} cascades input case count"
        );
        assert_eq!(
            output_xut
                .as_array()
                .unwrap_or_else(|| panic!("{name} cascades output cases must be an array"))
                .len(),
            output.len(),
            "{name} cascades output case count"
        );
    }
}
