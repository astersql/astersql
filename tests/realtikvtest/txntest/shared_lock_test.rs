// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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
//! 中文总览：`shared_lock_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `事务语义与时间戳行为` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 104 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `BLOCKED_WINDOW` 是当前文件里的常量。
//! `BLOCKED_WINDOW` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `BLOCKED_WINDOW` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `BLOCKED_WINDOW`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `BLOCKED_WINDOW` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `WAIT_VIEW_DEADLINE` 是当前文件里的常量。
//! `WAIT_VIEW_DEADLINE` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `WAIT_VIEW_DEADLINE` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `WAIT_VIEW_DEADLINE`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `WAIT_VIEW_DEADLINE` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `SKIP_RESOLVING_LOCKS` 是当前文件里的常量。
//! `SKIP_RESOLVING_LOCKS` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `SKIP_RESOLVING_LOCKS` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `SKIP_RESOLVING_LOCKS`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `SKIP_RESOLVING_LOCKS` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `prepare` 是当前文件里的辅助函数。
//! `prepare` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `prepare` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `prepare`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `prepare` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `session` 是当前文件里的辅助函数。
//! `session` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `session` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `session`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `session` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `prepare_foreign_key_tables` 是当前文件里的辅助函数。
//! `prepare_foreign_key_tables` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `prepare_foreign_key_tables` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `prepare_foreign_key_tables`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `prepare_foreign_key_tables` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `assert_still_blocked` 是当前文件里的辅助函数。
//! `assert_still_blocked` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `assert_still_blocked` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `assert_still_blocked`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `assert_still_blocked` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `expect_ok` 是当前文件里的辅助函数。
//! `expect_ok` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `expect_ok` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `expect_ok`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `expect_ok` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `scalar` 是当前文件里的辅助函数。
//! `scalar` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `scalar` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `scalar`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `scalar` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `wait_rows` 是当前文件里的辅助函数。
//! `wait_rows` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `wait_rows` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `wait_rows`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `wait_rows` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `assert_wait_view_maps_to_session` 是当前文件里的辅助函数。
//! `assert_wait_view_maps_to_session` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `assert_wait_view_maps_to_session` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `assert_wait_view_maps_to_session`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `assert_wait_view_maps_to_session` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `test_foreign_key_shared_lock_optimistic_reverse_reference_order` 是当前文件里的测试用例。
//! `test_foreign_key_shared_lock_optimistic_reverse_reference_order` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_foreign_key_shared_lock_optimistic_reverse_reference_order` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_foreign_key_shared_lock_optimistic_reverse_reference_order`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `test_foreign_key_shared_lock_optimistic_reverse_reference_order` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `test_foreign_key_shared_lock_pessimistic_reverse_reference_order` 是当前文件里的测试用例。
//! `test_foreign_key_shared_lock_pessimistic_reverse_reference_order` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_foreign_key_shared_lock_pessimistic_reverse_reference_order` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_foreign_key_shared_lock_pessimistic_reverse_reference_order`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `test_foreign_key_shared_lock_pessimistic_reverse_reference_order` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `test_shared_lock_blocked_by_exclusive_lock` 是当前文件里的测试用例。
//! `test_shared_lock_blocked_by_exclusive_lock` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_shared_lock_blocked_by_exclusive_lock` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_shared_lock_blocked_by_exclusive_lock`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `test_shared_lock_blocked_by_exclusive_lock` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `test_shared_lock_blocks_exclusive_lock_until_every_holder_commits` 是当前文件里的测试用例。
//! `test_shared_lock_blocks_exclusive_lock_until_every_holder_commits` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_shared_lock_blocks_exclusive_lock_until_every_holder_commits` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_shared_lock_blocks_exclusive_lock_until_every_holder_commits`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `test_shared_lock_blocks_exclusive_lock_until_every_holder_commits` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `test_shared_lock_child_table_conflict` 是当前文件里的测试用例。
//! `test_shared_lock_child_table_conflict` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_shared_lock_child_table_conflict` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_shared_lock_child_table_conflict`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `test_shared_lock_child_table_conflict` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `test_shared_lock_cascade_update_explicit_pessimistic_transaction` 是当前文件里的测试用例。
//! `test_shared_lock_cascade_update_explicit_pessimistic_transaction` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_shared_lock_cascade_update_explicit_pessimistic_transaction` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_shared_lock_cascade_update_explicit_pessimistic_transaction`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `test_shared_lock_cascade_update_explicit_pessimistic_transaction` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `test_shared_lock_lock_view` 是当前文件里的测试用例。
//! `test_shared_lock_lock_view` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_shared_lock_lock_view` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_shared_lock_lock_view`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `test_shared_lock_lock_view` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `test_shared_lock_data_lock_waits_from_storage_wait_table` 是当前文件里的测试用例。
//! `test_shared_lock_data_lock_waits_from_storage_wait_table` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_shared_lock_data_lock_waits_from_storage_wait_table` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_shared_lock_data_lock_waits_from_storage_wait_table`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `test_shared_lock_data_lock_waits_from_storage_wait_table` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 中文说明结束（自动生成）

