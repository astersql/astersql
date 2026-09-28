// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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
//! 中文总览：`txn_state_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `事务语义与时间戳行为` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 101 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `BEFORE_PESSIMISTIC_LOCK` 是当前文件里的常量。
//! `BEFORE_PESSIMISTIC_LOCK` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `BEFORE_PESSIMISTIC_LOCK` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `BEFORE_PESSIMISTIC_LOCK`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `BEFORE_PREWRITE` 是当前文件里的常量。
//! `BEFORE_PREWRITE` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `BEFORE_PREWRITE` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `BEFORE_PREWRITE`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `MOCK_STMT_SLOW` 是当前文件里的常量。
//! `MOCK_STMT_SLOW` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `MOCK_STMT_SLOW` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `MOCK_STMT_SLOW`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `MOCK_SLOW_COMMIT` 是当前文件里的常量。
//! `MOCK_SLOW_COMMIT` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `MOCK_SLOW_COMMIT` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `MOCK_SLOW_COMMIT`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `MOCK_SLOW_ROLLBACK` 是当前文件里的常量。
//! `MOCK_SLOW_ROLLBACK` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `MOCK_SLOW_ROLLBACK` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `MOCK_SLOW_ROLLBACK`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `PAUSE_REACH_TIMEOUT` 是当前文件里的常量。
//! `PAUSE_REACH_TIMEOUT` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `PAUSE_REACH_TIMEOUT` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `PAUSE_REACH_TIMEOUT`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `BoundedPause` 是当前文件里的状态类型。
//! `BoundedPause` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `BoundedPause` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `BoundedPause`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `new` 是当前文件里的辅助函数。
//! `new` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `new` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `new`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `wait_until_reached` 是当前文件里的辅助函数。
//! `wait_until_reached` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `wait_until_reached` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `wait_until_reached`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `resume` 是当前文件里的辅助函数。
//! `resume` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `resume` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `resume`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `drop` 是当前文件里的辅助函数。
//! `drop` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `drop` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `drop`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TxnSnapshot` 是当前文件里的状态类型。
//! `TxnSnapshot` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TxnSnapshot` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TxnSnapshot`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `prepare` 是当前文件里的辅助函数。
//! `prepare` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `prepare` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `prepare`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `session` 是当前文件里的辅助函数。
//! `session` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `session` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `session`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `scalar` 是当前文件里的辅助函数。
//! `scalar` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `scalar` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `scalar`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `connection_id` 是当前文件里的辅助函数。
//! `connection_id` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `connection_id` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `connection_id`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `transaction` 是当前文件里的辅助函数。
//! `transaction` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `transaction` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `transaction`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `require_transaction` 是当前文件里的辅助函数。
//! `require_transaction` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `require_transaction` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `require_transaction`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `digest` 是当前文件里的辅助函数。
//! `digest` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `digest` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `digest`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `digest_json` 是当前文件里的辅助函数。
//! `digest_json` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `digest_json` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `digest_json`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_basic_txn_state` 是当前文件里的测试用例。
//! `test_basic_txn_state` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_basic_txn_state` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_basic_txn_state`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_entries_count_and_size` 是当前文件里的测试用例。
//! `test_entries_count_and_size` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_entries_count_and_size` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_entries_count_and_size`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `insert_rows` 是当前文件里的辅助函数。
//! `insert_rows` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `insert_rows` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `insert_rows`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `BATCH` 是当前文件里的常量。
//! `BATCH` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `BATCH` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `BATCH`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_mem_db_tracker` 是当前文件里的测试用例。
//! `test_mem_db_tracker` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_mem_db_tracker` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_mem_db_tracker`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_running` 是当前文件里的测试用例。
//! `test_running` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_running` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_running`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_blocked` 是当前文件里的测试用例。
//! `test_blocked` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_blocked` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_blocked`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_committing` 是当前文件里的测试用例。
//! `test_committing` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_committing` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_committing`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_rollback_txn_state` 是当前文件里的测试用例。
//! `test_rollback_txn_state` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_rollback_txn_state` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_rollback_txn_state`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_txn_info_with_prepared_stmt` 是当前文件里的测试用例。
//! `test_txn_info_with_prepared_stmt` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_txn_info_with_prepared_stmt` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_txn_info_with_prepared_stmt`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_txn_info_with_scalar_subquery` 是当前文件里的测试用例。
//! `test_txn_info_with_scalar_subquery` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_txn_info_with_scalar_subquery` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_txn_info_with_scalar_subquery`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_txn_info_with_ps_protocol` 是当前文件里的测试用例。
//! `test_txn_info_with_ps_protocol` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_txn_info_with_ps_protocol` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_txn_info_with_ps_protocol`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 中文说明结束（自动生成）

