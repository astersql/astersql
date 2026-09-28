// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 对应 Go `planreplayer.TestMain`：执行通用测试初始化，设置 autoid 步长与
// Plan Replayer 所需全局配置。TiKV Go client 的进程级 failpoint 开关没有可执行的直接对应物。

/// 执行 Go `TestMain` 在 Rust 中有直接对应的进程级初始化。
fn apply_plan_replayer_test_main() {
    astersql_testkit_testsetup::SetupForCommonTest();
    astersql_meta_autoid::set_step(5_000);
    astersql_config::update_global(|config| {
        config.instance.slow_threshold = 30_000;
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
        config.experimental.allows_expression_index = true;
        config.performance.enable_stats_cache_mem_quota = true;
    });
}

#[test]
fn test_main_applies_and_restores_go_process_contract() {
    let old_step = astersql_meta_autoid::get_step();
    let restore_config = astersql_config::restore_func();

    apply_plan_replayer_test_main();

    let config = astersql_config::get_global_config();
    assert_eq!(astersql_meta_autoid::get_step(), 5_000);
    assert_eq!(config.instance.slow_threshold, 30_000);
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);
    assert!(config.performance.enable_stats_cache_mem_quota);
    astersql_meta_autoid::set_step(old_step);
    restore_config();
}