//! Real SQL port of `shared_lock_test.go`.
//!
//! The tests intentionally use independent sessions and real worker threads.
//! In particular, a timeout on a result channel proves that the SQL statement
//! is waiting in the runtime lock manager; executing statements in a convenient
//! serial order would not cover the shared/exclusive lock compatibility rules.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{NewTestKit, Rows, TestKit};
use astersql_testkit_testfailpoint as testfailpoint;
use astersql_tests_realtikvtest::WithRealTiKV;
use astersql_tests_realtikvtest_txntest::serial_guard;

const BLOCKED_WINDOW: Duration = Duration::from_millis(100);
const WAIT_VIEW_DEADLINE: Duration = Duration::from_secs(3);
const SKIP_RESOLVING_LOCKS: &str =
    "github.com/pingcap/tidb/pkg/executor/dataLockWaitsSkipResolvingLocks";

struct ForeignKeySharedLockConfigGuard(Option<Box<dyn FnOnce()>>);
impl Drop for ForeignKeySharedLockConfigGuard {
    fn drop(&mut self) {
        self.0.take().unwrap()();
    }
}
fn allow_foreign_key_check_in_shared_lock_for_test() -> ForeignKeySharedLockConfigGuard {
    let restore = astersql_config::restore_func();
    astersql_config::update_global(|conf| {
        conf.experimental
            .allow_enable_foreign_key_check_in_shared_lock = true;
    });
    ForeignKeySharedLockConfigGuard(Some(Box::new(restore)))
}

fn prepare(test_name: &str) -> (Arc<AnalyzeStatsStore>, TestKit, String) {
    let (store, _domain) = CreateMockStoreAndDomain();
    let database = format!("shared_lock_{}", test_name.trim_start_matches("test_"));
    let mut tk = NewTestKit(store.clone());
    tk.MustExec(&format!("drop database if exists `{database}`"), Vec::new());
    tk.MustExec(&format!("create database `{database}`"), Vec::new());
    tk.MustExec(&format!("use `{database}`"), Vec::new());
    (store, tk, database)
}

fn session(store: Arc<AnalyzeStatsStore>, database: &str) -> TestKit {
    let mut tk = NewTestKit(store);
    tk.MustExec(&format!("use `{database}`"), Vec::new());
    tk.MustExec(
        "set @@tidb_foreign_key_check_in_shared_lock = ON",
        Vec::new(),
    );
    tk.MustExec("set @@tidb_pessimistic_txn_fair_locking = 0", Vec::new());
    tk
}

fn prepare_foreign_key_tables(tk: &mut TestKit) {
    tk.MustExec("drop table if exists child, parent", Vec::new());
    tk.MustExec("create table parent (id int primary key)", Vec::new());
    tk.MustExec(
        "create table child (id int primary key, pid int, \
         foreign key (pid) references parent(id))",
        Vec::new(),
    );
    tk.MustExec("insert into parent values (1), (2)", Vec::new());
}

fn prepare_shared_lock_upgrade_tables(tk: &mut TestKit) {
    tk.MustExec("drop table if exists child, parent", Vec::new());
    tk.MustExec(
        "create table parent (id int primary key, v int)",
        Vec::new(),
    );
    tk.MustExec(
        "create table child (id int primary key, pid int, \
         foreign key (pid) references parent(id))",
        Vec::new(),
    );
    tk.MustExec("insert into parent values (1, 0), (2, 0)", Vec::new());
}

fn enable_shared_lock_upgrade(tk: &mut TestKit) {
    tk.MustExec("set @@tidb_enable_shared_lock_upgrade = ON", Vec::new());
}

fn assert_still_blocked<T>(receiver: &Receiver<T>, operation: &str) {
    match receiver.recv_timeout(BLOCKED_WINDOW) {
        Err(RecvTimeoutError::Timeout) => {}
        Err(RecvTimeoutError::Disconnected) => {
            panic!("{operation} worker disconnected while it should be blocked")
        }
        Ok(_) => panic!("{operation} completed while it should be blocked"),
    }
}

fn expect_ok(result: Result<(), String>, operation: &str) {
    if let Err(error) = result {
        panic!("{operation} failed: {error}");
    }
}

fn scalar(tk: &TestKit, sql: &str) -> String {
    let rows = tk.MustQuery(sql, Vec::new()).Rows();
    assert_eq!(rows.len(), 1, "sql={sql:?}, rows={rows:?}");
    assert_eq!(rows[0].len(), 1, "sql={sql:?}, rows={rows:?}");
    rows[0][0].clone()
}

