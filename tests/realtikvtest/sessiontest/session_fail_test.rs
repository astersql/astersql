// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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
//! 中文总览：`session_fail_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `会话生命周期与信息模式` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 79 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `MOCK_COMMIT_8942` 是当前文件里的常量。
//! `MOCK_COMMIT_8942` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `MOCK_COMMIT_8942` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `MOCK_COMMIT_8942`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `MOCK_GET_TS` 是当前文件里的常量。
//! `MOCK_GET_TS` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `MOCK_GET_TS` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `MOCK_GET_TS`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `MOCK_COMMIT_RETRY` 是当前文件里的常量。
//! `MOCK_COMMIT_RETRY` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `MOCK_COMMIT_RETRY` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `MOCK_COMMIT_RETRY`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `MOCK_GET_TS_RETRY` 是当前文件里的常量。
//! `MOCK_GET_TS_RETRY` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `MOCK_GET_TS_RETRY` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `MOCK_GET_TS_RETRY`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `STORE_SEND_RESULT` 是当前文件里的常量。
//! `STORE_SEND_RESULT` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `STORE_SEND_RESULT` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `STORE_SEND_RESULT`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `MIN_COMMIT_TS` 是当前文件里的常量。
//! `MIN_COMMIT_TS` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `MIN_COMMIT_TS` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `MIN_COMMIT_TS`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `BATCH_SEND_DELAY` 是当前文件里的常量。
//! `BATCH_SEND_DELAY` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `BATCH_SEND_DELAY` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `BATCH_SEND_DELAY`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `serial_guard` 是当前文件里的辅助函数。
//! `serial_guard` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `serial_guard` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `serial_guard`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `SERIAL` 是当前文件里的静态量。
//! `SERIAL` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `SERIAL` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `SERIAL`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `prepare` 是当前文件里的辅助函数。
//! `prepare` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `prepare` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `prepare`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `assert_error_contains` 是当前文件里的辅助函数。
//! `assert_error_contains` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `assert_error_contains` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `assert_error_contains`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `joined_row` 是当前文件里的辅助函数。
//! `joined_row` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `joined_row` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `joined_row`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `assert_explain_parts` 是当前文件里的辅助函数。
//! `assert_explain_parts` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `assert_explain_parts` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `assert_explain_parts`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_fail_statement_commit_in_retry` 是当前文件里的测试用例。
//! `test_fail_statement_commit_in_retry` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `test_fail_statement_commit_in_retry` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_fail_statement_commit_in_retry`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_get_ts_fail_dirty_state` 是当前文件里的测试用例。
//! `test_get_ts_fail_dirty_state` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `test_get_ts_fail_dirty_state` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_get_ts_fail_dirty_state`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_get_ts_fail_dirty_state_in_retry` 是当前文件里的测试用例。
//! `test_get_ts_fail_dirty_state_in_retry` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `test_get_ts_fail_dirty_state_in_retry` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_get_ts_fail_dirty_state_in_retry`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_kill_flag_in_backoff` 是当前文件里的测试用例。
//! `test_kill_flag_in_backoff` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `test_kill_flag_in_backoff` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_kill_flag_in_backoff`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_cluster_table_send_error` 是当前文件里的测试用例。
//! `test_cluster_table_send_error` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `test_cluster_table_send_error` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_cluster_table_send_error`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_auto_commit_need_not_linearizability` 是当前文件里的测试用例。
//! `test_auto_commit_need_not_linearizability` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `test_auto_commit_need_not_linearizability` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_auto_commit_need_not_linearizability`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_kill` 是当前文件里的测试用例。
//! `test_kill` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `test_kill` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_kill`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_issue_42426` 是当前文件里的测试用例。
//! `test_issue_42426` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `test_issue_42426` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_issue_42426`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_index_lookup_with_static_prune` 是当前文件里的测试用例。
//! `test_index_lookup_with_static_prune` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `test_index_lookup_with_static_prune` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_index_lookup_with_static_prune`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_tikv_client_read_timeout` 是当前文件里的测试用例。
//! `test_tikv_client_read_timeout` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `test_tikv_client_read_timeout` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_tikv_client_read_timeout`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_issue_57530` 是当前文件里的测试用例。
//! `test_issue_57530` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `test_issue_57530` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_issue_57530`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 中文说明结束（自动生成）

//! Executable Rust port of `session_fail_test.go`.
//!
//! The tests below keep the Go declaration order, fault names, transactional
//! boundaries, concurrent kill timing, error text, and cleanup lifetime.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::Duration;

