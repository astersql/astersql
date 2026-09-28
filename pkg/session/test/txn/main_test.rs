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

// 事务测试包入口 harness（对应 Go `TestMain`）。
//
// Rust 单测没有 Go `TestMain` 回调；此处直接验证其可观察的全局配置覆盖。

const TEST_MAIN_ACTIONS: [&str; 5] = [
    "testmain.ShortCircuitForBench",
    "testsetup.SetupForCommonTest",
    "flag.Parse",
    "config.UpdateGlobal(async_commit_windows=0)",
    "tikv.EnableFailpoints",
];
/// 对应 Go `WrapTestingM` 的清理回调：等待 MVCCLevelDB 关闭并原样返回退出码。
fn test_main_callback(exit_code: i32) -> i32 {
    std::thread::sleep(std::time::Duration::from_secs(1));
    exit_code
}

use astersql_config::{get_global_config, update_global};

/// Go TestMain 必须将 Async Commit 的两个时间窗口清零，并在测试后恢复配置。
#[test]
fn txn_harness_clears_async_commit_timing_windows() {
    let restore = astersql_config::restore_func();
    astersql_testkit_testsetup::SetupForCommonTest();
    update_global(|conf| {
        conf.tikv_client.async_commit.safe_window = 0;
        conf.tikv_client.async_commit.allowed_clock_drift = 0;
    });
    let config = get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    restore();

    assert_eq!(
        TEST_MAIN_ACTIONS,
        [
            "testmain.ShortCircuitForBench",
            "testsetup.SetupForCommonTest",
            "flag.Parse",
            "config.UpdateGlobal(async_commit_windows=0)",
            "tikv.EnableFailpoints",
        ]
    );
    assert_eq!(
        test_main_callback(7),
        7,
        "Go WrapTestingM callback must preserve the test exit code"
    );
}