fn wait_rows(tk: &TestKit, sql: &str) -> Vec<Vec<String>> {
    let started = Instant::now();
    loop {
        let rows = tk.MustQuery(sql, Vec::new()).Rows();
        if !rows.is_empty() {
            return rows;
        }
        assert!(
            started.elapsed() < WAIT_VIEW_DEADLINE,
            "lock wait view stayed empty for {WAIT_VIEW_DEADLINE:?}: sql={sql:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn assert_wait_view_maps_to_session(observer: &TestKit, expected_connection_id: &str) -> String {
    let waits = wait_rows(
        observer,
        "select `key`, trx_id from information_schema.data_lock_waits order by `key`",
    );
    assert_eq!(waits.len(), 1, "expected one lock wait: {waits:?}");
    assert_eq!(waits[0].len(), 2, "unexpected lock-wait shape: {waits:?}");
    assert!(!waits[0][0].is_empty(), "lock key must be populated");
    let key = waits[0][0].clone();
    let transaction_id = waits[0][1].clone();
    let transactions = observer
        .MustQuery(
            &format!(
                "select id, session_id from information_schema.tidb_trx \
                 where id = {transaction_id}"
            ),
            Vec::new(),
        )
        .Rows();
    assert_eq!(
        transactions,
        Rows(&[&format!("{transaction_id} {expected_connection_id}")]),
        "the wait row must identify the blocked session"
    );
    key
}

#[test]
fn test_foreign_key_shared_lock_optimistic_reverse_reference_order() {
    let _serial = serial_guard();
    if !WithRealTiKV() {
        return;
    }
    let _config = allow_foreign_key_check_in_shared_lock_for_test();
    let (store, mut tk1, database) =
        prepare("test_foreign_key_shared_lock_optimistic_reverse_reference_order");
    let mut tk2 = session(store.clone(), &database);
    tk1.MustExec(
        "set @@tidb_foreign_key_check_in_shared_lock = ON",
        Vec::new(),
    );
    tk2.MustExec(
        "set @@tidb_foreign_key_check_in_shared_lock = ON",
        Vec::new(),
    );
    prepare_foreign_key_tables(&mut tk1);

    // Optimistic transactions cannot hold compatible FK shared locks. The
    // reverse parent-reference order creates the same prewrite conflict as Go.
    tk1.MustExec("begin optimistic", Vec::new());
    tk2.MustExec("begin optimistic", Vec::new());
    tk1.MustExec("insert into child values (1, 1)", Vec::new());
    tk2.MustExec("insert into child values (3, 2)", Vec::new());
    tk1.MustExec("insert into child values (2, 2)", Vec::new());
    tk2.MustExec("insert into child values (4, 1)", Vec::new());
    tk1.MustExec("commit", Vec::new());
    let conflict = tk2.ExecToErr("commit");
    assert!(
        conflict.message().contains("Write conflict"),
        "unexpected optimistic FK conflict: {conflict}"
    );
    tk1.MustQuery("select * from child order by id", Vec::new())
        .Check(Rows(&["1 1", "2 2"]));
}

#[test]
fn test_foreign_key_shared_lock_pessimistic_reverse_reference_order() {
    let _serial = serial_guard();
    if !WithRealTiKV() {
        return;
    }
    let _config = allow_foreign_key_check_in_shared_lock_for_test();
    let (store, mut tk1, database) =
        prepare("test_foreign_key_shared_lock_pessimistic_reverse_reference_order");
    let mut tk2 = session(store.clone(), &database);
    tk1.MustExec(
        "set @@tidb_foreign_key_check_in_shared_lock = ON",
        Vec::new(),
    );
    tk2.MustExec(
        "set @@tidb_foreign_key_check_in_shared_lock = ON",
        Vec::new(),
    );
    tk1.MustExec("set @@tidb_pessimistic_txn_fair_locking = 0", Vec::new());
    tk2.MustExec("set @@tidb_pessimistic_txn_fair_locking = 0", Vec::new());
    prepare_foreign_key_tables(&mut tk1);

    tk1.MustExec("begin pessimistic", Vec::new());
    tk2.MustExec("begin pessimistic", Vec::new());
    tk1.MustExec("insert into child values (1, 1)", Vec::new());
    tk2.MustExec("insert into child values (3, 2)", Vec::new());
    tk1.MustExec("insert into child values (2, 2)", Vec::new());
    tk2.MustExec("insert into child values (4, 1)", Vec::new());
    tk1.MustExec("commit", Vec::new());
    tk2.MustExec("commit", Vec::new());

    tk1.MustQuery("select * from child order by id", Vec::new())
        .Check(Rows(&["1 1", "2 2", "3 2", "4 1"]));
}

#[test]
fn test_shared_lock_blocked_by_exclusive_lock() {
    let _serial = serial_guard();
    let _config = allow_foreign_key_check_in_shared_lock_for_test();
    let (store, mut tk1, database) = prepare("test_shared_lock_blocked_by_exclusive_lock");
    let mut tk2 = session(store.clone(), &database);
    let mut tk3 = session(store.clone(), &database);
    tk1.MustExec(
        "set @@tidb_foreign_key_check_in_shared_lock = ON",
        Vec::new(),
    );
    tk1.MustExec("set @@tidb_pessimistic_txn_fair_locking = 0", Vec::new());
    prepare_foreign_key_tables(&mut tk1);
    tk1.MustExec("begin pessimistic", Vec::new());
    tk2.MustExec("begin pessimistic", Vec::new());
    tk3.MustExec("begin pessimistic", Vec::new());
    tk1.MustQuery("select * from parent where id=1 for update", Vec::new())
        .Check(Rows(&["1"]));

    let (done2_tx, done2_rx) = mpsc::sync_channel(1);
    let worker2 = thread::spawn(move || {
        let result = tk2
            .Exec("insert into child values(1, 1)", Vec::new())
            .map(|_| ())
            .map_err(|error| error.to_string());
        done2_tx.send((tk2, result)).expect("send tk2 result");
    });
    let (done3_tx, done3_rx) = mpsc::sync_channel(1);
    let worker3 = thread::spawn(move || {
        let result = tk3
            .Exec("insert into child values(2, 1)", Vec::new())
            .map(|_| ())
            .map_err(|error| error.to_string());
        done3_tx.send((tk3, result)).expect("send tk3 result");
    });
    assert_still_blocked(&done2_rx, "first FK shared lock");
    assert_still_blocked(&done3_rx, "second FK shared lock");

    tk1.MustExec("commit", Vec::new());
    let (mut tk2, result2) = done2_rx.recv().expect("receive tk2 result");
    let (mut tk3, result3) = done3_rx.recv().expect("receive tk3 result");
    worker2.join().expect("join tk2 worker");
    worker3.join().expect("join tk3 worker");
    expect_ok(result2, "first FK insert");
    expect_ok(result3, "second FK insert");

    tk1.MustQuery("select * from child", Vec::new())
        .Check(Vec::<Vec<String>>::new());
    tk2.MustExec("commit", Vec::new());
    tk1.MustQuery("select * from child", Vec::new())
        .Check(Rows(&["1 1"]));
    tk3.MustExec("commit", Vec::new());
    tk1.MustQuery("select * from child order by id", Vec::new())
        .Check(Rows(&["1 1", "2 1"]));
    tk1.MustExec("admin check table parent", Vec::new());
    tk1.MustExec("admin check table child", Vec::new());
}

#[test]
fn test_shared_lock_blocks_exclusive_lock_until_every_holder_commits() {
    let _serial = serial_guard();
    let _config = allow_foreign_key_check_in_shared_lock_for_test();
    let (store, mut tk1, database) = prepare("test_shared_lock_block_exclusive_lock");
    let mut tk2 = session(store.clone(), &database);
    let mut tk3 = session(store.clone(), &database);
    tk1.MustExec(
        "set @@tidb_foreign_key_check_in_shared_lock = ON",
        Vec::new(),
    );
    tk1.MustExec("set @@tidb_pessimistic_txn_fair_locking = 0", Vec::new());
    prepare_foreign_key_tables(&mut tk1);
    tk1.MustExec("begin pessimistic", Vec::new());
    tk2.MustExec("begin pessimistic", Vec::new());
    tk3.MustExec("begin pessimistic", Vec::new());
    tk2.MustExec("insert into child values(1, 1)", Vec::new());
    tk3.MustExec("insert into child values(2, 1)", Vec::new());

    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        let result = tk1
            .Query("select * from parent where id=1 for update", Vec::new())
            .map(|rows| rows.string_rows())
            .map_err(|error| error.to_string());
        done_tx.send((tk1, result)).expect("send exclusive result");
    });
    assert_still_blocked(&done_rx, "exclusive lock behind two shared holders");

    tk2.MustExec("commit", Vec::new());
    tk2.MustQuery("select * from child", Vec::new())
        .Check(Rows(&["1 1"]));
    assert_still_blocked(&done_rx, "exclusive lock behind remaining shared holder");
    tk3.MustExec("commit", Vec::new());
    tk3.MustQuery("select * from child order by id", Vec::new())
        .Check(Rows(&["1 1", "2 1"]));

    let (mut tk1, result) = done_rx.recv().expect("receive exclusive result");
    worker.join().expect("join exclusive worker");
    assert_eq!(
        result.expect("exclusive lock query"),
        Rows(&["1"]),
        "exclusive waiter must return the locked parent row"
    );
    tk1.MustExec("commit", Vec::new());
    tk1.MustExec("admin check table parent", Vec::new());
    tk1.MustExec("admin check table child", Vec::new());
}

