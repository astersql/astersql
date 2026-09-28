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

// Rust 的 libtest 不提供 Go `TestMain` 的包级入口，因此每个测试先调用
// `setup`；Once 保证公共日志、AutoID 与全局配置只初始化一次。

use std::sync::Once;

static SETUP: Once = Once::new();

/// 执行 Go TestMain 中对 Rust 运行时确有对应物的进程级副作用。
pub fn setup() {
    SETUP.call_once(|| {
        testsetup_crate::SetupForCommonTest();
        meta_autoid_crate::set_step(5000);
        config_crate::update_global(|config| {
            config.instance.slow_threshold = 30_000;
            config.tikv_client.async_commit.safe_window = 0;
            config.tikv_client.async_commit.allowed_clock_drift = 0;
            config.experimental.allows_expression_index = true;
        });
    });
}

#[test]
/// 验证 Rust 测试入口确实应用配置，而非只保存 Go 调用名称。
fn test_main_applies_go_equivalent_process_settings() {
    setup();

    let config = config_crate::get_global_config();
    assert_eq!(meta_autoid_crate::get_step(), 5000);
    assert_eq!(config.instance.slow_threshold, 30_000);
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);
}
