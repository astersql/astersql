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

// 外键 DDL 测试包的 `TestMain` 等价初始化与回归。
//
// failpoint 开关；domain schema retry 与 ddl RunInGoTest 在 Rust 侧尚无可执行接口。
// 其余已有 Rust API 的进程级设置在这里完整接线并由回归测试观察。

/// 应用 Rust 侧已有等价接口的 Go `TestMain` 环境设置。
fn setup_test_environment() {
    astersql_testkit_testsetup::SetupForCommonTest();
    astersql_meta_autoid::set_step(5_000);
    astersql_config::update_global(|config| {
        config.instance.slow_threshold = 10_000;
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
        config.experimental.allows_expression_index = true;
    });
}

#[test]
fn test_main_applies_go_environment() {
    let original_step = astersql_meta_autoid::get_step();
    let restore_config = astersql_config::restore_func();

    setup_test_environment();

    let config = astersql_config::get_global_config();
    assert_eq!(astersql_meta_autoid::get_step(), 5_000);
    assert_eq!(config.instance.slow_threshold, 10_000);
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);

    astersql_meta_autoid::set_step(original_step);
    restore_config();
}
