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

// 外键测试包级 TestMain 配置语义测试。
//
// 对应 Go `pkg/executor/test/fktest/main_test.go`：设置 autoid 步长、慢日志阈值、
// async commit 时钟窗口与表达式索引开关。Rust 在独立测试中应用真实生产配置，
// 验证后恢复。

fn setup_test_environment() {
    astersql_meta_autoid::set_step(5_000);
    astersql_config::update_global(|config| {
        config.instance.slow_threshold = 30_000;
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
        config.experimental.allows_expression_index = true;
    });
}

/// 按 Go TestMain 修改全局测试环境，并验证测试退出时恢复配置。
#[test]
fn test_main_applies_go_slow_log_threshold_to_real_config() {
    let original_step = astersql_meta_autoid::get_step();
    let before = astersql_config::get_global_config().instance.slow_threshold;
    let restore = astersql_config::restore_func();
    setup_test_environment();
    assert_eq!(
        astersql_config::get_global_config().instance.slow_threshold,
        30_000
    );
    let configured = astersql_config::get_global_config();
    assert_eq!(astersql_meta_autoid::get_step(), 5_000);
    assert_eq!(configured.tikv_client.async_commit.safe_window, 0);
    assert_eq!(configured.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(configured.experimental.allows_expression_index);
    restore();
    astersql_meta_autoid::set_step(original_step);
    assert_eq!(
        astersql_config::get_global_config().instance.slow_threshold,
        before
    );
}
