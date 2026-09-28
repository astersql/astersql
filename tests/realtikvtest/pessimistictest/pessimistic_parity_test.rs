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

//! 中文说明开始（自动生成）
//! 中文总览：`pessimistic_parity_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `pessimistic_parity_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 62 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `LOCK_EXPIRED` 是当前文件里的常量。
//! 阅读 `LOCK_EXPIRED` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `LOCK_EXPIRED` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `LOCK_EXPIRED`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `LOCK_EXPIRED` 的重要阅读参照。
//! 符号 `LOCK_NOWAIT` 是当前文件里的常量。
//! 阅读 `LOCK_NOWAIT` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `LOCK_NOWAIT` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `LOCK_NOWAIT`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `LOCK_NOWAIT` 的重要阅读参照。
//! 符号 `LOCK_TIMEOUT` 是当前文件里的常量。
//! 阅读 `LOCK_TIMEOUT` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `LOCK_TIMEOUT` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `LOCK_TIMEOUT`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `LOCK_TIMEOUT` 的重要阅读参照。
//! 符号 `MAX_EXECUTION_TIME` 是当前文件里的常量。
//! 阅读 `MAX_EXECUTION_TIME` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `MAX_EXECUTION_TIME` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `MAX_EXECUTION_TIME`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `MAX_EXECUTION_TIME` 的重要阅读参照。
//! 符号 `fixture` 是当前文件里的辅助函数。
//! 阅读 `fixture` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `fixture` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `fixture`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `fixture` 的重要阅读参照。
//! 符号 `session` 是当前文件里的辅助函数。
//! 阅读 `session` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `session` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `session`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `session` 的重要阅读参照。
//! 符号 `assert_exact_error` 是当前文件里的辅助函数。
//! 阅读 `assert_exact_error` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `assert_exact_error` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `assert_exact_error`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `assert_exact_error` 的重要阅读参照。
//! 符号 `expired_locks_abort_reads_and_writes_and_release_every_key` 是当前文件里的辅助函数。
//! 阅读 `expired_locks_abort_reads_and_writes_and_release_every_key` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `expired_locks_abort_reads_and_writes_and_release_every_key` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `expired_locks_abort_reads_and_writes_and_release_every_key`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `expired_locks_abort_reads_and_writes_and_release_every_key` 的重要阅读参照。
//! 符号 `commit_failure_rolls_back_changes_and_releases_pessimistic_locks` 是当前文件里的辅助函数。
//! 阅读 `commit_failure_rolls_back_changes_and_releases_pessimistic_locks` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `commit_failure_rolls_back_changes_and_releases_pessimistic_locks` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `commit_failure_rolls_back_changes_and_releases_pessimistic_locks`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `commit_failure_rolls_back_changes_and_releases_pessimistic_locks` 的重要阅读参照。
//! 符号 `expiration_batch_resolves_locks_across_multiple_tables` 是当前文件里的辅助函数。
//! 阅读 `expiration_batch_resolves_locks_across_multiple_tables` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `expiration_batch_resolves_locks_across_multiple_tables` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `expiration_batch_resolves_locks_across_multiple_tables`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `expiration_batch_resolves_locks_across_multiple_tables` 的重要阅读参照。
//! 符号 `commit_and_rollback_text_and_binary_are_safe_after_lock_expiry` 是当前文件里的辅助函数。
//! 阅读 `commit_and_rollback_text_and_binary_are_safe_after_lock_expiry` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `commit_and_rollback_text_and_binary_are_safe_after_lock_expiry` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `commit_and_rollback_text_and_binary_are_safe_after_lock_expiry`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `commit_and_rollback_text_and_binary_are_safe_after_lock_expiry` 的重要阅读参照。
//! 符号 `for_share_promotion_matches_all_four_variable_combinations` 是当前文件里的辅助函数。
//! 阅读 `for_share_promotion_matches_all_four_variable_combinations` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `for_share_promotion_matches_all_four_variable_combinations` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `for_share_promotion_matches_all_four_variable_combinations`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `for_share_promotion_matches_all_four_variable_combinations` 的重要阅读参照。
//! 符号 `max_execution_time_applies_to_locking_selects_but_not_dml_lock_waits` 是当前文件里的辅助函数。
//! 阅读 `max_execution_time_applies_to_locking_selects_but_not_dml_lock_waits` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `max_execution_time_applies_to_locking_selects_but_not_dml_lock_waits` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `max_execution_time_applies_to_locking_selects_but_not_dml_lock_waits`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `max_execution_time_applies_to_locking_selects_but_not_dml_lock_waits` 的重要阅读参照。
//! 中文说明结束（自动生成）

