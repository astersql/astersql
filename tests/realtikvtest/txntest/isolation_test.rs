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
//! 中文总览：`isolation_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `事务语义与时间戳行为` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 83 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `IsolationMode` 是当前文件里的分支类型。
//! `IsolationMode` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `IsolationMode` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `IsolationMode`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `MODES` 是当前文件里的常量。
//! `MODES` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `MODES` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `MODES`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `prepare` 是当前文件里的辅助函数。
//! `prepare` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `prepare` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `prepare`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `session` 是当前文件里的辅助函数。
//! `session` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `session` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `session`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `configure` 是当前文件里的辅助函数。
//! `configure` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `configure` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `configure`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `reset_x` 是当前文件里的辅助函数。
//! `reset_x` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `reset_x` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `reset_x`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `reset_xy` 是当前文件里的辅助函数。
//! `reset_xy` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `reset_xy` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `reset_xy`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_get_cached_store` 是当前文件里的测试用例。
//! `test_get_cached_store` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_get_cached_store` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_get_cached_store`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_p0_dirty_write` 是当前文件里的测试用例。
//! `test_p0_dirty_write` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_p0_dirty_write` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_p0_dirty_write`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_p1_dirty_read` 是当前文件里的测试用例。
//! `test_p1_dirty_read` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_p1_dirty_read` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_p1_dirty_read`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_p2_non_repeatable_read` 是当前文件里的测试用例。
//! `test_p2_non_repeatable_read` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_p2_non_repeatable_read` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_p2_non_repeatable_read`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_p3_phantom` 是当前文件里的测试用例。
//! `test_p3_phantom` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_p3_phantom` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_p3_phantom`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_p4_lost_update` 是当前文件里的测试用例。
//! `test_p4_lost_update` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_p4_lost_update` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_p4_lost_update`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_p4c_lost_update_cursor_is_unsupported` 是当前文件里的测试用例。
//! `test_p4c_lost_update_cursor_is_unsupported` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_p4c_lost_update_cursor_is_unsupported` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_p4c_lost_update_cursor_is_unsupported`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_a3_phantom` 是当前文件里的测试用例。
//! `test_a3_phantom` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_a3_phantom` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_a3_phantom`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_a5a_read_skew` 是当前文件里的测试用例。
//! `test_a5a_read_skew` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_a5a_read_skew` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_a5a_read_skew`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_a5b_write_skew` 是当前文件里的测试用例。
//! `test_a5b_write_skew` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_a5b_write_skew` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_a5b_write_skew`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_read_after_write` 是当前文件里的测试用例。
//! `test_read_after_write` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_read_after_write` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_read_after_write`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_phantom_read_in_innodb` 是当前文件里的测试用例。
//! `test_phantom_read_in_innodb` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_phantom_read_in_innodb` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_phantom_read_in_innodb`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 中文说明结束（自动生成）

//! ANSI isolation phenomena exercised through independent concrete SQL
//! sessions sharing one transactional store.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use astersql_store_driver::{InMemoryBackend, TiKVDriver};
use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{NewTestKit, Rows, TestKit};
use astersql_tests_realtikvtest::TiKVPath;
use astersql_tests_realtikvtest_txntest::serial_guard;

#[derive(Clone, Copy, Debug)]
enum IsolationMode {
    OptimisticRepeatableRead,
    PessimisticRepeatableRead,
    PessimisticReadCommitted,
}

const MODES: [IsolationMode; 3] = [
    IsolationMode::OptimisticRepeatableRead,
    IsolationMode::PessimisticRepeatableRead,
    IsolationMode::PessimisticReadCommitted,
];

