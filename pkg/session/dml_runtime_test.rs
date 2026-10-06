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

// DML 运行时集成测试：真实 SQL 经 ConcreteSession 落到 mock KV。
//
// 覆盖 INSERT/UPDATE/DELETE/REPLACE、显式事务与 IGNORE/ON DUPLICATE、
// 生成列 tablecodec 编解码、autocommit 失败不落盘，以及 EXPLAIN ANALYZE
// 中 auto_id_allocator 计数矩阵（对齐 Go DML 语义）。

use std::collections::HashMap;
use std::sync::Arc;

use astersql_domain::KvInfoSchemaLoader;
use astersql_kv as kv;
use astersql_store_mockstore_mockstorage::{KVStore, NewMockStorage, mockStorage};

use crate::runtime::{ConcreteSession, ConcreteTestRuntime, RuntimeDomain};
use crate::testutil::{TestRecordSet, TestRuntime};
use crate::{SessionError, SessionResult};

/// 构造可独占的内存 mock 存储。
fn new_storage() -> SessionResult<mockStorage> {
    Arc::try_unwrap(
        NewMockStorage(KVStore::NewMemory(), None)
            .map_err(|error| SessionError::new(error.to_string()))?,
    )
    .map_err(|_| SessionError::new("mock storage retained an unexpected owner"))
}

/// Bootstrap Domain 后包装为 ConcreteSession，供 DML 用例执行真实 SQL。
fn concrete_session() -> ConcreteSession {
    let runtime = ConcreteTestRuntime::new(new_storage, Arc::new(KvInfoSchemaLoader::new()), false);
    let store = runtime.NewMockStore().expect("create DML mock store");
    let domain = runtime
        .BootstrapSession(store)
        .expect("bootstrap DML domain");
    let domain = domain
        .as_any()
        .downcast_ref::<RuntimeDomain>()
        .expect("DML runtime domain");
    ConcreteSession::new(Arc::clone(domain.domain()))
}

fn canonical_mlog_session() -> ConcreteSession {
    let bootstrap = concrete_session();
    crate::runtime::BootstrapCanonicalDomain(Arc::clone(bootstrap.domain())).unwrap()
}

fn install_mlog_metadata(
    session: &ConcreteSession,
    base_name: &str,
    log_name: &str,
    base_id: i64,
    log_id: i64,
) {
    use astersql_meta_model::{MaterializedViewBaseInfo, MaterializedViewLogInfo};
    use astersql_parser_ast::NewCIStr;

    let domain = session.domain();
    let mut base = domain.stats_table("test", base_name).unwrap().1;
    let mut log = domain.stats_table("test", log_name).unwrap().1;
    base.ID = base_id;
    log.ID = log_id;
    base.MaterializedViewBase = Some(MaterializedViewBaseInfo {
        MLogID: log.ID,
        ..Default::default()
    });
    log.MaterializedViewLog = Some(MaterializedViewLogInfo {
        BaseTableID: base.ID,
        Columns: vec![NewCIStr("a"), NewCIStr("b")],
        ..Default::default()
    });
    session
        .execute(&format!("drop table `{base_name}`, `{log_name}`"))
        .unwrap();
    domain.ddl_create_table("test", base, false).unwrap();
    domain.ddl_create_table("test", log, false).unwrap();
}

#[test]
fn go_merge_49_canonical_bootstrap_installs_purge_history() {
    let session = canonical_mlog_session();
    assert!(
        session
            .domain()
            .stats_table("mysql", "tidb_mview_refresh_info")
            .is_some()
    );
    assert!(
        session
            .domain()
            .stats_table("mysql", "tidb_mlog_purge_hist")
            .is_some()
    );
}

#[test]
fn go_merge_49_mlog_scan_reads_record_commit_ts() {
    let session = canonical_mlog_session();
    session
        .execute("create table t_mlog_commit_ts (a int)")
        .unwrap();
    session
        .execute("create materialized view log on t_mlog_commit_ts (a)")
        .unwrap();
    session
        .execute("insert into t_mlog_commit_ts values (1)")
        .unwrap();
    let log = session
        .domain()
        .stats_table("test", "$mlog$t_mlog_commit_ts")
        .unwrap()
        .1;
    let commits = session.domain().storage().with_storage(|store| {
        let version = store.CurrentVersion("global").unwrap();
        let snapshot = store.GetSnapshot(version);
        crate::runtime::scan_mlog_record_commit_ts(snapshot.as_ref(), log.ID).unwrap()
    });
    assert_eq!(commits.len(), 1);
    assert!(commits[0].1 > 0);
}

#[test]
fn go_merge_49_mlog_purge_batch_respects_commit_fence() {
    let session = canonical_mlog_session();
    session
        .execute("create table t_mlog_fence (a int)")
        .unwrap();
    session
        .execute("create materialized view log on t_mlog_fence (a)")
        .unwrap();
    let log = session
        .domain()
        .stats_table("test", "$mlog$t_mlog_fence")
        .unwrap()
        .1;
    session
        .execute("insert into t_mlog_fence values (1)")
        .unwrap();
    let first_commit = session.domain().storage().with_storage(|store| {
        let snapshot = store.GetSnapshot(store.CurrentVersion("global").unwrap());
        crate::runtime::scan_mlog_record_commit_ts(snapshot.as_ref(), log.ID).unwrap()[0].1
    });
    session
        .execute("insert into t_mlog_fence values (2)")
        .unwrap();
    let purged = session.domain().storage().with_storage(|store| {
        let mut txn = store.Begin(&[]).unwrap();
        let count =
            crate::runtime::purge_mlog_snapshot_batch(txn.as_mut(), log.ID, None, first_commit, 16)
                .unwrap();
        txn.Commit(&kv::Context::default()).unwrap();
        count
    });
    assert_eq!(purged, 1);
    let mut rows = session
        .execute("select a from `$mlog$t_mlog_fence`")
        .unwrap()
        .remove(0);
    assert_eq!(rows.Next().unwrap().unwrap(), vec!["2".to_owned()]);
    assert!(rows.Next().unwrap().is_none());
}

#[test]
fn go_merge_49_sql_purge_uses_configured_batch_size() {
    let session = canonical_mlog_session();
    session
        .execute("create table t_mlog_batch (a int)")
        .unwrap();
    session
        .execute("create materialized view log on t_mlog_batch (a)")
        .unwrap();
    session
        .execute("insert into t_mlog_batch values (1), (2), (3)")
        .unwrap();
    session
        .execute("set @@tidb_mlog_purge_batch_size=1")
        .unwrap();
    let mut configured = session
        .execute("select @@tidb_mlog_purge_batch_size")
        .unwrap()
        .remove(0);
    assert_eq!(configured.Next().unwrap().unwrap(), vec!["1".to_owned()]);
    session
        .execute("purge materialized view log on t_mlog_batch")
        .unwrap();
    let mut rows = session
        .execute("select PURGE_ROWS from mysql.tidb_mlog_purge_hist")
        .unwrap()
        .remove(0);
    assert_eq!(rows.Next().unwrap().unwrap(), vec!["3".to_owned()]);
    session
        .execute("set @@tidb_mlog_purge_min_rate=2500")
        .unwrap();
    session
        .execute("set @@tidb_mlog_purge_rate_budget_ratio=0.25")
        .unwrap();
    assert!(
        session
            .execute("set @@tidb_mlog_purge_rate_budget_ratio=0")
            .is_err()
    );
    session
        .execute("set @@tidb_mlog_purge_batch_size=0")
        .unwrap();
    let mut minimum = session
        .execute("select @@tidb_mlog_purge_batch_size")
        .unwrap()
        .remove(0);
    assert_eq!(minimum.Next().unwrap().unwrap(), vec!["1".to_owned()]);
}

#[test]
fn go_merge_49_sql_purge_advances_past_checkpointed_keys() {
    let session = canonical_mlog_session();
    session
        .execute("create table t_mlog_cursor (a int)")
        .unwrap();
    session
        .execute("create materialized view log on t_mlog_cursor (a)")
        .unwrap();
    let log = session
        .domain()
        .stats_table("test", "$mlog$t_mlog_cursor")
        .unwrap()
        .1;
    session
        .execute("insert into t_mlog_cursor values (1)")
        .unwrap();
    let first_commit = session.domain().storage().with_storage(|store| {
        let snapshot = store.GetSnapshot(store.CurrentVersion("global").unwrap());
        crate::runtime::scan_mlog_record_commit_ts(snapshot.as_ref(), log.ID).unwrap()[0].1
    });
    session
        .execute("insert into t_mlog_cursor values (2)")
        .unwrap();
    session
        .execute(&format!(
            "update mysql.tidb_mlog_purge_info set LAST_PURGED_TSO={first_commit} where MLOG_ID={}",
            log.ID
        ))
        .unwrap();
    session
        .execute("set @@tidb_mlog_purge_batch_size=1")
        .unwrap();
    session
        .execute("purge materialized view log on t_mlog_cursor")
        .unwrap();
    let mut rows = session
        .execute("select a from `$mlog$t_mlog_cursor`")
        .unwrap()
        .remove(0);
    assert_eq!(rows.Next().unwrap().unwrap(), vec!["1".to_owned()]);
    assert!(rows.Next().unwrap().is_none());
}

#[test]
fn go_merge_49_sql_purge_requires_operate_view_privilege() {
    let admin = canonical_mlog_session();
    admin.execute("create table t_mlog_priv (a int)").unwrap();
    admin
        .execute("create materialized view log on t_mlog_priv (a)")
        .unwrap();
    admin
        .execute("create user 'mlog_purge_user'@'localhost'")
        .unwrap();
    let mut restricted = ConcreteSession::new(Arc::clone(&admin.domain()));
    restricted
        .AuthenticateUserForTest(&astersql_parser_auth::parser::auth::auth::UserIdentity {
            username: "mlog_purge_user".to_owned(),
            hostname: "localhost".to_owned(),
            ..Default::default()
        })
        .unwrap();
    let denied = match restricted.execute("purge materialized view log on test.t_mlog_priv") {
        Ok(_) => panic!("OPERATE VIEW is required on the log table"),
        Err(error) => error,
    };
    assert!(denied.to_string().contains("OPERATE VIEW"), "{denied}");
    admin
        .execute("grant operate view on test.`$mlog$t_mlog_priv` to 'mlog_purge_user'@'localhost'")
        .unwrap();
    restricted
        .execute("purge materialized view log on test.t_mlog_priv")
        .unwrap();
}

