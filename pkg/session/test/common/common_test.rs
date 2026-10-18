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

// `session/test/common` 通用 session 行为测试（Rust mock-store 断言）。
//
// Rust 测试用 CreateMockStoreAndDomain + analyze 行数校验可见性、
// Prepare 参数化写入、affected rows 与前缀索引元信息。
//
// Affected rows（影响行数）：DML 后 session 报告的变更行数；CLIENT_FOUND_ROWS
// 会把“命中但未修改”的行也计入。

use astersql_domain::Domain;
use astersql_testkit::mockstore::{
    CreateMockStoreAndDomain, CreateMockStoreAndDomainWithoutSystemSchemasForTest,
};
use astersql_testkit::{DbValue, TestKit};

/// 从 Domain 统计句柄读取表的 realtime_count（ANALYZE 后的可见行数近似）。
fn table_row_count(domain: &Domain, database: &str, table: &str) -> i64 {
    // InfoSchema 按库表名定位 TableInfo，再取 stats_meta 中的实时行数。
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

/// 校验 DDL/INSERT 后 ROLLBACK 不落盘，提交路径行数可见。
// 对应 TestMiscs：DDL/insert/rollback 后以 analyze 行数校验可见性。
#[test]
fn miscs_session_ddl_insert_and_rollback_are_stable() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("drop table if exists t1", Vec::new());
    tk.MustExec(
        "create table t1(id varchar(16) primary key, v int)",
        Vec::new(),
    );
    // 事务内两次 insert 后 ROLLBACK，ANALYZE 后行数应为 0。
    tk.MustExec("begin", Vec::new());
    tk.MustExec("insert into t1 values ('a', 1)", Vec::new());
    tk.MustExec("insert into t1 values ('b', 2)", Vec::new());
    tk.MustExec("ROLLBACK", Vec::new());
    tk.MustExec("analyze table t1", Vec::new());
    assert_eq!(table_row_count(&domain, "test", "t1"), 0);

    // 事务外 insert 应持久化并反映在统计行数中。
    tk.MustExec("insert into t1 values ('a', 1)", Vec::new());
    tk.MustExec("analyze table t1", Vec::new());
    assert_eq!(table_row_count(&domain, "test", "t1"), 1);
}

/// 校验参数化 INSERT 走绑定路径后行数可见。
// 对应 TestPrepare：参数化 INSERT 走 PrepareStmt 绑定路径，行数可见。
#[test]
fn prepare_parameterized_insert_match_go_prepare_execute_shape() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec(
        "create table multiexec (a int, b int, primary key(b))",
        Vec::new(),
    );
    // 两次参数化 insert，对应 Go ExecutePreparedStmt 多轮执行形状。
    tk.MustExec(
        "insert into multiexec values (?, ?)",
        vec![DbValue::I64(1), DbValue::I64(1)],
    );
    tk.MustExec(
        "insert into multiexec values (?, ?)",
        vec![DbValue::I64(2), DbValue::I64(2)],
    );
    tk.MustExec("analyze table multiexec", Vec::new());
    assert_eq!(table_row_count(&domain, "test", "multiexec"), 2);
}

/// 校验 INSERT/UPDATE/DELETE 的 affected_rows 与最终可见行数一致。
// 对应 TestTableInfoMeta：INSERT/UPDATE/DELETE 后 affected_rows 与最终行数一致。
#[test]
fn table_info_meta_tracks_affected_rows_and_visible_data() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec(
        "CREATE TABLE tbl_test(id INT NOT NULL, name varchar(255), PRIMARY KEY(id))",
        Vec::new(),
    );
    tk.MustExec("INSERT INTO tbl_test VALUES (1, 'hello')", Vec::new());
    tk.CheckExecResult(1, 0);
    tk.MustExec("INSERT INTO tbl_test VALUES (2, 'hello')", Vec::new());
    tk.CheckExecResult(1, 0);
    tk.MustExec("UPDATE tbl_test SET name = 'abc' where id = 2", Vec::new());
    tk.CheckExecResult(1, 0);
    tk.MustExec("DELETE from tbl_test where id = 2", Vec::new());
    tk.CheckExecResult(1, 0);
    tk.MustExec("analyze table tbl_test", Vec::new());
    assert_eq!(table_row_count(&domain, "test", "tbl_test"), 1);
    // 列元信息应能通过 Domain InfoSchema 读到 name 列。
    let table = domain.table_by_name("test", "tbl_test").expect("tbl_test");
    assert!(table.Columns.iter().any(|c| c.Name.L == "name"));
}