fn prepare(name: &str) -> (Arc<AnalyzeStatsStore>, TestKit, TestKit, String) {
    let (store, _domain) = CreateMockStoreAndDomain();
    let database = format!("txn_iso_{name}");
    let mut first = NewTestKit(store.clone());
    first.MustExec(&format!("drop database if exists `{database}`"), Vec::new());
    first.MustExec(&format!("create database `{database}`"), Vec::new());
    first.MustExec(&format!("use `{database}`"), Vec::new());
    let mut second = NewTestKit(store.clone());
    second.MustExec(&format!("use `{database}`"), Vec::new());
    (store, first, second, database)
}

fn session(store: Arc<AnalyzeStatsStore>, database: &str) -> TestKit {
    let mut tk = NewTestKit(store);
    tk.MustExec(&format!("use `{database}`"), Vec::new());
    tk
}

fn configure(first: &mut TestKit, second: &mut TestKit, mode: IsolationMode) {
    match mode {
        IsolationMode::OptimisticRepeatableRead => {
            first.MustExec("set tidb_txn_mode = 'optimistic'", Vec::new());
            second.MustExec("set tidb_txn_mode = 'optimistic'", Vec::new());
            first.MustExec(
                "set transaction isolation level repeatable read",
                Vec::new(),
            );
            second.MustExec(
                "set transaction isolation level repeatable read",
                Vec::new(),
            );
        }
        IsolationMode::PessimisticRepeatableRead => {
            first.MustExec("set tidb_txn_mode = 'pessimistic'", Vec::new());
            second.MustExec("set tidb_txn_mode = 'pessimistic'", Vec::new());
            first.MustExec(
                "set transaction isolation level repeatable read",
                Vec::new(),
            );
            second.MustExec(
                "set transaction isolation level repeatable read",
                Vec::new(),
            );
        }
        IsolationMode::PessimisticReadCommitted => {
            first.MustExec("set tidb_txn_mode = 'pessimistic'", Vec::new());
            second.MustExec("set tidb_txn_mode = 'pessimistic'", Vec::new());
            first.MustExec("set transaction isolation level read committed", Vec::new());
            second.MustExec("set transaction isolation level read committed", Vec::new());
        }
    }
}

fn reset_x(tk: &mut TestKit) {
    tk.MustExec("drop table if exists x", Vec::new());
    tk.MustExec("create table x (id int primary key, c int)", Vec::new());
    tk.MustExec("insert into x values (1,1)", Vec::new());
}

fn reset_xy(tk: &mut TestKit) {
    reset_x(tk);
    tk.MustExec("drop table if exists y", Vec::new());
    tk.MustExec("create table y (id int primary key, c int)", Vec::new());
    tk.MustExec("insert into y values (1,1)", Vec::new());
}

/// Opening the same TiKV path reuses the cached store. Closing either handle
/// closes their shared inner store, which proves identity rather than merely
/// comparing two copied configuration values.
#[test]
fn test_get_cached_store() {
    let _serial = serial_guard();
    // PD/TiKV availability is outside this cache-identity test. Keep the real
    // driver/cache lifecycle while replacing only that external boundary.
    let mut driver = TiKVDriver::with_backend(Arc::new(InMemoryBackend::default()));
    let path = TiKVPath();
    let first = driver.Open(&path).expect("open first cached store");
    let second = driver.Open(&path).expect("open second cached store");
    assert_eq!(first.GetClusterID(), second.GetClusterID());
    assert_eq!(first.GetKeyspace(), second.GetKeyspace());
    first.Close().expect("close cached store");
    assert!(
        second.is_closed(),
        "both handles must share one cached store"
    );
    second.Close().expect("cached close is idempotent");
}

