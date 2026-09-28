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

// pdhelper 包测试入口，对应 Go `TestMain` 的环境准备。
//
// 提供通用测试配置（autoid 步长、慢查询阈值、异步提交窗口等）；返回的 guard
// 在测试期间保持 failpoint 场景存活。
use std::sync::{Mutex, MutexGuard};

static TEST_MAIN_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Eq, PartialEq)]
/// 包级测试期望的全局配置快照，便于断言与 Go TestMain 一致。
pub struct TestMainConfig {
    /// 自增 ID（autoid）分配步长。
    pub autoid_step: i64,
    /// 慢查询阈值（毫秒）。
    pub slow_threshold_ms: u64,
    /// 异步提交（async commit）安全窗口。
    pub async_commit_safe_window_ms: u64,
    /// 异步提交允许的时钟漂移。
    pub async_commit_allowed_clock_drift_ms: u64,
    /// 是否允许表达式索引。
    pub allows_expression_index: bool,
}

/// `test_main` 返回的守卫：持有配置与活跃的 failpoint 场景。
pub struct TestMainGuard {
    /// 串行化进程级配置修改，避免 Rust 并行测试相互恢复全局状态。
    _exclusive: MutexGuard<'static, ()>,
    /// 期望的全局配置值。
    pub config: TestMainConfig,
    /// 保持 failpoint 场景至 guard 析构。
    _fail_scenario: fail::FailScenario<'static>,
    /// Rust 测试进程会继续运行其它测试，析构时恢复原 autoid 步长。
    previous_autoid_step: i64,
    /// Rust 测试进程会继续运行其它测试，析构时恢复原全局配置。
    previous_config: astersql_config::Config,
}

impl Drop for TestMainGuard {
    fn drop(&mut self) {
        astersql_meta_autoid::set_step(self.previous_autoid_step);
        astersql_config::store_global_config(self.previous_config.clone());
    }
}

/// 执行与 Go `TestMain` 等价的包级初始化；返回的 guard 在整个测试期间
/// 保持 Rust failpoint 激活，并保留配置供断言。
/// Performs the package-level setup from Go's `TestMain`. The returned guard
/// keeps Rust failpoints active for the complete test and retains the exact
/// configuration for assertions.
pub fn test_main() -> TestMainGuard {
    let exclusive = TEST_MAIN_LOCK
        .lock()
        .expect("pdhelper TestMain lock poisoned");
    // 对应 Go testsetup.SetupForCommonTest：安装通用测试钩子。
    testsetup::SetupForCommonTest();
    let previous_autoid_step = astersql_meta_autoid::get_step();
    let previous_config = astersql_config::get_global_config();

    astersql_meta_autoid::set_step(5_000);
    astersql_config::update_global(|config| {
        config.instance.slow_threshold = 30_000;
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
        config.experimental.allows_expression_index = true;
    });

    assert!(fail::has_failpoints());
    // 开启 failpoint 场景，供后续测试注入故障。
    let fail_scenario = fail::FailScenario::setup();

    TestMainGuard {
        _exclusive: exclusive,
        config: TestMainConfig {
            autoid_step: 5_000,
            slow_threshold_ms: 30_000,
            async_commit_safe_window_ms: 0,
            async_commit_allowed_clock_drift_ms: 0,
            allows_expression_index: true,
        },
        _fail_scenario: fail_scenario,
        previous_autoid_step,
        previous_config: previous_config.as_ref().clone(),
    }
}

#[test]
fn test_main_applies_go_process_configuration() {
    let setup = test_main();
    // The package lock is acquired by test_main. Capture the state it will
    // restore, rather than a value another parallel test may still own.
    let previous_step = setup.previous_autoid_step;
    let previous_config = setup.previous_config.clone();

    assert_eq!(astersql_meta_autoid::get_step(), setup.config.autoid_step);
    let actual = astersql_config::get_global_config();
    assert_eq!(
        actual.instance.slow_threshold,
        setup.config.slow_threshold_ms
    );
    assert_eq!(
        actual.tikv_client.async_commit.safe_window,
        setup.config.async_commit_safe_window_ms as i64
    );
    assert_eq!(
        actual.tikv_client.async_commit.allowed_clock_drift,
        setup.config.async_commit_allowed_clock_drift_ms as i64
    );
    assert_eq!(
        actual.experimental.allows_expression_index,
        setup.config.allows_expression_index
    );

    drop(setup);
    assert_eq!(astersql_meta_autoid::get_step(), previous_step);
    let restored = astersql_config::get_global_config();
    assert_eq!(
        restored.instance.slow_threshold,
        previous_config.instance.slow_threshold
    );
    assert_eq!(
        restored.tikv_client.async_commit.safe_window,
        previous_config.tikv_client.async_commit.safe_window
    );
    assert_eq!(
        restored.tikv_client.async_commit.allowed_clock_drift,
        previous_config.tikv_client.async_commit.allowed_clock_drift
    );
    assert_eq!(
        restored.experimental.allows_expression_index,
        previous_config.experimental.allows_expression_index
    );
}