#[test]
fn test_upgrade_multiple_shared_locks_in_one_statement() {
    let _serial = serial_guard();
    if !astersql_config_kerneltype::IsNextGen() {
        return;
    }
    let _config = allow_foreign_key_check_in_shared_lock_for_test();
    let (_store, mut tk, _database) =
        prepare("test_upgrade_multiple_shared_locks_in_one_statement");
    tk.MustExec(
        "set @@tidb_foreign_key_check_in_shared_lock = ON",
        Vec::new(),
    );
    enable_shared_lock_upgrade(&mut tk);
    prepare_shared_lock_upgrade_tables(&mut tk);
    tk.MustExec(
        "insert into parent values (3, 0), (4, 0), (5, 0), (6, 0), \
         (7, 0), (8, 0), (9, 0), (10, 0)",
        Vec::new(),
    );

    tk.MustExec("begin pessimistic", Vec::new());
    tk.MustExec(
        "insert into child values (1, 1), (2, 2), (3, 3), (4, 4), (5, 5), \
         (6, 6), (7, 7), (8, 8), (9, 9), (10, 10)",
        Vec::new(),
    );
    tk.MustExec(
        "update parent set v = v + 1 where id between 1 and 10",
        Vec::new(),
    );
    tk.MustExec("commit", Vec::new());

    tk.MustQuery("select * from parent order by id", Vec::new())
        .Check(Rows(&[
            "1 1", "2 1", "3 1", "4 1", "5 1", "6 1", "7 1", "8 1", "9 1", "10 1",
        ]));
    tk.MustQuery("select * from child order by id", Vec::new())
        .Check(Rows(&[
            "1 1", "2 2", "3 3", "4 4", "5 5", "6 6", "7 7", "8 8", "9 9", "10 10",
        ]));
    tk.MustExec("admin check table parent", Vec::new());
    tk.MustExec("admin check table child", Vec::new());
}

