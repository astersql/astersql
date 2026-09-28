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

use astersql_domain::Domain;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{DbValue, TestKit};
use std::sync::{Arc, Barrier, Mutex, MutexGuard, mpsc};
use std::thread;
use std::time::Duration;

/// TiDB restricted-read-only is process-global.  Keep these shared-runtime
/// transaction tests mutually exclusive so a temporary toggle cannot race an
/// unrelated commit assertion.
static TXN_TEST_SERIAL: Mutex<()> = Mutex::new(());

fn transaction_test_lock() -> MutexGuard<'static, ()> {
    TXN_TEST_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 从 Domain 统计句柄读取表的 realtime_count，用于断言提交/回滚后的行可见性。
fn table_row_count(domain: &Domain, database: &str, table: &str) -> i64 {
    let table_info = domain
        .table_by_name(database, table)
        .unwrap_or_else(|error| panic!("typed InfoSchema lookup for {database}.{table}: {error}"));
    domain
        .stats_handle()
        .lock()
        .expect("statistics handle")
        .stats_meta(table_info.ID)
        .cloned()
        .unwrap_or_else(|| panic!("no statistics recorded for {database}.{table}"))
        .realtime_count
}

fn current_transaction_ts(testkit: &TestKit) -> u64 {
    testkit
        .MustQuery("select @@tidb_current_ts", Vec::new())
        .Rows()[0][0]
        .parse::<u64>()
        .expect("numeric transaction timestamp")
}

fn assert_retryable_write_conflict(error: &astersql_testkit::TestError) {
    assert!(
        error.message().contains("conflict") || error.message().contains("retry"),
        "unexpected write-conflict error: {error}"
    );
    assert!(
        error.message().contains("[try again later]"),
        "retryable marker was lost: {error}"
    );
}

// 对应 TestAutocommit：autocommit=0 下显式 commit/rollback 决定可见性，
// 且从 0 切换为 1 必须隐式提交正在进行的事务。
#[test]
fn autocommit_mode_switch_and_explicit_commit_rollback() {
    let _serial = transaction_test_lock();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("create table t (id int primary key)", Vec::new());
    tk.MustQuery("select @@autocommit", Vec::new())
        .Check(vec![vec!["1".to_owned()]]);

    // Rolling a transaction back must not roll back a successful global SET.
    tk.MustExec("set @@global.autocommit = 0", Vec::new());
    tk.MustQuery("select @@global.autocommit", Vec::new())
        .Check(vec![vec!["0".to_owned()]]);
    // A global mode change must not alter the existing session's mode.
    tk.MustQuery("select @@autocommit", Vec::new())
        .Check(vec![vec!["1".to_owned()]]);
    tk.MustExec("begin", Vec::new());
    tk.MustExec("insert into t values (1)", Vec::new());
    tk.MustExec("set @@global.autocommit = 1", Vec::new());
    tk.MustExec("rollback", Vec::new());
    tk.MustQuery("select id from t where id = 1", Vec::new())
        .Check(Vec::<Vec<String>>::new());
    tk.MustQuery("select @@global.autocommit", Vec::new())
        .Check(vec![vec!["1".to_owned()]]);

    // Changing the session mode from 0 to 1 implicitly commits.
    tk.MustExec("set autocommit = 0", Vec::new());
    tk.MustExec("begin", Vec::new());
    tk.MustExec("insert into t values (1)", Vec::new());
    tk.MustExec("set autocommit = 1", Vec::new());
    tk.MustExec("rollback", Vec::new());
    tk.MustQuery("select id from t where id = 1", Vec::new())
        .Check(vec![vec!["1".to_owned()]]);
    tk.MustQuery("select @@autocommit", Vec::new())
        .Check(vec![vec!["1".to_owned()]]);

    // The implicit transaction created by a write follows the same mode-switch rule.
    tk.MustExec("set autocommit = 0", Vec::new());
    tk.MustExec("insert into t values (2)", Vec::new());
    tk.MustExec("set autocommit = 1", Vec::new());
    tk.MustExec("rollback", Vec::new());
    tk.MustQuery("select id from t where id = 2", Vec::new())
        .Check(vec![vec!["2".to_owned()]]);

    // Repeating the current mode must not commit an open transaction.
    tk.MustExec("set autocommit = 0", Vec::new());
    tk.MustExec("begin", Vec::new());
    tk.MustExec("insert into t values (3)", Vec::new());
    tk.MustExec("set autocommit = 0", Vec::new());
    tk.MustExec("rollback", Vec::new());
    tk.MustQuery("select id from t where id = 3", Vec::new())
        .Check(Vec::<Vec<String>>::new());
    tk.MustQuery("select @@autocommit", Vec::new())
        .Check(vec![vec!["0".to_owned()]]);

    tk.MustExec("set autocommit = 1", Vec::new());
    tk.MustExec("begin", Vec::new());
    tk.MustExec("insert into t values (4)", Vec::new());
    tk.MustExec("set autocommit = 1", Vec::new());
    tk.MustExec("rollback", Vec::new());
    tk.MustQuery("select id from t where id = 4", Vec::new())
        .Check(Vec::<Vec<String>>::new());
    tk.MustQuery("select @@autocommit", Vec::new())
        .Check(vec![vec!["1".to_owned()]]);
}

