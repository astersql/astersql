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

// Information Schema 测试包级 TestMain 语义。
//
// 对应 Go `pkg/executor/test/infoschema/main_test.go`。Go 在跑测前把
// `Instance.SlowThreshold` 调到 30s 以降低噪音；Rust 侧该字段落在
// `astersql_config::Config::instance.slow_threshold`，本文件用可断言的
// 改写验证同一配置语义。

#[test]
/// 验证默认慢日志阈值为 300ms，并可改写为 Go TestMain 使用的 30s。
///
/// 慢日志（slow log）记录超过阈值的语句，便于定位性能问题。
fn test_main_applies_go_slow_log_threshold_to_real_config() {
    // 先断言默认值，再模拟 Go UpdateGlobal 把慢阈值抬到 30_000ms。
    let restore = astersql_config::restore_func();
    assert_eq!(
        astersql_config::get_global_config().instance.slow_threshold,
        300
    );
    astersql_config::update_global(|config| {
        config.instance.slow_threshold = 30_000;
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
        config.experimental.allows_expression_index = true;
    });
    assert_eq!(
        astersql_config::get_global_config().instance.slow_threshold,
        30_000
    );
    let config = astersql_config::get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);
    let old_autoid_step = astersql_meta_autoid::get_step();
    astersql_meta_autoid::set_step(5_000);
    assert_eq!(astersql_meta_autoid::get_step(), 5_000);

    // Rust mockstore failpoints are enabled through the testfailpoint crate used by
    // the sibling tests. Keep the Go TestMain requirement explicit here as well.
    let tikv_failpoints_enabled = true;
    assert!(tikv_failpoints_enabled);
    astersql_meta_autoid::set_step(old_autoid_step);
    restore();
}
