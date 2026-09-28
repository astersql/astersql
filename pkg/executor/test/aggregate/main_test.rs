// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! 聚合执行器测试包的公共初始化契约。
//!
//! 该文件复刻 Go `TestMain` 的通用测试环境与全局配置。

#[test]
/// 验证公共初始化及四项进程级配置与 Go `TestMain` 保持一致。
fn test_main_matches_go_common_setup_and_global_config() {
    astersql_testkit_testsetup::SetupForCommonTest();
    // 修改前快照原始配置；完成断言后显式恢复，避免影响同一进程中的后续测试。
    let restore = astersql_config::restore_func();
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
    restore();
}
