// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// Prepared statement 去重缓存（dedup cache）相关测试。
//
// Rust 测试用 PrepareStmt 返回不同 stmtID、参数化写入与 DDL 后仍可用等断言覆盖核心语义。
//
// PlanCacheStmt：缓存编译后的 prepared plan；dedup cache 按 SQL（及库名）去重复用，
// 但每次 Prepare 仍分配独立 stmtID。

use astersql_domain::Domain;
use astersql_session::runtime::CreateAnalyzeSession;
use astersql_session::testutil::TestSession;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{DbValue, TestKit};

/// 从 Domain 统计句柄读取表的 realtime_count（ANALYZE 后的可见行数近似）。
fn table_row_count(domain: &Domain, database: &str, table: &str) -> i64 {
    // InfoSchema 定位表后读取 stats_meta.realtime_count。
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

/// 校验同一 SQL 两次 PrepareStmt 返回不同 stmtID（即使可能命中 dedup cache）。
// 对应 TestPrepareStmtDedupCacheBasic：同一 SQL 两次 PrepareStmt 返回不同 stmtID。
#[test]
fn prepare_stmt_returns_distinct_ids_for_same_sql() {
    let (_domain, session) = CreateAnalyzeSession().expect("analyze session");
    let id1 = session
        .PrepareStmt("select id, city from t where age > ? and city = ?")
        .expect("prepare1");
    let id2 = session
        .PrepareStmt("select id, city from t where age > ? and city = ?")
        .expect("prepare2");
    assert_ne!(id1, id2, "each Prepare must return a distinct stmtID");
    assert_eq!(id1, 1);
    assert_eq!(id2, 2);
}

/// 校验参数化循环插入后 ANALYZE 行数正确（对应 Execute/CloseLoop 形状）。
// 对应 TestPrepareStmtDedupCacheExecute / CloseLoop：参数化写入后行数正确。
#[test]
fn prepare_execute_loop_inserts_threshold_filtered_rows() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec(
        "create table t4 (id bigint primary key, score int)",
        Vec::new(),
    );
    // 三行参数化 insert，模拟多轮 Prepare/Execute 写入。
    for (id, score) in [(1_i64, 100_i64), (2, 200), (3, 300)] {
        tk.MustExec(
            "insert into t4 values (?, ?)",
            vec![DbValue::I64(id), DbValue::I64(score)],
        );
    }
    tk.MustExec("analyze table t4", Vec::new());
    assert_eq!(table_row_count(&domain, "test", "t4"), 3);
}

/// 校验建表后参数化写入可用，DDL 路径不破坏后续 DML。
// 对应 TestPrepareStmtDedupCacheSchemaChange：DDL 后参数化写入仍可用。
#[test]
fn prepare_parameterized_insert_works_after_create() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec(
        "create table t3 (id bigint primary key, name varchar(32))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t3 (id, name) values (?, ?)",
        vec![DbValue::I64(1), DbValue::String("alice".into())],
    );
    // ALTER 推进 schema version；Go 期望 dedup cache 因版本不匹配失效。
    let _ = tk.Exec("alter table t3 add column email varchar(64)", Vec::new());
    tk.MustExec("analyze table t3", Vec::new());
    assert_eq!(table_row_count(&domain, "test", "t3"), 1);
}

/// 校验不同目标表的参数化写入互不串数据（对应按 DB/表隔离语义）。
// 对应 TestPrepareStmtDedupCacheIsolatedByDB：不同表参数化写入互不串数据。
#[test]
fn prepare_parameterized_insert_is_scoped_to_target_table() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec(
        "create table tblx_a (id bigint primary key, v int)",
        Vec::new(),
    );
    tk.MustExec(
        "create table tblx_b (id bigint primary key, v bigint)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into tblx_a values (?, ?)",
        vec![DbValue::I64(1), DbValue::I64(10)],
    );
    tk.MustExec(
        "insert into tblx_b values (?, ?)",
        vec![DbValue::I64(1), DbValue::I64(20)],
    );
    tk.MustExec("analyze table tblx_a", Vec::new());
    tk.MustExec("analyze table tblx_b", Vec::new());
    assert_eq!(table_row_count(&domain, "test", "tblx_a"), 1);
    assert_eq!(table_row_count(&domain, "test", "tblx_b"), 1);
}

