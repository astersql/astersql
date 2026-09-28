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

// Rust counterpart of Go `TestMain` for `pkg/expression/test/constantpropagation`.
//
// 常量传播集成测试包的 `TestMain` 对等实现。
// 一次性完成公共测试环境初始化，并固定系统时区，避免时间相关用例抖动。

use std::sync::Once;

use astersql_config::{get_global_config, update_global};
use astersql_testkit_testfailpoint::{enable, is_active};
use astersql_testkit_testsetup::SetupForCommonTest;
use astersql_util_timeutil::time_zone::{GetSystemTZ, SetSystemTZ};

static INIT: Once = Once::new();

/// 幂等地执行测试环境初始化：公共 testsetup + 将系统时区钉为 `"system"`。
pub(crate) fn ensure_test_env() {
    // Once 保证并行测试下只初始化一次。
    INIT.call_once(|| {
        SetupForCommonTest();
        update_global(|config| {
            config.tikv_client.async_commit.safe_window = 0;
            config.tikv_client.async_commit.allowed_clock_drift = 0;
            config.experimental.allows_expression_index = true;
        });

        // Rust failpoint 支持在编译期接入，无额外全局开关；实际注册并观察一次，
        // 对齐 Go `tikv.EnableFailpoints()` 所保证的运行时可用性。
        let guard = enable("constantpropagation-test-main", "return");
        assert!(is_active("constantpropagation-test-main"));
        drop(guard);

        SetSystemTZ("system");
    });
}

/// 冒烟：确认 TestMain 的真实全局配置副作用。
#[test]
fn test_main_enables_expression_index_and_pins_system_tz() {
    ensure_test_env();
    let config = get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);
    assert_eq!(GetSystemTZ().unwrap(), "system");
}
