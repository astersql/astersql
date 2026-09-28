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

// 对应 `pkg/executor/test/cte/main_test.go` 的进程级测试入口语义。
//
// Go 的 `TestMain` 负责包级初始化与全局配置覆盖；Rust 侧
// 用一个可执行测试承载同样的公共 setup 与配置断言。

/// 对应 Go `TestMain`：先初始化公共测试环境，再应用包级配置覆盖。
#[test]
fn test_main_matches_go_common_setup_and_global_config() {
    astersql_testkit_testsetup::SetupForCommonTest();

    astersql_config::update_global(|config| {
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
        config.experimental.allows_expression_index = true;
        config.performance.enable_stats_cache_mem_quota = true;
    });

    let config = astersql_config::get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);
    assert!(config.performance.enable_stats_cache_mem_quota);
}