#[test]
fn test_p0_dirty_write() {
    let _serial = serial_guard();
    let (store, mut first, mut second, database) = prepare("p0_dirty_write");

    configure(
        &mut first,
        &mut second,
        IsolationMode::OptimisticRepeatableRead,
    );
    reset_x(&mut first);
    first.MustExec("begin", Vec::new());
    first.MustExec("update x set c=c+1 where id=1", Vec::new());
    second.MustExec("begin", Vec::new());
    second.MustExec("update x set c=c+1 where id=1", Vec::new());
    first.MustExec("commit", Vec::new());
    let conflict = second.Exec("commit", Vec::new()).expect_err("dirty write");
    assert!(
        conflict.to_string().contains("[kv:9007]Write conflict"),
        "unexpected optimistic conflict: {conflict}"
    );

    for mode in [
        IsolationMode::PessimisticRepeatableRead,
        IsolationMode::PessimisticReadCommitted,
    ] {
        configure(&mut first, &mut second, mode);
        reset_x(&mut first);
        first.MustExec("begin", Vec::new());
        first.MustExec("update x set c=c+1 where id=1", Vec::new());
        let peer_store = store.clone();
        let peer_db = database.clone();
        let waiter = thread::spawn(move || {
            let mut peer = session(peer_store, &peer_db);
            peer.MustExec("set tidb_txn_mode = 'pessimistic'", Vec::new());
            if matches!(mode, IsolationMode::PessimisticReadCommitted) {
                peer.MustExec("set transaction isolation level read committed", Vec::new());
            }
            peer.MustExec("begin", Vec::new());
            peer.MustExec("update x set c=c+1 where id=1", Vec::new());
            peer.MustExec("commit", Vec::new());
        });
        thread::sleep(Duration::from_millis(25));
        first.MustExec("commit", Vec::new());
        waiter.join().expect("pessimistic waiter");
        first
            .MustQuery("select * from x", Vec::new())
            .Check(Rows(&["1 3"]));
    }
}

#[test]
fn test_p1_dirty_read() {
    let _serial = serial_guard();
    let (_store, mut first, mut second, _database) = prepare("p1_dirty_read");
    for mode in MODES {
        configure(&mut first, &mut second, mode);
        reset_x(&mut first);
        first.MustExec("begin", Vec::new());
        first.MustExec("update x set c=c+1 where id=1", Vec::new());
        first
            .MustQuery("select c from x where id=1", Vec::new())
            .Check(Rows(&["2"]));
        second.MustExec("begin", Vec::new());
        second
            .MustQuery("select c from x where id=1", Vec::new())
            .Check(Rows(&["1"]));
        first.MustExec("commit", Vec::new());
        second.MustExec("commit", Vec::new());
    }
}

#[test]
fn test_p2_non_repeatable_read() {
    let _serial = serial_guard();
    let (_store, mut first, mut second, _database) = prepare("p2_non_repeatable");
    for mode in MODES {
        configure(&mut first, &mut second, mode);
        reset_xy(&mut first);
        first.MustExec("begin", Vec::new());
        second.MustExec("begin", Vec::new());
        first
            .MustQuery("select c from x where id=1", Vec::new())
            .Check(Rows(&["1"]));
        second
            .MustQuery("select c from x where id=1", Vec::new())
            .Check(Rows(&["1"]));
        second.MustExec("update x set c=c+1 where id=1", Vec::new());
        second
            .MustQuery("select c from y where id=1", Vec::new())
            .Check(Rows(&["1"]));
        second.MustExec("update y set c=c+1 where id=1", Vec::new());
        second.MustExec("commit", Vec::new());
        let expected = if matches!(mode, IsolationMode::PessimisticReadCommitted) {
            "2"
        } else {
            "1"
        };
        first
            .MustQuery("select c from y where id=1", Vec::new())
            .Check(Rows(&[expected]));
        first.MustExec("commit", Vec::new());
    }
}