// 对应 TestInTrans：begin/insert/rollback 与 autocommit=0 下 insert→commit。
#[test]
fn in_trans_begin_insert_rollback_and_autocommit_off_commit() {
    let _serial = transaction_test_lock();
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("create table t (id int primary key)", Vec::new());
    tk.MustExec("begin", Vec::new());
    assert_ne!(current_transaction_ts(&tk), 0);
    tk.MustExec("insert into t values (1)", Vec::new());
    assert_ne!(current_transaction_ts(&tk), 0);
    tk.MustExec("rollback", Vec::new());
    assert_eq!(current_transaction_ts(&tk), 0);
    tk.MustExec("analyze table t", Vec::new());
    assert_eq!(table_row_count(&domain, "test", "t"), 0);

    tk.MustExec("set autocommit=0", Vec::new());
    tk.MustExec("begin", Vec::new());
    assert_ne!(current_transaction_ts(&tk), 0);
    tk.MustExec("insert into t values (1)", Vec::new());
    tk.MustExec("commit", Vec::new());
    assert_eq!(current_transaction_ts(&tk), 0);
    tk.MustExec("analyze table t", Vec::new());
    assert_eq!(table_row_count(&domain, "test", "t"), 1);
    tk.MustExec("insert into t values (2)", Vec::new());
    assert_ne!(current_transaction_ts(&tk), 0);
    tk.MustExec("commit", Vec::new());
    assert_eq!(current_transaction_ts(&tk), 0);
    tk.MustExec("analyze table t", Vec::new());
    assert_eq!(table_row_count(&domain, "test", "t"), 2);
    tk.MustExec("set autocommit=1", Vec::new());

    tk.MustExec("begin", Vec::new());
    assert_ne!(current_transaction_ts(&tk), 0);
    tk.MustExec("insert into t values (3)", Vec::new());
    tk.MustExec("rollback", Vec::new());
    assert_eq!(current_transaction_ts(&tk), 0);
}

// 对应 TestTxnLazyInitialize：autocommit=0 不会让 SELECT 常量、读取
// current_ts 或 SET 语句过早创建事务；EXPLAIN、BEGIN、读表和写表会创建事务。
fn assert_txn_lazy_initialization(is_pessimistic: bool) {
    let _serial = transaction_test_lock();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("create table t (id int primary key)", Vec::new());
    if is_pessimistic {
        tk.MustExec("set tidb_txn_mode = 'pessimistic'", Vec::new());
    }
    tk.MustExec("set @@autocommit = 0", Vec::new());

    for sql in ["select @@tidb_current_ts", "select 1"] {
        tk.MustQuery(sql, Vec::new());
        tk.MustQuery("select @@tidb_current_ts", Vec::new())
            .Check(vec![vec!["0".to_owned()]]);
    }
    tk.MustExec("set @@tidb_general_log = 0", Vec::new());
    tk.MustQuery("select @@tidb_current_ts", Vec::new())
        .Check(vec![vec!["0".to_owned()]]);

    tk.MustQuery("explain select * from t", Vec::new());
    let explain_ts = tk.MustQuery("select @@tidb_current_ts", Vec::new()).Rows()[0][0]
        .parse::<u64>()
        .expect("numeric transaction timestamp after EXPLAIN");
    assert_ne!(explain_ts, 0, "EXPLAIN must initialize a transaction");
    tk.MustExec("rollback", Vec::new());

    for sql in ["begin", "select * from t", "insert into t values (1)"] {
        if sql.starts_with("select") {
            tk.MustQuery(sql, Vec::new());
        } else {
            tk.MustExec(sql, Vec::new());
        }
        let current_ts = tk.MustQuery("select @@tidb_current_ts", Vec::new()).Rows()[0][0]
            .parse::<u64>()
            .expect("numeric active transaction timestamp");
        assert_ne!(current_ts, 0, "{sql} must initialize a transaction");
        tk.MustExec("rollback", Vec::new());
    }
}