/// 校验匹配更新 affected_rows=1，无匹配更新为 0。
// 对应 TestAffectedRows：匹配更新 affected_rows=1；无匹配更新为 0。
#[test]
fn affected_rows_reflect_matched_and_unchanged_updates() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("create table t(id varchar(8) primary key)", Vec::new());
    tk.MustExec("INSERT INTO t VALUES ('a')", Vec::new());
    tk.CheckExecResult(1, 0);
    tk.MustExec("INSERT INTO t VALUES ('b')", Vec::new());
    tk.CheckExecResult(1, 0);
    // 命中并修改：affected_rows=1。
    tk.MustExec("UPDATE t set id = 'c' where id = 'a'", Vec::new());
    tk.CheckExecResult(1, 0);
    // 条件无命中：affected_rows=0。
    tk.MustExec("UPDATE t set id = 'a' where id = 'a'", Vec::new());
    tk.CheckExecResult(0, 0);
    tk.MustExec("analyze table t", Vec::new());
    assert_eq!(table_row_count(&domain, "test", "t"), 2);
}

/// 对应 Go `TestLastMessage` 的 INSERT 分支，直接从真实 session 读取 OK packet 消息。
#[test]
fn last_message_matches_go_for_single_and_multi_row_insert() {
    let (store, _domain) = CreateMockStoreAndDomainWithoutSystemSchemasForTest();
    let mut tk = TestKit::new(store);
    tk.MustExec("create table t(id varchar(8) primary key)", Vec::new());

    tk.MustExec("INSERT INTO t VALUES ('a')", Vec::new());
    assert_eq!(tk.Session().LastMessage(), "");
    tk.MustExec("INSERT INTO t VALUES ('b'), ('c')", Vec::new());
    assert_eq!(
        tk.Session().LastMessage(),
        "Records: 2  Duplicates: 0  Warnings: 0"
    );
}

/// 对应 Go `TestLastMessage` 的 UPDATE 分支，覆盖命中修改和条件无命中。
#[test]
fn last_message_matches_go_for_changed_and_unmatched_update() {
    let (store, _domain) = CreateMockStoreAndDomainWithoutSystemSchemasForTest();
    let mut tk = TestKit::new(store);
    tk.MustExec("create table t(id varchar(8) primary key)", Vec::new());
    tk.MustExec("INSERT INTO t VALUES ('a')", Vec::new());
    tk.MustExec("UPDATE t set id = 'd' where id = 'a'", Vec::new());
    tk.CheckExecResult(1, 0);
    assert_eq!(
        tk.Session().LastMessage(),
        "Rows matched: 1  Changed: 1  Warnings: 0"
    );
    tk.MustExec("UPDATE t set id = 'a' where id = 'a'", Vec::new());
    tk.CheckExecResult(0, 0);
    assert_eq!(
        tk.Session().LastMessage(),
        "Rows matched: 0  Changed: 0  Warnings: 0"
    );
}