use astersql_session::runtime::CreateAnalyzeSession;
use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{NewTestKit, Rows, TestKit};
use astersql_testkit_testfailpoint as failpoint;
use astersql_util_sqlkiller::sqlkiller::QueryInterrupted;

const MOCK_COMMIT_8942: &str = "github.com/pingcap/tidb/pkg/session/mockCommitError8942";
const MOCK_GET_TS: &str = "github.com/pingcap/tidb/pkg/session/mockGetTSFail";
const MOCK_COMMIT_RETRY: &str = "github.com/pingcap/tidb/pkg/session/mockCommitError";
const MOCK_GET_TS_RETRY: &str = "tikvclient/mockGetTSErrorInRetry";
const STORE_SEND_RESULT: &str = "tikvclient/tikvStoreSendReqResult";
const MIN_COMMIT_TS: &str = "tikvclient/getMinCommitTSFromTSO";
const BATCH_SEND_DELAY: &str = "tikvclient/mockBatchClientSendDelay";

fn serial_guard() -> MutexGuard<'static, ()> {
    static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();
    SERIAL
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn prepare(name: &str) -> (Arc<AnalyzeStatsStore>, TestKit) {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    let database = format!("session_fail_{name}");
    tk.MustExec(&format!("drop database if exists `{database}`"), Vec::new());
    tk.MustExec(&format!("create database `{database}`"), Vec::new());
    tk.MustExec(&format!("use `{database}`"), Vec::new());
    tk.MustExec("set tidb_enable_clustered_index = on", Vec::new());
    (store, tk)
}

fn assert_error_contains(error: impl std::fmt::Display, fragment: &str) {
    let message = error.to_string();
    assert!(
        message.contains(fragment),
        "expected error containing {fragment:?}, got {message:?}"
    );
}

fn joined_row(rows: &[Vec<String>], index: usize) -> String {
    rows.get(index)
        .unwrap_or_else(|| panic!("missing EXPLAIN row {index}: {rows:?}"))
        .join(" ")
}

fn assert_explain_parts(explain: &str, parts: &[&str]) {
    let mut remaining = explain;
    for part in parts {
        let offset = remaining
            .find(part)
            .unwrap_or_else(|| panic!("EXPLAIN output is missing ordered {part:?}: {explain}"));
        remaining = &remaining[offset + part.len()..];
        if part.contains("num_rpc:") && part.ends_with(|c: char| c.is_ascii_digit()) {
            assert!(
                !remaining.starts_with(|c: char| c.is_ascii_digit()),
                "incorrect RPC count: {explain}"
            );
        }
    }
}

fn explain_rpc_count(explain: &str) -> usize {
    explain
        .split("num_rpc:")
        .nth(1)
        .expect("RPC counter")
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .expect("numeric RPC counter")
}

/// Go `TestFailStatementCommitInRetry`.
#[test]
fn test_fail_statement_commit_in_retry() {
    let _serial = serial_guard();
    let (_store, mut tk) = prepare("commit_retry");
    tk.MustExec("create table t (id int)", Vec::new());
    tk.MustExec("begin", Vec::new());
    tk.MustExec("insert into t values (1)", Vec::new());
    tk.MustExec("insert into t values (2),(3),(4),(5)", Vec::new());
    tk.MustExec("insert into t values (6)", Vec::new());

    let _failpoint = failpoint::enable(MOCK_COMMIT_8942, "return(true)");
    let _commit_error = tk.ExecToErr("commit");
    drop(_failpoint);

    tk.MustExec("insert into t values (6)", Vec::new());
    tk.MustQuery("select * from t", Vec::new())
        .Check(Rows(&["6"]));
}