#[test]
fn test_p3_phantom() {
    let _serial = serial_guard();
    let (_store, mut first, mut second, _database) = prepare("p3_phantom");
    for mode in MODES {
        configure(&mut first, &mut second, mode);
        reset_x(&mut first);
        first.MustExec("drop table if exists z", Vec::new());
        first.MustExec("create table z (id int primary key, c int)", Vec::new());
        first.MustExec("insert into z values (1,1)", Vec::new());
        first.MustExec("begin", Vec::new());
        second.MustExec("begin", Vec::new());
        first
            .MustQuery("select c from x where id<5", Vec::new())
            .Check(Rows(&["1"]));
        second.MustExec("insert into x values (2,1)", Vec::new());
        second
            .MustQuery("select c from z where id=1", Vec::new())
            .Check(Rows(&["1"]));
        second.MustExec("update z set c=c+1 where id=1", Vec::new());
        second.MustExec("commit", Vec::new());
        let expected = if matches!(mode, IsolationMode::PessimisticReadCommitted) {
            "2"
        } else {
            "1"
        };
        first
            .MustQuery("select c from z where id=1", Vec::new())
            .Check(Rows(&[expected]));
        first.MustExec("commit", Vec::new());
    }
}

#[test]
fn test_p4_lost_update() {
    let _serial = serial_guard();
    let (_store, mut first, mut second, _database) = prepare("p4_lost_update");
    for mode in MODES {
        configure(&mut first, &mut second, mode);
        reset_x(&mut first);
        first.MustExec("begin", Vec::new());
        first
            .MustQuery("select c from x where id=1", Vec::new())
            .Check(Rows(&["1"]));
        second.MustExec("begin", Vec::new());
        second
            .MustQuery("select c from x where id=1", Vec::new())
            .Check(Rows(&["1"]));
        second.MustExec("update x set c=c+1 where id=1", Vec::new());
        second.MustExec("commit", Vec::new());
        first.MustExec("update x set c=c+1 where id=1", Vec::new());
        if matches!(mode, IsolationMode::OptimisticRepeatableRead) {
            let conflict = first.Exec("commit", Vec::new()).expect_err("lost update");
            assert!(
                conflict.to_string().contains("[kv:9007]Write conflict"),
                "unexpected optimistic conflict: {conflict}"
            );
        } else {
            first.MustExec("commit", Vec::new());
            first
                .MustQuery("select * from x", Vec::new())
                .Check(Rows(&["1 3"]));
        }
    }
}

/// The Go source intentionally leaves this case empty because SQL cursors are
/// unsupported. The Rust port verifies that boundary explicitly.
#[test]
fn test_p4c_lost_update_cursor_is_unsupported() {
    let _serial = serial_guard();
    let (_store, mut first, _second, _database) = prepare("p4c_cursor");
    let error = first.ExecToErr("declare txn_cursor cursor for select * from x");
    assert!(
        error.to_string().contains("unsupported")
            || error.to_string().contains("parse")
            || error.to_string().contains("statement"),
        "unexpected cursor error: {error}"
    );
}

#[test]
fn test_a3_phantom() {
    let _serial = serial_guard();
    let (_store, mut first, mut second, _database) = prepare("a3_phantom");
    for mode in MODES {
        configure(&mut first, &mut second, mode);
        reset_x(&mut first);
        first.MustExec("begin", Vec::new());
        second.MustExec("begin", Vec::new());
        second
            .MustQuery("select c from x where id<5", Vec::new())
            .Check(Rows(&["1"]));
        first.MustExec("insert into x values (2,1)", Vec::new());
        first.MustExec("commit", Vec::new());
        let expected = if matches!(mode, IsolationMode::PessimisticReadCommitted) {
            Rows(&["1", "1"])
        } else {
            Rows(&["1"])
        };
        second
            .MustQuery("select c from x where id<5", Vec::new())
            .Check(expected);
        second.MustExec("commit", Vec::new());
    }
}

#[test]
fn test_a5a_read_skew() {
    let _serial = serial_guard();
    let (_store, mut first, mut second, _database) = prepare("a5a_read_skew");
    for mode in MODES {
        configure(&mut first, &mut second, mode);
        reset_xy(&mut first);
        first.MustExec("begin", Vec::new());
        second.MustExec("begin", Vec::new());
        first
            .MustQuery("select c from x where id=1", Vec::new())
            .Check(Rows(&["1"]));
        second.MustExec("update x set c=c+1 where id=1", Vec::new());
        second.MustExec("update y set c=c+1 where id=1", Vec::new());
        second.MustExec("commit", Vec::new());
        let expected = if matches!(mode, IsolationMode::PessimisticReadCommitted) {
            "2"
        } else {
            "1"
        };
        first
            .MustQuery("select c from y where id=1", Vec::new())
            .Check(Rows(&[expected]));
        first.MustExec("commit", Vec::new());
    }
}

