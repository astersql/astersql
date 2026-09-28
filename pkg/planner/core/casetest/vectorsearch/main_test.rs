// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 向量检索 casetest 的 TestMain 语义。
//

// 本文件对应 pkg/planner/core/casetest/vectorsearch/main_test.go 的 TestMain：Go 版本做
// common test 初始化、加载 ann_index_suite、UpdateGlobal 关掉 AsyncCommit 窗口并打开
// 并用 update_global 设置对应的三项全局配置。

use std::path::Path;

use astersql_testkit::testdata::LoadTestSuiteDataWithCascades;

#[test]
fn test_main_matches_go_common_test_setup_and_stats_cache_quota() {
    // 对齐 Go testsetup.SetupForCommonTest() 的进程级公共初始化。
    astersql_testkit_testsetup::SetupForCommonTest();

    // Go TestMain 在运行任何测试前加载 ann_index_suite；逐组检查 input、out
    // 和 xut 的命名与数量，防止 Rust 侧悄悄丢弃 golden 场景。
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
    let suite = LoadTestSuiteDataWithCascades(
        directory
            .to_str()
            .expect("vectorsearch testdata path must be valid UTF-8"),
        "ann_index_suite",
        true,
    )
    .unwrap_or_else(|error| panic!("load ann_index_suite: {error}"));
    let expected_cases = [
        ("TestTiFlashANNIndex", 22),
        ("TestTiFlashANNIndexForPartition", 14),
        ("TestVectorSearchWithPKAuto", 11),
        ("TestVectorSearchWithPKForceTiKV", 11),
        ("TestVectorSearchHeavyFunction", 21),
    ];
    for (name, count) in expected_cases {
        let (input, output) = suite
            .LoadTestCasesByName(name, true)
            .unwrap_or_else(|error| panic!("load {name} cascades cases: {error}"));
        assert_eq!(
            input
                .as_array()
                .expect("input cases must be an array")
                .len(),
            count
        );
        assert_eq!(
            output
                .as_array()
                .expect("output cases must be an array")
                .len(),
            count
        );
    }

    // 对齐 Go UpdateGlobal 的全部三项副作用，测完后 restore。
    let restore = astersql_config::restore_func();
    astersql_config::update_global(|conf| {
        conf.tikv_client.async_commit.safe_window = 0;
        conf.tikv_client.async_commit.allowed_clock_drift = 0;
        conf.performance.enable_stats_cache_mem_quota = true;
    });
    let config = astersql_config::get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.performance.enable_stats_cache_mem_quota);
    restore();
}