#[test]
fn go_merge_49_sql_purge_mlog_updates_history_and_checkpoint() {
    let session = canonical_mlog_session();
    session
        .execute("create table t_mlog_sql_purge (a int)")
        .unwrap();
    session
        .execute("create materialized view log on t_mlog_sql_purge (a)")
        .unwrap();
    session
        .execute("insert into t_mlog_sql_purge values (1), (2)")
        .unwrap();
    session
        .execute("purge materialized view log on t_mlog_sql_purge")
        .unwrap();
    let mut rows = session
        .execute("select count(*) from `$mlog$t_mlog_sql_purge`")
        .unwrap()
        .remove(0);
    assert_eq!(rows.Next().unwrap().unwrap(), vec!["0".to_owned()]);
    let mut history = session
        .execute("select PURGE_STATUS, PURGE_ROWS from mysql.tidb_mlog_purge_hist")
        .unwrap()
        .remove(0);
    assert_eq!(
        history.Next().unwrap().unwrap(),
        vec!["success".to_owned(), "2".to_owned()]
    );
    let mut duration = session
        .execute("select PURGE_DURATION_SEC from mysql.tidb_mlog_purge_hist")
        .unwrap()
        .remove(0);
    assert!(duration.Next().unwrap().unwrap()[0].parse::<f64>().unwrap() >= 0.0);
}

#[test]
fn go_merge_49_sql_purge_respects_dependent_view_read_tso() {
    use astersql_meta_model::MaterializedViewBaseInfo;

    let session = canonical_mlog_session();
    session.execute("create table t_mlog_dep (a int)").unwrap();
    let mut base = session
        .domain()
        .stats_table("test", "t_mlog_dep")
        .unwrap()
        .1;
    session.execute("drop table t_mlog_dep").unwrap();
    base.MaterializedViewBase = Some(MaterializedViewBaseInfo {
        MLogID: 0,
        MViewIDs: vec![990_049],
    });
    session
        .domain()
        .ddl_create_table("test", base, false)
        .unwrap();
    session
        .execute("create materialized view log on t_mlog_dep (a)")
        .unwrap();
    session.execute("insert into mysql.tidb_mview_refresh_info (MVIEW_ID, LAST_SUCCESS_READ_TSO) values (990049, 0)").unwrap();
    session
        .execute("insert into t_mlog_dep values (1)")
        .unwrap();
    session
        .execute("purge materialized view log on t_mlog_dep")
        .unwrap();
    let mut count = session
        .execute("select count(*) from `$mlog$t_mlog_dep`")
        .unwrap()
        .remove(0);
    assert_eq!(count.Next().unwrap().unwrap(), vec!["1".to_owned()]);
    let current = session
        .domain()
        .storage()
        .with_storage(|store| store.CurrentVersion("global").unwrap().Ver);
    session.execute(&format!("update mysql.tidb_mview_refresh_info set LAST_SUCCESS_READ_TSO={current} where MVIEW_ID=990049")).unwrap();
    session
        .execute("purge materialized view log on t_mlog_dep")
        .unwrap();
    let mut count = session
        .execute("select count(*) from `$mlog$t_mlog_dep`")
        .unwrap()
        .remove(0);
    assert_eq!(count.Next().unwrap().unwrap(), vec!["0".to_owned()]);
}

#[test]
fn go_merge_49_sql_purge_records_failure_before_delete() {
    use astersql_meta_model::MaterializedViewBaseInfo;

    let session = canonical_mlog_session();
    session.execute("create table t_mlog_fail (a int)").unwrap();
    let mut base = session
        .domain()
        .stats_table("test", "t_mlog_fail")
        .unwrap()
        .1;
    session.execute("drop table t_mlog_fail").unwrap();
    base.MaterializedViewBase = Some(MaterializedViewBaseInfo {
        MLogID: 0,
        MViewIDs: vec![990_050],
    });
    session
        .domain()
        .ddl_create_table("test", base, false)
        .unwrap();
    session
        .execute("create materialized view log on t_mlog_fail (a)")
        .unwrap();
    session
        .execute("insert into t_mlog_fail values (1)")
        .unwrap();
    assert!(
        session
            .execute("purge materialized view log on t_mlog_fail")
            .is_err()
    );
    let mut count = session
        .execute("select count(*) from `$mlog$t_mlog_fail`")
        .unwrap()
        .remove(0);
    assert_eq!(count.Next().unwrap().unwrap(), vec!["1".to_owned()]);
    let mut history = session
        .execute("select PURGE_STATUS from mysql.tidb_mlog_purge_hist")
        .unwrap()
        .remove(0);
    assert_eq!(history.Next().unwrap().unwrap(), vec!["failed".to_owned()]);
}

#[test]
fn go_merge_49_scheduled_mlog_purge_uses_auto_history() {
    let session = canonical_mlog_session();
    session.execute("create table t_mlog_auto (a int)").unwrap();
    session.execute("create materialized view log on t_mlog_auto (a) purge next cast('2030-01-02 00:00:00' as datetime)").unwrap();
    session
        .execute("insert into t_mlog_auto values (1)")
        .unwrap();
    assert_eq!(
        crate::runtime::run_mlog_purge_tick(session.domain(), 1_893_542_400).unwrap(),
        1
    );
    let mut count = session
        .execute("select count(*) from `$mlog$t_mlog_auto`")
        .unwrap()
        .remove(0);
    assert_eq!(count.Next().unwrap().unwrap(), vec!["0".to_owned()]);
    let mut history = session
        .execute("select PURGE_METHOD, PURGE_STATUS from mysql.tidb_mlog_purge_hist")
        .unwrap()
        .remove(0);
    assert_eq!(
        history.Next().unwrap().unwrap(),
        vec!["auto".to_owned(), "success".to_owned()]
    );
}

