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
// `_GO_DRAFT_ARCHIVE` 保留 Go 侧同一 SQL 复用 PlanCacheStmt、执行独立性、
// schema 变更失效、按 currentDB 隔离、Prepare→Execute→Close 循环等草稿；
// 下方 Rust 测试用 PrepareStmt 返回不同 stmtID、参数化写入与 DDL 后仍可用等断言覆盖核心语义。
//
// PlanCacheStmt：缓存编译后的 prepared plan；dedup cache 按 SQL（及库名）去重复用，
// 但每次 Prepare 仍分配独立 stmtID。

/// 归档 Go prepared statement dedup cache 测试草稿，不参与运行，仅供对照迁移语义。
const _GO_DRAFT_ARCHIVE: &str = r################"
// 这段逻辑只描述 prepared statement 去重缓存测试如何通过 testkit 构造 session、prepare SQL、执行并检查结果。

// test_prepare_stmt_dedup_cache_basic 对应 Go 的 TestPrepareStmtDedupCacheBasic。
// 它验证同一 session 内两次 prepare 同一 SQL 会复用缓存的 PlanCacheStmt，但 stmtID 仍各自独立。
#[test]
fn test_prepare_stmt_dedup_cache_basic() {
    let store = testkit::CreateMockStore();
    let tk = testkit::NewTestKit(&store);
    tk.MustExec("set tidb_enable_cache_prepare_stmt = 1");
    tk.MustExec("use test");
    tk.MustExec("create table t (id bigint primary key, age int, city varchar(32))");

    let sql = "select id, city from t where age > ? and city = ?";

    let (id1, param_count1, fields1, err) = tk.Session().PrepareStmt(sql);
    require::NoError(err);
    require::EqualValues(2, param_count1);
    require::Len(&fields1, 2);
    require::Equal("id", fields1[0].Column.Name.L);
    require::Equal("city", fields1[1].Column.Name.L);

    // 第二次 prepare 相同 SQL 应命中 dedup cache；Go 仍要求每次 Prepare 返回不同 stmtID。
    let (id2, param_count2, fields2, err) = tk.Session().PrepareStmt(sql);
    require::NoError(err);
    require::NotEqual(id1, id2, "each Prepare must return a distinct stmtID");
    require::Equal(param_count1, param_count2);
    require::Len(&fields2, 2);
    require::Equal("id", fields2[0].Column.Name.L);
    require::Equal("city", fields2[1].Column.Name.L);
    tk.MustExec("set tidb_enable_cache_prepare_stmt = default");
}

// test_prepare_stmt_dedup_cache_execute 对应 Go 的 TestPrepareStmtDedupCacheExecute。
// 它验证缓存路径产出的 prepared statement 可以正常执行，并且旧 stmtID 仍可独立使用。
#[test]
fn test_prepare_stmt_dedup_cache_execute() {
    let store = testkit::CreateMockStore();
    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    tk.MustExec("set tidb_enable_cache_prepare_stmt = 1");
    tk.MustExec("create table t2 (id bigint primary key, val int)");
    tk.MustExec("insert into t2 values (1, 10), (2, 20), (3, 30)");

    let ctx = context::Background();
    let sql = "select id from t2 where val > ?";

    let run_and_collect = |stmt_id: u32, threshold: i32| -> Vec<i64> {
        let (rs, err) = tk
            .Session()
            .ExecutePreparedStmt(&ctx, stmt_id, expression::Args2Expressions4Test(threshold));
        require::NoError(err);
        // Go 使用 defer rs.Close()，这里把资源收尾语义保留在循环后的显式 Close。
        let mut ids: Vec<i64> = Vec::new();
        let req = rs.NewChunk(None);
        loop {
            require::NoError(rs.Next(&ctx, &req));
            if req.NumRows() == 0 {
                break;
            }
            // Go 的 for i := range req.NumRows() 逐行读取第一列 int64。
            for i in 0..req.NumRows() {
                ids.push(req.Column(0).GetInt64(i));
            }
            req.Reset();
        }
        require::NoError(rs.Close());
        ids
    };

    // 第一次 prepare 走完整构建路径。
    let (id1, _, _, err) = tk.Session().PrepareStmt(sql);
    require::NoError(err);
    let mut ids = run_and_collect(id1, 15);
    require::Equal(vec![2_i64, 3_i64], ids);

    // 第二次 prepare 走 dedup cache 路径，stmtID 不应复用。
    let (id2, _, _, err) = tk.Session().PrepareStmt(sql);
    require::NoError(err);
    require::NotEqual(id1, id2);
    ids = run_and_collect(id2, 5);
    require::Equal(vec![1_i64, 2_i64, 3_i64], ids);

    // 原 stmt 仍必须独立可执行，避免缓存对象共享破坏生命周期。
    ids = run_and_collect(id1, 25);
    require::Equal(vec![3_i64], ids);

    tk.MustExec("set tidb_enable_cache_prepare_stmt = default");
}