#[test]
fn txn_lazy_initialize_matches_optimistic_and_pessimistic_modes() {
    assert_txn_lazy_initialization(false);
    assert_txn_lazy_initialization(true);
}

// 对应 TestErrorRollback：重复键错误会回滚失败语句而不污染事务状态，
// 此后每次更新仍须生效。
#[test]
fn error_rollback_keeps_subsequent_updates_working() {
    let _serial = transaction_test_lock();
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store.clone());
    tk.MustExec(
        "create table t_rollback (c1 int, c2 int, primary key(c1))",
        Vec::new(),
    );
    tk.MustExec("insert into t_rollback values (0, 0)", Vec::new());
    let workers = 4;
    let iterations = 20;
    let barrier = Arc::new(Barrier::new(workers));
    let mut handles = Vec::new();
    for _ in 0..workers {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            let mut worker = TestKit::new(store);
            worker.MustExec("set @@session.tidb_retry_limit = 100", Vec::new());
            barrier.wait();
            for _ in 0..iterations {
                let _ = worker.Exec("insert into t_rollback values (1, 1)", Vec::new());
                worker.MustExec("update t_rollback set c2 = c2 + 1 where c1 = 0", Vec::new());
            }
        }));
    }
    for handle in handles {
        handle.join().expect("rollback worker must not panic");
    }
    tk.MustQuery("select c2 from t_rollback where c1 = 0", Vec::new())
        .Check(vec![vec![(workers * iterations).to_string()]]);
    tk.MustExec("analyze table t_rollback", Vec::new());
    assert_eq!(table_row_count(&domain, "test", "t_rollback"), 2);
}

// 对应 Go TestDisableTxnAutoRetry：显式事务遇到并发提交后的写冲突必须向调用方报错，
// 而不是静默发布过期写入；rollback 后会话仍可继续使用。
#[test]
fn disable_txn_auto_retry_reports_conflict_and_recovers_after_rollback() {
    let _serial = transaction_test_lock();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk1 = TestKit::new(store.clone());
    let mut tk2 = TestKit::new(store);
    tk1.MustExec(
        "create table no_retry (id int primary key, v int)",
        Vec::new(),
    );
    tk1.MustExec("insert into no_retry values (1, 0)", Vec::new());

    tk1.MustExec("set tidb_txn_mode = 'optimistic'", Vec::new());
    tk2.MustExec("set tidb_txn_mode = 'optimistic'", Vec::new());
    tk1.MustExec("set @@tidb_disable_txn_auto_retry = 1", Vec::new());
    tk1.MustExec("begin", Vec::new());
    tk1.MustExec("update no_retry set v = 1 where id = 1", Vec::new());
    tk2.MustExec("update no_retry set v = 2 where id = 1", Vec::new());
    let error = tk1.ExecToErr("commit");
    assert_retryable_write_conflict(&error);
    tk1.MustExec("rollback", Vec::new());
    tk1.MustQuery("select v from no_retry where id = 1", Vec::new())
        .Check(vec![vec!["2".to_owned()]]);

    // Autocommit statements still retry after an earlier explicit transaction failed.
    tk1.MustQuery("select 1", Vec::new());
    tk2.MustExec("update no_retry set v = 3 where id = 1", Vec::new());
    tk1.MustExec("update no_retry set v = 4 where id = 1", Vec::new());
    tk1.MustQuery("select v from no_retry where id = 1", Vec::new())
        .Check(vec![vec!["4".to_owned()]]);

    // Restricted/internal SQL retains its retry behavior.
    let internal = astersql_kv::WithInternalSourceType(
        astersql_kv::Context::todo(),
        astersql_kv::InternalTxnOthers,
    );
    tk1.Session()
        .ExecuteInternal(&internal, "begin", &[])
        .expect("begin internal transaction");
    tk2.MustExec("update no_retry set v = 6 where id = 1", Vec::new());
    tk1.Session()
        .ExecuteInternal(&internal, "update no_retry set v = 7 where id = 1", &[])
        .expect("execute internal update");
    tk1.Session()
        .ExecuteInternal(&internal, "commit", &[])
        .expect("internal transaction retries its conflict");

    // Disabling local latches must still surface a storage write conflict.
    let restore_config = astersql_config::restore_func();
    astersql_config::update_global(|config| {
        config.txn_local_latches.enabled = false;
    });
    tk1.MustExec("begin", Vec::new());
    tk1.MustExec("update no_retry set v = 9 where id = 1", Vec::new());
    tk2.MustExec("update no_retry set v = 8 where id = 1", Vec::new());
    let error = tk1.ExecToErr("commit");
    assert_retryable_write_conflict(&error);
    tk1.MustExec("rollback", Vec::new());
    restore_config();

    // A schema change between begin and commit invalidates the old transaction.
    tk1.MustExec("begin", Vec::new());
    tk2.MustExec("alter table no_retry add index idx(v)", Vec::new());
    tk2.MustQuery("select v from no_retry", Vec::new())
        .Check(vec![vec!["8".to_owned()]]);
    tk1.MustExec("update no_retry set v = 10 where id = 1", Vec::new());
    assert!(
        tk1.Exec("commit", Vec::new()).is_err(),
        "schema-changing commit must fail"
    );

    // Both the SET-autocommit commit boundary and COMMIT preserve retry errors.
    for (seen, next, write, commit_sql) in [
        ("8", "11", "12", "set autocommit = 1"),
        ("11", "13", "14", "commit"),
    ] {
        tk1.MustExec("set autocommit = 0", Vec::new());
        tk1.MustQuery("select v from no_retry", Vec::new())
            .Check(vec![vec![seen.to_owned()]]);
        tk2.MustExec(
            &format!("update no_retry set v = {next} where id = 1"),
            Vec::new(),
        );
        tk1.MustExec(
            &format!("update no_retry set v = {write} where id = 1"),
            Vec::new(),
        );
        let error = tk1.ExecToErr(commit_sql);
        assert_retryable_write_conflict(&error);
        tk1.MustExec("rollback", Vec::new());
        tk2.MustQuery("select v from no_retry", Vec::new())
            .Check(vec![vec![next.to_owned()]]);
    }
    tk1.MustExec("set autocommit = 1", Vec::new());
}