//! High-risk pessimistic-transaction parity scenarios from the Go suite.
//!
//! These tests deliberately exercise the live `ConcreteSession` adapter and
//! its shared `Domain`; no SQL result or lock outcome is mocked.

use std::sync::Arc;
use std::time::{Duration, Instant};

use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{NewTestKit, Rows, TestKit};
use astersql_tests_realtikvtest_pessimistictest::serial_guard;

const LOCK_EXPIRED: &str =
    "[tikv:8220]TTL manager has timed out, pessimistic transaction has been rolled back";
const LOCK_NOWAIT: &str =
    "[tikv:3572]Statement could not be acquired immediately and NOWAIT is set";
const LOCK_TIMEOUT: &str = "[tikv:1205]Lock wait timeout exceeded; try restarting transaction";
const MAX_EXECUTION_TIME: &str =
    "[executor:3024]Query execution was interrupted, maximum statement execution time exceeded";

fn fixture(name: &str) -> (Arc<AnalyzeStatsStore>, TestKit, String) {
    let (store, _domain) = CreateMockStoreAndDomain();
    let database = format!("pess_parity_{name}");
    let mut setup = NewTestKit(store.clone());
    setup.MustExec(&format!("drop database if exists `{database}`"), Vec::new());
    setup.MustExec(&format!("create database `{database}`"), Vec::new());
    setup.MustExec(&format!("use `{database}`"), Vec::new());
    (store, setup, database)
}

fn session(store: Arc<AnalyzeStatsStore>, database: &str) -> TestKit {
    let mut tk = NewTestKit(store);
    tk.MustExec(&format!("use `{database}`"), Vec::new());
    tk
}

fn assert_exact_error(error: impl std::fmt::Display, expected: &str) {
    assert_eq!(error.to_string(), expected);
}

#[test]
fn expired_locks_abort_reads_and_writes_and_release_every_key() {
    let _serial = serial_guard();
    let (store, mut setup, database) = fixture("expired_locks");
    setup.MustExec(
        "create table t(c1 int primary key, c2 int unique, c3 int)",
        Vec::new(),
    );
    setup.MustExec("insert into t values (1,1,1),(5,5,5)", Vec::new());

    for statement in [
        "select * from t where c1 in (1,5)",
        "update t set c2=c2+10 where c1 in (1,5)",
    ] {
        // Open the holder last so the store hook addresses this exact session.
        let mut holder = session(store.clone(), &database);
        holder.MustExec("begin pessimistic", Vec::new());
        holder
            .MustQuery(
                "select * from t where c1 in (1,5) order by c1 for update",
                Vec::new(),
            )
            .Check(Rows(&["1 1 1", "5 5 5"]));
        store
            .set_latest_pessimistic_lock_ttl_for_test(Duration::from_millis(1))
            .expect("set holder lock TTL");
        store
            .expire_latest_pessimistic_locks_for_test()
            .expect("expire holder locks");
        assert_exact_error(holder.ExecToErr(statement), LOCK_EXPIRED);
        // The expiration boundary has rolled back the transaction; ending it
        // remains idempotent, as in Go's transaction cleanup path.
        holder.MustExec("commit", Vec::new());

        let mut probe = session(store.clone(), &database);
        probe.MustExec("begin pessimistic", Vec::new());
        probe
            .MustQuery(
                "select * from t where c1 in (1,5) order by c1 for update nowait",
                Vec::new(),
            )
            .Check(Rows(&["1 1 1", "5 5 5"]));
        probe.MustExec("rollback", Vec::new());
        setup
            .MustQuery("select * from t order by c1", Vec::new())
            .Check(Rows(&["1 1 1", "5 5 5"]));
    }
}

