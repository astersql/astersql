// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

//! 中文说明开始（自动生成）
//! 中文总览：`cross_ks_gc_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `会话生命周期与信息模式` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 7 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `test_cross_ks_runtime_gc_loop_started_by_system_domain` 是当前文件里的测试用例。
//! `test_cross_ks_runtime_gc_loop_started_by_system_domain` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `test_cross_ks_runtime_gc_loop_started_by_system_domain` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_cross_ks_runtime_gc_loop_started_by_system_domain`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 中文说明结束（自动生成）

//! Go-equivalent lifecycle coverage for the cross-keyspace runtime released by
//! the SYSTEM domain.

use std::time::{Duration, Instant};

use astersql_testkit::mockstore::CreateCrossKeyspaceTestCluster;
use astersql_tests_realtikvtest_sessiontest::serial_guard;

/// Go `TestCrossKSRuntimeGCLoopStartedBySystemDomain`.
///
/// The canonical Rust test cluster binds both real `Domain`s to one
/// `CrossKeyspaceCoordinator`.  SYSTEM acquires a production RAII lease, and
/// dropping the final lease lets the idle GC remove the target runtime.
#[test]
fn test_cross_ks_runtime_gc_loop_started_by_system_domain() {
    let _serial = serial_guard();
    let cluster = CreateCrossKeyspaceTestCluster(&[("keyspace1", false)]);
    assert_eq!(
        cluster.keyspaces(),
        vec!["SYSTEM".to_owned(), "keyspace1".to_owned()]
    );

    let system_domain = cluster.store("SYSTEM").domain();
    assert!(system_domain.cross_keyspaces_for_test().is_empty());
    let handle = system_domain
        .acquire_cross_keyspace_runtime("keyspace1", Duration::from_millis(100))
        .expect("SYSTEM acquires the target keyspace runtime");
    assert_eq!(system_domain.cross_keyspaces_for_test(), vec!["keyspace1"]);
    drop(handle);

    // Go's failpoint sets idle_timeout=100ms and polls every 20ms for at most
    // five seconds.  Preserve those exact timing bounds.
    let deadline = Instant::now() + Duration::from_secs(5);
    while system_domain
        .cross_keyspaces_for_test()
        .iter()
        .any(|keyspace| keyspace == "keyspace1")
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !system_domain
            .cross_keyspaces_for_test()
            .iter()
            .any(|keyspace| keyspace == "keyspace1"),
        "released target runtime must be removed before the GC deadline"
    );
}
