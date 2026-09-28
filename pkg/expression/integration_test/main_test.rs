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

// 集成测试主配置桩（对应 Go `TestMain` 的可移植初始化）。

#![allow(non_snake_case)]

use std::sync::Once;

use astersql_config::{get_global_config, update_global};
use astersql_testkit_testfailpoint::{enable, is_active};
use astersql_testkit_testsetup::SetupForCommonTest;
use astersql_util_timeutil::time_zone::SetSystemTZ;

static TEST_ENV: Once = Once::new();

/// 对齐 Go `TestMain` 的进程级公共初始化与全局配置副作用。
fn configure_test_environment() {
    TEST_ENV.call_once(|| {
        SetupForCommonTest();
        update_global(|config| {
            config.tikv_client.async_commit.safe_window = 0;
            config.tikv_client.async_commit.allowed_clock_drift = 0;
            config.experimental.allows_expression_index = true;
        });

        // Go 的 TiKV 开关使 failpoint 在测试进程中可用；Rust 侧注册并观察一次。
        let guard = enable("expression-integration-test-main", "return");
        assert!(is_active("expression-integration-test-main"));
        drop(guard);

        // 固定系统时区，避免测试顺序改变 `SystemLocation`。
        SetSystemTZ("system");
    });
}

#[test]
/// 校验 TestMain 的真实全局配置副作用。
fn TestMainConfiguration() {
    configure_test_environment();
    let config = get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);
}