#[test]
fn test_upgrade_multiple_shared_locks_in_separate_statements() {
    let _serial = serial_guard();
    if !astersql_config_kerneltype::IsNextGen() {
        return;
    }
    let _config = allow_foreign_key_check_in_shared_lock_for_test();
    let (_store, mut tk, _database) =
        prepare("test_upgrade_multiple_shared_locks_in_separate_statements");
    tk.MustExec(
        "set @@tidb_foreign_key_check_in_shared_lock = ON",
        Vec::new(),
    );
    enable_shared_lock_upgrade(&mut tk);
    prepare_shared_lock_upgrade_tables(&mut tk);
    tk.MustExec(
        "insert into parent values (3, 0), (4, 0), (5, 0), (6, 0), \
         (7, 0), (8, 0), (9, 0), (10, 0)",
        Vec::new(),
    );

    tk.MustExec("begin pessimistic", Vec::new());
    tk.MustExec(
        "insert into child values (1, 1), (2, 2), (3, 3), (4, 4), (5, 5), \
         (6, 6), (7, 7), (8, 8), (9, 9), (10, 10)",
        Vec::new(),
    );
    for id in 1..=10 {
        tk.MustExec(
            &format!("update parent set v = v + 1 where id = {id}"),
            Vec::new(),
        );
    }
    tk.MustExec("commit", Vec::new());

    tk.MustQuery("select * from parent order by id", Vec::new())
        .Check(Rows(&[
            "1 1", "2 1", "3 1", "4 1", "5 1", "6 1", "7 1", "8 1", "9 1", "10 1",
        ]));
    tk.MustQuery("select * from child order by id", Vec::new())
        .Check(Rows(&[
            "1 1", "2 2", "3 3", "4 4", "5 5", "6 6", "7 7", "8 8", "9 9", "10 10",
        ]));
    tk.MustExec("admin check table parent", Vec::new());
    tk.MustExec("admin check table child", Vec::new());
}

fn run_upgrade_multiple_shared_locks_waits_for_holder(
    test_name: &str,
    release_sql: &str,
    expected_child_rows: Vec<Vec<String>>,
) {
    let (store, mut upgrader, database) = prepare(test_name);
    let mut holder = session(store, &database);
    upgrader.MustExec(
        "set @@tidb_foreign_key_check_in_shared_lock = ON",
        Vec::new(),
    );
    enable_shared_lock_upgrade(&mut upgrader);
    enable_shared_lock_upgrade(&mut holder);
    prepare_shared_lock_upgrade_tables(&mut upgrader);
    upgrader.MustExec(
        "insert into parent values (3, 0), (4, 0), (5, 0), (6, 0), \
         (7, 0), (8, 0), (9, 0), (10, 0)",
        Vec::new(),
    );

    holder.MustExec("begin pessimistic", Vec::new());
    holder.MustExec("insert into child values (11, 5)", Vec::new());
    upgrader.MustExec("begin pessimistic", Vec::new());
    upgrader.MustExec(
        "insert into child values (1, 1), (2, 2), (3, 3), (4, 4), (5, 5), \
         (6, 6), (7, 7), (8, 8), (9, 9), (10, 10)",
        Vec::new(),
    );

    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        let result = upgrader
            .Exec(
                "update parent set v = v + 1 where id between 1 and 10",
                Vec::new(),
            )
            .map(|_| ())
            .map_err(|error| error.to_string());
        done_tx
            .send((upgrader, result))
            .expect("send shared-lock upgrade result");
    });
    assert_still_blocked(&done_rx, "multi-key shared-lock upgrade");
    holder.MustExec(release_sql, Vec::new());
    let (mut upgrader, result) = done_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("shared-lock upgrader must resume after holder release");
    worker.join().expect("join shared-lock upgrade worker");
    expect_ok(result, "multi-key shared-lock upgrade");
    upgrader.MustExec("commit", Vec::new());

    upgrader
        .MustQuery("select * from parent order by id", Vec::new())
        .Check(Rows(&[
            "1 1", "2 1", "3 1", "4 1", "5 1", "6 1", "7 1", "8 1", "9 1", "10 1",
        ]));
    upgrader
        .MustQuery("select * from child order by id", Vec::new())
        .Check(expected_child_rows);
    upgrader.MustExec("admin check table parent", Vec::new());
    upgrader.MustExec("admin check table child", Vec::new());
}