// 对应 Go TestAutoCommitRespectsReadOnly：全局只读开关应在 autocommit 写入的
// 提交边界再次生效；恢复开关后同一会话必须能继续写入。
#[test]
fn autocommit_respects_restricted_read_only_at_commit_boundary() {
    let _serial = transaction_test_lock();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut admin = TestKit::new(store.clone());
    let identity = astersql_parser_auth::parser::auth::auth::UserIdentity {
        username: "root".to_owned(),
        hostname: "%".to_owned(),
        ..Default::default()
    };
    admin
        .Session()
        .AuthenticateUserForTest(&identity)
        .expect("authenticate read-only administrator");
    admin.MustExec("create table auto_commit_read_only (a int)", Vec::new());

    let mut writer = TestKit::new(store.clone());
    writer
        .Session()
        .AuthenticateUserForTest(&identity)
        .expect("authenticate read-only writer");
    let (started, ready) = mpsc::channel();
    let rejected = thread::spawn(move || {
        started.send(()).expect("signal long write start");
        writer.Exec(
            "insert into auto_commit_read_only values (sleep(1))",
            Vec::new(),
        )
    });
    ready.recv().expect("wait for long write start");
    thread::sleep(Duration::from_millis(100));
    admin.MustExec("set global tidb_restricted_read_only = 1", Vec::new());
    let rejected_write = rejected
        .join()
        .expect("long read-only writer must not panic");
    admin.MustExec("set global tidb_restricted_read_only = 0", Vec::new());
    admin.MustExec("set global tidb_super_read_only = 0", Vec::new());
    let error = rejected_write.expect_err("read-only autocommit write must fail");
    assert!(
        error.message().contains("read only") || error.message().contains("read-only"),
        "unexpected restricted-read-only error: {error}"
    );

    admin.MustExec(
        "grant RESTRICTED_REPLICA_WRITER_ADMIN on *.* to 'root'",
        Vec::new(),
    );
    let mut privileged_writer = TestKit::new(store.clone());
    privileged_writer
        .Session()
        .AuthenticateUserForTest(&identity)
        .expect("authenticate privileged writer");
    let (started, ready) = mpsc::channel();
    let allowed = thread::spawn(move || {
        started
            .send(())
            .expect("signal privileged long write start");
        privileged_writer.Exec(
            "insert into auto_commit_read_only values (sleep(1))",
            Vec::new(),
        )
    });
    ready.recv().expect("wait for privileged long write start");
    thread::sleep(Duration::from_millis(100));
    admin.MustExec("set global tidb_restricted_read_only = 1", Vec::new());
    admin.MustExec("insert into auto_commit_read_only values (0)", Vec::new());
    allowed
        .join()
        .expect("privileged writer must not panic")
        .expect("privileged writer bypasses read-only mode");
    admin.MustExec("set global tidb_restricted_read_only = 0", Vec::new());
    admin.MustExec("set global tidb_super_read_only = 0", Vec::new());
    let observer = TestKit::new(store);
    observer
        .MustQuery("select count(*) from auto_commit_read_only", Vec::new())
        .Check(vec![vec!["2".to_owned()]]);
}