#[test]
fn commit_failure_rolls_back_changes_and_releases_pessimistic_locks() {
    let _serial = serial_guard();
    let (store, mut setup, database) = fixture("commit_failure");
    setup.MustExec(
        "create table t(id int primary key, value int not null)",
        Vec::new(),
    );
    setup.MustExec("insert into t values (1,100),(2,200)", Vec::new());

    let mut failed = session(store.clone(), &database);
    failed.MustExec("begin pessimistic", Vec::new());
    failed
        .MustQuery("select * from t where id=1 for update", Vec::new())
        .Check(Rows(&["1 100"]));
    failed.MustExec("update t set value=101 where id=1", Vec::new());
    let mut unaffected = session(store.clone(), &database);
    failed
        .Session()
        .InjectNextDmlCommitErrorForTest("injected schema-validity failure")
        .expect("inject commit failure into holder");
    unaffected.MustExec("begin pessimistic", Vec::new());
    unaffected.MustExec("update t set value=201 where id=2", Vec::new());
    unaffected.MustExec("commit", Vec::new());
    assert_exact_error(
        failed.ExecToErr("commit"),
        "injected schema-validity failure",
    );

    let mut follower = session(store.clone(), &database);
    follower.MustExec("begin pessimistic", Vec::new());
    follower
        .MustQuery("select * from t where id=1 for update nowait", Vec::new())
        .Check(Rows(&["1 100"]));
    follower.MustExec("update t set value=value+1 where id=1", Vec::new());
    follower.MustExec("commit", Vec::new());

    failed.MustExec("begin pessimistic", Vec::new());
    failed.MustExec("update t set value=value+1 where id=2", Vec::new());
    failed.MustExec("commit", Vec::new());
    setup
        .MustQuery("select * from t order by id", Vec::new())
        .Check(Rows(&["1 101", "2 202"]));
}

#[test]
fn expiration_batch_resolves_locks_across_multiple_tables() {
    let _serial = serial_guard();
    let (store, mut setup, database) = fixture("batch_resolve");
    for table in ["t1", "t2", "t3"] {
        setup.MustExec(
            &format!("create table {table}(id int primary key, value int)"),
            Vec::new(),
        );
        setup.MustExec(
            &format!("insert into {table} values (1,1),(2,2)"),
            Vec::new(),
        );
    }

    let mut stale = session(store.clone(), &database);
    stale.MustExec("begin pessimistic", Vec::new());
    for table in ["t1", "t2", "t3"] {
        stale
            .MustQuery(
                &format!("select * from {table} where id in (1,2) order by id for update"),
                Vec::new(),
            )
            .Check(Rows(&["1 1", "2 2"]));
    }
    store
        .expire_latest_pessimistic_locks_for_test()
        .expect("expire the multi-table lock set");
    assert_exact_error(stale.QueryToErr("select * from t2"), LOCK_EXPIRED);

    let mut resolver = session(store.clone(), &database);
    resolver.MustExec("begin pessimistic", Vec::new());
    for table in ["t1", "t2", "t3"] {
        resolver
            .MustQuery(
                &format!("select * from {table} where id in (1,2) order by id for update nowait"),
                Vec::new(),
            )
            .Check(Rows(&["1 1", "2 2"]));
        resolver.MustExec(
            &format!("update {table} set value=value+10 where id in (1,2)"),
            Vec::new(),
        );
    }
    resolver.MustExec("commit", Vec::new());
    for table in ["t1", "t2", "t3"] {
        setup
            .MustQuery(&format!("select * from {table} order by id"), Vec::new())
            .Check(Rows(&["1 11", "2 12"]));
        setup.MustExec(&format!("admin check table {table}"), Vec::new());
    }
}

#[test]
fn commit_and_rollback_text_and_binary_are_safe_after_lock_expiry() {
    let _serial = serial_guard();
    let (store, mut setup, database) = fixture("end_after_expiry");
    setup.MustExec("create table t(a int primary key, b int)", Vec::new());
    setup.MustExec("insert into t values (1,1)", Vec::new());

    for (name, end_sql, binary) in [
        ("commit text", "commit", false),
        ("commit binary", "commit", true),
        ("rollback text", "rollback", false),
        ("rollback binary", "rollback", true),
    ] {
        let mut tk = session(store.clone(), &database);
        let prepared = binary.then(|| tk.Prepare(end_sql));
        tk.MustExec("begin pessimistic", Vec::new());
        tk.MustExec("update t set b=10 where a=1", Vec::new());
        store
            .expire_latest_pessimistic_locks_for_test()
            .unwrap_or_else(|error| panic!("{name}: expire lock: {error}"));
        let error = tk.QueryToErr("select * from t");
        assert_eq!(error.message(), LOCK_EXPIRED, "{name}");
        match prepared {
            Some(statement) => {
                statement
                    .execute(&[])
                    .unwrap_or_else(|error| panic!("{name}: binary end transaction: {error}"));
            }
            None => tk.MustExec(end_sql, Vec::new()),
        }

        let mut probe = session(store.clone(), &database);
        probe.MustExec("begin pessimistic", Vec::new());
        probe
            .MustQuery("select * from t where a=1 for update nowait", Vec::new())
            .Check(Rows(&["1 1"]));
        probe.MustExec("rollback", Vec::new());
    }
}