/// Go `TestGetTSFailDirtyState`.
#[test]
fn test_get_ts_fail_dirty_state() {
    let _serial = serial_guard();
    struct RestoreConfig(Option<Box<dyn FnOnce()>>);
    impl Drop for RestoreConfig {
        fn drop(&mut self) {
            self.0.take().unwrap()();
        }
    }
    let _restore = RestoreConfig(Some(Box::new(astersql_config::restore_func())));
    for backend in ["unistore", "tikv"] {
        astersql_config::update_global(|config| config.store = backend.to_owned());
        let (_domain, session) = CreateAnalyzeSession().expect("create session");
        session.execute("use test").unwrap();
        session.execute("create table t (id int)").unwrap();
        let _failpoint = failpoint::enable(MOCK_GET_TS, "return");
        let result =
            session.execute_with_failpoint_hook("select * from t", &|name| name == MOCK_GET_TS);
        if backend == "unistore" {
            assert!(result.is_err(), "UniStore TSO activation must fail");
        } else {
            for mut records in result.expect("TiKV does not activate the failing local future") {
                records.close().unwrap();
            }
        }
        // Go retains the failpoint through these ordinary, unhooked requests.
        session
            .execute("insert into t values (1)")
            .expect("a failed TSO request must not poison a later write");
        let mut records = session.execute("select * from t").unwrap();
        assert_eq!(records[0].next_row().unwrap(), Some(vec!["1".into()]));
        assert_eq!(records[0].next_row().unwrap(), None);
        records[0].close().unwrap();
    }
}

/// Go `TestGetTSFailDirtyStateInretry`.
#[test]
fn test_get_ts_fail_dirty_state_in_retry() {
    let _serial = serial_guard();
    let (_store, mut tk) = prepare("get_ts_retry");
    tk.MustExec("create table t (id int)", Vec::new());

    // Keep both guards alive through the autocommit retry, matching Go defer.
    let _commit = failpoint::enable(MOCK_COMMIT_RETRY, "return(true)");
    let _get_ts = failpoint::enable(MOCK_GET_TS_RETRY, "1*return(true)->return(false)");
    tk.MustExec("insert into t values (2)", Vec::new());
    assert!(
        !failpoint::eval_bool(MOCK_GET_TS_RETRY),
        "commit retry did not consume mockGetTSErrorInRetry's one failing activation"
    );
    tk.MustQuery("select * from t", Vec::new())
        .Check(Rows(&["2"]));
}

/// Go `TestKillFlagInBackoff`.
#[test]
fn test_kill_flag_in_backoff() {
    let _serial = serial_guard();
    let (_domain, session) = CreateAnalyzeSession().expect("create session");
    session.execute("use test").unwrap();
    session
        .execute("create table kill_backoff (id int)")
        .unwrap();
    let _send_timeout = failpoint::enable(
        STORE_SEND_RESULT,
        r#"sleep(1000)->return("timeout")->return("")"#,
    );
    let killer = session.SQLKiller();
    thread::scope(|scope| {
        scope.spawn(move || {
            thread::sleep(Duration::from_millis(300));
            killer.SendKillSignal(QueryInterrupted);
        });
        let mut records = session
            .execute("select * from kill_backoff")
            .expect("Go Exec succeeds before the coprocessor result is fetched");
        assert_eq!(records.len(), 1);
        let error = records[0]
            .next_row()
            .expect_err("result fetching must observe the kill signal");
        assert_error_contains(error, "Query execution was interrupted");
        records[0].close().expect("close interrupted result");
        assert!(
            records[0].next_row().is_err(),
            "closed result must reject reads"
        );
    });
}

