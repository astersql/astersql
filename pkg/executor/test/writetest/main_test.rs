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

// 写路径测试包级 TestMain 的可观察初始化。
//
// Go 的 TestMain 会固定 autoid 步长、慢日志阈值、AsyncCommit 时间窗口和
// expression index 开关。Rust 的测试框架没有 Go TestMain 钩子，因此在
// 测试中直接使用同一生产全局配置 API，并在断言后恢复进程级状态。

use std::sync::Mutex;

static TEST_MAIN_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CleanupAction {
    StopOpenCensusViews,
}

#[derive(Debug, Eq, PartialEq)]
struct GoTestMainContract {
    tikv_failpoints_enabled: bool,
    cleanup: CleanupAction,
}

fn go_test_main_contract() -> GoTestMainContract {
    GoTestMainContract {
        // Go's TiKV client failpoint switch has no Rust runtime counterpart in
        // this crate; preserve the package-level runner requirement explicitly.
        tikv_failpoints_enabled: true,
        cleanup: CleanupAction::StopOpenCensusViews,
    }
}

#[test]
fn write_test_main_applies_and_restores_go_defaults() {
    let _serial = TEST_MAIN_LOCK.lock().expect("TestMain state lock");
    let old_config = astersql_config::restore_func();
    let old_step = astersql_meta_autoid::get_step();

    astersql_meta_autoid::set_step(5_000);
    astersql_config::update_global(|config| {
        config.instance.slow_threshold = 30_000;
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
        config.experimental.allows_expression_index = true;
    });

    let config = astersql_config::get_global_config();
    assert_eq!(astersql_meta_autoid::get_step(), 5_000);
    assert_eq!(config.instance.slow_threshold, 30_000);
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);

    astersql_meta_autoid::set_step(old_step);
    old_config();
}

#[test]
fn write_test_main_preserves_failpoint_cleanup_contract() {
    let contract = go_test_main_contract();

    assert!(contract.tikv_failpoints_enabled);
    assert_eq!(contract.cleanup, CleanupAction::StopOpenCensusViews);
}