#[test]
fn for_share_promotion_matches_all_four_variable_combinations() {
    let _serial = serial_guard();
    let (store, mut setup, database) = fixture("for_share_matrix");
    setup.MustExec("create table t(a int primary key, b int)", Vec::new());
    setup.MustExec("insert into t values (1,10)", Vec::new());
    let mut contender = session(store.clone(), &database);
    let mut holder = session(store.clone(), &database);

    for (noop, promotion) in [(false, false), (false, true), (true, false), (true, true)] {
        contender.MustExec(
            &format!("set tidb_enable_noop_functions={}", u8::from(noop)),
            Vec::new(),
        );
        contender.MustExec(
            &format!(
                "set tidb_enable_shared_lock_promotion={}",
                u8::from(promotion)
            ),
            Vec::new(),
        );
        contender.MustExec("set innodb_lock_wait_timeout=1", Vec::new());
        holder.MustExec("begin pessimistic", Vec::new());
        holder
            .MustQuery("select * from t for update", Vec::new())
            .Check(Rows(&["1 10"]));
        contender.MustExec("begin pessimistic", Vec::new());

        if promotion {
            for sql in [
                "select * from t where a=1 for share nowait",
                "select * from t for share nowait",
            ] {
                assert_exact_error(contender.QueryToErr(sql), LOCK_NOWAIT);
            }
            for sql in [
                "select * from t where a=1 for share",
                "select * from t for share",
            ] {
                assert_exact_error(contender.QueryToErr(sql), LOCK_TIMEOUT);
            }
        } else if noop {
            for sql in [
                "select * from t where a=1 for share nowait",
                "select * from t where a=1 for share",
                "select * from t for share",
                "select * from t",
            ] {
                contender.MustQuery(sql, Vec::new()).Check(Rows(&["1 10"]));
            }
        } else {
            for sql in [
                "select * from t where a=1 for share nowait",
                "select * from t for share",
                "select * from t for share nowait",
            ] {
                assert!(
                    contender
                        .QueryToErr(sql)
                        .message()
                        .contains("use tidb_enable_noop_functions to enable"),
                    "{sql} must preserve the FOR SHARE feature-gate error"
                );
            }
        }
        contender.MustExec("rollback", Vec::new());
        holder.MustExec("rollback", Vec::new());
    }
}

#[test]
fn max_execution_time_applies_to_locking_selects_but_not_dml_lock_waits() {
    let _serial = serial_guard();
    let (store, mut setup, database) = fixture("max_execution_time");
    setup.MustExec(
        "create table test_lock(id int primary key, value int)",
        Vec::new(),
    );
    setup.MustExec("insert into test_lock values (1,100)", Vec::new());
    let mut holder = session(store.clone(), &database);
    let mut waiter = session(store.clone(), &database);

    holder.MustExec("begin pessimistic", Vec::new());
    holder
        .MustQuery("select * from test_lock where id=1 for update", Vec::new())
        .Check(Rows(&["1 100"]));

    for sql in [
        "select * from test_lock where id=1 for update",
        "(select * from test_lock where id=1 for update)",
    ] {
        waiter.MustExec("begin pessimistic", Vec::new());
        waiter.MustExec("set innodb_lock_wait_timeout=30", Vec::new());
        waiter.MustExec("set max_execution_time=1000", Vec::new());
        let started = Instant::now();
        assert_exact_error(waiter.QueryToErr(sql), MAX_EXECUTION_TIME);
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(900) && elapsed < Duration::from_secs(2),
            "{sql} used the wrong deadline: {elapsed:?}"
        );
        waiter.MustExec("rollback", Vec::new());
    }

    waiter.MustExec("begin pessimistic", Vec::new());
    waiter.MustExec("set innodb_lock_wait_timeout=1", Vec::new());
    waiter.MustExec("set max_execution_time=300", Vec::new());
    let started = Instant::now();
    assert_exact_error(
        waiter.ExecToErr("update test_lock set value=value+1 where id=1"),
        LOCK_TIMEOUT,
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(900) && elapsed < Duration::from_secs(2),
        "DML must use innodb_lock_wait_timeout, elapsed {elapsed:?}"
    );
    waiter.MustExec("rollback", Vec::new());
    holder.MustExec("rollback", Vec::new());
}
