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

// `handletest` 包级测试 harness（对应 Go `TestMain` + mock store/domain 工厂）。
//
// 提供共享的 mock 存储（AnalyzeStatsStore）、Domain（会话/元数据域）与 TestKit，
// 以及 failpoint（故障注入点，进程级全局）互斥：普通测例共享读锁，
// 启用 failpoint 的测例独占写锁，避免并发互相干扰。

use std::sync::{Arc, Once, RwLock, RwLockReadGuard, RwLockWriteGuard};

use astersql_domain::Domain;
use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{Database, TestKit};
use astersql_testkit_testsetup::SetupForCommonTest;

/// 进程内只执行一次的公共测试 harness 初始化门闩。
static COMMON_SETUP: Once = Once::new();

/// Failpoints are process wide, so a test that arms one has to run alone.
/// failpoint 进程级全局，启用它的测例必须独占运行。
static FAILPOINT_EXCLUSION: RwLock<()> = RwLock::new(());

/// 测例对 failpoint 互斥锁的持有方式：共享读或独占写。
enum Exclusion {
    Shared(RwLockReadGuard<'static, ()>),
    Exclusive(RwLockWriteGuard<'static, ()>),
}

/// 测试生命周期守卫：持有互斥锁并在 Drop 时关闭 mock 数据库。
pub(crate) struct TestGuard {
    database: Arc<dyn Database>,
    _exclusion: Exclusion,
}

impl Drop for TestGuard {
    fn drop(&mut self) {
        // 关闭数据库并等待 session worker 退出，避免泄漏后台线程。
        self.database
            .close()
            .expect("close canonical test database and join its session worker");
    }
}

/// 触发公共测试环境初始化（对应 Go `testsetup.SetupForCommonTest`）。
pub(crate) fn setup_common_tests() {
    COMMON_SETUP.call_once(SetupForCommonTest);
}

/// Same harness shape as Go `testkit.CreateMockStoreAndDomain(t)`.
/// 与 Go `CreateMockStoreAndDomain` 同形：共享读锁下创建 store/domain/TestKit。
pub(crate) fn new_store_and_domain() -> (Arc<AnalyzeStatsStore>, Arc<Domain>, TestKit, TestGuard) {
    new_harness(Exclusion::Shared(
        FAILPOINT_EXCLUSION
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    ))
}

/// Harness for tests that enable a process-wide failpoint: it blocks every
/// other test in the package for as long as the failpoint is armed.
/// 独占写锁 harness：failpoint 武装期间阻塞包内其他测例。
pub(crate) fn new_exclusive_store_and_domain()
-> (Arc<AnalyzeStatsStore>, Arc<Domain>, TestKit, TestGuard) {
    new_harness(Exclusion::Exclusive(
        FAILPOINT_EXCLUSION
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    ))
}

/// 在给定互斥模式下创建 mock store、Domain、TestKit 与生命周期守卫。
fn new_harness(exclusion: Exclusion) -> (Arc<AnalyzeStatsStore>, Arc<Domain>, TestKit, TestGuard) {
    setup_common_tests();
    let (store, domain) = CreateMockStoreAndDomain();
    let guard = TestGuard {
        database: store.clone(),
        _exclusion: exclusion,
    };
    let testkit = TestKit::new(store.clone());
    (store, domain, testkit, guard)
}

/// 仅需要 TestKit 的便捷工厂（内部仍走共享 harness）。
pub(crate) fn new_testkit() -> (TestKit, TestGuard) {
    let (_store, _domain, testkit, guard) = new_store_and_domain();
    (testkit, guard)
}

/// 冒烟：建表后 Domain 可见，且 Analyze 统计上下文已挂载。
#[test]
fn go_test_main_common_setup_and_domain_harness() {
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("create table setup_probe(a int)", Vec::new());
    assert!(domain.table_by_name("test", "setup_probe").is_ok());
    assert!(testkit.AnalyzeStatsContext().is_some());
}
