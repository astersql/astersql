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

// 临时表测试包入口 harness（对应 Go `TestMain`）。
//
// 可执行测试校验 temporarytabletest 依赖的 bootstrap 变量名常量。
use std::time::Duration;

use astersql_config::{get_global_config, restore_func, update_global};
use astersql_session::bootstrap::{tidbClusterID, tidbDDLTableVersion};

/// 应用 Go `TestMain` 中会影响临时表测试行为的公共初始化与全局配置副作用。
fn apply_temporarytabletest_harness_config() {
    astersql_testkit_testsetup::SetupForCommonTest();
    update_global(|conf| {
        conf.tikv_client.async_commit.safe_window = 0;
        conf.tikv_client.async_commit.allowed_clock_drift = 0;
    });
}

/// 等待 MVCCLevelDB 关闭后原样返回测试退出码，对应 Go `WrapTestingM` 回调。
fn wait_for_mvcc_leveldb(exit_code: i32) -> i32 {
    std::thread::sleep(Duration::from_secs(1));
    exit_code
}

/// Go `TestMain` 必须真实清零 Async Commit 的两个时间窗口，并可由测试恢复全局状态。
#[test]
fn temporarytabletest_harness_clears_async_commit_timing_windows() {
    let restore = restore_func();
    apply_temporarytabletest_harness_config();

    let config = get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);

    restore();
}
/// 资源收尾回调不改变测试结果，并保留 Go 等待一秒关闭 MVCCLevelDB 的语义。
#[test]
fn temporarytabletest_harness_waits_for_mvcc_cleanup() {
    let started = std::time::Instant::now();
    assert_eq!(wait_for_mvcc_leveldb(7), 7);
    assert!(started.elapsed() >= Duration::from_secs(1));
}

/// 断言 temporarytabletest harness 依赖的 `cluster_id` / `ddl_table_version` 变量名。
// 对应 TestMain 中 temporarytabletest 测试入口依赖的公共 bootstrap 变量名，
// 验证 temporarytabletest 包实际引用的 session 常量与 Go 保持一致。
#[test]
fn temporarytabletest_harness_exposes_canonical_bootstrap_variable_names() {
    assert_eq!(tidbClusterID, "cluster_id");
    assert_eq!(tidbDDLTableVersion, "ddl_table_version");
}