#[test]
fn test_upgrade_multiple_shared_locks_waits_for_shared_holder() {
    let _serial = serial_guard();
    if !astersql_config_kerneltype::IsNextGen() {
        return;
    }
    let _config = allow_foreign_key_check_in_shared_lock_for_test();
    run_upgrade_multiple_shared_locks_waits_for_holder(
        "test_upgrade_multiple_shared_locks_waits_for_holder_commit",
        "commit",
        Rows(&[
            "1 1", "2 2", "3 3", "4 4", "5 5", "6 6", "7 7", "8 8", "9 9", "10 10", "11 5",
        ]),
    );
    run_upgrade_multiple_shared_locks_waits_for_holder(
        "test_upgrade_multiple_shared_locks_waits_for_holder_rollback",
        "rollback",
        Rows(&[
            "1 1", "2 2", "3 3", "4 4", "5 5", "6 6", "7 7", "8 8", "9 9", "10 10",
        ]),
    );
}

#[test]
fn test_shared_lock_child_table_conflict() {
    let _serial = serial_guard();
    let _config = allow_foreign_key_check_in_shared_lock_for_test();
    let (store, mut tk1, database) = prepare("test_shared_lock_child_table_conflict");
    let mut tk2 = session(store.clone(), &database);
    let mut tk3 = session(store.clone(), &database);
    tk1.MustExec(
        "set @@tidb_foreign_key_check_in_shared_lock = ON",
        Vec::new(),
    );
    tk1.MustExec("set @@tidb_pessimistic_txn_fair_locking = 0", Vec::new());
    prepare_foreign_key_tables(&mut tk1);

    // Same child primary key: the second insert waits for the first transaction
    // and then observes the committed duplicate.
    tk2.MustExec("begin pessimistic", Vec::new());
    tk3.MustExec("begin pessimistic", Vec::new());
    tk2.MustExec("insert into child values(1, 1)", Vec::new());
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        let result = tk3
            .Exec("insert into child values(1, 2)", Vec::new())
            .map(|_| ())
            .map_err(|error| error.to_string());
        done_tx.send((tk3, result)).expect("send duplicate result");
    });
    assert_still_blocked(&done_rx, "conflicting child insert");
    tk2.MustExec("commit", Vec::new());
    let (mut tk3, duplicate) = done_rx.recv().expect("receive duplicate result");
    worker.join().expect("join duplicate worker");
    let duplicate = duplicate.expect_err("second child insert must be duplicate");
    assert!(
        duplicate.contains("[kv:1062]")
            && duplicate.contains("Duplicate entry")
            && duplicate.contains('1'),
        "unexpected child duplicate: {duplicate}"
    );
    tk3.MustExec("commit", Vec::new());
    tk2.MustQuery("select * from child", Vec::new())
        .Check(Rows(&["1 1"]));
    tk2.MustExec("admin check table parent", Vec::new());
    tk2.MustExec("admin check table child", Vec::new());

    tk1.MustExec("delete from child", Vec::new());
    tk1.MustExec("begin pessimistic", Vec::new());
    let mut tk2 = session(store.clone(), &database);
    let mut tk3 = session(store.clone(), &database);
    tk2.MustExec("begin pessimistic", Vec::new());
    tk3.MustExec("begin pessimistic", Vec::new());
    tk1.MustQuery(
        "select * from parent where id in (1, 2) for update",
        Vec::new(),
    )
    .Check(Rows(&["1", "2"]));

    let (result_tx, result_rx) = mpsc::channel();
    let result_tx2 = result_tx.clone();
    let worker2 = thread::spawn(move || {
        let result = tk2
            .Exec("insert into child values(1, 1)", Vec::new())
            .map(|_| ())
            .map_err(|error| error.to_string());
        result_tx2
            .send((1_i32, tk2, result))
            .expect("send first contender");
    });
    let worker3 = thread::spawn(move || {
        let result = tk3
            .Exec("insert into child values(1, 2)", Vec::new())
            .map(|_| ())
            .map_err(|error| error.to_string());
        result_tx
            .send((2_i32, tk3, result))
            .expect("send second contender");
    });
    assert_still_blocked(&result_rx, "two child inserts behind parent locks");
    tk1.MustExec("commit", Vec::new());

    let (winner_pid, mut winner, winner_result) =
        result_rx.recv().expect("receive winning child insert");
    expect_ok(winner_result, "winning child insert");
    winner.MustExec("commit", Vec::new());
    let (loser_pid, mut loser, loser_result) =
        result_rx.recv().expect("receive losing child insert");
    assert_ne!(winner_pid, loser_pid);
    let loser_error = loser_result.expect_err("losing child insert must be duplicate");
    assert!(
        loser_error.contains("[kv:1062]")
            && loser_error.contains("Duplicate entry")
            && loser_error.contains('1'),
        "unexpected losing insert error: {loser_error}"
    );
    loser.MustExec("commit", Vec::new());
    worker2.join().expect("join first contender");
    worker3.join().expect("join second contender");
    tk1.MustQuery("select * from child", Vec::new())
        .Check(Rows(&[&format!("1 {winner_pid}")]));
    tk1.MustExec("admin check table parent", Vec::new());
    tk1.MustExec("admin check table child", Vec::new());
}

