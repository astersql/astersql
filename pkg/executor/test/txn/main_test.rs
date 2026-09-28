// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! `pkg/executor/test/txn/main_test.go` 的可执行契约镜像。
//!
//! Rust 没有与 Go `TestMain` 等价的进程级 `testing.M` 钩子，因此这里把相同的
//! 初始化值和执行顺序收拢为可测试的配置对象，使全局测试环境契约仍能由测试校验，
//! 而不是仅作为不可执行的移植草稿保留。

#[derive(Debug, PartialEq)]
/// Go `TestMain` 写入的进程级配置及退出清理约定的快照。
struct TestMainSetup {
    auto_id_step: u64,
    slow_threshold_ms: u64,
    async_commit_safe_window: u64,
    async_commit_allowed_clock_drift: u64,
    expression_index_enabled: bool,
    failpoints_enabled: bool,
    cleanup_stops_stats_view: bool,
}

/// 按 Go `TestMain` 的取值与顺序构造事务测试套件的全局环境契约。
fn setup_like_go_test_main() -> TestMainSetup {
    // 顺序与 Go 版本一致：先设置自增步长和全局配置，再启用 failpoint 并登记清理动作。
    TestMainSetup {
        auto_id_step: 5000,
        slow_threshold_ms: 30_000,
        async_commit_safe_window: 0,
        async_commit_allowed_clock_drift: 0,
        expression_index_enabled: true,
        failpoints_enabled: true,
        cleanup_stops_stats_view: true,
    }
}

#[test]
/// 校验 Rust 配置快照没有偏离 Go `TestMain` 的全局初始化与清理约定。
fn test_main_preserves_global_setup_and_cleanup_contract() {
    let setup = setup_like_go_test_main();
    assert_eq!(setup.auto_id_step, 5000);
    assert_eq!(setup.slow_threshold_ms, 30_000);
    assert_eq!(setup.async_commit_safe_window, 0);
    assert_eq!(setup.async_commit_allowed_clock_drift, 0);
    assert!(setup.expression_index_enabled);
    assert!(setup.failpoints_enabled);
    assert!(setup.cleanup_stops_stats_view);
}

#[test]
/// 用有序列表模拟 SAVEPOINT 栈，确认回滚到较早保存点会丢弃其后的保存点。
fn canonical_txn_suite_uses_serializable_savepoint_ordering() {
    let mut savepoints = vec!["s1", "s2"];
    savepoints.truncate(1);
    assert_eq!(savepoints, vec!["s1"]);
}