/// 对应 Go `TestLastMessage` 的 REPLACE 和 INSERT SELECT upsert 分支。
#[test]
fn last_message_matches_go_for_replace_select_and_upsert() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("create table t (c1 int primary key, c2 int)", Vec::new());
    tk.MustExec("create table t1 (a1 int, a2 int)", Vec::new());
    tk.MustExec("INSERT INTO t VALUES (1, 1)", Vec::new());
    tk.MustExec("REPLACE INTO t VALUES (2, 2)", Vec::new());
    assert_eq!(tk.Session().LastMessage(), "");
    tk.MustExec("INSERT INTO t1 VALUES (1, 10), (3, 30)", Vec::new());
    assert_eq!(
        tk.Session().LastMessage(),
        "Records: 2  Duplicates: 0  Warnings: 0"
    );
    tk.MustExec("REPLACE INTO t SELECT * from t1", Vec::new());
    assert_eq!(
        tk.Session().LastMessage(),
        "Records: 2  Duplicates: 1  Warnings: 0"
    );

    tk.Session()
        .SetClientCapability(astersql_parser_mysql::r#const::ClientFoundRows)
        .expect("set CLIENT_FOUND_ROWS");
    tk.MustExec("drop table t", Vec::new());
    tk.MustExec("create table t (c1 int primary key, c2 int)", Vec::new());
    tk.MustExec("delete from t1", Vec::new());
    tk.MustExec("INSERT INTO t1 VALUES (1, 10), (2, 2), (3, 30)", Vec::new());
    tk.MustExec(
        "INSERT INTO t1 VALUES (1, 10), (2, 20), (3, 30)",
        Vec::new(),
    );
    tk.MustExec(
        "INSERT INTO t SELECT * FROM t1 ON DUPLICATE KEY UPDATE c2=a2",
        Vec::new(),
    );
    assert_eq!(
        tk.Session().LastMessage(),
        "Records: 6  Duplicates: 3  Warnings: 0"
    );
}

/// 对应 Go `TestAffectedRows` 的 upsert 分支：新增、实际更新和 no-op
/// 分别报告 1、2、0；重复执行表达式更新持续报告 2。
#[test]
fn affected_rows_match_go_for_duplicate_key_updates() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("create table t (c1 int primary key, c2 int)", Vec::new());
    tk.MustExec("insert t values(1, 1)", Vec::new());
    tk.CheckExecResult(1, 0);
    tk.MustExec(
        "insert into t values (1, 1) on duplicate key update c2=2",
        Vec::new(),
    );
    tk.CheckExecResult(2, 0);
    tk.MustExec(
        "insert into t values (1, 1) on duplicate key update c2=2",
        Vec::new(),
    );
    tk.CheckExecResult(0, 0);

    tk.MustExec(
        "create table factor_test (\
         id varchar(36) primary key not null,\
         factor int not null default 2)",
        Vec::new(),
    );
    let insert_sql = "INSERT INTO factor_test(id) VALUES('id') \
                      ON DUPLICATE KEY UPDATE factor=factor+3";
    tk.MustExec(insert_sql, Vec::new());
    tk.CheckExecResult(1, 0);
    tk.MustExec(insert_sql, Vec::new());
    tk.CheckExecResult(2, 0);
    tk.MustExec(insert_sql, Vec::new());
    tk.CheckExecResult(2, 0);
}

/// 对应 Go `TestAffectedRows` 的 `CLIENT_FOUND_ROWS` 分支：命中两行时，
/// 即使其中一行保持原值，也按匹配行数报告 affected rows。
#[test]
fn affected_rows_include_matched_rows_with_client_found_rows() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("create table t (id int, data int)", Vec::new());
    tk.MustExec("INSERT INTO t VALUES (1, 0), (0, 0), (1, 1)", Vec::new());
    tk.Session()
        .SetClientCapability(astersql_parser_mysql::r#const::ClientFoundRows)
        .expect("set CLIENT_FOUND_ROWS");
    tk.MustExec("UPDATE t set id = 1 where data = 0", Vec::new());
    tk.CheckExecResult(2, 0);
}

