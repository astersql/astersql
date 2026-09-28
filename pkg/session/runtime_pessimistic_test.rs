// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 会话运行时的事务与悲观锁集成回归测试。
//
// 覆盖冲突检测、锁生命周期、死锁与保存点，以及锁定读、唯一键间隙锁和
// READ COMMITTED 可见性等事务语义；同时验证这些语义与计划缓存、元数据快照、
// 执行统计及故障重试等运行时能力协同工作。

use std::sync::{Arc, Barrier};

use astersql_kv as kv;

use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
use crate::testutil::TestRecordSet;

/// 仅允许按键读取的快照，用于证明提交冲突检查不会退化为范围扫描。
struct PointOnlySnapshot {
    values: std::collections::HashMap<Vec<u8>, Vec<u8>>,
}

impl PointOnlySnapshot {
    fn new(values: impl IntoIterator<Item = (Vec<u8>, Vec<u8>)>) -> Self {
        Self {
            values: values.into_iter().collect(),
        }
    }
}

impl kv::Getter for PointOnlySnapshot {
    fn Get(
        &self,
        context: &kv::Context,
        key: kv::Key,
        options: &[kv::GetOption],
    ) -> Result<kv::ValueEntry, kv::errors::SharedError> {
        self.values
            .get(key.as_ref())
            .cloned()
            .map(|value| kv::NewValueEntry(value, 0))
            .map_or_else(
                || kv::Getter::Get(&kv::EmptyRetriever, context, key, options),
                Ok,
            )
    }
}

impl kv::Retriever for PointOnlySnapshot {
    fn Iter(
        &self,
        _: kv::Key,
        _: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        panic!("commit conflict checks must not scan snapshots")
    }

    fn IterReverse(
        &self,
        _: Option<kv::Key>,
        _: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        panic!("commit conflict checks must not scan snapshots")
    }
}

impl kv::Snapshot for PointOnlySnapshot {
    fn BatchGet(
        &self,
        _: &kv::Context,
        keys: &[kv::Key],
        _: &[kv::BatchGetOption],
    ) -> Result<std::collections::HashMap<String, kv::ValueEntry>, kv::errors::SharedError> {
        Ok(keys
            .iter()
            .filter_map(|key| {
                self.values
                    .get(key.as_ref())
                    .cloned()
                    .map(|value| (kv::KeyMapName(key.as_ref()), kv::NewValueEntry(value, 0)))
            })
            .collect())
    }

    fn SetOption(&mut self, _: i32, _: Option<Box<dyn std::any::Any>>) {}
}

fn rows(session: &ConcreteSession, sql: &str) -> Vec<Vec<String>> {
    let mut sets = session.execute(sql).expect("execute query");
    let mut set = sets.pop().expect("query record set");
    let mut rows = Vec::new();
    while let Some(row) = set.Next().expect("read query row") {
        rows.push(row);
    }
    rows
}

#[test]
fn optimistic_commit_conflict_check_reads_only_written_keys() {
    let base = PointOnlySnapshot::new([
        (b"written".to_vec(), b"old".to_vec()),
        (b"unrelated".to_vec(), b"before".to_vec()),
    ]);
    let latest = PointOnlySnapshot::new([
        (b"written".to_vec(), b"new".to_vec()),
        (b"unrelated".to_vec(), b"after".to_vec()),
    ]);

    assert!(
        ConcreteSession::snapshots_differ_on_keys(&base, &latest, &[b"written".to_vec()])
            .expect("check written key")
    );
    assert!(
        !ConcreteSession::snapshots_differ_on_keys(&base, &latest, &[b"missing".to_vec()])
            .expect("ignore unrelated snapshot changes")
    );
}

