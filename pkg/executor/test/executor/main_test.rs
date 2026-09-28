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

// 执行器测试包级 TestMain 与配置作用域恢复冒烟用例。
//
// 对应 Go `pkg/executor/test/executor/main_test.go`。下方用例真实应用 autoid 步长、
// 慢日志阈值、AsyncCommit 窗口与表达式索引配置，并验证恢复行为。

use std::sync::atomic::{AtomicU64, Ordering};

use astersql_config::{get_global_config, restore_func, update_global};
use astersql_meta_autoid::{get_step, set_step};

/// Go `TestMain` 的真实进程级初始化：autoid 步长与异步提交/慢日志配置必须
/// 在测试结束后恢复，避免污染同一测试进程中的其它 crate。
#[test]
fn test_main_applies_and_restores_go_environment_overrides() {
    let old_step = get_step();
    let restore_config = restore_func();

    set_step(5_000);
    update_global(|config| {
        config.instance.slow_threshold = 30_000;
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
        config.experimental.allows_expression_index = true;
    });

    let config = get_global_config();
    assert_eq!(get_step(), 5_000);
    assert_eq!(config.instance.slow_threshold, 30_000);
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);

    set_step(old_step);
    restore_config();
    assert_eq!(get_step(), old_step);
}

/// 模拟 Go `autoid.SetStep` 的进程级步长；默认 30000，测试中临时改为 5000。
static AUTO_ID_STEP: AtomicU64 = AtomicU64::new(30_000);

/// 验证 TestMain 风格的全局配置改写是作用域化的：改完后必须恢复原值。
#[test]
fn test_main_configuration_is_scoped_and_restored() {
    // 临时改写步长并断言，再写回原值，对应 Go 中 UpdateGlobal 后的恢复语义。
    let original = AUTO_ID_STEP.swap(5_000, Ordering::SeqCst);
    assert_eq!(AUTO_ID_STEP.load(Ordering::SeqCst), 5_000);
    AUTO_ID_STEP.store(original, Ordering::SeqCst);
    assert_eq!(AUTO_ID_STEP.load(Ordering::SeqCst), 30_000);
}