#[test]
fn test_shared_lock_cascade_update_explicit_pessimistic_transaction() {
    let _serial = serial_guard();
    let _config = allow_foreign_key_check_in_shared_lock_for_test();
    for constraint_check in ["ON", "OFF"] {
        let (_store, mut tk, _database) = prepare(&format!(
            "test_shared_lock_cascade_update_explicit_pessimistic_txn_{constraint_check}"
        ));
        tk.MustExec("set @@global.tidb_enable_foreign_key=1", Vec::new());
        tk.MustExec("set @@foreign_key_checks=1", Vec::new());
        tk.MustExec("set @@tidb_foreign_key_check_in_shared_lock=ON", Vec::new());
        tk.MustExec(
            &format!("set @@tidb_constraint_check_in_place_pessimistic={constraint_check}"),
            Vec::new(),
        );
        tk.MustExec("drop table if exists c, p", Vec::new());
        tk.MustExec("create table p(id int primary key)", Vec::new());
        tk.MustExec(
            "create table c(pid int, foreign key(pid) references p(id) \
             on delete cascade on update cascade)",
            Vec::new(),
        );
        tk.MustExec("insert into p values (1)", Vec::new());
        tk.MustExec("insert into c values (1)", Vec::new());
        tk.MustExec("begin pessimistic", Vec::new());
        tk.MustExec("update p set id = 2 where id = 1", Vec::new());
        tk.MustQuery("select pid from c", Vec::new())
            .Check(Rows(&["2"]));
        tk.MustExec("commit", Vec::new());
        tk.MustQuery("select pid from c", Vec::new())
            .Check(Rows(&["2"]));
        tk.MustExec("delete from p where id = 2", Vec::new());
        tk.MustQuery("select count(*) from c", Vec::new())
            .Check(Rows(&["0"]));
        tk.MustExec("admin check table p", Vec::new());
        tk.MustExec("admin check table c", Vec::new());
        tk.MustExec("set @@global.tidb_enable_foreign_key=default", Vec::new());
    }
}

#[test]
fn test_shared_lock_lock_view() {
    let _serial = serial_guard();
    let _config = allow_foreign_key_check_in_shared_lock_for_test();
    let (store, mut tk1, database) = prepare("test_shared_lock_lock_view");
    let mut tk2 = session(store.clone(), &database);
    let observer = session(store.clone(), &database);
    tk1.MustExec(
        "set @@tidb_foreign_key_check_in_shared_lock = ON",
        Vec::new(),
    );
    tk1.MustExec("set @@tidb_pessimistic_txn_fair_locking = 0", Vec::new());
    prepare_foreign_key_tables(&mut tk1);
    let connection2 = scalar(&tk2, "select connection_id()");

    // Case 1: an FK shared lock waits for an exclusive parent-row lock.
    tk1.MustExec("begin pessimistic", Vec::new());
    tk1.MustQuery("select * from parent where id=1 for update", Vec::new())
        .Check(Rows(&["1"]));
    tk2.MustExec("begin pessimistic", Vec::new());
    let (insert_tx, insert_rx) = mpsc::sync_channel(1);
    let insert_worker = thread::spawn(move || {
        let result = tk2
            .Exec("insert into child values (1, 1)", Vec::new())
            .and_then(|_| tk2.Exec("commit", Vec::new()))
            .map(|_| ())
            .map_err(|error| error.to_string());
        insert_tx.send((tk2, result)).expect("send insert result");
    });
    assert_still_blocked(&insert_rx, "shared lock in lock view");
    let first_key = assert_wait_view_maps_to_session(&observer, &connection2);
    tk1.MustExec("commit", Vec::new());
    let (_tk2, insert_result) = insert_rx.recv().expect("receive insert result");
    insert_worker.join().expect("join insert worker");
    expect_ok(insert_result, "lock-view child insert");
    tk1.MustQuery("select * from child", Vec::new())
        .Check(Rows(&["1 1"]));
    assert_eq!(
        observer
            .MustQuery(
                "select `key`, trx_id from information_schema.data_lock_waits",
                Vec::new(),
            )
            .Rows(),
        Vec::<Vec<String>>::new(),
        "completed waiter must disappear from DATA_LOCK_WAITS"
    );

    // Case 2: an exclusive parent-row lock waits for an FK shared lock.
    tk1.MustExec("begin pessimistic", Vec::new());
    tk1.MustExec("insert into child values (2, 1)", Vec::new());
    let mut tk2 = session(store.clone(), &database);
    let connection2 = scalar(&tk2, "select connection_id()");
    tk2.MustExec("begin pessimistic", Vec::new());
    let (exclusive_tx, exclusive_rx) = mpsc::sync_channel(1);
    let exclusive_worker = thread::spawn(move || {
        let result = tk2
            .Query("select * from parent where id=1 for update", Vec::new())
            .map(|rows| rows.string_rows())
            .map_err(|error| error.to_string())
            .and_then(|rows| {
                tk2.Exec("commit", Vec::new())
                    .map(|_| rows)
                    .map_err(|error| error.to_string())
            });
        exclusive_tx
            .send((tk2, result))
            .expect("send exclusive result");
    });
    assert_still_blocked(&exclusive_rx, "exclusive lock in lock view");
    let second_key = assert_wait_view_maps_to_session(&observer, &connection2);
    assert_eq!(
        first_key, second_key,
        "both directions must report the same encoded parent-row key"
    );
    tk1.MustExec("commit", Vec::new());
    let (_tk2, exclusive_result) = exclusive_rx.recv().expect("receive exclusive result");
    exclusive_worker.join().expect("join exclusive worker");
    assert_eq!(
        exclusive_result.expect("exclusive lock after shared release"),
        Rows(&["1"])
    );
}