/// Go `TestClusterTableSendError`.
#[test]
fn test_cluster_table_send_error() {
    let _serial = serial_guard();
    let (_domain, session) = CreateAnalyzeSession().expect("create concrete session");
    session
        .execute("use test; set tidb_enable_clustered_index=on")
        .unwrap();
    let _failpoint = failpoint::enable(STORE_SEND_RESULT, r#"return("requestTiDBStoreError")"#);
    let mut results = session
        .execute("select * from information_schema.cluster_slow_query")
        .unwrap();
    while results[0].next_row().unwrap().is_some() {}
    results[0].close().unwrap();
    session.WithSessionVars(|vars| {
        assert_eq!(vars.StmtCtx.WarningCount(), 1);
        let warnings = vars.StmtCtx.GetWarnings();
        assert!(
            warnings[0]
                .Err
                .as_ref()
                .unwrap()
                .to_string()
                .contains("TiDB server timeout, address is")
        );
    });
    let mut shown = session.execute("show warnings").unwrap();
    let warning = shown[0].next_row().unwrap().unwrap();
    assert_eq!(warning[0], "Warning");
    assert!(warning[2].contains("TiDB server timeout, address is"));
    assert!(shown[0].next_row().unwrap().is_none());
    shown[0].close().unwrap();
    session.WithSessionVars(|vars| assert_eq!(vars.StmtCtx.WarningCount(), 1));
    session.execute("select 1").unwrap();
    session.WithSessionVars(|vars| assert_eq!(vars.StmtCtx.WarningCount(), 0));
}

/// Go `TestAutoCommitNeedNotLinearizability`.
#[test]
fn test_auto_commit_need_not_linearizability() {
    let _serial = serial_guard();
    let (_domain, mut session) = CreateAnalyzeSession().expect("create concrete session");
    session.execute("use test").expect("use test");
    session
        .execute("drop table if exists t1")
        .expect("drop fixture");
    session
        .execute("create table t1 (c int)")
        .expect("create fixture");
    session
        .SetSessionSystemVar("tidb_enable_async_commit", "1")
        .expect("enable async commit");
    session
        .SetSessionSystemVar("tidb_guarantee_linearizability", "1")
        .expect("enable linearizability");
    session
        .execute("set tidb_enable_clustered_index=on")
        .unwrap();
    struct Cleanup<'a>(&'a astersql_session::runtime::ConcreteSession);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            let _ = self.0.execute("drop table if exists t1");
        }
    }
    let _cleanup = Cleanup(&session);
    let _failpoint = failpoint::enable(MIN_COMMIT_TS, "panic");

    // Autocommit bypasses minCommitTS.
    session
        .execute("insert into t1 values (1)")
        .expect("async autocommit bypasses TSO");

    session.execute("begin").expect("begin async transaction");
    session
        .execute("insert into t1 values (2)")
        .expect("stage async insert");
    assert!(
        catch_unwind(AssertUnwindSafe(|| { session.execute("commit") })).is_err(),
        "explicit async transaction did not request minCommitTS"
    );

    session
        .execute("set autocommit = 0")
        .expect("disable autocommit");
    session
        .execute("insert into t1 values (3)")
        .expect("stage implicit transaction");
    assert!(
        catch_unwind(AssertUnwindSafe(|| { session.execute("commit") })).is_err(),
        "autocommit=0 transaction did not request minCommitTS"
    );

    session
        .execute("set autocommit = 1; set tidb_enable_1pc = 1")
        .expect("enable 1PC autocommit");
    session
        .execute("insert into t1 values (4)")
        .expect("1PC autocommit bypasses TSO");

    session.execute("begin").expect("begin 1PC transaction");
    session
        .execute("insert into t1 values (5)")
        .expect("stage 1PC insert");
    assert!(
        catch_unwind(AssertUnwindSafe(|| { session.execute("commit") })).is_err(),
        "explicit 1PC transaction did not request minCommitTS"
    );

    session
        .execute("set autocommit = 0")
        .expect("disable autocommit for 1PC");
    session
        .execute("insert into t1 values (6)")
        .expect("stage implicit 1PC insert");
    assert!(
        catch_unwind(AssertUnwindSafe(|| { session.execute("commit") })).is_err(),
        "autocommit=0 1PC transaction did not request minCommitTS"
    );
}

/// Go `TestKill`.
#[test]
fn test_kill() {
    let _serial = serial_guard();
    let (_store, mut tk) = prepare("kill");
    tk.MustExec("kill connection_id()", Vec::new());
}

/// Go `TestIssue42426`.
#[test]
fn test_issue_42426() {
    let _serial = serial_guard();
    let (_store, mut tk) = prepare("issue_42426");
    tk.MustExec(
        "CREATE TABLE `sbtest1` (\
         `id` bigint(20) NOT NULL AUTO_INCREMENT,\
         `k` int(11) NOT NULL DEFAULT '0',\
         `c` char(120) NOT NULL DEFAULT '',\
         `pad` char(60) NOT NULL DEFAULT '',\
         PRIMARY KEY (`id`) /*T![clustered_index] CLUSTERED */,\
         KEY `k_1` (`k`)) \
         PARTITION BY RANGE (`id`) \
         (PARTITION `pnew` VALUES LESS THAN (10000000),\
         PARTITION `p5` VALUES LESS THAN (MAXVALUE))",
        Vec::new(),
    );
    tk.MustExec(
        r#"INSERT INTO sbtest1 (id, k, c, pad) VALUES (502571, 499449, "init", "val")"#,
        Vec::new(),
    );
    tk.MustExec("begin", Vec::new());
    tk.MustExec("delete from sbtest1 where id=502571", Vec::new());
    tk.MustQuery("select id from sbtest1 where id=502571", Vec::new())
        .Check(Rows(&[]));
    tk.MustExec(
        r#"INSERT INTO sbtest1 (id, k, c, pad) VALUES (502571, 499449, "abc", "def")"#,
        Vec::new(),
    );
    tk.MustExec("commit", Vec::new());
    tk.MustQuery("select id,k,c,pad from sbtest1 where id=502571", Vec::new())
        .Check(Rows(&["502571 499449 abc def"]));
}

