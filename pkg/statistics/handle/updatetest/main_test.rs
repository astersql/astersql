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

// 统计增量更新测试的公共会话夹具。
//
// 对齐 Go 侧 `CreateMockStoreAndDomain` + `NewTestKit`：一次性公共初始化后，
// 为每个用例提供可执行 SQL 的 `TestKit`、可访问 Domain/统计句柄的 `AnalyzeStatsStore`，
// 以及在 Drop 时关闭数据库、回收会话工作线程的守卫。

use std::sync::{Arc, Once};

use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{Database, TestKit};
use astersql_testkit_testsetup::SetupForCommonTest;

/// 进程内只执行一次的公共测试环境初始化门闩。
static COMMON_SETUP: Once = Once::new();

/// 持有测试数据库引用；析构时关闭库并等待会话工作线程退出。
pub(crate) struct TestGuard {
    /// 被测 Mock 数据库实例。
    database: Arc<dyn Database>,
}

impl Drop for TestGuard {
    fn drop(&mut self) {
        // 关闭规范测试库并 join 会话 worker，避免用例间泄漏后台线程
        self.database
            .close()
            .expect("close canonical test database and join its session worker");
    }
}

/// Same-path harness as Go `CreateMockStoreAndDomain` + `NewTestKit`.
/// Call `store.domain()` for the live Domain (stats handle / catalog / lock).
///
/// 与 Go 同路径的夹具：创建 MockStore 与 Domain，并返回绑定其上的 `TestKit`。
/// 可通过 `store.domain()` 取得实时 Domain（统计句柄 / 目录 / 锁）。
pub(crate) fn new_mock_store_and_domain() -> (TestKit, Arc<AnalyzeStatsStore>, TestGuard) {
    COMMON_SETUP.call_once(SetupForCommonTest);
    let (store, _domain) = CreateMockStoreAndDomain();
    let guard = TestGuard {
        database: store.clone(),
    };
    (TestKit::new(store.clone()), store, guard)
}

/// 仅需要执行 SQL 的精简夹具，丢弃对 AnalyzeStatsStore 的直接持有。
pub(crate) fn new_testkit() -> (TestKit, TestGuard) {
    let (testkit, _store, guard) = new_mock_store_and_domain();
    (testkit, guard)
}

/// 冒烟：公共 setup 后建表插入，并确认 Domain 能看到表统计上下文。
#[test]
fn support_common_setup_and_database_cleanup_wrap_the_session() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    testkit.MustExec("create table setup_probe(a int)", Vec::new());
    testkit.MustExec("insert into setup_probe values (1)", Vec::new());
    assert!(store.domain().stats_table("test", "setup_probe").is_some());
    assert!(testkit.AnalyzeStatsContext().is_some());
}