#[test]
fn pessimistic_nowait_locks_and_release_follow_transaction_lifetime() {
    let (domain, first) = CreateAnalyzeSession().expect("create runtime");
    let second = ConcreteSession::new(Arc::clone(&domain));
    first
        .execute("create table lock_t (id int primary key, v int)")
        .expect("create lock table");
    first
        .execute("insert into lock_t values (1, 10)")
        .expect("seed lock table");

    first.execute("begin pessimistic").expect("begin holder");
    first
        .execute("update lock_t set v = 11 where id = 1")
        .expect("lock row");
    assert!(first.TransactionIsPessimistic());
    assert_eq!(first.HeldRowLockCount(), 1);

    second.execute("begin pessimistic").expect("begin waiter");
    let error = match second.execute("select * from lock_t where id = 1 for update nowait") {
        Ok(_) => panic!("NOWAIT must reject a held row"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("NOWAIT"));
    first.execute("rollback").expect("release holder");
    second
        .execute("select * from lock_t where id = 1 for update nowait")
        .expect("row can be locked after rollback");
    second.execute("rollback").expect("release waiter");
}

#[test]
fn pessimistic_wait_graph_detects_a_two_transaction_deadlock() {
    let (domain, setup) = CreateAnalyzeSession().expect("create runtime");
    setup.ClearRuntimeDeadlockHistory();
    setup
        .execute("create table deadlock_t (id int primary key, v int)")
        .expect("create deadlock table");
    setup
        .execute("insert into deadlock_t values (1, 10), (2, 20)")
        .expect("seed deadlock table");
    // 两个事务各持有一把锁后再同时请求对方的锁，稳定构造双向等待环。
    let ready = Arc::new(Barrier::new(2));

    let run = |first_id: i64, second_id: i64, ready: Arc<Barrier>, domain: Arc<_>| {
        std::thread::spawn(move || {
            let session = ConcreteSession::new(domain);
            session.execute("begin pessimistic").expect("begin");
            session
                .execute(&format!(
                    "update deadlock_t set v = v + 1 where id = {first_id}"
                ))
                .expect("take first lock");
            ready.wait();
            let result = session.execute(&format!(
                "update deadlock_t set v = v + 1 where id = {second_id}"
            ));
            let deadlock = result
                .as_ref()
                .err()
                .is_some_and(|error| error.to_string().contains("Deadlock"));
            if deadlock {
                session.execute("rollback").expect("rollback victim");
            } else {
                session.execute("commit").expect("commit survivor");
            }
            deadlock
        })
    };
    let first = run(1, 2, Arc::clone(&ready), Arc::clone(&domain));
    let second = run(2, 1, ready, domain);
    let victims = usize::from(first.join().expect("first worker"))
        + usize::from(second.join().expect("second worker"));
    assert_eq!(victims, 1);
    assert_eq!(setup.RuntimeDeadlockHistoryCount(), 2);
    assert_eq!(
        rows(&setup, "select count(*) from information_schema.deadlocks"),
        vec![vec!["2".to_owned()]]
    );
}

#[test]
fn savepoint_restores_transaction_rows_and_releases_later_locks() {
    let (_domain, session) = CreateAnalyzeSession().expect("create runtime");
    session
        .execute("create table savepoint_t (id int primary key, v int)")
        .expect("create savepoint table");
    session
        .execute("insert into savepoint_t values (1, 10), (2, 20)")
        .expect("seed savepoint table");
    session.execute("begin pessimistic").expect("begin");
    session
        .execute("update savepoint_t set v = 11 where id = 1")
        .expect("update before savepoint");
    session.execute("savepoint s1").expect("savepoint");
    session
        .execute("update savepoint_t set v = 22 where id = 2")
        .expect("update after savepoint");
    assert_eq!(session.HeldRowLockCount(), 2);
    // 回滚到保存点既要撤销之后的写入，也要仅释放之后取得的行锁。
    session
        .execute("rollback to savepoint s1")
        .expect("rollback to savepoint");
    assert_eq!(session.HeldRowLockCount(), 1);
    assert_eq!(
        rows(&session, "select * from savepoint_t order by id"),
        vec![
            vec!["1".to_owned(), "11".to_owned()],
            vec!["2".to_owned(), "20".to_owned()]
        ]
    );
    session.execute("commit").expect("commit");
}

#[test]
fn cross_session_database_in_predicate_and_unique_index_are_real() {
    let (domain, first) = CreateAnalyzeSession().expect("create runtime");
    let second = ConcreteSession::new(domain);
    first
        .execute("create database lock_db")
        .expect("create shared database");
    second.execute("use lock_db").expect("cross-session USE");
    second
        .execute("create table u (id int primary key, uk int unique, v int)")
        .expect("create unique table");
    second
        .execute("insert into u values (1, 10, 100), (2, 20, 200)")
        .expect("seed unique table");
    second
        .execute("update u set v = 201 where id in (2)")
        .expect("IN updates only its member");
    assert_eq!(
        rows(&second, "select * from u order by id"),
        vec![
            vec!["1".to_owned(), "10".to_owned(), "100".to_owned()],
            vec!["2".to_owned(), "20".to_owned(), "201".to_owned()]
        ]
    );
    let error = match second.execute("update u set uk = 10 where id in (2)") {
        Ok(_) => panic!("unique secondary index must reject a duplicate"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("Duplicate entry"));
}

#[test]
fn expired_pessimistic_locks_end_the_transaction_and_are_observable() {
    let (_domain, session) = CreateAnalyzeSession().expect("create runtime");
    session
        .execute("create table expire_t (id int primary key, v int)")
        .expect("create expiration table");
    session
        .execute("insert into expire_t values (1, 10)")
        .expect("seed expiration table");
    session.SetPessimisticLockTTLForTest(std::time::Duration::from_millis(1));
    session.execute("begin pessimistic").expect("begin");
    session
        .execute("update expire_t set v = 11 where id = 1")
        .expect("take lock");
    session.ExpirePessimisticLocksForTest();
    let error = match session.execute("select * from expire_t") {
        Ok(_) => panic!("expired lock must abort the transaction"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("TTL manager has timed out"));
    assert!(!session.TransactionIsPessimistic());
    assert_eq!(session.HeldRowLockCount(), 0);
    assert_eq!(
        rows(&session, "select * from expire_t"),
        vec![vec!["1".to_owned(), "10".to_owned()]]
    );
}

#[test]
fn managed_pessimistic_lock_ttl_is_renewed_while_the_session_is_active() {
    let (_domain, session) = CreateAnalyzeSession().expect("create runtime");
    session
        .execute("create table renew_t (id int primary key, v int)")
        .expect("create renewal table");
    session
        .execute("insert into renew_t values (1, 10)")
        .expect("seed renewal table");
    session.SetPessimisticLockTTLForTest(std::time::Duration::from_millis(1));
    session.execute("begin pessimistic").expect("begin");
    session
        .execute("update renew_t set v = 11 where id = 1")
        .expect("take lock");

    std::thread::sleep(std::time::Duration::from_millis(5));

    assert_eq!(
        rows(&session, "select * from renew_t"),
        vec![vec!["1".to_owned(), "11".to_owned()]]
    );
    assert!(session.TransactionIsPessimistic());
    session.execute("rollback").expect("rollback");
}

#[test]
fn explicit_transaction_changes_are_private_until_commit_and_discarded_on_rollback() {
    let (domain, holder) = CreateAnalyzeSession().expect("create runtime");
    let peer = ConcreteSession::new(domain);
    holder
        .execute("create table visibility_t (id int primary key, v int)")
        .expect("create visibility table");
    holder
        .execute("insert into visibility_t values (1, 100)")
        .expect("seed visibility table");

    holder.execute("begin pessimistic").expect("begin holder");
    holder
        .execute("update visibility_t set v = 999 where id = 1")
        .expect("stage holder update");
    assert_eq!(
        rows(&holder, "select * from visibility_t"),
        vec![vec!["1".to_owned(), "999".to_owned()]]
    );
    assert_eq!(
        rows(
            &holder,
            "select * from visibility_t where id = 1 for update"
        ),
        vec![vec!["1".to_owned(), "999".to_owned()]]
    );
    assert_eq!(
        rows(&peer, "select * from visibility_t"),
        vec![vec!["1".to_owned(), "100".to_owned()]]
    );
    holder.execute("rollback").expect("rollback holder");
    assert_eq!(
        rows(&peer, "select * from visibility_t"),
        vec![vec!["1".to_owned(), "100".to_owned()]]
    );

    holder.execute("begin pessimistic").expect("begin holder");
    holder
        .execute("update visibility_t set v = 101 where id = 1")
        .expect("stage committed update");
    holder.execute("commit").expect("commit holder");
    assert_eq!(
        rows(&peer, "select * from visibility_t"),
        vec![vec!["1".to_owned(), "101".to_owned()]]
    );
}

#[test]
fn locking_read_merges_latest_commits_with_local_insert_update_and_delete() {
    let (domain, transaction) = CreateAnalyzeSession().expect("create runtime");
    let peer = ConcreteSession::new(domain);
    transaction
        .execute("create table overlay_t (id int primary key, v int)")
        .expect("create overlay table");
    transaction
        .execute("insert into overlay_t values (1, 10), (2, 20), (3, 30)")
        .expect("seed overlay table");

    transaction
        .execute("begin pessimistic")
        .expect("begin transaction");
    transaction
        .execute("insert into overlay_t values (4, 40)")
        .expect("local insert");
    transaction
        .execute("update overlay_t set v = 22 where id = 2")
        .expect("local update");
    transaction
        .execute("delete from overlay_t where id = 3")
        .expect("local delete");
    peer.execute("update overlay_t set v = 11 where id = 1")
        .expect("external commit");
    // 锁定读以最新已提交版本为底，再叠加本事务尚未提交的增、改、删。
    assert_eq!(
        rows(
            &transaction,
            "select * from overlay_t where id between 1 and 4 order by id for update"
        ),
        vec![
            vec!["1".to_owned(), "11".to_owned()],
            vec!["2".to_owned(), "22".to_owned()],
            vec!["4".to_owned(), "40".to_owned()]
        ]
    );
    transaction
        .execute("rollback")
        .expect("rollback transaction");
}

#[test]
fn optimistic_commit_rejects_a_concurrent_write_conflict() {
    let (domain, first) = CreateAnalyzeSession().expect("create runtime");
    let second = ConcreteSession::new(domain);
    first
        .execute("create table conflict_t (id int primary key, v int)")
        .expect("create conflict table");
    first
        .execute("insert into conflict_t values (1, 10)")
        .expect("seed conflict table");
    first.execute("begin optimistic").expect("begin first");
    second.execute("begin optimistic").expect("begin second");
    first
        .execute("update conflict_t set v = 11 where id = 1")
        .expect("first update");
    second
        .execute("update conflict_t set v = 12 where id = 1")
        .expect("second update");
    first.execute("commit").expect("first commit");
    let error = match second.execute("commit") {
        Ok(_) => panic!("second commit must conflict"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("Write conflict"));
}

#[test]
fn insert_ignore_skips_secondary_unique_conflicts_without_losing_other_rows() {
    let (_domain, session) = CreateAnalyzeSession().expect("create runtime");
    session
        .execute("create table ignore_t (id int primary key, uk int unique)")
        .expect("create unique table");
    session
        .execute("insert into ignore_t values (1, 10), (2, 20)")
        .expect("seed unique table");
    session
        .execute("insert ignore into ignore_t values (3, 20), (4, 40)")
        .expect("ignore secondary conflict");
    assert_eq!(
        rows(&session, "select * from ignore_t order by id"),
        vec![
            vec!["1".to_owned(), "10".to_owned()],
            vec!["2".to_owned(), "20".to_owned()],
            vec!["4".to_owned(), "40".to_owned()]
        ]
    );
}

#[test]
fn missing_unique_values_are_locked_for_nowait_batch_point_gets() {
    let (domain, holder) = CreateAnalyzeSession().expect("create runtime");
    let waiter = ConcreteSession::new(domain);
    holder
        .execute("create table gap_t (id int primary key, uk int unique)")
        .expect("create unique table");
    holder
        .execute("insert into gap_t values (1, 10)")
        .expect("seed unique table");
    holder.execute("begin pessimistic").expect("begin holder");
    // 即使唯一键不存在，批量点查也要锁住对应间隙，阻止并发事务抢占。
    holder
        .execute("select * from gap_t where uk in (30, 40) for update")
        .expect("lock absent unique values");
    waiter.execute("begin pessimistic").expect("begin waiter");
    let error = match waiter.execute("select * from gap_t where uk in (30, 40) for update nowait") {
        Ok(_) => panic!("NOWAIT must reject locked unique gaps"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("NOWAIT"));
    waiter.execute("rollback").expect("rollback waiter");
    holder.execute("rollback").expect("rollback holder");
}

#[test]
fn foreign_key_cascades_share_the_explicit_transaction() {
    let (_domain, session) = CreateAnalyzeSession().expect("create runtime");
    session
        .execute("create table parent_t (id int primary key)")
        .expect("create parent");
    session
        .execute(
            "create table child_t (id int primary key, pid int, constraint fk_parent \
             foreign key (pid) references parent_t(id) on delete cascade on update cascade)",
        )
        .expect("create child");
    session
        .execute("insert into parent_t values (1), (2), (3)")
        .expect("seed parent");
    session
        .execute("insert into child_t values (1, 1), (2, 2), (3, 3)")
        .expect("seed child");
    session.execute("begin pessimistic").expect("begin");
    session
        .execute("delete from parent_t where id = 1")
        .expect("cascade delete");
    session
        .execute("update parent_t set id = 22 where id = 2")
        .expect("cascade update");
    session.execute("commit").expect("commit");
    assert_eq!(
        rows(&session, "select * from child_t order by id"),
        vec![
            vec!["2".to_owned(), "22".to_owned()],
            vec!["3".to_owned(), "3".to_owned()]
        ]
    );
}

#[test]
fn for_share_promotion_does_not_require_noop_functions() {
    let (domain, holder) = CreateAnalyzeSession().expect("create runtime");
    let waiter = ConcreteSession::new(domain);
    holder
        .execute("create table share_t (id int primary key, v int)")
        .expect("create share table");
    holder
        .execute("insert into share_t values (1, 10)")
        .expect("seed share table");
    holder.execute("begin pessimistic").expect("begin holder");
    holder
        .execute("update share_t set v = 11 where id = 1")
        .expect("lock row");
    waiter
        .execute("set tidb_enable_shared_lock_promotion = 1")
        .expect("enable promotion");
    waiter.execute("begin pessimistic").expect("begin waiter");
    let error = match waiter.execute("select * from share_t where id = 1 for share nowait") {
        Ok(_) => panic!("promoted FOR SHARE NOWAIT must observe the lock"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("NOWAIT"));
    waiter.execute("rollback").expect("rollback waiter");
    holder.execute("rollback").expect("rollback holder");
}

#[test]
fn read_committed_plain_reads_refresh_without_locking_unrelated_rows() {
    let (domain, transaction) = CreateAnalyzeSession().expect("create runtime");
    let peer = ConcreteSession::new(domain);
    transaction
        .execute("create table rc_t (id int primary key, v int)")
        .expect("create RC table");
    transaction
        .execute("insert into rc_t values (1, 10), (2, 20)")
        .expect("seed RC table");
    transaction
        .execute("set transaction isolation level read committed")
        .expect("set RC");
    transaction
        .execute("begin pessimistic")
        .expect("begin RC transaction");
    assert_eq!(transaction.TransactionIsolation(), "READ-COMMITTED");
    transaction
        .execute("select * from rc_t where id = 1 for update")
        .expect("lock only id 1");
    peer.execute("update rc_t set v = 22 where id = 2")
        .expect("update unrelated id 2");
    assert_eq!(
        rows(&transaction, "select * from rc_t order by id"),
        vec![
            vec!["1".to_owned(), "10".to_owned()],
            vec!["2".to_owned(), "22".to_owned()]
        ]
    );
    transaction.execute("rollback").expect("rollback RC");
}

#[test]
fn shared_lock_promotion_change_invalidates_named_prepared_plan_cache() {
    let (_domain, session) = CreateAnalyzeSession().expect("create runtime");
    session
        .execute("create table cache_share_t (id int primary key, v int)")
        .expect("create cache table");
    session
        .execute("insert into cache_share_t values (1, 10)")
        .expect("seed cache table");
    session
        .execute("set tidb_enable_noop_functions=1")
        .expect("enable noop");
    session
        .execute("set tidb_enable_shared_lock_promotion=0")
        .expect("disable promotion");
    session
        .execute("set tidb_enable_prepared_plan_cache=1")
        .expect("enable plan cache");
    session.execute("set @pk=1").expect("set parameter");
    session
        .execute(
            "prepare cache_share_stmt from \
             'select id,v from cache_share_t where id=? for share'",
        )
        .expect("prepare");
    session
        .execute("execute cache_share_stmt using @pk")
        .expect("warm plan");
    session.execute("begin pessimistic").expect("begin");
    session
        .execute("execute cache_share_stmt using @pk")
        .expect("transaction execution");
    assert!(!session.LastPlanFromCache());
    session.execute("rollback").expect("rollback");
    session
        .execute("execute cache_share_stmt using @pk")
        .expect("warm after transaction");
    // 锁提升开关改变执行语义，因此已有命名预处理计划不能继续复用。
    session
        .execute("set tidb_enable_shared_lock_promotion=1")
        .expect("change promotion");
    session
        .execute("execute cache_share_stmt using @pk")
        .expect("rebuild plan");
    assert!(!session.LastPlanFromCache());
}

#[test]
fn split_partitioned_table_reports_splits_for_every_physical_table() {
    let (_domain, session) = CreateAnalyzeSession().expect("create runtime");
    session
        .execute(
            "create table partition_split_t(id int primary key) \
             partition by hash(id) partitions 4",
        )
        .expect("create partitioned table");

    assert_eq!(
        rows(
            &session,
            "split table partition_split_t between (0) and (100) regions 2",
        ),
        vec![vec!["4".to_owned(), "1".to_owned()]]
    );
}

#[test]
fn information_schema_regions_and_snapshot_tables_follow_domain_metadata() {
    let (_domain, session) = CreateAnalyzeSession().expect("create runtime");
    session
        .execute("create table region_t (id int primary key, key idx(id))")
        .expect("create region table");
    session
        .execute("split table region_t between (0) and (100) regions 4")
        .expect("split record regions");
    session
        .execute("split table region_t index idx between (0) and (100) regions 4")
        .expect("split index regions");
    let table_id = rows(
        &session,
        "select tidb_table_id from information_schema.tables \
         where table_schema='test' and table_name='region_t'",
    )[0][0]
        .clone();
    assert_eq!(
        rows(
            &session,
            &format!(
                "select count(*) from information_schema.tikv_region_status \
                 where table_id={table_id} and is_index=0"
            ),
        ),
        vec![vec!["4".to_owned()]]
    );
    assert_eq!(
        rows(
            &session,
            &format!(
                "select count(*) from information_schema.tikv_region_status \
                 where table_id={table_id} and is_index=1"
            ),
        ),
        vec![vec!["4".to_owned()]]
    );
    // 删除当前表后切换到旧 schema 时间戳，历史元数据仍应通过信息模式可见。
    session
        .execute("set @schema_ts=@@tidb_current_ts")
        .expect("capture schema timestamp");
    session
        .execute("drop table region_t")
        .expect("drop current table");
    session
        .execute("set @@tidb_snapshot=@schema_ts")
        .expect("activate schema snapshot");
    assert_eq!(
        rows(
            &session,
            "select table_name,table_type,avg_row_length \
             from information_schema.tables where table_schema='test' \
             and table_name='region_t'",
        ),
        vec![vec![
            "region_t".to_owned(),
            "BASE TABLE".to_owned(),
            "0".to_owned()
        ]]
    );
    session
        .execute("set @@tidb_snapshot=''")
        .expect("clear schema snapshot");
    assert!(
        rows(
            &session,
            "select table_name from information_schema.tables where table_schema='test' \
             and table_name='region_t'",
        )
        .is_empty()
    );
}

#[test]
fn explain_analyze_select_reports_scan_rows_process_keys_and_rpc_info() {
    let (_domain, session) = CreateAnalyzeSession().expect("create runtime");
    session
        .execute("create table paging_t (id int primary key, b int, index idx(b))")
        .expect("create paging table");
    session
        .execute("insert into paging_t values (1,1),(2,2),(3,3)")
        .expect("seed paging table");
    let limited = rows(
        &session,
        "explain analyze select * from paging_t limit 1 offset 1",
    );
    assert_eq!(limited[0][0], "Limit");
    assert_eq!(limited[0][2], "1");
    assert_eq!(limited[0][6], "offset:1, count:1");
    let full_scan = limited
        .iter()
        .find(|row| row[0].contains("TableFullScan"))
        .expect("limited plan table scan");
    assert_eq!(full_scan[2], "2");
    assert!(full_scan[5].contains("total_process_keys: 2"));
    let lookup = rows(
        &session,
        "explain analyze select * from paging_t use index(idx) \
         where b > 0 and b < 4",
    );
    assert_eq!(lookup.len(), 3);
    assert!(lookup[1][0].contains("IndexRangeScan"));
    assert!(lookup[1][5].contains("Cop:{num_rpc:1, total_time:"));
    assert!(lookup[1][5].contains("total_process_keys: 3"));
    let automatic_lookup = rows(
        &session,
        "explain analyze select * from paging_t where b > 0 limit 1",
    );
    assert_eq!(automatic_lookup[0][0], "Limit");
    assert!(automatic_lookup[1][0].contains("IndexLookUp"));
    assert!(automatic_lookup[2][0].contains("IndexRangeScan"));
    assert!(automatic_lookup[2][5].contains("total_process_keys: 1"));
    let point = rows(
        &session,
        "explain analyze select * from paging_t where id=1",
    );
    assert_eq!(point.len(), 1);
    assert_eq!(point[0][0], "Point_Get");
    assert!(point[0][5].contains("Get:{num_rpc:1"));
}

#[test]
fn topology_and_runtime_faults_drive_cluster_rows_rpc_counts_and_commit_retries() {
    let (domain, session) = CreateAnalyzeSession().expect("create runtime");
    crate::runtime::RegisterRuntimeTopology(
        &domain,
        vec![
            (41, "tikv-a:20160".to_owned()),
            (42, "tikv-b:20160".to_owned()),
        ],
    );
    assert_eq!(
        rows(
            &session,
            "select count(*) from information_schema.cluster_info \
             where `type`='tikv'",
        ),
        vec![vec!["2".to_owned()]]
    );
    session
        .execute("create table topology_t (id int primary key, b int)")
        .expect("create topology table");
    session
        .execute("insert into topology_t values (1,1),(2,2)")
        .expect("seed topology table");
    session
        .execute("split table topology_t between (0) and (100) regions 5")
        .expect("split topology table");
    let scan = rows(
        &session,
        "explain analyze select * from topology_t where b > 0",
    );
    assert!(scan[0][5].contains("cop_task: {num: 5}"));
    assert!(scan[0][5].contains("num_rpc:5"));

    {
        // 注入客户端发送延迟，验证点查超时重试会反映到 RPC 统计中。
        let _delay = astersql_testkit_testfailpoint::enable(
            "tikvclient/mockBatchClientSendDelay",
            "return(100)",
        );
        let point = rows(
            &session,
            "explain analyze select /*+ set_var(tikv_client_read_timeout=1) */ \
             * from topology_t where id=1",
        );
        assert!(point[0][5].contains("Get:{num_rpc:3"));
    }

    session.InjectAutocommitRetryForTest(1);
    session
        .execute("insert into topology_t values (3,3)")
        .expect("retry autocommit insert");
    assert_eq!(session.LastAutocommitRetryAttempts(), 3);
    assert_eq!(
        rows(&session, "select count(*) from topology_t"),
        vec![vec!["3".to_owned()]]
    );
}

#[test]
fn autocommit_delete_waits_for_indexed_update_without_lock_order_deadlock() {
    let (domain, holder) = CreateAnalyzeSession().expect("create runtime");
    holder
        .execute("create table indexed_wait (id int primary key, v int, key idx_v(v))")
        .unwrap();
    holder
        .execute("insert into indexed_wait values (1,10)")
        .unwrap();
    holder.execute("begin pessimistic").unwrap();
    rows(&holder, "select * from indexed_wait where id=1 for update");
    let peer_domain = Arc::clone(&domain);
    let waiter = std::thread::spawn(move || {
        let session = ConcreteSession::new(peer_domain);
        session
            .execute("delete from indexed_wait where id=1")
            .map(|_| ())
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while holder.RuntimeLockWaitCount() == 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let waiting = holder.RuntimeLockWaitCount() > 0;
    let update = holder.execute("update indexed_wait set v=11 where id=1");
    if update.is_ok() {
        holder.execute("commit").unwrap();
    } else {
        holder.execute("rollback").unwrap();
    }
    let deleted = waiter.join().expect("delete worker");
    assert!(waiting, "DELETE must contend with the held record lock");
    update.expect("record owner must not wait on an index lock held by its waiter");
    deleted.expect("DELETE completes after the UPDATE commits");
    assert!(rows(&holder, "select * from indexed_wait").is_empty());
    holder
        .execute("admin check table indexed_wait")
        .expect("DELETE must remove the current index entry");
}