// 对应 Go TestTxnRetryErrMsg：冲突提交必须保留可诊断的 retry/conflict 标记，
// 不能被转换为成功或丢失错误原因。
#[test]
fn txn_retry_error_keeps_retryable_diagnostic() {
    let _serial = transaction_test_lock();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk1 = TestKit::new(store.clone());
    let mut tk2 = TestKit::new(store);
    tk1.MustExec(
        "create table retry_message (id int primary key, v int)",
        Vec::new(),
    );
    tk1.MustExec("insert into retry_message values (1, 0)", Vec::new());
    tk1.MustExec("set tidb_txn_mode = 'optimistic'", Vec::new());
    tk2.MustExec("set tidb_txn_mode = 'optimistic'", Vec::new());
    tk1.MustExec("set @@tidb_disable_txn_auto_retry = 1", Vec::new());
    tk1.MustExec("begin", Vec::new());
    tk1.MustExec("update retry_message set v = 1 where id = 1", Vec::new());
    tk2.MustExec("update retry_message set v = 2 where id = 1", Vec::new());
    tk1.Session()
        .InjectNextDmlCommitErrorForTest("mock retryable error [try again later]")
        .expect("inject retryable commit error");
    let error = tk1.ExecToErr("commit");
    assert!(error.message().contains("mock retryable error"), "{error}");
    assert!(error.message().contains("[try again later]"), "{error}");
    tk1.MustExec("rollback", Vec::new());
}

// 对应 Go TestCommitTSOrderCheck：构造未来 last_commit_ts 后，下一次普通表读
// 必须拒绝早于该提交时间戳的 start_ts。
#[test]
fn commit_ts_order_rejects_read_before_future_last_commit_ts() {
    let _serial = transaction_test_lock();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec(
        "create table commit_ts_order (id int primary key)",
        Vec::new(),
    );
    tk.MustExec("begin", Vec::new());
    // TSO physical milliseconds occupy the high bits; move one minute ahead,
    // matching oracle.GoTimeToTS(GetTimeFromTS(ts).Add(time.Minute)) in Go.
    let future_ts = current_transaction_ts(&tk).saturating_add(60_000_u64 << 18);
    tk.MustExec("rollback", Vec::new());
    let _future_commit = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/session/mockFutureCommitTS",
        &format!("return({future_ts})"),
    );
    tk.MustExec("insert into commit_ts_order values (1)", Vec::new());
    let error = tk.ExecToErr("select * from commit_ts_order");
    assert!(
        error
            .message()
            .contains(&format!("is before session last_commit_ts:{future_ts}")),
        "unexpected commit timestamp order error: {error}"
    );
}