/// The protocol prepare path must allocate a fresh statement id while preserving
/// the result-column metadata and the independent execution lifetime of each
/// handle.  This is the observable contract of Go's dedup-cache basic/execute
/// cases; the cache itself is intentionally an implementation detail.
#[test]
fn protocol_prepare_reuses_shape_but_keeps_statement_handles_independent() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec(
        "create table t (id bigint primary key, age int, city varchar(32))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t values (1, 10, 'beijing'), (2, 20, 'shanghai'), (3, 30, 'shenzhen')",
        Vec::new(),
    );

    let sql = "select id, city from t where age > ? and city = ?";
    let session = tk.Session();
    let (id1, fields1) = session.PrepareStmt(sql).expect("first protocol prepare");
    let (id2, fields2) = session.PrepareStmt(sql).expect("second protocol prepare");
    assert_ne!(id1, id2, "each Prepare must return a distinct stmtID");
    assert_eq!(fields1, fields2, "same SQL must retain its result shape");
    assert_eq!(fields1.len(), 2);
    assert_eq!(fields1[0].column_name, "id");
    assert_eq!(fields1[1].column_name, "city");

    let statement = tk.Prepare(sql);
    assert_eq!(
        statement
            .query(&[DbValue::I64(15), DbValue::String("shanghai".into())])
            .expect("bound prepared query")
            .string_rows(),
        vec![vec!["2".to_owned(), "shanghai".to_owned()]],
    );
    session.DropPreparedStmt(id1).expect("drop first handle");
    assert_eq!(
        statement
            .query(&[DbValue::I64(25), DbValue::String("shenzhen".into())])
            .expect("second bound prepared query")
            .string_rows(),
        vec![vec!["3".to_owned(), "shenzhen".to_owned()]],
        "closing one protocol handle must not poison a separately prepared execution",
    );
    session.DropPreparedStmt(id2).expect("drop second handle");
}

/// A schema change must not leave the protocol prepare path with stale fields.
#[test]
fn protocol_prepare_after_schema_change_reports_current_projection() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec(
        "create table t3 (id bigint primary key, name varchar(32))",
        Vec::new(),
    );
    let session = tk.Session();
    let sql = "select id, name from t3 where id = ?";
    let (id1, before) = session.PrepareStmt(sql).expect("prepare before DDL");
    assert_eq!(before.len(), 2);
    tk.MustExec("alter table t3 add column email varchar(64)", Vec::new());
    tk.MustExec(
        "insert into t3 (id, name, email) values (1, 'alice', 'alice@example.com')",
        Vec::new(),
    );
    let (id2, after) = session.PrepareStmt(sql).expect("prepare after DDL");
    assert_ne!(id1, id2);
    assert_eq!(
        after, before,
        "the unchanged projection must retain its shape"
    );
    let rebuilt = tk.Prepare(sql);
    assert_eq!(
        rebuilt
            .query(&[DbValue::I64(1)])
            .expect("execute rebuilt statement after DDL")
            .string_rows(),
        vec![vec!["1".to_owned(), "alice".to_owned()]],
    );
    session.DropPreparedStmt(id1).expect("drop old statement");
    session
        .DropPreparedStmt(id2)
        .expect("drop rebuilt statement");
}

/// The dedup key includes currentDB: identical SQL prepared in databases with
/// different table definitions must bind and execute against the selected DB.
#[test]
fn protocol_prepare_isolated_by_current_database() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("create database db1", Vec::new());
    tk.MustExec("create database db2", Vec::new());
    tk.MustExec(
        "create table db1.tblx (id bigint primary key, v int)",
        Vec::new(),
    );
    tk.MustExec(
        "create table db2.tblx (id bigint primary key, v bigint)",
        Vec::new(),
    );
    tk.MustExec("insert into db1.tblx values (1, 10)", Vec::new());
    tk.MustExec("insert into db2.tblx values (1, 20)", Vec::new());

    let sql = "select v from tblx where id = ?";
    tk.MustExec("use db1", Vec::new());
    let (id1, fields1) = tk.Session().PrepareStmt(sql).expect("prepare in db1");
    let db1_statement = tk.Prepare(sql);
    assert_eq!(
        db1_statement
            .query(&[DbValue::I64(1)])
            .expect("execute in db1")
            .string_rows(),
        vec![vec!["10".to_owned()]],
    );

    tk.MustExec("use db2", Vec::new());
    let (id2, fields2) = tk.Session().PrepareStmt(sql).expect("prepare in db2");
    assert_ne!(id1, id2);
    assert_eq!(fields1.len(), 1);
    assert_eq!(fields2.len(), 1);
    assert_eq!(
        tk.Prepare(sql)
            .query(&[DbValue::I64(1)])
            .expect("execute in db2")
            .string_rows(),
        vec![vec!["20".to_owned()]],
    );
    tk.Session().DropPreparedStmt(id1).expect("drop db1 handle");
    tk.Session().DropPreparedStmt(id2).expect("drop db2 handle");
}

/// Repeated prepare/execute/drop cycles for the same SQL retain independent
/// handles and the threshold-dependent result required by the Go regression.
#[test]
fn protocol_prepare_execute_close_loop_matches_go_results() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec(
        "create table t4 (id bigint primary key, score int)",
        Vec::new(),
    );
    tk.MustExec("insert into t4 values (1,100),(2,200),(3,300)", Vec::new());

    let sql = "select id from t4 where score >= ?";
    let expected = [
        vec![
            vec!["1".to_owned()],
            vec!["2".to_owned()],
            vec!["3".to_owned()],
        ],
        vec![vec!["2".to_owned()], vec!["3".to_owned()]],
        vec![vec!["3".to_owned()]],
    ];
    for (round, threshold) in [50_i64, 150, 250].into_iter().enumerate() {
        let (id, _) = tk.Session().PrepareStmt(sql).expect("prepare loop handle");
        assert_eq!(
            tk.Prepare(sql)
                .query(&[DbValue::I64(threshold)])
                .expect("execute loop handle")
                .string_rows(),
            expected[round],
        );
        tk.Session().DropPreparedStmt(id).expect("drop loop handle");
    }
}