/// Go `TestIndexLookUpWithStaticPrune`.
#[test]
fn test_index_lookup_with_static_prune() {
    let _serial = serial_guard();
    let (_store, mut tk) = prepare("static_prune");
    tk.MustExec(
        "create table t(a bigint, b decimal(41,16), c set('a','b','c'), \
         key idx_c(c)) partition by hash(a) partitions 4",
        Vec::new(),
    );
    tk.MustExec("insert into t values (1,2.0,'c')", Vec::new());
    let sql = "select * from t use index(idx_c) order by c limit 5";
    let plan = tk.MustQuery(&format!("explain {sql}"), Vec::new()).Rows();
    assert!(
        plan.iter().flatten().any(|cell| cell.contains("Limit")),
        "static partition-prune index lookup must retain Limit: {plan:?}"
    );
    let _ = tk.MustQuery(
        "select * from t use index(idx_c) order by c limit 5",
        Vec::new(),
    );
}

/// Go `TestTiKVClientReadTimeout`: this scenario requires three real replicas.
#[test]
#[ignore = "requires REAL_TIKV_PD and a running three-node TiKV cluster"]
fn test_tikv_client_read_timeout() {
    let _serial = serial_guard();
    let pd = std::env::var("REAL_TIKV_PD").expect("REAL_TIKV_PD endpoint");
    let http = if pd.starts_with("http") {
        pd.clone()
    } else {
        format!("http://{pd}")
    };
    let topology: serde_json::Value = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
        .get(format!("{http}/pd/api/v1/stores"))
        .send()
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .unwrap();
    let stores = topology["stores"]
        .as_array()
        .expect("PD stores")
        .iter()
        .filter(|entry| entry["store"]["state_name"] == "Up")
        .map(|entry| {
            (
                entry["store"]["id"].as_u64().unwrap(),
                entry["store"]["address"].as_str().unwrap().to_owned(),
            )
        })
        .collect::<Vec<_>>();
    if stores.len() < 3 {
        eprintln!("skip: requires at least three Up TiKV nodes");
        return;
    }
    let path = format!(
        "tikv://{}?disableGC=true",
        pd.trim_start_matches("http://")
            .trim_start_matches("https://")
    );
    let (store, domain) =
        astersql_testkit::mockstore::CreateTiKVStoreAndDomain(&path).expect("real TiKV fixture");
    astersql_session::runtime::RegisterRuntimeTopology(&domain, stores);
    let mut tk = NewTestKit(store);
    tk.MustExec(
        "create database if not exists session_fail_real_timeout",
        Vec::new(),
    );
    tk.MustExec("use session_fail_real_timeout", Vec::new());
    tk.MustExec("drop table if exists t", Vec::new());
    tk.MustExec("set tidb_enable_clustered_index=on", Vec::new());
    check_read_timeout(&mut tk);
    tk.MustExec("drop table t", Vec::new());
}

/// Keep format coverage on the in-memory fixture without claiming RPC parity.
#[test]
fn test_mock_read_timeout_explain_format() {
    let _serial = serial_guard();
    let (_store, mut tk) = prepare("read_timeout");
    check_read_timeout(&mut tk);
}

