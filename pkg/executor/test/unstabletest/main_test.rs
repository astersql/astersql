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

// 不稳定测试包的 TestMain 等价初始化与包级冒烟用例。
//
// Go `TestMain` 设置 autoid 步长、慢日志阈值、AsyncCommit 安全窗口与 failpoint。
// TiKV Go client 的 failpoint 开关没有 Rust 等价入口；其余已有 Rust API 的进程级设置
// 在这里完整接线并由回归测试观察。

/// 应用 Rust 侧已有等价接口的 Go `TestMain` 环境设置。
fn setup_test_environment() {
    astersql_meta_autoid::set_step(5_000);
    astersql_config::update_global(|config| {
        config.instance.slow_threshold = 30_000;
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
        config.experimental.allows_expression_index = true;
    });
}

/// Rust 侧已有等价 API 的 Go `TestMain` 前置条件必须真实生效。
#[test]
fn test_main_applies_available_go_environment_settings() {
    let original_step = astersql_meta_autoid::get_step();
    let restore_config = astersql_config::restore_func();

    setup_test_environment();

    let config = astersql_config::get_global_config();
    assert_eq!(astersql_meta_autoid::get_step(), 5_000);
    assert_eq!(config.instance.slow_threshold, 30_000);
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);

    astersql_meta_autoid::set_step(original_step);
    restore_config();
}

/// 额外练习本包 `memory_test.rs` 真正依赖的生产 Tracker，确认独立可用。
#[test]
fn tracker_consume_accounts_bytes_for_minimal_package_smoke_check() {
    let tracker = astersql_util_memory::tracker::Tracker::new(0, -1);
    tracker.Consume(42);
    assert_eq!(tracker.BytesConsumed(), 42);
    tracker.Consume(-42);
    assert_eq!(tracker.BytesConsumed(), 0);
}