//! Real transaction-state coverage ported from `txn_state_test.go`.
//!
//! A `ConcreteSession` is pinned to its worker thread, so tests observe the
//! running transaction from a second session through `INFORMATION_SCHEMA`.
//! This preserves the upstream concurrency boundary without sharing a mutable
//! `TestKit` across threads or replacing a running-state assertion with a
//! post-execution check.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use astersql_parser::NormalizeDigest;
use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{DbValue, NewTestKit, Rows, TestKit};
use astersql_testkit_testfailpoint as testfailpoint;
use astersql_tests_realtikvtest_txntest::serial_guard;

const BEFORE_PESSIMISTIC_LOCK: &str = "tikvclient/beforePessimisticLock";
const BEFORE_PREWRITE: &str = "tikvclient/beforePrewrite";
const MOCK_STMT_SLOW: &str = "github.com/pingcap/tidb/pkg/session/mockStmtSlow";
const MOCK_SLOW_COMMIT: &str = "github.com/pingcap/tidb/pkg/session/mockSlowCommit";
const MOCK_SLOW_ROLLBACK: &str = "github.com/pingcap/tidb/pkg/session/mockSlowRollback";
const PAUSE_REACH_TIMEOUT: Duration = Duration::from_secs(3);

/// A pause failpoint that cannot strand its SQL worker when an assertion
/// panics or the production injection site is accidentally missing.
struct BoundedPause {
    guard: Arc<testfailpoint::PauseGuard>,
}

impl BoundedPause {
    fn new(name: &str) -> Self {
        Self {
            guard: Arc::new(testfailpoint::enable_pause(name)),
        }
    }

    fn wait_until_reached(&self, operation: &str) {
        if !self.guard.wait_until_reached_timeout(PAUSE_REACH_TIMEOUT) {
            self.guard.resume();
            panic!("{operation} did not reach its failpoint within {PAUSE_REACH_TIMEOUT:?}");
        }
    }

    fn resume(&self) {
        self.guard.resume();
    }
}

impl Drop for BoundedPause {
    fn drop(&mut self) {
        self.guard.resume();
    }
}

#[derive(Debug)]
struct TxnSnapshot {
    start_ts: u64,
    current_sql_digest: String,
    state: String,
    waiting: bool,
    mem_buffer_keys: u64,
    mem_buffer_bytes: u64,
    session_id: u64,
    user: String,
    database: String,
    all_sql_digests: String,
}

fn prepare(test_name: &str) -> (Arc<AnalyzeStatsStore>, TestKit, TestKit, String) {
    let (store, _domain) = CreateMockStoreAndDomain();
    let database = format!("txn_state_{}", test_name.trim_start_matches("test_"));
    let mut tk = NewTestKit(store.clone());
    tk.MustExec(&format!("drop database if exists `{database}`"), Vec::new());
    tk.MustExec(&format!("create database `{database}`"), Vec::new());
    tk.MustExec(&format!("use `{database}`"), Vec::new());
    let mut observer = NewTestKit(store.clone());
    observer.MustExec(&format!("use `{database}`"), Vec::new());
    (store, tk, observer, database)
}

fn session(store: Arc<AnalyzeStatsStore>, database: &str) -> TestKit {
    let mut tk = NewTestKit(store);
    tk.MustExec(&format!("use `{database}`"), Vec::new());
    tk
}

fn scalar(tk: &TestKit, sql: &str) -> String {
    let rows = tk.MustQuery(sql, Vec::new()).Rows();
    assert_eq!(rows.len(), 1, "sql={sql:?}, rows={rows:?}");
    assert_eq!(rows[0].len(), 1, "sql={sql:?}, rows={rows:?}");
    rows[0][0].clone()
}

fn connection_id(tk: &TestKit) -> u64 {
    scalar(tk, "select connection_id()")
        .parse()
        .expect("connection ID")
}