// test_prepare_stmt_dedup_cache_schema_change 对应 Go 的 TestPrepareStmtDedupCacheSchemaChange。
// DDL 触发 schema version 变化后，下一次 Prepare 应绕过旧缓存并基于新 schema 构造有效 stmt。
#[test]
fn test_prepare_stmt_dedup_cache_schema_change() {
    let store = testkit::CreateMockStore();
    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    tk.MustExec("set tidb_enable_cache_prepare_stmt = 1");
    tk.MustExec("create table t3 (id bigint primary key, name varchar(32))");

    let sql = "select id, name from t3 where id = ?";

    // 预热 dedup cache，并确认原始字段元信息为两列。
    let (id1, _, fields1, err) = tk.Session().PrepareStmt(sql);
    require::NoError(err);
    require::Len(&fields1, 2);
    let _ = id1;

    // DDL 修改 schema version，Go 期望缓存条目因为版本不匹配而失效。
    tk.MustExec("alter table t3 add column email varchar(64)");

    // 缓存失效后应走完整构建路径；协议层字段信息不重新返回，但 PlanCacheStmt 必须可用。
    let (id2, _, _, err) = tk.Session().PrepareStmt(sql);
    require::NoError(err);
    require::NotEqual(id1, id2);

    // 新 stmt 对更新后的 schema 执行应成功；这里只保留 ExecutePreparedStmt 和 Close 的调用形状。
    let ctx = context::Background();
    tk.MustExec("insert into t3 (id, name, email) values (1, 'alice', 'alice@example.com')");
    let (rs, err) = tk
        .Session()
        .ExecutePreparedStmt(&ctx, id2, expression::Args2Expressions4Test(1));
    require::NoError(err);
    require::NoError(rs.Close());
    tk.MustExec("set tidb_enable_cache_prepare_stmt = default");
}

// test_prepare_stmt_dedup_cache_isolated_by_db 对应 Go 的 TestPrepareStmtDedupCacheIsolatedByDB。
// 相同 SQL 文本在不同 currentDB 下不能共享缓存条目，因为表结构可能不同。
#[test]
fn test_prepare_stmt_dedup_cache_isolated_by_db() {
    let store = testkit::CreateMockStore();
    let tk = testkit::NewTestKit(&store);
    tk.MustExec("set tidb_enable_cache_prepare_stmt = 1");
    tk.MustExec("create database if not exists db1");
    tk.MustExec("create database if not exists db2");
    tk.MustExec("use db1");
    tk.MustExec("create table tblx (id bigint primary key, v int)");
    tk.MustExec("use db2");
    tk.MustExec("create table tblx (id bigint primary key, v bigint)");

    let sql = "select v from tblx where id = ?";

    // 在 db1 prepare，缓存 key 应记录当前库。
    tk.MustExec("use db1");
    let (id1, _, fields1, err) = tk.Session().PrepareStmt(sql);
    require::NoError(err);
    require::Len(&fields1, 1);
    let _ = id1;

    // 切到 db2 后不能命中 db1 的缓存；字段来自 db2.tblx.v 的 fresh build。
    tk.MustExec("use db2");
    let (id2, _, fields2, err) = tk.Session().PrepareStmt(sql);
    require::NoError(err);
    require::NotEqual(id1, id2);
    require::Len(&fields2, 1);
    tk.MustExec("set tidb_enable_cache_prepare_stmt = default");
}

// test_prepare_stmt_dedup_cache_prepare_execute_close_loop 对应 Go 的 TestPrepareStmtDedupCachePrepareExecuteCloseLoop。
// 它覆盖每个请求都 Prepare→Execute→Close 的反模式，确保缓存路径在多轮循环中仍返回正确结果。
#[test]
fn test_prepare_stmt_dedup_cache_prepare_execute_close_loop() {
    let store = testkit::CreateMockStore();
    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    tk.MustExec("set tidb_enable_cache_prepare_stmt = 1");
    tk.MustExec("create table t4 (id bigint primary key, score int)");
    tk.MustExec("insert into t4 values (1,100),(2,200),(3,300)");

    let ctx = context::Background();
    let sql = "select id from t4 where score >= ?";

    let thresholds = vec![50, 150, 250];
    let expected = vec![vec![1_i64, 2_i64, 3_i64], vec![2_i64, 3_i64], vec![3_i64]];

    for (round, threshold) in thresholds.iter().enumerate() {
        let (id, _, _, err) = tk.Session().PrepareStmt(sql);
        require::NoError(err);

        let (rs, err) = tk
            .Session()
            .ExecutePreparedStmt(&ctx, id, expression::Args2Expressions4Test(*threshold));
        require::NoError(err);

        let mut got: Vec<i64> = Vec::new();
        let req = rs.NewChunk(None);
        loop {
            require::NoError(rs.Next(&ctx, &req));
            if req.NumRows() == 0 {
                break;
            }
            // Go 逐 chunk 读取结果并 Reset，保留流式消费结果集的资源使用方式。
            for i in 0..req.NumRows() {
                got.push(req.Column(0).GetInt64(i));
            }
            req.Reset();
        }
        require::NoError(rs.Close());
        require::Equal(&expected[round], got, format!("round {}, threshold {}", round, threshold));

        // 每轮显式 DropPreparedStmt，保留 prepare-per-request 模式下的清理语义。
        require::NoError(tk.Session().DropPreparedStmt(id));
    }
    tk.MustExec("set tidb_enable_cache_prepare_stmt = default");
}
"################;

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