#[test]
fn test_a5b_write_skew() {
    let _serial = serial_guard();
    let (_store, mut first, mut second, _database) = prepare("a5b_write_skew");
    for mode in MODES {
        configure(&mut first, &mut second, mode);
        reset_xy(&mut first);
        first.MustExec("begin", Vec::new());
        second.MustExec("begin", Vec::new());
        first
            .MustQuery("select c from x where id=1", Vec::new())
            .Check(Rows(&["1"]));
        second
            .MustQuery("select c from y where id=1", Vec::new())
            .Check(Rows(&["1"]));
        first.MustExec("update y set c=c+1 where id=1", Vec::new());
        second.MustExec("update x set c=c+1 where id=1", Vec::new());
        second.MustExec("commit", Vec::new());
        first.MustExec("commit", Vec::new());
        first
            .MustQuery("select c from x", Vec::new())
            .Check(Rows(&["2"]));
        first
            .MustQuery("select c from y", Vec::new())
            .Check(Rows(&["2"]));

        // The Go case repeats write skew while moving disjoint primary keys.
        first.MustExec("update y set id=2 where id=1", Vec::new());
        first.MustExec("begin", Vec::new());
        second.MustExec("begin", Vec::new());
        first
            .MustQuery("select id from x where id=1", Vec::new())
            .Check(Rows(&["1"]));
        second
            .MustQuery("select id from y where id=2", Vec::new())
            .Check(Rows(&["2"]));
        first.MustExec("update y set id=1 where id=2", Vec::new());
        second.MustExec("update x set id=2 where id=1", Vec::new());
        second.MustExec("commit", Vec::new());
        first.MustExec("commit", Vec::new());
        first
            .MustQuery("select id from x", Vec::new())
            .Check(Rows(&["2"]));
        first
            .MustQuery("select id from y", Vec::new())
            .Check(Rows(&["1"]));
    }
}

#[test]
fn test_read_after_write() {
    let _serial = serial_guard();
    let (_store, mut first, mut second, _database) = prepare("read_after_write");
    for mode in MODES {
        configure(&mut first, &mut second, mode);
        reset_x(&mut first);
        first.MustExec("begin", Vec::new());
        first.MustExec("update x set c=c+1 where id=1", Vec::new());
        first.MustExec("commit", Vec::new());
        second.MustExec("begin", Vec::new());
        second
            .MustQuery("select c from x where id=1", Vec::new())
            .Check(Rows(&["2"]));
        second.MustExec("commit", Vec::new());
    }
}

#[test]
fn test_phantom_read_in_innodb() {
    let _serial = serial_guard();
    let (_store, mut first, mut second, _database) = prepare("phantom_innodb");
    for mode in MODES {
        configure(&mut first, &mut second, mode);
        reset_x(&mut first);
        first.MustExec("begin", Vec::new());
        first
            .MustQuery("select c from x where id<5", Vec::new())
            .Check(Rows(&["1"]));
        second.MustExec("begin", Vec::new());
        second.MustExec("insert into x values (2,1)", Vec::new());
        second.MustExec("commit", Vec::new());
        first.MustExec("update x set c=c+1 where id<5", Vec::new());
        let expected = if matches!(mode, IsolationMode::OptimisticRepeatableRead) {
            Rows(&["2"])
        } else {
            Rows(&["2", "2"])
        };
        first
            .MustQuery("select c from x where id<5", Vec::new())
            .Check(expected);
        first.MustExec("commit", Vec::new());
    }
}