// 对应 Go TestMemBufferSnapshotRead：事务内 INSERT ... SELECT ... ON DUPLICATE
// 的 UnionScan 读取必须看到一致快照，提交前后均维持 a+b=100。
#[test]
fn mem_buffer_snapshot_read_preserves_union_scan_consistency() {
    let _serial = transaction_test_lock();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec(
        "create table mem_buffer (a int primary key, b int, index i(b))",
        Vec::new(),
    );
    for sql in [
        "set session tidb_distsql_scan_concurrency = 1",
        "set session tidb_index_lookup_join_concurrency = 1",
        "set session tidb_projection_concurrency = 1",
        "set session tidb_init_chunk_size = 1",
        "set session tidb_max_chunk_size = 40",
        "set session tidb_index_join_batch_size = 10",
        "begin",
    ] {
        tk.MustExec(sql, Vec::new());
    }
    let values = (0..=100)
        .map(|value| format!("({value}, {value})"))
        .collect::<Vec<_>>()
        .join(", ");
    tk.MustExec(
        &format!("insert into mem_buffer values {values}"),
        Vec::new(),
    );
    tk.MustExec(
        "insert into mem_buffer (select /*+ INL_JOIN(t1) */ 100 - t1.a as a, t1.b from mem_buffer t1, \
         (select a, b from mem_buffer) t2 where t1.b = t2.b) \
         on duplicate key update b = values(b)",
        Vec::new(),
    );
    tk.MustQuery("select a, b from mem_buffer where a + b != 100", Vec::new())
        .Check(Vec::<Vec<String>>::new());
    tk.MustExec("commit", Vec::new());
    tk.MustQuery("select a, b from mem_buffer where a + b != 100", Vec::new())
        .Check(Vec::<Vec<String>>::new());

    for sql in [
        "set session tidb_distsql_scan_concurrency = 15",
        "set session tidb_index_lookup_join_concurrency = -1",
        "set session tidb_projection_concurrency = -1",
        "set session tidb_init_chunk_size = 32",
        "set session tidb_max_chunk_size = 1024",
        "set session tidb_index_join_batch_size = 25000",
    ] {
        tk.MustExec(sql, Vec::new());
    }
}

// 对应 Go TestMemBufferCleanupMemoryLeak：重复键失败属于单语句回滚，不能阻断
// 同一事务随后提交的写入。
#[test]
fn mem_buffer_duplicate_cleanup_leaves_transaction_usable() {
    let _serial = transaction_test_lock();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec(
        "create table mem_cleanup (a varchar(255) primary key)",
        Vec::new(),
    );
    let key1 = "a".repeat(255);
    let key2 = "b".repeat(255);
    tk.MustExec("set global tidb_mem_oom_action = 'cancel'", Vec::new());
    tk.MustExec("set session tidb_mem_quota_query = 10240", Vec::new());
    tk.MustExec("begin", Vec::new());
    tk.MustExec(
        "insert into mem_cleanup values (?)",
        vec![DbValue::String(key2.clone())],
    );
    let baseline = tk.Session().GetSessionVars().MemTracker().BytesConsumed();
    for attempt in 0..100 {
        let duplicate = tk.Exec(
            "insert into mem_cleanup values (?), (?)",
            vec![DbValue::String(key1.clone()), DbValue::String(key2.clone())],
        );
        let error = duplicate.expect_err("duplicate key statement must fail");
        assert!(
            error.message().contains("Duplicate") || error.message().contains("duplicate"),
            "unexpected cleanup error at attempt {attempt}: {error}"
        );
    }
    let consumed = tk.Session().GetSessionVars().MemTracker().BytesConsumed();
    assert!(
        consumed <= baseline.saturating_add(10_240),
        "failed statement cleanup leaked memory: baseline={baseline}, current={consumed}"
    );
    tk.MustExec("commit", Vec::new());
    tk.MustQuery("select count(*) from mem_cleanup", Vec::new())
        .Check(vec![vec!["1".to_owned()]]);
}

// 对应 Go TestPanicOnRollbackKilledTxn：关闭带有未决悲观事务的会话必须完成
// rollback；另一个会话随后可写入同一行，证明没有遗留锁或 panic。
#[test]
fn closing_pessimistic_transaction_releases_rollback_state() {
    let _serial = transaction_test_lock();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut observer = TestKit::new(store.clone());
    let mut holder = TestKit::new(store.clone());
    holder.MustExec("create table rollback_close (id int)", Vec::new());
    holder.MustExec("begin pessimistic", Vec::new());
    holder.MustExec("insert into rollback_close values (1)", Vec::new());
    for _ in 0..6 {
        holder.MustExec(
            "insert into rollback_close select * from rollback_close",
            Vec::new(),
        );
    }
    store
        .sql_killer()
        .SendKillSignal(astersql_util_sqlkiller::sqlkiller::QueryInterrupted);
    holder
        .Session()
        .close()
        .expect("closing session rolls back transaction");
    observer.MustExec("insert into rollback_close values (1)", Vec::new());
    observer
        .MustQuery("select count(*) from rollback_close", Vec::new())
        .Check(vec![vec!["1".to_owned()]]);
}
