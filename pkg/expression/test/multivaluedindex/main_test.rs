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

// Rust counterpart of Go `TestMain` for `pkg/expression/test/multivaluedindex`.
//
// Go enables expression indexes and failpoints. Rust keeps the same
// package-level setup: common testsetup, system TZ pinning, and the expression
// index requirement that multi-valued index DDL depends on.
//
// 多值索引测试包的 `TestMain` 对等实现。
// 开启表达式索引（DDL 依赖）、公共 testsetup，并将系统时区钉死，避免时区抖动。

use std::sync::Once;

use astersql_config::{get_global_config, update_global};
use astersql_testkit_testfailpoint::{enable, is_active};
use astersql_testkit_testsetup::SetupForCommonTest;
use astersql_util_timeutil::time_zone::SetSystemTZ;

static INIT: Once = Once::new();

/// Compatibility flag consumed by the multi-valued-index behavior tests.
/// The authoritative value is also applied to and checked from global config.
pub(crate) const ALLOWS_EXPRESSION_INDEX: bool = true;

/// 幂等初始化测试环境；时区注释说明为何必须调用 `SetSystemTZ`。
pub(crate) fn ensure_test_env() {
    INIT.call_once(|| {
        SetupForCommonTest();
        update_global(|config| {
            config.tikv_client.async_commit.safe_window = 0;
            config.tikv_client.async_commit.allowed_clock_drift = 0;
            config.experimental.allows_expression_index = true;
        });

        // Rust failpoint support is compiled in; registering and observing one
        // proves the runtime hook is enabled, matching Go `tikv.EnableFailpoints()`.
        let guard = enable("multivaluedindex-test-main", "return");
        assert!(is_active("multivaluedindex-test-main"));
        drop(guard);

        // Some test depends on the values of timeutil.SystemLocation()
        // If we don't SetSystemTZ() here, the value would change unpredictable.
        SetSystemTZ("system");
    });
}

/// 冒烟：表达式索引开启且系统时区已固定。
#[test]
fn test_main_enables_expression_index_and_pins_system_tz() {
    ensure_test_env();
    let config = get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);
}