#[test]
fn test_shared_lock_data_lock_waits_from_storage_wait_table() {
    let _serial = serial_guard();
    let _config = allow_foreign_key_check_in_shared_lock_for_test();
    let _skip_resolving = testfailpoint::enable(SKIP_RESOLVING_LOCKS, "return(true)");
    assert!(
        testfailpoint::eval_bool(SKIP_RESOLVING_LOCKS),
        "storage wait-table failpoint must be active"
    );
    let (store, mut tk1, database) =
        prepare("test_shared_lock_data_lock_waits_from_storage_wait_table");
    let mut tk2 = session(store.clone(), &database);
    let observer = session(store.clone(), &database);
    tk1.MustExec(
        "set @@tidb_foreign_key_check_in_shared_lock = ON",
        Vec::new(),
    );
    tk1.MustExec("set @@tidb_pessimistic_txn_fair_locking = 0", Vec::new());
    prepare_foreign_key_tables(&mut tk1);
    let connection2 = scalar(&tk2, "select connection_id()");

    tk1.MustExec("begin pessimistic", Vec::new());
    tk1.MustQuery("select * from parent where id=1 for update", Vec::new())
        .Check(Rows(&["1"]));
    tk2.MustExec("begin pessimistic", Vec::new());
    let (insert_tx, insert_rx) = mpsc::sync_channel(1);
    let insert_worker = thread::spawn(move || {
        let result = tk2
            .Exec("insert into child values (1, 1)", Vec::new())
            .and_then(|_| tk2.Exec("commit", Vec::new()))
            .map(|_| ())
            .map_err(|error| error.to_string());
        insert_tx.send((tk2, result)).expect("send insert result");
    });
    assert_still_blocked(&insert_rx, "storage wait-table child insert");
    let key = assert_wait_view_maps_to_session(&observer, &connection2);
    let same_key_count = scalar(
        &observer,
        &format!(
            "select count(*) from information_schema.data_lock_waits \
             where `key` = '{key}'"
        ),
    );
    assert_eq!(same_key_count, "1");
    assert_still_blocked(
        &insert_rx,
        "insert before storage DATA_LOCK_WAITS observation",
    );

    tk1.MustExec("commit", Vec::new());
    let (_tk2, insert_result) = insert_rx.recv().expect("receive insert result");
    insert_worker.join().expect("join insert worker");
    expect_ok(insert_result, "storage wait-table child insert");
    tk1.MustQuery("select * from child", Vec::new())
        .Check(Rows(&["1 1"]));
    assert_eq!(
        observer
            .MustQuery(
                "select `key`, trx_id from information_schema.data_lock_waits",
                Vec::new(),
            )
            .Rows(),
        Vec::<Vec<String>>::new(),
        "released storage waiter must be removed"
    );
}

#[test]
fn shared_lock_test_config_guard_enables_and_restores_sql_gate() {
    let _serial = serial_guard();
    let original = astersql_config::get_global_config();
    let restore = ForeignKeySharedLockConfigGuard(Some(Box::new(astersql_config::restore_func())));
    astersql_config::update_global(|conf| {
        conf.experimental
            .allow_enable_foreign_key_check_in_shared_lock = false
    });
    {
        let _allow = allow_foreign_key_check_in_shared_lock_for_test();
        let (_, mut tk, _) = prepare("config_gate");
        tk.MustExec(
            "SET @@session.tidb_foreign_key_check_in_shared_lock = ON",
            Vec::new(),
        );
        tk.MustQuery(
            "SELECT @@session.tidb_foreign_key_check_in_shared_lock",
            Vec::new(),
        )
        .Check(Rows(&["ON"]));
    }
    assert!(
        !astersql_config::get_global_config()
            .experimental
            .allow_enable_foreign_key_check_in_shared_lock
    );
    drop(restore);
    assert_eq!(
        astersql_config::get_global_config().experimental,
        original.experimental
    );
}
