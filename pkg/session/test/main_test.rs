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

//! `session/test` 顶层测试 harness 与 Go `TestMain` 的可执行语义对照。

use std::cell::Cell;
use std::time::Duration;

use astersql_config::{get_global_config, update_global};
use astersql_testkit_testmain::{TestingM, WrapTestingM, benchmark_exit_code};

/// Go `TestMain` 的执行顺序。Rust 测试框架没有同构的进程级入口，
/// 因此将不可移植的 flag/failpoint/goleak 阶段保留为显式迁移契约。
const TEST_MAIN_STAGES: [&str; 6] = [
    "short-circuit-for-bench",
    "setup-for-common-test",
    "parse-flags",
    "clear-async-commit-timing-windows",
    "enable-tikv-failpoints",
    "verify-goroutine-leaks",
];

/// 执行 Go harness 中在 Rust 可观察、可复用的初始化副作用。
fn apply_session_test_harness_config() {
    astersql_testkit_testsetup::SetupForCommonTest();
    update_global(|config| {
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
    });
}

/// 对应 Go `WrapTestingM` 回调：等待 MVCCLevelDB 收尾并原样返回退出码。
/// 注入 sleep 使单测能够验证一秒契约而无需真的延迟测试套件。
fn cleanup_exit_code_with<F>(status: i32, sleep: F) -> i32
where
    F: FnOnce(Duration),
{
    sleep(Duration::from_secs(1));
    status
}

#[derive(Clone, Copy)]
struct FixedExitCode(i32);

impl TestingM for FixedExitCode {
    fn run(&self) -> i32 {
        self.0
    }
}

#[test]
fn session_harness_applies_go_global_configuration() {
    let restore = astersql_config::restore_func();
    apply_session_test_harness_config();

    let config = get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    restore();
}

#[test]
fn session_harness_preserves_testmain_control_flow_and_cleanup() {
    let runner = FixedExitCode(7);
    assert_eq!(benchmark_exit_code(&runner, ["session-test"]), None);
    assert_eq!(
        benchmark_exit_code(&runner, ["session-test", "--test.bench=Session"]),
        Some(7)
    );

    let observed_delay = Cell::new(Duration::ZERO);
    let wrapped = WrapTestingM(
        runner,
        Some(Box::new(|status| {
            cleanup_exit_code_with(status, |delay| observed_delay.set(delay))
        })),
    );
    assert_eq!(wrapped.run(), 7);
    assert_eq!(observed_delay.get(), Duration::from_secs(1));
}