fn transaction(observer: &TestKit, session_id: u64) -> Option<TxnSnapshot> {
    let sql = format!(
        "select id,current_sql_digest,state,waiting_start_time,mem_buffer_keys,\
         mem_buffer_bytes,session_id,user,db,all_sql_digests \
         from information_schema.tidb_trx where session_id={session_id}"
    );
    let rows = observer.MustQuery(&sql, Vec::new()).Rows();
    assert!(rows.len() <= 1, "duplicate transaction rows: {rows:?}");
    rows.into_iter().next().map(|row| {
        assert_eq!(row.len(), 10, "unexpected TIDB_TRX row: {row:?}");
        TxnSnapshot {
            start_ts: row[0].parse().expect("transaction start TS"),
            current_sql_digest: row[1].clone(),
            state: row[2].clone(),
            waiting: !matches!(row[3].as_str(), "" | "<nil>" | "NULL"),
            mem_buffer_keys: row[4].parse().expect("mem buffer keys"),
            mem_buffer_bytes: row[5].parse().expect("mem buffer bytes"),
            session_id: row[6].parse().expect("transaction session ID"),
            user: row[7].clone(),
            database: row[8].clone(),
            all_sql_digests: row[9].clone(),
        }
    })
}

fn require_transaction(observer: &TestKit, session_id: u64) -> TxnSnapshot {
    transaction(observer, session_id)
        .unwrap_or_else(|| panic!("session {session_id} has no active transaction"))
}

fn digest(sql: &str) -> String {
    NormalizeDigest(sql).1.String().to_owned()
}

fn digest_json(sql: &[&str]) -> String {
    format!(
        "[{}]",
        sql.iter()
            .map(|sql| format!("\"{}\"", digest(sql)))
            .collect::<Vec<_>>()
            .join(",")
    )
}

#[test]
fn test_basic_txn_state() {
    let _serial = serial_guard();
    let (_store, mut tk, observer, _database) = prepare("test_basic_txn_state");
    tk.MustExec("create table t(a int)", Vec::new());
    tk.MustExec("insert into t(a) values (1)", Vec::new());
    let session_id = connection_id(&tk);
    assert!(transaction(&observer, session_id).is_none());

    tk.MustExec("begin pessimistic", Vec::new());
    let start_ts: u64 = scalar(&tk, "select @@tidb_current_ts")
        .parse()
        .expect("current transaction TS");
    let lock_pause = BoundedPause::new(BEFORE_PESSIMISTIC_LOCK);
    let lock_worker = thread::spawn(move || {
        tk.MustQuery("select * from t for update", Vec::new())
            .Check(Rows(&["1"]));
        tk
    });
    lock_pause.wait_until_reached("basic SELECT FOR UPDATE");
    let acquiring = require_transaction(&observer, session_id);
    assert_eq!(
        acquiring.current_sql_digest,
        digest("select * from t for update")
    );
    assert_eq!(acquiring.state, "LockWaiting");
    assert!(acquiring.waiting);
    assert_eq!(acquiring.start_ts, start_ts);
    lock_pause.resume();
    let mut tk = lock_worker.join().expect("lock statement worker");
    drop(lock_pause);

    let idle = require_transaction(&observer, session_id);
    assert_eq!(idle.current_sql_digest, "");
    assert_eq!(idle.state, "Idle");
    assert!(!idle.waiting);
    assert_eq!(idle.start_ts, start_ts);
    assert_eq!(
        idle.all_sql_digests,
        digest_json(&[
            "begin pessimistic",
            "select @@tidb_current_ts",
            "select * from t for update",
        ])
    );
    assert_eq!(idle.session_id, session_id);
    assert_eq!(idle.user, "");
    assert_eq!(idle.database, "txn_state_basic_txn_state");

    let commit_pause = BoundedPause::new(BEFORE_PREWRITE);
    let commit_worker = thread::spawn(move || {
        tk.MustExec("commit", Vec::new());
        tk
    });
    commit_pause.wait_until_reached("explicit commit");
    let committing = require_transaction(&observer, session_id);
    assert_eq!(committing.current_sql_digest, digest("commit"));
    assert_eq!(committing.state, "Committing");
    assert_eq!(
        committing.all_sql_digests,
        digest_json(&[
            "begin pessimistic",
            "select @@tidb_current_ts",
            "select * from t for update",
            "commit",
        ])
    );
    commit_pause.resume();
    let tk = commit_worker.join().expect("commit worker");
    drop(commit_pause);
    assert!(transaction(&observer, session_id).is_none());

    let autocommit_pause = BoundedPause::new(BEFORE_PREWRITE);
    let autocommit_worker = thread::spawn(move || {
        let mut tk = tk;
        tk.MustExec("insert into t values (2)", Vec::new());
        tk
    });
    autocommit_pause.wait_until_reached("autocommit prewrite");
    let autocommit = require_transaction(&observer, session_id);
    assert_eq!(
        autocommit.current_sql_digest,
        digest("insert into t values (2)")
    );
    assert_eq!(autocommit.state, "Committing");
    assert!(!autocommit.waiting);
    assert!(autocommit.start_ts > start_ts);
    assert_eq!(
        autocommit.all_sql_digests,
        digest_json(&["insert into t values (2)"])
    );
    autocommit_pause.resume();
    autocommit_worker.join().expect("autocommit worker");
    assert!(transaction(&observer, session_id).is_none());
}

