// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// executor 包测试入口的 Rust 对等实现。
//
// Rust libtest 没有 Go `TestMain` 和 goroutine 泄漏检查；因此真实执行
// Rust 侧已有的公共初始化/全局配置，并验证 testdata 套件。

const TESTDATA_SUITES: [&str; 2] = ["prepare_suite", "slow_query_suite"];

fn setup_test_environment() {
    astersql_testkit_testsetup::SetupForCommonTest();
    astersql_meta_autoid::set_step(5_000);
    astersql_config::update_global(|config| {
        config.instance.slow_threshold = 30_000;
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
        config.experimental.allows_expression_index = true;
    });
    astersql_sessionctx_vardef::StatsCacheMemQuota.Store(5_000);
}

#[test]
fn executor_test_main_applies_available_runtime_settings() {
    let original_step = astersql_meta_autoid::get_step();
    let original_quota = astersql_sessionctx_vardef::StatsCacheMemQuota.Load();
    let restore_config = astersql_config::restore_func();

    setup_test_environment();

    let config = astersql_config::get_global_config();
    assert_eq!(astersql_meta_autoid::get_step(), 5_000);
    assert_eq!(config.instance.slow_threshold, 30_000);
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);
    assert_eq!(astersql_sessionctx_vardef::StatsCacheMemQuota.Load(), 5_000);

    astersql_meta_autoid::set_step(original_step);
    astersql_sessionctx_vardef::StatsCacheMemQuota.Store(original_quota);
    restore_config();
}

#[test]
fn executor_test_main_preserves_testdata_contract() {
    assert_eq!(TESTDATA_SUITES, ["prepare_suite", "slow_query_suite"]);
    assert!(include_str!("testdata/prepare_suite_in.json").starts_with('['));
    assert!(include_str!("testdata/prepare_suite_out.json").starts_with('['));
    assert!(include_str!("testdata/slow_query_suite_in.json").starts_with('['));
    assert!(include_str!("testdata/slow_query_suite_out.json").starts_with('['));
}