/// 对应 Go `TestQueryString`：多语句、二进制协议 prepared DDL 和 SQL
/// PREPARE/EXECUTE 都应留下真正执行的 DDL 文本。
#[test]
fn query_string_tracks_multi_statement_and_prepared_ddl() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec(
        "create table mutil1 (a int);create table multi2 (a int)",
        Vec::new(),
    );
    assert_eq!(tk.Session().QueryString(), "create table multi2 (a int)");

    tk.MustExec(
        "CREATE TABLE t (id bigint PRIMARY KEY, age int)",
        Vec::new(),
    );
    let prepared_ddl = "CREATE TABLE t2(id bigint PRIMARY KEY, age int)";
    let (statement_id, fields) = tk.Session().PrepareStmt(prepared_ddl).expect("prepare DDL");
    assert!(fields.is_empty());
    tk.Session()
        .ExecutePreparedStmt(statement_id, &[])
        .expect("execute prepared DDL");
    assert_eq!(tk.Session().QueryString(), prepared_ddl);
    tk.Session()
        .DropPreparedStmt(statement_id)
        .expect("drop prepared DDL");

    tk.MustExec("drop table t2", Vec::new());
    tk.MustExec(
        "prepare stmt from 'CREATE TABLE t2(id bigint PRIMARY KEY, age int)'",
        Vec::new(),
    );
    tk.MustExec("execute stmt", Vec::new());
    assert_eq!(tk.Session().QueryString(), prepared_ddl);
}

/// 校验普通索引 Length 未指定、BLOB 前缀索引 Length=6 写入 InfoSchema。
// 对应 TestIndexColumnLength：建表时声明前缀索引，长度进入 domain InfoSchema。
#[test]
fn index_column_length_is_visible_on_blob_prefix_index() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec(
        "create table t (c1 int, c2 blob, key idx_c1(c1), key idx_c2(c2(6)))",
        Vec::new(),
    );

    let table = domain
        .table_by_name("test", "t")
        .unwrap_or_else(|error| panic!("table lookup: {error}"));
    let idx_c1 = table
        .Indices
        .iter()
        .find(|idx| idx.Name.L == "idx_c1")
        .expect("idx_c1");
    let idx_c2 = table
        .Indices
        .iter()
        .find(|idx| idx.Name.L == "idx_c2")
        .expect("idx_c2");
    // 全列索引 Length 为 UnspecifiedLength（负值或 -1）；前缀索引为 6。
    assert!(
        idx_c1.Columns[0].Length < 0 || idx_c1.Columns[0].Length == -1,
        "full column index length={}",
        idx_c1.Columns[0].Length
    );
    assert_eq!(idx_c2.Columns[0].Length, 6);
}

/// Covers the Go `TestPrepare` protocol path instead of merely exercising a
/// parameterized insert: metadata, binding, result rows, and statement cleanup
/// are all observable from the canonical test session.
#[test]
fn protocol_prepare_executes_bound_select_and_releases_statement() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("create table t(id varchar(16) primary key)", Vec::new());
    tk.MustExec("insert into t values ('id')", Vec::new());

    let session = tk.Session();
    let (statement_id, fields) = session
        .PrepareStmt("select id from t where id = ?")
        .expect("prepare protocol statement");
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].column_name, "id");
    assert_eq!(
        tk.Prepare("select id from t where id = ?")
            .query(&[DbValue::String("id".into())])
            .expect("execute bound statement")
            .string_rows(),
        vec![vec!["id".to_owned()]],
    );
    session
        .DropPreparedStmt(statement_id)
        .expect("drop protocol statement");
}

/// The original misc case closes an explicitly rolled-back session.  Retain
/// that resource boundary on the real TestKit session rather than replacing it
/// with a statistics-only scenario.
#[test]
fn rolled_back_session_closes_cleanly() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("begin", Vec::new());
    tk.MustExec("rollback", Vec::new());
    tk.Session().close().expect("close rolled-back session");
}