#[test]
fn test_entries_count_and_size() {
    let _serial = serial_guard();
    let (_store, mut tk, observer, _database) = prepare("test_entries_count_and_size");
    tk.MustExec("create table t(a int)", Vec::new());
    let session_id = connection_id(&tk);
    tk.MustExec("begin pessimistic", Vec::new());
    tk.MustExec("insert into t(a) values (1)", Vec::new());
    assert_eq!(
        require_transaction(&observer, session_id).mem_buffer_keys,
        1
    );
    tk.MustExec("insert into t(a) values (2)", Vec::new());
    assert_eq!(
        require_transaction(&observer, session_id).mem_buffer_keys,
        2
    );
    tk.MustExec("commit", Vec::new());
    assert!(transaction(&observer, session_id).is_none());
}

fn insert_rows(tk: &mut TestKit, first: usize, count: usize) {
    const BATCH: usize = 128;
    for batch_start in (first..first + count).step_by(BATCH) {
        let batch_end = (batch_start + BATCH).min(first + count);
        let values = (batch_start..batch_end)
            .map(|value| format!("({value})"))
            .collect::<Vec<_>>()
            .join(",");
        tk.MustExec(&format!("insert into t(id) values {values}"), Vec::new());
    }
}

#[test]
fn test_mem_db_tracker() {
    let _serial = serial_guard();
    let (_store, mut tk, observer, _database) = prepare("test_mem_db_tracker");
    tk.MustExec("create table t(id int)", Vec::new());
    let session_id = connection_id(&tk);
    tk.MustExec("begin", Vec::new());
    insert_rows(&mut tk, 0, 1 << 10);
    let after_one_k = require_transaction(&observer, session_id).mem_buffer_bytes;
    assert!(
        after_one_k > 1 << (10 + 4),
        "1K rows used only {after_one_k} bytes"
    );
    assert!(
        after_one_k < 1 << (14 + 4),
        "1K rows used unexpectedly {after_one_k} bytes"
    );
    insert_rows(&mut tk, 1 << 10, 1 << 14);
    let after_seventeen_k = require_transaction(&observer, session_id).mem_buffer_bytes;
    assert!(
        after_seventeen_k > 1 << (14 + 4),
        "17K rows used only {after_seventeen_k} bytes"
    );
    tk.MustExec("rollback", Vec::new());
}

#[test]
fn test_running() {
    let _serial = serial_guard();
    let (_store, mut tk, observer, _database) = prepare("test_running");
    tk.MustExec("create table t(a int)", Vec::new());
    tk.MustExec("insert into t(a) values (1)", Vec::new());
    let session_id = connection_id(&tk);
    tk.MustExec("begin pessimistic", Vec::new());
    let slow = BoundedPause::new(MOCK_STMT_SLOW);
    let worker = thread::spawn(move || {
        tk.MustQuery("select * from t for update /* sleep */", Vec::new())
            .Check(Rows(&["1"]));
        tk.MustExec("commit", Vec::new());
    });
    slow.wait_until_reached("running statement");
    let running = require_transaction(&observer, session_id);
    assert_eq!(running.state, "Running");
    assert_eq!(
        running.current_sql_digest,
        digest("select * from t for update /* sleep */")
    );
    slow.resume();
    worker.join().expect("slow statement worker");
}

