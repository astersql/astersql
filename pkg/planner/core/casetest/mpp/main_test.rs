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

// MPP 用例包的 TestMain 语义对照。
//
// Rust 无进程级 TestMain；用可执行初始化测试保留 SetupForCommonTest、suite 加载与
// 可用的全局配置语义，主体用例见 `mpp_test.rs`。

// 本文件对应 pkg/planner/core/casetest/mpp/main_test.go 的 TestMain：Go 版本做 common
// 但必须仍加载同一套 integration_suite，避免 fixture 在迁移中被静默丢弃。

use astersql_testkit::testdata::{LoadTestSuiteDataWithCascades, TestData};

/// 对应 Go `GetIntegrationSuiteData`，读取本 crate 的标准与 Cascades fixture。
pub(crate) fn load_integration_suite() -> TestData {
    LoadTestSuiteDataWithCascades(
        concat!(env!("CARGO_MANIFEST_DIR"), "/testdata"),
        "integration_suite",
        true,
    )
    .unwrap_or_else(|error| panic!("load mpp integration_suite fixture: {error}"))
}

/// 回归：真实调用 SetupForCommonTest，加载 suite，并覆盖 Go TestMain 的可用配置项。
#[test]
fn test_main_matches_go_common_test_setup() {
    astersql_testkit_testsetup::SetupForCommonTest();

    let suite = load_integration_suite();
    let (input, output) = suite
        .LoadTestCasesByName("TestMPPJoin", false)
        .expect("TestMain-loaded integration_suite must contain TestMPPJoin");
    assert_eq!(
        input.as_array().expect("MPP input cases").len(),
        output.as_array().expect("MPP output cases").len()
    );

    // Go TestMain 设置该配置以稳定统计缓存行为；AsyncCommit 两个字段在 Rust
    // config 中尚未提供，因此只驱动现有且实际可观察的配置项。
    let restore = astersql_config::restore_func();
    astersql_config::update_global(|config| {
        config.performance.enable_stats_cache_mem_quota = true;
    });
    assert!(
        astersql_config::get_global_config()
            .as_ref()
            .performance
            .enable_stats_cache_mem_quota
    );
    restore();
}
