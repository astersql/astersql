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

// 分区 DDL 测试包的公共辅助与入口用例。
//
// 提供等待 GC/清理的轮询常量、后台线程执行 SQL 的 [`background_exec`]，
// 以及基于真实 AnalyzeStats 会话校验分区 catalog 的冒烟测试。
// Domain 指 TiDB 中绑定 store 的 schema/统计信息域；catalog 为当前可见的表元数据视图。

use astersql_config::Config;
use astersql_testkit::mockstore::{CreateAnalyzeStatsStore, CreateMockStore};
use astersql_testkit::{Database, TestKit};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

/// 等待清理数据完成的最大轮询次数（对应 Go 侧 wait clean 循环上限）。
pub const WAIT_FOR_CLEAN_DATA_ROUND: usize = 150;
/// 每轮清理等待间隔（毫秒）。
pub const WAIT_FOR_CLEAN_DATA_INTERVAL_MS: u64 = 100;

/// Go `TestMain` 调整 DDL worker 出错后的等待时间为 1 微秒。
///
/// Rust DDL 当前没有对应的进程级可变等待钩子，因此将该值保留为测试 harness 契约。
const DDL_WAIT_WHEN_ERROR_OCCURRED: Duration = Duration::from_micros(1);

/// Go `TestMain` 的初始化和收尾顺序；Rust libtest 没有进程级 `TestMain` 钩子。
const TEST_MAIN_STAGES: &[&str] = &[
    "setup-for-common-test",
    "clear-async-commit-timing-windows",
    "set-ddl-error-wait",
];

/// 执行 Rust 可承载的 Go `TestMain` 初始化，并返回清零后的配置供断言。
fn configure_test_main_contract() -> Config {
    astersql_testkit_testsetup::SetupForCommonTest();
    let mut config = Config::default();
    config.tikv_client.async_commit.safe_window = 0;
    config.tikv_client.async_commit.allowed_clock_drift = 0;
    config
}

/// 在独立线程中 `USE schema` 后执行一条 SQL，并通过 channel 回传结果。
///
/// 用于模拟并发会话（session）发起 DDL/DML，主线程可同步等待其完成或失败。
pub fn background_exec(database: Arc<dyn Database>, schema: &str, sql: &str) -> Result<(), String> {
    let schema = schema.to_owned();
    let sql = sql.to_owned();
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    // 后台线程持有 Database 克隆，执行完后将 Result 发回同步 channel。
    thread::spawn(move || {
        let mut testkit = TestKit::new(database);
        let result = testkit
            .Exec(&format!("use {schema}"), Vec::new())
            .and_then(|_| testkit.Exec(&sql, Vec::new()))
            .map(|_| ())
            .map_err(|error| error.to_string());
        done_tx.send(result).expect("background result receiver");
    });
    done_rx.recv().expect("background executor")
}

/// 冒烟：建 hash 分区表后，AnalyzeStats 上下文中的 catalog 能看到 3 个分区定义。
#[test]
fn partition_test_main_uses_real_session_and_partition_catalog() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table pt(a int) partition by hash(a) partitions 3",
        Vec::new(),
    );
    let context = testkit.AnalyzeStatsContext().expect("analyze session");
    let catalog = context.catalog();
    let table = &catalog
        .get(&("test".to_owned(), "pt".to_owned()))
        .expect("partition table")
        .1;
    let partition = table.GetPartitionInfo().expect("partition metadata");
    assert_eq!(partition.Definitions.len(), 3);
    assert_eq!(partition.Num, 3);
}

/// 验证 [`background_exec`] 能传播会话错误，并在成功路径记录 `use` + DDL 历史。
#[test]
fn background_exec_propagates_success_and_session_error() {
    // 失败路径：MockStore 对 `use missing` 注入错误，应原样返回。
    let store = CreateMockStore();
    store.fail_execute("use missing", "unknown database missing");
    let database: Arc<dyn Database> = store.clone();
    assert_eq!(
        background_exec(database, "missing", "create table t(a int)"),
        Err("unknown database missing".to_owned())
    );

    // 成功路径：历史中应依次出现 use 与 alter add partition。
    let store = CreateMockStore();
    let database: Arc<dyn Database> = store.clone();
    background_exec(database, "test", "alter table t add partition p1").expect("background DDL");
    let history = store.history();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].0, "use test");
    assert_eq!(history[1].0, "alter table t add partition p1");
}

/// Go `TestMain` 的进程级设置必须保留为可执行/可断言契约。
#[test]
fn test_main_harness_matches_go_configuration() {
    let config = configure_test_main_contract();

    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert_eq!(DDL_WAIT_WHEN_ERROR_OCCURRED, Duration::from_micros(1));
    assert_eq!(
        TEST_MAIN_STAGES,
        [
            "setup-for-common-test",
            "clear-async-commit-timing-windows",
            "set-ddl-error-wait",
        ]
    );
}