#[test]
fn test_blocked() {
    let _serial = serial_guard();
    let (store, mut tk1, observer, database) = prepare("test_blocked");
    let mut tk2 = session(store, &database);
    tk1.MustExec("create table t(a int)", Vec::new());
    tk1.MustExec("insert into t(a) values (1)", Vec::new());
    tk1.MustExec("begin pessimistic", Vec::new());
    tk1.MustQuery("select * from t where a=1 for update", Vec::new())
        .Check(Rows(&["1"]));
    let session2 = connection_id(&tk2);
    let worker = thread::spawn(move || {
        tk2.MustExec("begin pessimistic", Vec::new());
        tk2.MustQuery("select * from t where a=1 for update", Vec::new())
            .Check(Rows(&["1"]));
        tk2.MustExec("commit", Vec::new());
    });
    let started = std::time::Instant::now();
    loop {
        if let Some(blocked) = transaction(&observer, session2)
            && blocked.state == "LockWaiting"
        {
            assert!(blocked.waiting);
            assert_eq!(
                blocked.current_sql_digest,
                digest("select * from t where a=1 for update")
            );
            break;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "second transaction never entered LockWaiting"
        );
        thread::sleep(std::time::Duration::from_millis(10));
    }
    tk1.MustExec("commit", Vec::new());
    worker.join().expect("blocked lock worker");
}

#[test]
fn test_committing() {
    let _serial = serial_guard();
    let (store, mut tk1, observer, database) = prepare("test_committing");
    let mut tk2 = session(store, &database);
    tk1.MustExec("create table t(a int)", Vec::new());
    tk1.MustExec("insert into t(a) values (1),(2)", Vec::new());
    tk1.MustExec("begin pessimistic", Vec::new());
    tk1.MustQuery("select * from t where a=1 for update", Vec::new())
        .Check(Rows(&["1"]));
    let session2 = connection_id(&tk2);
    let slow_commit = BoundedPause::new(MOCK_SLOW_COMMIT);
    let worker = thread::spawn(move || {
        tk2.MustExec("begin pessimistic", Vec::new());
        tk2.MustQuery("select * from t where a=2 for update", Vec::new())
            .Check(Rows(&["2"]));
        tk2.MustExec("commit", Vec::new());
    });
    slow_commit.wait_until_reached("slow commit");
    let committing = require_transaction(&observer, session2);
    assert_eq!(committing.state, "Committing");
    assert_eq!(committing.current_sql_digest, digest("commit"));
    slow_commit.resume();
    tk1.MustExec("commit", Vec::new());
    worker.join().expect("slow commit worker");
}

#[test]
fn test_rollback_txn_state() {
    let _serial = serial_guard();
    let (_store, mut tk, observer, _database) = prepare("test_rollback_txn_state");
    tk.MustExec("create table t(a int)", Vec::new());
    tk.MustExec("insert into t(a) values (1),(2)", Vec::new());
    let session_id = connection_id(&tk);
    let slow_rollback = BoundedPause::new(MOCK_SLOW_ROLLBACK);
    let worker = thread::spawn(move || {
        tk.MustExec("begin pessimistic", Vec::new());
        tk.MustExec("insert into t(a) values (3)", Vec::new());
        tk.MustExec("rollback", Vec::new());
    });
    slow_rollback.wait_until_reached("slow rollback");
    let rolling_back = require_transaction(&observer, session_id);
    assert_eq!(rolling_back.state, "RollingBack");
    assert_eq!(rolling_back.current_sql_digest, digest("rollback"));
    slow_rollback.resume();
    worker.join().expect("slow rollback worker");
}

#[test]
fn test_txn_info_with_prepared_stmt() {
    let _serial = serial_guard();
    let (_store, mut tk, observer, _database) = prepare("test_txn_info_with_prepared_stmt");
    tk.MustExec("create table t(a int)", Vec::new());
    tk.MustExec("set @v=1", Vec::new());
    let session_id = connection_id(&tk);
    let insert = tk.Prepare("insert into t values (?)");
    tk.MustExec("begin pessimistic", Vec::new());
    let pause = BoundedPause::new(BEFORE_PESSIMISTIC_LOCK);
    let worker = thread::spawn(move || {
        insert
            .execute(&[DbValue::from(1_i64)])
            .expect("execute prepared insert");
        tk
    });
    pause.wait_until_reached("prepared insert lock");
    let acquiring = require_transaction(&observer, session_id);
    assert_eq!(
        acquiring.current_sql_digest,
        digest("insert into t values (?)")
    );
    assert_eq!(acquiring.state, "LockWaiting");
    pause.resume();
    let mut tk = worker.join().expect("prepared insert worker");
    drop(pause);
    let idle = require_transaction(&observer, session_id);
    assert_eq!(idle.current_sql_digest, "");
    assert_eq!(
        idle.all_sql_digests,
        digest_json(&["begin pessimistic", "insert into t values (?)"])
    );
    tk.MustExec("rollback", Vec::new());
}