#[test]
fn go_merge_49_mlog_purge_worker_stops_with_domain() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let session = concrete_session();
    let domain = Arc::clone(session.domain());
    let ticks = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&ticks);
    assert!(
        domain
            .start_mlog_purge_worker(std::time::Duration::from_millis(10), move |_| {
                observed.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap()
    );
    assert!(
        !domain
            .start_mlog_purge_worker(std::time::Duration::from_millis(10), |_| {})
            .unwrap()
    );
    std::thread::sleep(std::time::Duration::from_millis(30));
    assert!(ticks.load(Ordering::SeqCst) > 0);
    domain.close();
    let stopped = ticks.load(Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(30));
    assert_eq!(ticks.load(Ordering::SeqCst), stopped);
}

#[test]
fn go_merge_49_sql_purge_rejects_explicit_transaction() {
    let session = canonical_mlog_session();
    session.execute("create table t_mlog_txn (a int)").unwrap();
    session
        .execute("create materialized view log on t_mlog_txn (a)")
        .unwrap();
    session
        .execute("insert into t_mlog_txn values (1)")
        .unwrap();
    session.execute("begin").unwrap();
    assert!(
        session
            .execute("purge materialized view log on t_mlog_txn")
            .is_err()
    );
    session.execute("rollback").unwrap();
    let mut count = session
        .execute("select count(*) from `$mlog$t_mlog_txn`")
        .unwrap()
        .remove(0);
    assert_eq!(count.Next().unwrap().unwrap(), vec!["1".to_owned()]);
}

#[test]
fn go_merge_49_sql_purge_lock_conflict_preserves_log() {
    let session = canonical_mlog_session();
    session.execute("create table t_mlog_lock (a int)").unwrap();
    session
        .execute("create materialized view log on t_mlog_lock (a)")
        .unwrap();
    session
        .execute("insert into t_mlog_lock values (1)")
        .unwrap();
    let log_id = session
        .domain()
        .stats_table("test", "$mlog$t_mlog_lock")
        .unwrap()
        .1
        .ID;
    let blocker = ConcreteSession::new(Arc::clone(session.domain()));
    blocker.execute("begin pessimistic").unwrap();
    blocker.execute(&format!("select LAST_PURGED_TSO from mysql.tidb_mlog_purge_info where MLOG_ID={log_id} for update nowait")).unwrap();
    assert!(
        session
            .execute("purge materialized view log on t_mlog_lock")
            .is_err()
    );
    blocker.execute("rollback").unwrap();
    let mut count = session
        .execute("select count(*) from `$mlog$t_mlog_lock`")
        .unwrap()
        .remove(0);
    assert_eq!(count.Next().unwrap().unwrap(), vec!["1".to_owned()]);
}

#[test]
fn go_merge_49_sql_purge_respects_history_cutoff_fence() {
    let session = canonical_mlog_session();
    session
        .execute("create table t_mlog_hist_fence (a int)")
        .unwrap();
    session
        .execute("create materialized view log on t_mlog_hist_fence (a)")
        .unwrap();
    session
        .execute("insert into t_mlog_hist_fence values (1)")
        .unwrap();
    let log_id = session
        .domain()
        .stats_table("test", "$mlog$t_mlog_hist_fence")
        .unwrap()
        .1
        .ID;
    let future = session
        .domain()
        .storage()
        .with_storage(|store| store.CurrentVersion("global").unwrap().Ver + 1_000_000);
    session.execute(&format!("insert into mysql.tidb_mlog_purge_hist (PURGE_JOB_ID, MLOG_ID, PURGE_METHOD, PURGE_ROWS, PURGE_STATUS, PURGE_CUTOFF_TSO) values (1, {log_id}, 'manual', 0, 'success', {future})")).unwrap();
    session
        .execute("purge materialized view log on t_mlog_hist_fence")
        .unwrap();
    let mut count = session
        .execute("select count(*) from `$mlog$t_mlog_hist_fence`")
        .unwrap()
        .remove(0);
    assert_eq!(count.Next().unwrap().unwrap(), vec!["1".to_owned()]);
    let mut count = session
        .execute("select count(*) from mysql.tidb_mlog_purge_hist")
        .unwrap()
        .remove(0);
    assert_eq!(count.Next().unwrap().unwrap(), vec!["1".to_owned()]);
}

#[test]
fn go_merge_49_sql_cancel_mlog_purge_marks_running_job() {
    let session = canonical_mlog_session();
    session.execute("insert into mysql.tidb_mlog_purge_hist (PURGE_JOB_ID, MLOG_ID, PURGE_METHOD, PURGE_ROWS, PURGE_STATUS) values (49001, 1, 'manual', 0, 'running')").unwrap();
    session
        .execute("cancel materialized view log purge job 49001")
        .unwrap();
    let mut row = session.execute("select CANCEL_REQUEST_TIME is not null from mysql.tidb_mlog_purge_hist where PURGE_JOB_ID=49001").unwrap().remove(0);
    assert_eq!(row.Next().unwrap().unwrap(), vec!["1".to_owned()]);
    assert!(
        session
            .execute("cancel materialized view log purge job 49001")
            .is_err()
    );
}

#[test]
fn go_merge_49_sql_cancel_mlog_purge_checks_log_privilege() {
    let admin = canonical_mlog_session();
    admin
        .execute("create table t_mlog_cancel_priv (a int)")
        .unwrap();
    admin
        .execute("create materialized view log on t_mlog_cancel_priv (a)")
        .unwrap();
    let log_id = admin
        .domain()
        .stats_table("test", "$mlog$t_mlog_cancel_priv")
        .unwrap()
        .1
        .ID;
    admin.execute(&format!("insert into mysql.tidb_mlog_purge_hist (PURGE_JOB_ID, MLOG_ID, PURGE_METHOD, PURGE_ROWS, PURGE_STATUS) values (49002, {log_id}, 'manual', 0, 'running')")).unwrap();
    admin
        .execute("create user 'mlog_cancel_user'@'localhost'")
        .unwrap();
    let mut restricted = ConcreteSession::new(Arc::clone(&admin.domain()));
    restricted
        .AuthenticateUserForTest(&astersql_parser_auth::parser::auth::auth::UserIdentity {
            username: "mlog_cancel_user".to_owned(),
            hostname: "localhost".to_owned(),
            ..Default::default()
        })
        .unwrap();
    let denied = match restricted.execute("cancel materialized view log purge job 49002") {
        Ok(_) => panic!("OPERATE VIEW is required to cancel the MLog purge"),
        Err(error) => error,
    };
    assert!(denied.to_string().contains("OPERATE VIEW"), "{denied}");
    admin.execute("grant operate view on test.`$mlog$t_mlog_cancel_priv` to 'mlog_cancel_user'@'localhost'").unwrap();
    restricted
        .execute("cancel materialized view log purge job 49002")
        .unwrap();
    let mut requester = admin
        .execute(
            "select CANCEL_REQUESTED_BY from mysql.tidb_mlog_purge_hist where PURGE_JOB_ID=49002",
        )
        .unwrap()
        .remove(0);
    assert_eq!(
        requester.Next().unwrap().unwrap(),
        vec!["'mlog_cancel_user'@'localhost'".to_owned()]
    );
}

#[test]
fn go_merge_49_sql_purge_updates_mlog_stats_delta() {
    let session = canonical_mlog_session();
    session
        .execute("create table t_mlog_stats (a int)")
        .unwrap();
    session
        .execute("create materialized view log on t_mlog_stats (a)")
        .unwrap();
    let log_id = session
        .domain()
        .stats_table("test", "$mlog$t_mlog_stats")
        .unwrap()
        .1
        .ID;
    session
        .execute("insert into t_mlog_stats values (1), (2)")
        .unwrap();
    session.domain().dump_stats_delta_to_kv(true).unwrap();
    let mut before = session
        .execute(&format!(
            "select count from mysql.stats_meta where table_id={log_id}"
        ))
        .unwrap()
        .remove(0);
    assert_eq!(before.Next().unwrap().unwrap(), vec!["2".to_owned()]);
    session
        .execute("purge materialized view log on t_mlog_stats")
        .unwrap();
    session.domain().dump_stats_delta_to_kv(true).unwrap();
    let mut after = session
        .execute(&format!(
            "select count from mysql.stats_meta where table_id={log_id}"
        ))
        .unwrap()
        .remove(0);
    assert_eq!(after.Next().unwrap().unwrap(), vec!["0".to_owned()]);
}

#[test]
fn go_merge_49_sql_mlog_dml_shares_transaction() {
    use astersql_meta_model::{MaterializedViewBaseInfo, MaterializedViewLogInfo};
    use astersql_parser_ast::NewCIStr;

    let session = concrete_session();
    session
        .execute("create table t_mlog (a int primary key, b int)")
        .unwrap();
    session.execute("create table `$mlog$t_mlog` (a int, b int, `_MLOG$_DML_TYPE` varchar(1), `_MLOG$_OLD_NEW` int)").unwrap();
    let domain = session.domain();
    let mut base = domain.stats_table("test", "t_mlog").unwrap().1;
    let mut log = domain.stats_table("test", "$mlog$t_mlog").unwrap().1;
    base.ID = 990_001;
    log.ID = 990_002;
    base.MaterializedViewBase = Some(MaterializedViewBaseInfo {
        MLogID: log.ID,
        ..Default::default()
    });
    log.MaterializedViewLog = Some(MaterializedViewLogInfo {
        BaseTableID: base.ID,
        Columns: vec![NewCIStr("a"), NewCIStr("b")],
        ..Default::default()
    });
    session
        .execute("drop table t_mlog, `$mlog$t_mlog`")
        .unwrap();
    domain.ddl_create_table("test", base, false).unwrap();
    domain.ddl_create_table("test", log, false).unwrap();
    assert!(
        domain
            .stats_table("test", "t_mlog")
            .unwrap()
            .1
            .MaterializedViewBase
            .is_some()
    );

    session.execute("begin").unwrap();
    session
        .execute("insert into t_mlog values (1, 10)")
        .unwrap();
    let mut base_rows = session.execute("select a from t_mlog").unwrap().remove(0);
    assert_eq!(base_rows.Next().unwrap(), Some(vec!["1".to_owned()]));
    let mut rows = session
        .execute("select a, b, `_MLOG$_DML_TYPE`, `_MLOG$_OLD_NEW` from `$mlog$t_mlog`")
        .unwrap()
        .remove(0);
    assert_eq!(
        rows.Next().unwrap(),
        Some(vec![
            "1".to_owned(),
            "10".to_owned(),
            "I".to_owned(),
            "1".to_owned()
        ])
    );
    session.execute("rollback").unwrap();
    let mut rows = session
        .execute("select a from `$mlog$t_mlog`")
        .unwrap()
        .remove(0);
    assert_eq!(rows.Next().unwrap(), None);

    session
        .execute("insert into t_mlog values (2, 20)")
        .unwrap();
    session
        .execute("update t_mlog set b = 21 where a = 2")
        .unwrap();
    session.execute("delete from t_mlog where a = 2").unwrap();
    let mut rows = session
        .execute("select a, b, `_MLOG$_DML_TYPE`, `_MLOG$_OLD_NEW` from `$mlog$t_mlog`")
        .unwrap()
        .remove(0);
    let mut actual = Vec::new();
    while let Some(row) = rows.Next().unwrap() {
        actual.push(row);
    }
    actual.sort();
    let mut expected = vec![
        vec!["2", "20", "I", "1"],
        vec!["2", "20", "U", "-1"],
        vec!["2", "21", "U", "1"],
        vec!["2", "21", "D", "-1"],
    ]
    .into_iter()
    .map(|row| row.into_iter().map(str::to_owned).collect::<Vec<_>>())
    .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(actual, expected);

    session
        .execute("insert into t_mlog values (3, 30)")
        .unwrap();
    session
        .execute("update t_mlog set a = 4 where a = 3")
        .unwrap();
    session
        .execute("replace into t_mlog values (4, 40)")
        .unwrap();
    session
        .execute("insert into t_mlog values (4, 99) on duplicate key update b = 41")
        .unwrap();
    let mut rows = session
        .execute("select a, b, `_MLOG$_DML_TYPE`, `_MLOG$_OLD_NEW` from `$mlog$t_mlog`")
        .unwrap()
        .remove(0);
    let mut actual = Vec::new();
    while let Some(row) = rows.Next().unwrap() {
        if row[0] == "3" || row[0] == "4" {
            actual.push(row);
        }
    }
    actual.sort();
    let mut expected = vec![
        vec!["3", "30", "I", "1"],
        vec!["3", "30", "U", "-1"],
        vec!["4", "30", "U", "1"],
        vec!["4", "30", "U", "-1"],
        vec!["4", "40", "U", "1"],
        vec!["4", "40", "U", "-1"],
        vec!["4", "41", "U", "1"],
    ]
    .into_iter()
    .map(|row| row.into_iter().map(str::to_owned).collect::<Vec<_>>())
    .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(actual, expected);
}

#[test]
fn mlog_load_data_uses_insert_mutation_pipeline() {
    let session = concrete_session();
    session
        .execute("create table t_mlog_load (a int primary key, b int)")
        .unwrap();
    session.execute("create table `$mlog$t_mlog_load` (a int, b int, `_MLOG$_DML_TYPE` varchar(1), `_MLOG$_OLD_NEW` int)").unwrap();
    install_mlog_metadata(
        &session,
        "t_mlog_load",
        "$mlog$t_mlog_load",
        991_001,
        991_002,
    );

    session
        .execute_with_load_data_reader(
            "load data local infile 'mlog.csv' into table t_mlog_load fields terminated by ','",
            std::io::Cursor::new(b"1,10\n".to_vec()),
        )
        .unwrap();
    let mut rows = session
        .execute("select a, b, `_MLOG$_DML_TYPE`, `_MLOG$_OLD_NEW` from `$mlog$t_mlog_load`")
        .unwrap()
        .remove(0);
    assert_eq!(
        rows.Next().unwrap(),
        Some(vec!["1".into(), "10".into(), "I".into(), "1".into()])
    );
}

#[test]
fn mlog_rejects_import_into_and_partitioned_base_dml() {
    let session = concrete_session();
    session
        .execute("create table t_mlog_import (a int primary key, b int, untracked int)")
        .unwrap();
    session.execute("create table `$mlog$t_mlog_import` (a int, b int, `_MLOG$_DML_TYPE` varchar(1), `_MLOG$_OLD_NEW` int)").unwrap();
    install_mlog_metadata(
        &session,
        "t_mlog_import",
        "$mlog$t_mlog_import",
        991_011,
        991_012,
    );
    let error = match session.execute("import into t_mlog_import from 's3://bucket/input.csv'") {
        Ok(_) => panic!("IMPORT INTO must reject a base table with an MLog"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("IMPORT INTO on tables with materialized view log")
    );
    session
        .execute("alter table t_mlog_import drop column untracked")
        .unwrap();
    let error = match session.execute("alter table t_mlog_import drop column b") {
        Ok(_) => panic!("DROP COLUMN must reject an MLog-tracked base column"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("referenced by materialized view log"),
        "unexpected tracked-column error: {error}"
    );

    session
        .execute("create table t_mlog_partition (a int primary key, b int) partition by hash(a) partitions 2")
        .unwrap();
    session.execute("create table `$mlog$t_mlog_partition` (a int, b int, `_MLOG$_DML_TYPE` varchar(1), `_MLOG$_OLD_NEW` int)").unwrap();
    install_mlog_metadata(
        &session,
        "t_mlog_partition",
        "$mlog$t_mlog_partition",
        991_021,
        991_022,
    );
    let error = match session.execute("insert into t_mlog_partition values (1, 10)") {
        Ok(_) => panic!("DML must reject a partitioned base table with an MLog"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("partitioned tables"));
}

#[test]
fn go_merge_49_create_mlog_sql_installs_executable_metadata() {
    let session = canonical_mlog_session();
    session
        .execute("create table t_mlog_ddl (a int primary key, b int)")
        .unwrap();
    session
        .execute("create materialized view log on t_mlog_ddl (a, b) shard_row_id_bits=2 pre_split_regions=2 alert rows 10")
        .unwrap();
    let log = session
        .domain()
        .stats_table("test", "$mlog$t_mlog_ddl")
        .unwrap()
        .1;
    assert_eq!(log.ShardRowIDBits, 2);
    assert_eq!(log.PreSplitRegions, 2);
    assert_eq!(
        log.MaterializedViewLog
            .as_ref()
            .unwrap()
            .LogAccumulationAlertRows,
        Some(10)
    );
    session
        .execute("insert into t_mlog_ddl values (1, 7)")
        .unwrap();
    let mut rows = session
        .execute("select a, b, `_MLOG$_DML_TYPE`, `_MLOG$_OLD_NEW` from `$mlog$t_mlog_ddl`")
        .unwrap()
        .remove(0);
    assert_eq!(
        rows.Next().unwrap(),
        Some(vec![
            "1".to_owned(),
            "7".to_owned(),
            "I".to_owned(),
            "1".to_owned()
        ])
    );
}

#[test]
fn go_merge_49_create_mlog_truncates_long_physical_name() {
    let session = canonical_mlog_session();
    let base_name = "t".repeat(astersql_parser_mysql::r#const::MaxTableNameLength);
    session
        .execute(&format!("create table `{base_name}` (a int)"))
        .unwrap();
    session
        .execute(&format!(
            "create materialized view log on `{base_name}` (a)"
        ))
        .unwrap();
    let log_name = astersql_meta_model::MaterializedViewLogTableName(
        &astersql_parser_ast::NewCIStr(&base_name),
    );
    assert_eq!(
        log_name.O.chars().count(),
        astersql_parser_mysql::r#const::MaxTableNameLength
    );
    assert!(session.domain().stats_table("test", &log_name.O).is_some());
}

#[test]
fn go_merge_49_create_mlog_purge_schedule_metadata() {
    let session = canonical_mlog_session();
    session
        .execute("create table t_mlog_purge (a int)")
        .unwrap();
    session
        .execute("create materialized view log on t_mlog_purge (a) purge next cast('2030-01-02' as date)")
        .unwrap();
    let log = session
        .domain()
        .stats_table("test", "$mlog$t_mlog_purge")
        .unwrap()
        .1;
    let info = log.MaterializedViewLog.unwrap();
    assert_eq!(info.PurgeMethod, "DEFERRED");
    assert!(!info.PurgeNext.is_empty());
    assert_eq!(info.PurgeStartWith, "");
    let mut schedule = session
        .execute(&format!(
            "select next_purge_unix_seconds from mysql.tidb_mlog_purge_info where mlog_id={}",
            log.ID,
        ))
        .unwrap()
        .remove(0);
    assert_eq!(schedule.Next().unwrap(), Some(vec!["1893542400".into()]));
    session
        .execute("create table t_mlog_purge_start (a int)")
        .unwrap();
    session.execute("create materialized view log on t_mlog_purge_start (a) purge start with cast('2030-01-02 10:00:00' as datetime) next cast('2030-01-03 10:00:00' as datetime)").unwrap();
    let start_log_id = session
        .domain()
        .stats_table("test", "$mlog$t_mlog_purge_start")
        .unwrap()
        .1
        .ID;
    let mut schedule = session.execute(&format!("select next_purge_unix_seconds from mysql.tidb_mlog_purge_info where mlog_id={start_log_id}")).unwrap().remove(0);
    assert_eq!(schedule.Next().unwrap(), Some(vec!["1893578400".into()]));
    session
        .execute("set time_zone='America/Los_Angeles'")
        .unwrap();
    session
        .execute("create table t_mlog_purge_dst (a int)")
        .unwrap();
    session.execute("create materialized view log on t_mlog_purge_dst (a) purge next cast('2021-03-14 02:30:00' as datetime)").unwrap();
    let dst_log_id = session
        .domain()
        .stats_table("test", "$mlog$t_mlog_purge_dst")
        .unwrap()
        .1
        .ID;
    let mut schedule = session.execute(&format!("select next_purge_unix_seconds from mysql.tidb_mlog_purge_info where mlog_id={dst_log_id}")).unwrap().remove(0);
    assert_eq!(schedule.Next().unwrap(), Some(vec!["1615689000".into()]));
    session
        .execute("create table t_mlog_purge_bad (a int)")
        .unwrap();
    assert!(
        session
            .execute("create materialized view log on t_mlog_purge_bad (a) purge next 1")
            .is_err()
    );
    assert!(
        session
            .execute("create materialized view log on t_mlog_purge_bad (a) purge immediate")
            .is_err()
    );
}

#[test]
fn go_merge_49_create_mlog_rolls_back_when_purge_table_missing() {
    let session = concrete_session();
    session
        .execute("create table t_mlog_missing_purge (a int)")
        .unwrap();
    assert!(
        session
            .execute("create materialized view log on t_mlog_missing_purge (a)")
            .is_err()
    );
    let base = session
        .domain()
        .stats_table("test", "t_mlog_missing_purge")
        .unwrap()
        .1;
    assert!(
        base.MaterializedViewBase
            .as_ref()
            .is_none_or(|info| info.MLogID == 0)
    );
    assert!(
        session
            .domain()
            .stats_table("test", "$mlog$t_mlog_missing_purge")
            .is_none()
    );
}

#[test]
fn go_merge_49_partition_records_use_physical_ids() {
    let session = concrete_session();
    session
        .execute("create table t_mlog_partition (a int primary key, b int, key idx_b(b)) partition by hash(a) partitions 2")
        .unwrap();
    session
        .execute("insert into t_mlog_partition values (1, 11), (2, 22)")
        .unwrap();
    let table = session
        .domain()
        .stats_table("test", "t_mlog_partition")
        .unwrap()
        .1;
    let partition = table.GetPartitionInfo().unwrap();
    let expected_id = partition.Definitions[1].ID;
    let key = astersql_tablecodec::EncodeRowKeyWithHandle(
        expected_id,
        Box::new(astersql_tablecodec::kv::IntHandle(1)),
    );
    assert!(session.read_raw_kv(kv::Key(key.0)).unwrap().is_some());
    let mut rows = session
        .execute("select a, b from t_mlog_partition order by a")
        .unwrap()
        .remove(0);
    assert_eq!(rows.Next().unwrap(), Some(vec!["1".into(), "11".into()]));
    assert_eq!(rows.Next().unwrap(), Some(vec!["2".into(), "22".into()]));
    let mut limited = session
        .execute("select a from t_mlog_partition order by a limit 1 offset 1")
        .unwrap()
        .remove(0);
    assert_eq!(limited.Next().unwrap(), Some(vec!["2".into()]));
    session
        .execute("update t_mlog_partition set b=33 where a=1")
        .unwrap();
    let mut rows = session
        .execute("select a, b from t_mlog_partition where b=33")
        .unwrap()
        .remove(0);
    assert_eq!(rows.Next().unwrap(), Some(vec!["1".into(), "33".into()]));
    session
        .execute("delete from t_mlog_partition where a=2")
        .unwrap();
    let mut rows = session
        .execute("select a from t_mlog_partition order by a")
        .unwrap()
        .remove(0);
    assert_eq!(rows.Next().unwrap(), Some(vec!["1".into()]));
    assert_eq!(rows.Next().unwrap(), None);
    let mut count = session
        .execute("select count(*) from t_mlog_partition")
        .unwrap()
        .remove(0);
    assert_eq!(count.Next().unwrap(), Some(vec!["1".into()]));
    session
        .execute("update t_mlog_partition set a=4 where a=1")
        .unwrap();
    let mut rows = session
        .execute("select a,b from t_mlog_partition order by a")
        .unwrap()
        .remove(0);
    assert_eq!(rows.Next().unwrap(), Some(vec!["4".into(), "33".into()]));
    session.execute("begin").unwrap();
    session
        .execute("insert into t_mlog_partition values (5, 55)")
        .unwrap();
    let mut count = session
        .execute("select count(*) from t_mlog_partition")
        .unwrap()
        .remove(0);
    assert_eq!(count.Next().unwrap(), Some(vec!["2".into()]));
    session.execute("rollback").unwrap();
    let mut count = session
        .execute("select count(*) from t_mlog_partition")
        .unwrap()
        .remove(0);
    assert_eq!(count.Next().unwrap(), Some(vec!["1".into()]));
}

#[test]
fn go_merge_49_key_partition_uses_column_hash() {
    let session = concrete_session();
    session
        .execute("create table t_mlog_key (a varchar(20), b int) partition by key(a) partitions 4")
        .unwrap();
    session
        .execute("insert into t_mlog_key values ('alpha', 1)")
        .unwrap();
    let table = session
        .domain()
        .stats_table("test", "t_mlog_key")
        .unwrap()
        .1;
    let partition = table.GetPartitionInfo().unwrap();
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(
        &astersql_util_collate::GetCollatorWithCollate(
            astersql_tablecodec::collate::NewCollationEnabled(),
            table.Columns[0].FieldType.GetCollate(),
        )
        .Key("alpha"),
    );
    let physical_id = partition.Definitions[(hasher.finalize() as usize) % 4].ID;
    let prefix = kv::Key(astersql_tablecodec::GenTableRecordPrefix(physical_id).0);
    assert!(session.domain().storage().with_storage(|store| {
        let snapshot = store.GetSnapshot(store.CurrentVersion(kv::GlobalTxnScope).unwrap());
        let mut iter = snapshot
            .Iter(prefix.clone(), Some(prefix.PrefixNext()))
            .unwrap();
        let valid = iter.Valid();
        iter.Close();
        valid
    }));
}

#[test]
fn go_merge_49_list_columns_routes_with_column_collation() {
    let session = concrete_session();
    session.execute("create table t_mlog_list (a varchar(20) collate utf8mb4_general_ci, b int) partition by list columns(a) (partition p0 values in ('alpha'), partition p1 values in ('beta'))").unwrap();
    session
        .execute("insert into t_mlog_list values ('ALPHA', 1)")
        .unwrap();
    let mut rows = session
        .execute("select a,b from t_mlog_list")
        .unwrap()
        .remove(0);
    assert_eq!(rows.Next().unwrap(), Some(vec!["ALPHA".into(), "1".into()]));
}

#[test]
fn go_merge_49_range_columns_routes_with_column_collation() {
    let session = concrete_session();
    session.execute("create table t_mlog_range (a varchar(20) collate utf8mb4_general_ci, b int primary key) partition by range columns(a) (partition p0 values less than ('m'), partition p1 values less than (maxvalue))").unwrap();
    session
        .execute("insert into t_mlog_range values ('Z', 1)")
        .unwrap();
    let table = session
        .domain()
        .stats_table("test", "t_mlog_range")
        .unwrap()
        .1;
    let partition = table.GetPartitionInfo().unwrap();
    let key = astersql_tablecodec::EncodeRowKeyWithHandle(
        partition.Definitions[1].ID,
        Box::new(astersql_tablecodec::kv::IntHandle(1)),
    );
    assert!(session.read_raw_kv(kv::Key(key.0)).unwrap().is_some());
}

#[test]
fn mid_substr_and_substring_follow_mysql_character_bounds() {
    let row = HashMap::from([(
        "value".to_owned(),
        Some("0.400000000000000000000000000000".to_owned()),
    )]);
    for (expression, expected) in [
        ("mid(value, 6, 9)", Some("000000000")),
        ("substr(value, 1, 3)", Some("0.4")),
        ("substring(value, -3)", Some("000")),
        ("mid(value, 0, 2)", Some("")),
        ("mid(value, -99, 2)", Some("")),
        ("mid(value, 2, -1)", Some("")),
        ("mid(null, 1, 1)", None),
    ] {
        let expression =
            crate::dml_runtime::ParseGeneratedExpr(expression).expect("parse scalar expression");
        let actual = crate::dml_runtime::EvalExpr(&expression, &row, None)
            .expect("evaluate scalar expression");
        assert_eq!(actual.as_deref(), expected, "expression={expression:?}");
    }
}

#[test]
fn unary_float_literals_preserve_fractional_values() {
    for (expression, expected) in [("-0.393904", "-0.393904"), ("+1.25", "1.25")] {
        let expression =
            crate::dml_runtime::ParseGeneratedExpr(expression).expect("parse unary float");
        let actual = crate::dml_runtime::EvalExpr(&expression, &HashMap::new(), None)
            .expect("evaluate unary float");
        assert_eq!(actual.as_deref(), Some(expected));
    }
}

#[test]
fn statement_time_functions_follow_relational_runtime_format() {
    let row = HashMap::new();
    for (expression, expected_length) in [
        ("now()", 19),
        ("current_timestamp()", 19),
        ("localtimestamp(3)", 23),
        ("utc_timestamp(6)", 26),
    ] {
        let expression =
            crate::dml_runtime::ParseGeneratedExpr(expression).expect("parse time function");
        let value = crate::dml_runtime::EvalExpr(&expression, &row, None)
            .expect("evaluate time function")
            .expect("time function is non-NULL");
        assert_eq!(value.len(), expected_length);
        chrono::NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S%.f")
            .unwrap_or_else(|error| panic!("{value}: {error}"));
    }
}

#[test]
fn charset_introduced_literals_are_evaluated_in_generated_expressions() {
    let expression =
        crate::dml_runtime::ParseGeneratedExpr("deleted_at > _utf8mb4'1970-01-01 01:00:01.000'")
            .expect("parse generated expression with charset introducer");
    let row = HashMap::from([(
        "deleted_at".to_owned(),
        Some("2026-08-26 12:00:00".to_owned()),
    )]);

    assert_eq!(
        crate::dml_runtime::EvalExpr(&expression, &row, None)
            .expect("evaluate charset-introduced literal"),
        Some("1".to_owned())
    );
}

/// 构造含嵌套 JSON 生成列的 TableInfo，用于 tablecodec 读写路径。
fn generated_table_info() -> astersql_meta_model::TableInfo {
    fn column(id: i64, name: &str, tp: u8) -> astersql_meta_model::ColumnInfo {
        astersql_meta_model::ColumnInfo {
            ID: id,
            Name: astersql_parser_ast::NewCIStr(name),
            Offset: (id - 1) as isize,
            State: astersql_meta_model::StatePublic,
            FieldType: astersql_parser_types::NewFieldType(tp),
            ..Default::default()
        }
    }
    let mut columns = vec![
        column(1, "col1", astersql_parser_mysql::r#type::TypeLonglong),
        column(2, "col2", astersql_parser_mysql::r#type::TypeVarchar),
        column(3, "col3", astersql_parser_mysql::r#type::TypeLong),
        column(4, "col4", astersql_parser_mysql::r#type::TypeVarchar),
        column(5, "col5", astersql_parser_mysql::r#type::TypeVarchar),
        column(
            6,
            "modify_time",
            astersql_parser_mysql::r#type::TypeLonglong,
        ),
        column(
            7,
            "create_time",
            astersql_parser_mysql::r#type::TypeLonglong,
        ),
        column(8, "col6", astersql_parser_mysql::r#type::TypeJSON),
        column(9, "col7", astersql_parser_mysql::r#type::TypeJSON),
        column(10, "col8", astersql_parser_mysql::r#type::TypeJSON),
        column(11, "col9", astersql_parser_mysql::r#type::TypeVarchar),
        column(12, "col10", astersql_parser_mysql::r#type::TypeVarchar),
    ];
    // col9/col10 为依赖 JSON 的生成列表达式，对齐 Go 嵌套生成列用例。
    columns[9].GeneratedExprString =
        "json_merge_patch(ifnull(col6, '{}'), ifnull(col7, '{}'))".to_owned();
    columns[9].GeneratedStored = true;
    columns[10].GeneratedExprString =
        "left(json_unquote(json_extract(col8, '$.col9[0]')), 36)".to_owned();
    columns[11].GeneratedExprString =
        "left(json_unquote(json_extract(col8, '$.col10')), 30)".to_owned();
    astersql_meta_model::TableInfo {
        ID: 901,
        Name: astersql_parser_ast::NewCIStr("test1"),
        Columns: columns,
        ..Default::default()
    }
}

/// 构造带/不带自增或 auto_random 主键的表，供 auto_id_allocator 矩阵测试。
fn auto_id_table_info(
    unsigned: bool,
    auto_random: bool,
    has_auto_id: bool,
) -> astersql_meta_model::TableInfo {
    let mut a_type =
        astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeLonglong);
    if has_auto_id {
        // AutoRandomBits>0 时用 auto_random 而非 AUTO_INCREMENT 标志。
        let mut flags = astersql_parser_mysql::r#type::PriKeyFlag;
        if !auto_random {
            flags |= astersql_parser_mysql::r#type::AutoIncrementFlag;
        }
        if unsigned {
            flags |= astersql_parser_mysql::r#type::UnsignedFlag;
        }
        a_type.SetFlag(flags);
    }
    astersql_meta_model::TableInfo {
        ID: 902,
        Name: astersql_parser_ast::NewCIStr("t"),
        PKIsHandle: has_auto_id,
        AutoIncID: 1,
        AutoRandomBits: if auto_random { 5 } else { 0 },
        Columns: vec![
            astersql_meta_model::ColumnInfo {
                ID: 1,
                Name: astersql_parser_ast::NewCIStr("a"),
                Offset: 0,
                State: astersql_meta_model::StatePublic,
                FieldType: a_type,
                ..Default::default()
            },
            astersql_meta_model::ColumnInfo {
                ID: 2,
                Name: astersql_parser_ast::NewCIStr("b"),
                Offset: 1,
                State: astersql_meta_model::StatePublic,
                FieldType: astersql_parser_types::NewFieldType(
                    astersql_parser_mysql::r#type::TypeLong,
                ),
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

/// 验证 INSERT/UPDATE/DELETE/REPLACE 与 EXPLAIN ANALYZE 触及真实 KV 运行时。
#[test]
fn real_sql_dml_and_explain_analyze_reach_the_kv_runtime() {
    let session = concrete_session();
    session
        .execute("insert into aster_session_kv(k, v) values ('a', 'one'), ('b', 'two')")
        .expect("seed real KV rows");

    session
        .execute("update aster_session_kv set v = 'changed' where k = 'a'")
        .expect("update real KV row");
    let mut changed = session
        .execute("select v from aster_session_kv where k = 'a'")
        .expect("read updated KV row")
        .remove(0);
    assert_eq!(
        changed.Next().expect("updated row"),
        Some(vec!["changed".to_owned()])
    );

    session
        .execute("delete from aster_session_kv where k = 'b'")
        .expect("delete real KV row");
    let mut deleted = session
        .execute("select v from aster_session_kv where k = 'b'")
        .expect("read deleted KV row")
        .remove(0);
    assert_eq!(deleted.Next().expect("deleted row"), None);

    session
        .execute("replace into aster_session_kv(k, v) values ('a', 'replaced')")
        .expect("replace real KV row");
    let mut replaced = session
        .execute("select v from aster_session_kv where k = 'a'")
        .expect("read replaced KV row")
        .remove(0);
    assert_eq!(
        replaced.Next().expect("replaced row"),
        Some(vec!["replaced".to_owned()])
    );

    // EXPLAIN ANALYZE 应暴露 Replace 算子与 prewrite/commit 统计。
    let mut explained = session
        .execute("explain analyze replace into aster_session_kv(k, v) values ('c', 'three')")
        .expect("explain analyze real Replace")
        .remove(0);
    assert_eq!(explained.Columns()[0], "id");
    let row = explained
        .Next()
        .expect("explain row")
        .expect("one explain row");
    assert_eq!(row[0], "Replace_1");
    assert_eq!(row[2], "1");
    assert!(row[5].contains("prewrite_keys:1"));
    assert!(row[5].contains("committed:true"));
}

/// 显式事务内读写 mem-buffer，以及 INSERT IGNORE / ON DUPLICATE KEY UPDATE。
#[test]
fn explicit_transaction_ignore_and_on_duplicate_follow_go_dml_semantics() {
    let session = concrete_session();
    session.execute("begin").expect("begin real transaction");
    session
        .execute("insert into aster_session_kv(k, v) values ('txn', 'before')")
        .expect("insert in transaction");
    session
        .execute("update aster_session_kv set v = 'inside' where k = 'txn'")
        .expect("update transaction mem-buffer row");
    let mut inside = session
        .execute("select v from aster_session_kv where k = 'txn'")
        .expect("read transaction mem-buffer row")
        .remove(0);
    assert_eq!(
        inside.Next().expect("inside row"),
        Some(vec!["inside".to_owned()])
    );
    session.execute("commit").expect("commit transaction");

    session
        .execute("insert ignore into aster_session_kv(k, v) values ('txn', 'ignored')")
        .expect("ignore duplicate");
    session
        .execute(
            "insert into aster_session_kv(k, v) values ('txn', 'duplicate') \
             on duplicate key update v = values(v)",
        )
        .expect("update duplicate from VALUES(v)");
    let mut duplicate = session
        .execute("select v from aster_session_kv where k = 'txn'")
        .expect("read duplicate-updated row")
        .remove(0);
    assert_eq!(
        duplicate.Next().expect("duplicate row"),
        Some(vec!["duplicate".to_owned()])
    );
}

#[test]
fn insert_ignore_clamps_signed_int_overflow_like_go() {
    let session = concrete_session();
    session
        .execute("create table ignore_int_overflow (id int primary key, v int)")
        .expect("create overflow table");
    session
        .execute(
            "insert ignore into ignore_int_overflow values (1, 9223372036854775807), \
             (2, -9223372036854775807)",
        )
        .expect("INSERT IGNORE converts overflow to warnings");
    let mut result = session
        .execute("select id,v from ignore_int_overflow order by id")
        .expect("read clamped rows")
        .remove(0);
    assert_eq!(
        result.Next().expect("positive overflow row"),
        Some(vec!["1".to_owned(), i32::MAX.to_string()])
    );
    assert_eq!(
        result.Next().expect("negative overflow row"),
        Some(vec!["2".to_owned(), i32::MIN.to_string()])
    );
}

#[test]
fn insert_ignore_uses_temporal_zero_for_null_primary_key_like_go() {
    let session = concrete_session();
    session
        .execute("create table ignore_datetime_null (v datetime primary key)")
        .expect("create temporal table");
    session
        .execute("insert ignore into ignore_datetime_null values (null)")
        .expect("INSERT IGNORE converts NULL temporal key to its zero value");
    let mut result = session
        .execute("select v from ignore_datetime_null")
        .expect("read zero temporal row")
        .remove(0);
    assert_eq!(
        result.Next().expect("zero temporal row"),
        Some(vec!["0000-00-00 00:00:00".to_owned()])
    );
}

#[test]
fn temporal_column_compared_with_numeric_literal_uses_numeric_coercion() {
    let session = concrete_session();
    session
        .execute("create table temporal_numeric_cmp (v datetime)")
        .expect("create temporal comparison table");
    session
        .execute("insert into temporal_numeric_cmp values ('2024-01-01 00:00:00')")
        .expect("insert temporal value");
    let mut result = session
        .execute("select v from temporal_numeric_cmp where v > -0.5")
        .expect("compare temporal column in numeric context")
        .remove(0);
    assert_eq!(
        result.Next().expect("numeric comparison row"),
        Some(vec!["2024-01-01 00:00:00".to_owned()])
    );
    let mut result = session
        .execute("select v from temporal_numeric_cmp where v > '783'")
        .expect("invalid temporal constant follows MySQL coercion")
        .remove(0);
    assert_eq!(
        result.Next().expect("coerced string comparison row"),
        Some(vec!["2024-01-01 00:00:00".to_owned()])
    );
}

#[test]
fn sum_coerces_string_values_from_their_numeric_prefix() {
    let session = concrete_session();
    session
        .execute("create table sum_string_values (v varchar(32))")
        .expect("create string aggregate table");
    session
        .execute("insert into sum_string_values values ('12abc'), ('word'), ('-2.5tail')")
        .expect("insert string aggregate values");
    let mut result = session
        .execute("select sum(v) from sum_string_values")
        .expect("sum coerces string values")
        .remove(0);
    assert_eq!(
        result.Next().expect("string sum row"),
        Some(vec!["9.5".to_owned()])
    );
}

/// 嵌套生成列：INSERT/UPDATE 后经 tablecodec 解码校验派生列值。
#[test]
fn tablecodec_nested_generated_columns_follow_go_insert_update_delete() {
    let session = concrete_session();
    session
        .RegisterDmlTable(generated_table_info())
        .expect("register generated TableInfo");
    session
        .execute("insert into test1 values (-100000000, '123459789332', 1, '123459789332', 'BBBBB', 1675871896, 1675871896, '{\"col10\": \"CCCCC\",\"col9\": [\"ABCDEFG\"]}', null, default, default, default)")
        .expect("evaluate Go nested generated-column insert");
    let rows = session.ReadDmlRows("test1").expect("decode tablecodec row");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["col9"], Some("ABCDEFG".to_owned()));
    assert_eq!(rows[0]["col10"], Some("CCCCC".to_owned()));

    session
        .execute("update test1 set col7 = '{\"col10\":\"DDDDD\",\"col9\":[\"abcdefg\"]}' where col1 = -100000000")
        .expect("update base JSON and reevaluate Go generated dependency chain");
    let rows = session.ReadDmlRows("test1").expect("decode updated row");
    assert_eq!(rows[0]["col9"], Some("abcdefg".to_owned()));
    assert_eq!(rows[0]["col10"], Some("DDDDD".to_owned()));

    session
        .execute("delete from test1 where col1 < 0")
        .expect("delete tablecodec row");
    assert!(
        session
            .ReadDmlRows("test1")
            .expect("rows after delete")
            .is_empty()
    );
}

/// 注入 prewrite 失败：错误向上传播且失败写不可见。
#[test]
fn autocommit_failure_is_propagated_and_does_not_publish_kv_writes() {
    let session = concrete_session();
    session.InjectNextDmlCommitError("injected prewrite failure");
    let error = match session
        .execute("insert into aster_session_kv(k, v) values ('failed', 'not-visible')")
    {
        Ok(_) => panic!("injected commit error must reach the caller"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("injected prewrite failure"));
    let mut rows = session
        .execute("select v from aster_session_kv where k = 'failed'")
        .expect("read after failed commit")
        .remove(0);
    assert_eq!(rows.Next().expect("failed row visibility"), None);
}

#[test]
fn explain_analyze_commit_failure_closes_terminal_and_preserves_rows() {
    let session = concrete_session();
    session
        .execute("create table explain_commit_failure (a int primary key, b int)")
        .expect("create commit-failure fixture");
    session
        .execute("insert into explain_commit_failure values (1, 10)")
        .expect("seed commit-failure fixture");
    session.InjectNextDmlCommitError("injected EXPLAIN ANALYZE commit failure");

    let error = match session
        .execute("explain analyze update explain_commit_failure set b = b + 1 where a = 1")
    {
        Ok(_) => panic!("commit failure must abort EXPLAIN ANALYZE DML"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("injected EXPLAIN ANALYZE commit failure")
    );

    let mut rows = session
        .execute("select b from explain_commit_failure where a = 1")
        .expect("session remains usable after terminal cleanup")
        .remove(0);
    assert_eq!(
        rows.Next().expect("read preserved row"),
        Some(vec!["10".to_owned()])
    );
    assert_eq!(rows.Next().expect("read terminal row"), None);
}

/// EXPLAIN ANALYZE 报告 auto_id_allocator 的 alloc/rebase 计数（含无自增对照）。
#[test]
fn explain_analyze_dml2_reports_go_auto_id_allocator_matrix() {
    let cases = [
        ("insert into t () values ()", "", 1, 0),
        ("insert into t (a) values (99000000000)", "", 0, 1),
        ("insert into t (a) values (null), (99000000000)", "", 1, 1),
        (
            "insert ignore into t values (null,1), (2,2), (99000000000,3), (100000000000,4)",
            "",
            1,
            2,
        ),
        (
            "insert into t values (null,null), (1,1), (2,2) on duplicate key update a=a+100000000000",
            "",
            1,
            1,
        ),
        ("replace into t () values ()", "", 1, 0),
        ("replace into t (a) values (null), (99000000000)", "", 1, 1),
        (
            "update t set a=a*100000000000",
            "insert into t values (1,1),(2,2)",
            0,
            2,
        ),
    ];
    // 有符号/无符号自增与 auto_random 三组矩阵；auto_random 跳过 ON DUPLICATE。
    for (unsigned, auto_random) in [(false, false), (true, false), (false, true)] {
        for (sql, prepare, alloc_count, rebase_count) in cases {
            if auto_random && sql.contains("on duplicate key") {
                continue;
            }
            let session = concrete_session();
            session
                .RegisterDmlTable(auto_id_table_info(unsigned, auto_random, true))
                .expect("register auto-ID TableInfo");
            if auto_random {
                session
                    .execute("set @@allow_auto_random_explicit_insert=1")
                    .expect("allow explicit auto-random values like the Go matrix");
            }
            if !prepare.is_empty() {
                session.execute(prepare).expect("prepare auto-ID case");
            }
            let mut result = session
                .execute(&format!("explain analyze {sql}"))
                .unwrap_or_else(|error| panic!("auto-ID case `{sql}` failed: {error}"))
                .remove(0);
            let row = result
                .Next()
                .expect("explain row")
                .expect("one explain row");
            let expected = format!(
                "auto_id_allocator: {{alloc_cnt: {alloc_count}, rebase_cnt: {rebase_count}}}"
            );
            assert!(row[5].contains(&expected), "{sql}: {}", row[5]);
        }
    }

    // 无自增主键时不应出现 auto_id_allocator 字段。
    for (sql, prepare, _, _) in cases {
        let session = concrete_session();
        session
            .RegisterDmlTable(auto_id_table_info(false, false, false))
            .expect("register table without auto ID");
        if !prepare.is_empty() {
            session.execute(prepare).expect("prepare no-auto-ID case");
        }
        let mut result = session
            .execute(&format!("explain analyze {sql}"))
            .unwrap_or_else(|error| panic!("no-auto-ID case `{sql}` failed: {error}"))
            .remove(0);
        let row = result
            .Next()
            .expect("explain row")
            .expect("one explain row");
        assert!(!row[5].contains("auto_id_allocator"), "{sql}: {}", row[5]);
    }
}

#[test]
fn explain_analyze_insert_reports_foreign_key_check_phases() {
    let session = concrete_session();
    for sql in [
        "create table parent_runtime (id int key)",
        "create table child_runtime (id int key, parent_id int, foreign key (parent_id) references parent_runtime(id))",
        "insert into parent_runtime values (1)",
    ] {
        session
            .execute(sql)
            .expect("prepare foreign-key runtime stats");
    }

    let mut result = session
        .execute("explain analyze insert ignore into child_runtime values (1,1),(2,null)")
        .expect("explain foreign-key INSERT")
        .remove(0);
    let row = result
        .Next()
        .expect("explain row")
        .expect("one explain row");
    for field in [
        "time:",
        "loops:",
        "prepare:",
        "check_insert:",
        "total_time:",
        "mem_insert_time:",
        "prefetch:",
        "fk_check:",
    ] {
        assert!(
            row[5].contains(field),
            "missing {field} in INSERT execution info: {}",
            row[5]
        );
    }
}

#[test]
fn insert_on_duplicate_validates_the_updated_foreign_key_row() {
    let session = concrete_session();
    for sql in [
        "create table parent_upsert (a int, b int, unique index(a,b))",
        "create table child_upsert (id int key, a int, b int, foreign key(a,b) references parent_upsert(a,b))",
        "insert into parent_upsert values (11,21),(12,22)",
        "insert into child_upsert values (1,11,21)",
        "insert into child_upsert values (1,14,26) on duplicate key update a=12,b=22",
    ] {
        session.execute(sql).expect(sql);
    }
    let mut result = session
        .execute("select id,a,b from child_upsert")
        .expect("query updated child row")
        .remove(0);
    assert_eq!(
        result.Next().expect("updated row"),
        Some(vec!["1".to_owned(), "12".to_owned(), "22".to_owned()])
    );
}

#[test]
fn self_referencing_insert_sees_rows_staged_by_the_same_statement() {
    let session = concrete_session();
    session
        .execute(
            "create table employee_fk (id int key, leader int, foreign key(leader) references employee_fk(id) on delete cascade)",
        )
        .expect("create self-referencing table");
    session
        .execute("insert into employee_fk values (1,null),(10,1),(11,1),(20,10)")
        .expect("insert self-referencing hierarchy in one statement");
    let mut result = session
        .execute("select id,leader from employee_fk order by id")
        .expect("query self-referencing hierarchy")
        .remove(0);
    assert_eq!(
        result.Next().expect("root row"),
        Some(vec!["1".to_owned(), "<nil>".to_owned()])
    );
    assert_eq!(
        result.Next().expect("first child"),
        Some(vec!["10".to_owned(), "1".to_owned()])
    );
}

#[test]
fn replace_checks_only_the_unique_index_being_probed_for_dangling_entries() {
    let session = concrete_session();
    for sql in [
        "create table replace_parent (id int, a int, b int, unique index(id), unique index(a,b))",
        "create table replace_child (id int, a int, b int, unique index(id), unique index(a,b), foreign key(a,b) references replace_parent(a,b))",
        "replace into replace_parent values (1,1,1)",
        "replace into replace_child values (1,1,1)",
    ] {
        session.execute(sql).expect(sql);
    }
    let error = match session.execute("replace into replace_parent values (1,2,3)") {
        Err(error) => error,
        Ok(_) => panic!("referenced parent replacement must fail"),
    };
    assert!(
        error
            .to_string()
            .contains("Cannot delete or update a parent row"),
        "unexpected REPLACE error: {error}"
    );
}

#[test]
fn cascade_update_checks_tables_referencing_the_updated_child() {
    let session = concrete_session();
    for sql in [
        "create table cascade_parent (id int key)",
        "create table cascade_child (id int key, foreign key(id) references cascade_parent(id) on update cascade)",
        "create table cascade_grandchild (id int key, foreign key(id) references cascade_child(id))",
        "insert into cascade_parent values (1)",
        "insert into cascade_child values (1)",
        "insert into cascade_grandchild values (1)",
    ] {
        session.execute(sql).expect(sql);
    }
    let error = match session
        .execute("insert into cascade_parent values (1) on duplicate key update id=2")
    {
        Err(error) => error,
        Ok(_) => panic!("grandchild restrict edge must reject cascade update"),
    };
    assert!(
        error
            .to_string()
            .contains("Cannot delete or update a parent row"),
        "unexpected cascade restriction error: {error}"
    );
    for (table, expected) in [
        ("cascade_parent", "1"),
        ("cascade_child", "1"),
        ("cascade_grandchild", "1"),
    ] {
        let mut result = session
            .execute(&format!("select id from {table}"))
            .expect("query unchanged cascade table")
            .remove(0);
        assert_eq!(
            result.Next().expect("unchanged row"),
            Some(vec![expected.to_owned()])
        );
    }
}

#[test]
fn self_referencing_cascade_stops_at_the_depth_limit_without_recursing_on_itself() {
    let session = concrete_session();
    session
        .execute(
            "create table cascade_depth (id int key, pid int, foreign key(pid) references cascade_depth(id) on delete cascade)",
        )
        .expect("create cascade depth table");
    session
        .execute(
            "insert into cascade_depth values (0,0),(1,0),(2,1),(3,2),(4,3),(5,4),(6,5),(7,6),(8,7),(9,8),(10,9),(11,10),(12,11),(13,12),(14,13),(15,14)",
        )
        .expect("insert deep self-reference chain");
    let error = match session.execute("delete from cascade_depth where id=0") {
        Err(error) => error,
        Ok(_) => panic!("cascade deeper than 15 levels must fail"),
    };
    assert!(
        error.to_string().contains("cascade depth exceeded"),
        "unexpected cascade depth error: {error}"
    );
    session
        .execute("delete from cascade_depth where id=15")
        .expect("shorten cascade chain");
    session
        .execute("delete from cascade_depth where id=0")
        .expect("delete chain at supported depth");
}

#[test]
fn disabled_foreign_key_checks_skip_update_cascades() {
    let session = concrete_session();
    for sql in [
        "create table disabled_parent (id int key)",
        "create table disabled_child (id int key, pid int, foreign key(pid) references disabled_parent(id) on update cascade)",
        "insert into disabled_parent values (1)",
        "insert into disabled_child values (2,1)",
        "set foreign_key_checks=0",
        "update disabled_parent set id=10 where id=1",
    ] {
        session.execute(sql).expect(sql);
    }
    let mut result = session
        .execute("select pid from disabled_child")
        .expect("query child with checks disabled")
        .remove(0);
    assert_eq!(
        result.Next().expect("child row"),
        Some(vec!["1".to_owned()])
    );
}

#[test]
fn update_cascade_stops_at_the_depth_limit_atomically() {
    let session = concrete_session();
    session
        .execute("create table update_depth_0 (id int unique)")
        .expect("create cascade root");
    session
        .execute("insert into update_depth_0 values (1)")
        .expect("insert cascade root");
    for depth in 1..=16 {
        session
            .execute(&format!(
                "create table update_depth_{depth} (id int unique, foreign key(id) references update_depth_{}(id) on update cascade)",
                depth - 1
            ))
            .expect("create cascade child");
        session
            .execute(&format!("insert into update_depth_{depth} values (1)"))
            .expect("insert cascade child");
    }
    let error = match session.execute("update update_depth_0 set id=10 where id=1") {
        Err(error) => error,
        Ok(_) => panic!("update cascade deeper than 15 levels must fail"),
    };
    assert!(
        error.to_string().contains("cascade depth exceeded"),
        "unexpected cascade depth error: {error}"
    );
    let mut result = session
        .execute("select id from update_depth_0")
        .expect("query unchanged root")
        .remove(0);
    assert_eq!(result.Next().expect("root row"), Some(vec!["1".to_owned()]));

    session
        .execute("drop table update_depth_16")
        .expect("drop level beyond supported depth");
    session
        .execute("update update_depth_0 set id=10 where id=1")
        .expect("cascade through 15 levels");
    let mut result = session
        .execute("select id from update_depth_15")
        .expect("query deepest supported level")
        .remove(0);
    assert_eq!(
        result.Next().expect("deep row"),
        Some(vec!["10".to_owned()])
    );
}

#[test]
fn bit_arithmetic_uses_column_metadata_without_reinterpreting_strings() {
    let row = HashMap::from([
        ("bits".into(), Some("0xFF".into())),
        ("text_value".into(), Some("0xFF".into())),
        ("missing".into(), None),
    ]);
    let bit_columns = vec!["bits".to_owned(), "missing".to_owned()];
    for (sql, expected) in [
        ("bits+1", Some("256")),
        ("(bits)-1", Some("254")),
        ("bits*2", Some("510")),
        ("bits/2", Some("127.5")),
        ("bits%2", Some("1")),
        ("-bits", Some("-255")),
        ("+(bits)", Some("255")),
        ("(bits+1)*2", Some("512")),
        ("missing+1", None),
        ("bits", Some("0xFF")),
        ("text_value", Some("0xFF")),
    ] {
        let expr = crate::dml_runtime::ParseGeneratedExpr(sql).unwrap();
        assert_eq!(
            crate::dml_runtime::EvalExprWithBitColumns(&expr, &row, None, &bit_columns)
                .unwrap_or_else(|error| panic!("{sql}: {error}"))
                .as_deref(),
            expected,
            "{sql}"
        );
    }
    let expr = crate::dml_runtime::ParseGeneratedExpr("text_value+1").unwrap();
    assert!(crate::dml_runtime::EvalExprWithBitColumns(&expr, &row, None, &bit_columns).is_err());
}

#[test]
fn sql_defaults_and_bootstrap_upgrade() {
    struct Restore(u64, u64);
    impl Drop for Restore {
        fn drop(&mut self) {
            astersql_sessionctx_vardef::AnalyzeDefaultNumBuckets.Store(self.0);
            astersql_sessionctx_vardef::AnalyzeDefaultNumTopN.Store(self.1);
        }
    }
    let _restore = Restore(
        astersql_sessionctx_vardef::AnalyzeDefaultNumBuckets.Load(),
        astersql_sessionctx_vardef::AnalyzeDefaultNumTopN.Load(),
    );
    let se = canonical_mlog_session();
    se.execute("DELETE FROM mysql.global_variables WHERE variable_name='tidb_analyze_default_num_buckets' OR variable_name='tidb_analyze_default_num_topn'").unwrap();
    se.execute(
        "UPDATE mysql.tidb SET variable_value='262' WHERE variable_name='tidb_server_version'",
    )
    .unwrap();
    let se = crate::runtime::BootstrapCanonicalDomain(Arc::clone(se.domain())).unwrap();
    let mut rs = se
        .execute("SELECT variable_value FROM mysql.tidb WHERE variable_name='tidb_server_version'")
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(rs.Next().unwrap(), Some(vec!["283".into()]));
    for (name, value) in [
        ("tidb_analyze_default_num_buckets", "256"),
        ("tidb_analyze_default_num_topn", "100"),
    ] {
        let mut rs = se
            .execute(&format!(
                "SELECT variable_value FROM mysql.global_variables WHERE variable_name='{name}'"
            ))
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(rs.Next().unwrap(), Some(vec![value.into()]));
        let mut global = se
            .execute(&format!("SELECT @@global.{name}"))
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(global.Next().unwrap(), Some(vec![value.into()]));
        assert_eq!(rs.Next().unwrap(), None);
    }
    se.execute("SET GLOBAL tidb_analyze_default_num_topn=0")
        .unwrap();
    se.execute("SET GLOBAL tidb_analyze_default_num_buckets=4")
        .unwrap();
    se.execute("CREATE TABLE task5_stats (a BIGINT)").unwrap();
    se.execute("INSERT INTO task5_stats VALUES (1),(1),(1),(1),(2),(3),(4),(5),(6)")
        .unwrap();
    se.execute("ANALYZE TABLE task5_stats").unwrap();
    let mut rs = se
        .execute("SHOW STATS_TOPN WHERE table_name='task5_stats'")
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(rs.Next().unwrap(), None);
    se.execute("ANALYZE TABLE task5_stats WITH 5 TOPN").unwrap();
    let mut rs = se
        .execute("SHOW STATS_TOPN WHERE table_name='task5_stats'")
        .unwrap()
        .pop()
        .unwrap();
    let mut rows = Vec::new();
    while let Some(row) = rs.Next().unwrap() {
        rows.push(row);
    }
    assert_eq!(rows.len(), 5);
    se.execute("UPDATE mysql.global_variables SET variable_value='512' WHERE variable_name='tidb_analyze_default_num_buckets'").unwrap();
    se.execute("UPDATE mysql.global_variables SET variable_value='150' WHERE variable_name='tidb_analyze_default_num_topn'").unwrap();
    se.execute(
        "UPDATE mysql.tidb SET variable_value='262' WHERE variable_name='tidb_server_version'",
    )
    .unwrap();
    let se = crate::runtime::BootstrapCanonicalDomain(Arc::clone(se.domain())).unwrap();
    for (name, value) in [
        ("tidb_analyze_default_num_buckets", "512"),
        ("tidb_analyze_default_num_topn", "150"),
    ] {
        let mut rs = se
            .execute(&format!(
                "SELECT variable_value FROM mysql.global_variables WHERE variable_name='{name}'"
            ))
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(rs.Next().unwrap(), Some(vec![value.into()]));
        let mut global = se
            .execute(&format!("SELECT @@global.{name}"))
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(global.Next().unwrap(), Some(vec![value.into()]));
    }
}

#[test]
fn saved_options_precede_changed_globals() {
    struct Restore(u64, u64, bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            use astersql_sessionctx_vardef as v;
            v::AnalyzeDefaultNumBuckets.Store(self.0);
            v::AnalyzeDefaultNumTopN.Store(self.1);
            v::PersistAnalyzeOptions.Store(self.2);
        }
    }
    use astersql_sessionctx_vardef as v;
    let _restore = Restore(
        v::AnalyzeDefaultNumBuckets.Load(),
        v::AnalyzeDefaultNumTopN.Load(),
        v::PersistAnalyzeOptions.Load(),
    );
    let se = canonical_mlog_session();
    se.execute("SET GLOBAL tidb_persist_analyze_options=ON")
        .unwrap();
    se.execute("SET GLOBAL tidb_analyze_default_num_topn=0")
        .unwrap();
    se.execute("CREATE TABLE task5_saved (a BIGINT)").unwrap();
    se.execute("INSERT INTO task5_saved VALUES (1),(1),(1),(1),(2),(3),(4),(5),(6)")
        .unwrap();
    se.execute("ANALYZE TABLE task5_saved WITH 5 TOPN, 4 BUCKETS")
        .unwrap();
    let mut rs = se
        .execute("SELECT buckets,topn FROM mysql.analyze_options")
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(rs.Next().unwrap(), Some(vec!["4".into(), "5".into()]));
    se.execute("ANALYZE TABLE task5_saved").unwrap();
    let mut rs = se
        .execute("SHOW STATS_TOPN WHERE table_name='task5_saved'")
        .unwrap()
        .pop()
        .unwrap();
    let mut n = 0;
    while rs.Next().unwrap().is_some() {
        n += 1;
    }
    assert_eq!(n, 5);
    // The already-supported parser's DEFAULT must keep selecting the live
    // default when persistence is wired in; do not revive a saved literal.
    se.execute("ANALYZE TABLE task5_saved WITH DEFAULT TOPN")
        .unwrap();
    let mut defaults = se
        .execute("SHOW STATS_TOPN WHERE table_name='task5_saved'")
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(defaults.Next().unwrap(), None);
    se.execute("SET GLOBAL tidb_persist_analyze_options=OFF")
        .unwrap();
    se.execute("ANALYZE TABLE task5_saved").unwrap();
    let mut rs = se
        .execute("SHOW STATS_TOPN WHERE table_name='task5_saved'")
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(rs.Next().unwrap(), None);
    se.execute("SET GLOBAL tidb_persist_analyze_options=ON")
        .unwrap();
    se.execute("SET tidb_partition_prune_mode='static'")
        .unwrap();
    se.execute("CREATE TABLE task5_partition (a BIGINT) PARTITION BY HASH(a) PARTITIONS 2")
        .unwrap();
    se.execute("INSERT INTO task5_partition VALUES (1),(1),(2),(2),(3),(4),(5),(6)")
        .unwrap();
    se.execute("ANALYZE TABLE task5_partition WITH 5 TOPN, 4 BUCKETS")
        .unwrap();
    se.execute("ANALYZE TABLE task5_partition PARTITION p0 WITH 0 TOPN")
        .unwrap();
    se.execute("ANALYZE TABLE task5_partition").unwrap();
    let mut rs = se
        .execute("SHOW STATS_TOPN WHERE table_name='task5_partition' AND partition_name='p0'")
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(rs.Next().unwrap(), None);
    let mut rs = se
        .execute("SHOW STATS_TOPN WHERE table_name='task5_partition' AND partition_name='p1'")
        .unwrap()
        .pop()
        .unwrap();
    let mut n = 0;
    while rs.Next().unwrap().is_some() {
        n += 1;
    }
    assert_eq!(n, 3);
    se.execute("SET tidb_partition_prune_mode='dynamic'")
        .unwrap();
    se.execute("ANALYZE TABLE task5_partition PARTITION p0 WITH 0 TOPN")
        .unwrap();
    let mut warnings = se.execute("SHOW WARNINGS").unwrap().pop().unwrap();
    let mut messages = Vec::new();
    while let Some(row) = warnings.Next().unwrap() {
        messages.push(row);
    }
    assert!(
        messages
            .iter()
            .any(|row| row.last().is_some_and(|message| message
                == "Ignore columns and options when analyze partition in dynamic mode")),
        "{messages:?}"
    );

    let mut rs = se
        .execute("SHOW STATS_TOPN WHERE table_name='task5_partition' AND partition_name='p0'")
        .unwrap()
        .pop()
        .unwrap();
    let mut n = 0;
    while rs.Next().unwrap().is_some() {
        n += 1;
    }
    assert_eq!(n, 3);
    let mut saved = se
        .execute("SELECT topn FROM mysql.analyze_options WHERE topn=0")
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(saved.Next().unwrap(), Some(vec!["0".into()]));
    assert_eq!(saved.Next().unwrap(), None);
}

#[test]
fn dxf_metadata_case_update_preserves_exact_ids_and_lazy_branches() {
    use crate::dml_runtime::{EvalExpr, ParseGeneratedExpr};
    let id = "9007199254740993";
    let row = HashMap::from([("id".to_owned(), Some(id.to_owned()))]);
    let expr = ParseGeneratedExpr("case id when 9007199254740992 then 'wrong' when 9007199254740993 then 'redacted' when 9007199254740993 then 1/0 else 'unmatched' end").unwrap();
    assert_eq!(
        EvalExpr(&expr, &row, None).unwrap(),
        Some("redacted".into())
    );
    let expr = ParseGeneratedExpr("case id when 1 then 'wrong' else 'unchanged' end").unwrap();
    assert_eq!(
        EvalExpr(&expr, &row, None).unwrap(),
        Some("unchanged".into())
    );
    let expr = ParseGeneratedExpr("case id when 1 then 'wrong' end").unwrap();
    assert_eq!(EvalExpr(&expr, &row, None).unwrap(), None);
    let null_row = HashMap::from([("id".to_owned(), None)]);
    let expr = ParseGeneratedExpr("case id when null then 'wrong' else 'null-id' end").unwrap();
    assert_eq!(
        EvalExpr(&expr, &null_row, None).unwrap(),
        Some("null-id".into())
    );
}

#[test]
fn dxf_metadata_case_selects_binary_literal_without_utf8_conversion() {
    let row = HashMap::from([("id".to_owned(), Some("42".to_owned()))]);
    let expression =
        crate::dml_runtime::ParseGeneratedExpr("case id when 42 then x'00FF7B7D' end").unwrap();
    let result = crate::dml_runtime::CaseResult(&expression, &row, None, &[])
        .unwrap()
        .unwrap();
    let astersql_parser_ast::ExprKind::Value(value) = &result.Kind else {
        panic!("typed CASE result")
    };
    assert!(
        matches!(&value.Datum, astersql_parser_ast::ValueDatum::HexLiteral(bytes) if bytes == &[0, 255, 123, 125])
    );
}
