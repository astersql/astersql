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

// 会话系统变量（session/system variable）测试包入口。
//
// 对应 Go 的 `TestMain`：在跑用例前短路 bench、初始化公共测试环境、
// 清零 AsyncCommit 时间窗口并启用 failpoint；Rust 侧用可观察的全局配置覆盖做 harness 校验。

use astersql_config::{get_global_config, update_global};
use astersql_testkit_testmain::{TestingM, WrapTestingM, benchmark_exit_code};
use std::cell::Cell;
use std::time::Duration;

/// Go 回调等待 MVCCLevelDB 异步关闭一秒，并原样返回测试退出码。
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

/// Go TestMain 必须将 Async Commit 的两个时间窗口清零，并在测试后恢复配置。
#[test]
fn variable_harness_clears_async_commit_timing_windows() {
    let restore = astersql_config::restore_func();
    update_global(|conf| {
        conf.tikv_client.async_commit.safe_window = 0;
        conf.tikv_client.async_commit.allowed_clock_drift = 0;
    });
    let config = get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    restore();
}

/// Go TestMain 的 benchmark 短路与延迟清理回调必须保持可执行契约。
#[test]
fn variable_harness_preserves_testmain_control_flow() {
    let runner = FixedExitCode(7);
    assert_eq!(benchmark_exit_code(&runner, ["variable"]), None);
    assert_eq!(
        benchmark_exit_code(&runner, ["variable", "--test.bench=Variable"]),
        Some(7)
    );
    let cleanup_delay = Cell::new(Duration::ZERO);
    let wrapped = WrapTestingM(
        runner,
        Some(Box::new(|status| {
            cleanup_exit_code_with(status, |delay| cleanup_delay.set(delay))
        })),
    );
    assert_eq!(wrapped.run(), 7);
    assert_eq!(cleanup_delay.get(), Duration::from_secs(1));
}