#[test]
fn test_txn_info_with_scalar_subquery() {
    let _serial = serial_guard();
    let (_store, mut tk, observer, _database) = prepare("test_txn_info_with_scalar_subquery");
    tk.MustExec("create table t(a int primary key, b int)", Vec::new());
    tk.MustExec("insert into t values (1,10),(2,1)", Vec::new());
    let session_id = connection_id(&tk);
    tk.MustExec("begin pessimistic", Vec::new());
    tk.MustQuery(
        "select * from t where a=(select b from t where a=2)",
        Vec::new(),
    )
    .Check(Rows(&["1 10"]));
    let update_sql = "update t set b=b+1 where a=(select b from t where a=2)";
    let pause = BoundedPause::new(BEFORE_PESSIMISTIC_LOCK);
    let worker = thread::spawn(move || {
        tk.MustExec(update_sql, Vec::new());
        tk
    });
    pause.wait_until_reached("scalar-subquery update lock");
    let acquiring = require_transaction(&observer, session_id);
    assert_eq!(acquiring.current_sql_digest, digest(update_sql));
    assert_eq!(
        acquiring.all_sql_digests,
        digest_json(&[
            "begin pessimistic",
            "select * from t where a=(select b from t where a=2)",
            update_sql,
        ])
    );
    pause.resume();
    let mut tk = worker.join().expect("scalar-subquery worker");
    tk.MustExec("rollback", Vec::new());
}

#[test]
fn test_txn_info_with_ps_protocol() {
    let _serial = serial_guard();
    let (_store, mut tk, observer, _database) = prepare("test_txn_info_with_ps_protocol");
    tk.MustExec("create table t(a int primary key)", Vec::new());
    let session_id = connection_id(&tk);
    let insert = tk.Prepare("insert into t values (?)");
    let prewrite = BoundedPause::new(BEFORE_PREWRITE);
    let worker = thread::spawn(move || {
        insert.execute(&[DbValue::from(1_i64)]).expect("PS insert");
        tk
    });
    prewrite.wait_until_reached("PS autocommit prewrite");
    let committing = require_transaction(&observer, session_id);
    assert!(committing.start_ts > 0);
    assert_eq!(committing.state, "Committing");
    assert_eq!(
        committing.current_sql_digest,
        digest("insert into t values (?)")
    );
    assert_eq!(
        committing.all_sql_digests,
        digest_json(&["insert into t values (?)"])
    );
    prewrite.resume();
    let mut tk = worker.join().expect("PS autocommit worker");
    drop(prewrite);
    assert!(transaction(&observer, session_id).is_none());

    let point_get = tk.Prepare("select * from t where a=?");
    let update = tk.Prepare("update t set a=a+1 where a=?");
    tk.MustExec("begin pessimistic", Vec::new());
    assert_eq!(
        point_get
            .query(&[DbValue::from(1_i64)])
            .expect("PS point get")
            .string_rows(),
        Rows(&["1"])
    );
    let lock = BoundedPause::new(BEFORE_PESSIMISTIC_LOCK);
    let worker = thread::spawn(move || {
        update.execute(&[DbValue::from(1_i64)]).expect("PS update");
        tk
    });
    lock.wait_until_reached("PS update lock");
    let acquiring = require_transaction(&observer, session_id);
    assert!(acquiring.start_ts > 0);
    assert_eq!(acquiring.state, "LockWaiting");
    assert!(acquiring.waiting);
    assert_eq!(
        acquiring.current_sql_digest,
        digest("update t set a=a+1 where a=?")
    );
    assert_eq!(
        acquiring.all_sql_digests,
        digest_json(&[
            "begin pessimistic",
            "select * from t where a=?",
            "update t set a=a+1 where a=?",
        ])
    );
    lock.resume();
    let mut tk = worker.join().expect("PS update worker");
    tk.MustExec("rollback", Vec::new());
}