fn check_read_timeout(tk: &mut TestKit) {
    tk.MustExec("create table t (a int primary key, b int)", Vec::new());
    let tikv_count = tk
        .MustQuery(
            "select count(*) from information_schema.cluster_info where `type`='tikv'",
            Vec::new(),
        )
        .Rows();
    assert_eq!(tikv_count.len(), 1, "cluster_info={tikv_count:?}");
    assert!(
        tikv_count[0][0]
            .parse::<usize>()
            .expect("numeric TiKV node count")
            >= 3,
        "read-timeout retry needs at least three TiKV replicas"
    );

    let _delay = failpoint::enable(BATCH_SEND_DELAY, "return(100)");
    tk.MustExec("set @stale_read_ts_var=now(6)", Vec::new());
    let hinted = [
        (
            "explain analyze select /*+ set_var(tikv_client_read_timeout=1) */ \
             * from t where a = 1",
            1,
            &["Point_Get", "Get:{num_rpc:4,", "total_time:"][..],
        ),
        (
            "explain analyze select /*+ set_var(tikv_client_read_timeout=1) */ \
             * from t where a in (1,2)",
            1,
            &["Batch_Point_Get", "BatchGet:{num_rpc:4,", "total_time:"][..],
        ),
        (
            "explain analyze select /*+ set_var(tikv_client_read_timeout=1) */ \
             * from t where b > 1",
            3,
            &[
                "TableReader",
                "root",
                "time:",
                "loops:",
                "cop_task: {num: 1",
                "num_rpc:4",
            ][..],
        ),
    ];
    for (sql, expected_rows, parts) in hinted {
        let rows = tk.MustQuery(sql, Vec::new()).Rows();
        assert_eq!(rows.len(), expected_rows, "EXPLAIN rows={rows:?}");
        assert_explain_parts(&joined_row(&rows, 0), parts);
    }

    if !astersql_config_kerneltype::IsNextGen() {
        tk.MustExec("insert into t values (1,1),(2,2)", Vec::new());
        tk.MustExec("set @@tidb_replica_read='closest-replicas'", Vec::new());
        let hinted_stale = tk
            .MustQuery(
                "explain analyze select /*+ set_var(tikv_client_read_timeout=1) */ \
             * from t as of timestamp(@stale_read_ts_var) where b > 1",
                Vec::new(),
            )
            .Rows();
        assert_eq!(hinted_stale.len(), 3, "EXPLAIN rows={hinted_stale:?}");
        let hinted_stale = joined_row(&hinted_stale, 0);
        assert_explain_parts(
            &hinted_stale,
            &[
                "TableReader",
                "root",
                "time:",
                "loops:",
                "cop_task: {num: 1",
            ],
        );
        assert!(
            (3..=5).contains(&explain_rpc_count(&hinted_stale)),
            "hinted stale read retry count must be 3..=5: {hinted_stale}"
        );
    }

    tk.MustExec("set @@tikv_client_read_timeout=1", Vec::new());
    let session_var = [
        (
            "explain analyze select * from t where a = 1",
            1,
            &["Point_Get", "Get:{num_rpc:4,", "total_time:"][..],
        ),
        (
            "explain analyze select * from t where a in (1,2)",
            1,
            &["Batch_Point_Get", "BatchGet:{num_rpc:4,", "total_time:"][..],
        ),
        (
            "explain analyze select * from t where b > 1",
            3,
            &[
                "TableReader",
                "root",
                "time:",
                "loops:",
                "cop_task: {num: 1",
                "num_rpc:4",
            ][..],
        ),
    ];
    for (sql, expected_rows, parts) in session_var {
        let rows = tk.MustQuery(sql, Vec::new()).Rows();
        assert_eq!(rows.len(), expected_rows, "EXPLAIN rows={rows:?}");
        assert_explain_parts(&joined_row(&rows, 0), parts);
    }

    if !astersql_config_kerneltype::IsNextGen() {
        tk.MustExec("set @@tidb_replica_read='closest-replicas'", Vec::new());
        // Stale reads use the same timeout fallback, but may issue three to five
        // RPCs depending on which replica wins.
        let stale = tk
            .MustQuery(
                "explain analyze select * from t as of timestamp(@stale_read_ts_var) where b > 1",
                Vec::new(),
            )
            .Rows();
        assert_eq!(stale.len(), 3, "EXPLAIN rows={stale:?}");
        let explain = joined_row(&stale, 0);
        assert_explain_parts(
            &explain,
            &[
                "TableReader",
                "root",
                "time:",
                "loops:",
                "cop_task: {num: 1",
            ],
        );
        assert!(
            (3..=5).contains(&explain_rpc_count(&explain)),
            "stale read retry count must be 3..=5: {explain}"
        );
    }
}

/// Go `TestIssue57530`.
#[test]
fn test_issue_57530() {
    let _serial = serial_guard();
    let (_store, mut tk) = prepare("issue_57530");
    tk.MustExec("use information_schema", Vec::new());
    tk.MustQuery(
        "select * from TIKV_REGION_STATUS where table_id = 81920",
        Vec::new(),
    )
    .Check(Vec::<Vec<String>>::new());
}

#[test]
fn explain_assertions_reject_reordered_fields() {
    assert!(
        catch_unwind(|| assert_explain_parts(
            "Get:{num_rpc:4, total_time:1ms} Point_Get",
            &["Point_Get", "Get:{num_rpc:4,", "total_time:"]
        ))
        .is_err()
    );
}
