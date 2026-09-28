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

//! `issuetest` 的包级测试环境初始化，对应 Go 版本的 `TestMain`。
//!
//! 这里统一设置自增 ID 步长、进程级配置和 failpoint，确保 Rust 回归用例使用与原测试包一致的运行条件。

use std::sync::Once;

// 测试可能并行调用初始化入口；进程级状态必须只写入一次。
static SETUP: Once = Once::new();

/// 按 Go `TestMain` 的顺序安装 issuetest 共用的进程级测试环境。
fn setup_test_main() {
    SETUP.call_once(|| {
        // 固定 ID 分配步长及相关功能开关，使 issue 回归用例与 Go 版本共享同一基线。
        astersql_meta_autoid::set_step(5_000);
        astersql_config::update_global(|config| {
            config.instance.slow_threshold = 30_000;
            config.tikv_client.async_commit.safe_window = 0;
            config.tikv_client.async_commit.allowed_clock_drift = 0;
            config.experimental.allows_expression_index = true;
        });

        // Rust 的 fail 注册表对应 TiKV 的 failpoint 开关，此标记需在整个进程期间保持生效。
        let guard = astersql_testkit_testfailpoint::enable("issuetest/TestMain", "return(enabled)");
        assert_eq!(
            astersql_testkit_testfailpoint::eval_string("issuetest/TestMain").as_deref(),
            Some("enabled")
        );
        std::mem::forget(guard);
    });
}

#[test]
/// 验证一次性初始化后的全局状态仍符合 Go `TestMain` 的约定。
fn test_main_applies_go_test_environment() {
    setup_test_main();
    let config = astersql_config::get_global_config();
    assert_eq!(astersql_meta_autoid::get_step(), 5_000);
    assert_eq!(config.instance.slow_threshold, 30_000);
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);
    assert_eq!(
        astersql_testkit_testfailpoint::eval_string("issuetest/TestMain").as_deref(),
        Some("enabled")
    );
}
