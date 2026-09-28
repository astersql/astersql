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

// `session/test/common` 通用 session 行为测试（Go 草稿 + Rust mock-store 断言）。
//
// `_GO_DRAFT_ARCHIVE` 保留 Go 侧 TestMiscs / TestPrepare / TestIndexColumnLength /
// TestTableInfoMeta / TestLastMessage / TestQueryString / TestAffectedRows 草稿；
// 下方 Rust 测试用 CreateMockStoreAndDomain + analyze 行数校验可见性、
// Prepare 参数化写入、affected rows 与前缀索引元信息。
//
// Affected rows（影响行数）：DML 后 session 报告的变更行数；CLIENT_FOUND_ROWS
// 会把“命中但未修改”的行也计入。

/// 归档 Go common session 测试草稿，不参与运行，仅供对照迁移语义。
const _GO_DRAFT_ARCHIVE: &str = r################"
// 这段逻辑只描述 session common 测试对 testkit、prepared statement、InfoSchema 和 affected rows 的调用形状。

// test_miscs 对应 Go 的 TestMiscs：覆盖 Session.String、LastExecuteDDL 标记和 session 关闭路径。
#[test]
fn test_miscs() {
    let store = testkit::CreateMockStore();

    // TestString：Go 这里在事务提交后打印 Session.String，用来覆盖 txn 为 nil 时的 panic 回归。
    let mut tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    tk.MustExec("select 1");
    t::Log(tk.Session().String());

    // TestLastExecuteDDLFlag：DDL 后应能读到 LastExecuteDDL，普通 insert 后该标记应清空。
    tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t1");
    tk.MustExec("create table t1(id int)");
    require::NotNil(tk.Session().Value(sessionctx::LastExecuteDDL));
    tk.MustExec("insert into t1 values (1)");
    require::Nil(tk.Session().Value(sessionctx::LastExecuteDDL));

    // TestSession：保留 Go 中显式 ROLLBACK 后 Close 的资源收尾形状。
    tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    tk.MustExec("ROLLBACK;");
    tk.Session().Close();
}

// test_prepare 对应 Go 的 TestPrepare：覆盖 PrepareStmt、ExecutePreparedStmt、DropPreparedStmt 和 SQL PREPARE 语法。
#[test]
fn test_prepare() {
    let store = testkit::CreateMockStore();

    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    tk.MustExec("create table t(id TEXT)");
    tk.MustExec(r#"INSERT INTO t VALUES ("id");"#);
    let (mut id, ps, _, err) = tk.Session().PrepareStmt("select id+? from t");
    let ctx = context::Background();
    require::NoError(err);
    require::Equal(1_u32, id);
    require::Equal(1, ps);
    tk.MustExec("set @a=1");
    let (mut rs, err) = tk
        .Session()
        .ExecutePreparedStmt(&ctx, id, expression::Args2Expressions4Test("1"));
    require::NoError(err);
    require::NoError(rs.Close());
    let err = tk.Session().DropPreparedStmt(id);
    require::NoError(err);

    // Go 继续验证 SQL 层 prepared statement 可多次使用不同 session 变量。
    tk.MustExec("prepare stmt from 'select 1+?'");
    tk.MustExec("set @v1=100");
    tk.MustQuery("execute stmt using @v1").Check(testkit::Rows("101"));
    tk.MustExec("set @v2=200");
    tk.MustQuery("execute stmt using @v2").Check(testkit::Rows("201"));
    tk.MustExec("set @v3=300");
    tk.MustQuery("execute stmt using @v3").Check(testkit::Rows("301"));
    tk.MustExec("deallocate prepare stmt");

    // Execute prepared statements for more than one time.
    tk.MustExec("create table multiexec (a int, b int)");
    tk.MustExec("insert multiexec values (1, 1), (2, 2)");
    let (new_id, _, _, err) = tk
        .Session()
        .PrepareStmt("select a from multiexec where b = ? order by b");
    id = new_id;
    require::NoError(err);
    let (new_rs, err) = tk
        .Session()
        .ExecutePreparedStmt(&ctx, id, expression::Args2Expressions4Test(1));
    rs = new_rs;
    require::NoError(err);
    require::NoError(rs.Close());
    let (new_rs, err) = tk
        .Session()
        .ExecutePreparedStmt(&ctx, id, expression::Args2Expressions4Test(2));
    rs = new_rs;
    require::NoError(err);
    require::NoError(rs.Close());
}

// test_index_column_length 对应 Go 的 TestIndexColumnLength：检查普通索引和前缀索引的元信息长度。
#[test]
fn test_index_column_length() {
    let (store, dom) = testkit::CreateMockStoreAndDomain();

    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    tk.MustExec("create table t (c1 int, c2 blob);");
    tk.MustExec("create index idx_c1 on t(c1);");
    tk.MustExec("create index idx_c2 on t(c2(6));");

    let is = dom.InfoSchema();
    let (tab, err2) = is.TableByName(context::Background(), ast::NewCIStr("test"), ast::NewCIStr("t"));
    require::NoError(err2);

    let idx_c1_cols = tables::FindIndexByColName(tab, "c1").Meta().Columns;
    require::Equal(types::UnspecifiedLength, idx_c1_cols[0].Length);

    let idx_c2_cols = tables::FindIndexByColName(tab, "c2").Meta().Columns;
    require::Equal(6, idx_c2_cols[0].Length);
}

// test_table_info_meta 对应 Go 的 TestTableInfoMeta：校验 DML 后 AffectedRows 和 LastInsertID。
#[test]
fn test_table_info_meta() {
    let store = testkit::CreateMockStore();

    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");

    // Go 闭包读取当前 session 的 affected rows / last insert id 并与期望值比较。
    let check_result = |affected_rows: u64, insert_id: u64| {
        let got_rows = tk.Session().AffectedRows();
        require::Equal(affected_rows, got_rows);

        let got_id = tk.Session().LastInsertID();
        require::Equal(insert_id, got_id);
    };

    tk.MustExec("CREATE TABLE tbl_test(id INT NOT NULL DEFAULT 1, name varchar(255), PRIMARY KEY(id));");
    tk.MustExec(r#"INSERT INTO tbl_test VALUES (1, "hello");"#);
    check_result(1, 0);
    tk.MustExec(r#"INSERT INTO tbl_test VALUES (2, "hello");"#);
    check_result(1, 0);
    tk.MustExec(r#"UPDATE tbl_test SET name = "abc" where id = 2;"#);
    check_result(1, 0);
    tk.MustExec(r#"DELETE from tbl_test where id = 2;"#);
    check_result(1, 0);

    tk.MustQuery("select * from tbl_test").Check(testkit::Rows("1 hello"));
}

// test_last_message 对应 Go 的 TestLastMessage：检查 INSERT/UPDATE/REPLACE 以及 CLIENT_FOUND_ROWS 下的 last message。
#[test]
fn test_last_message() {
    let store = testkit::CreateMockStore();

    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t");
    tk.MustExec("create table t(id TEXT)");

    // Insert：单行 insert 不产生 records 消息，多行 insert 记录记录数、重复数和 warning 数。
    tk.MustExec(r#"INSERT INTO t VALUES ("a");"#);
    tk.CheckLastMessage("");
    tk.MustExec(r#"INSERT INTO t VALUES ("b"), ("c");"#);
    tk.CheckLastMessage("Records: 2  Duplicates: 0  Warnings: 0");

    // Update：Go 断言 affected rows 和 last message 同时符合预期。
    tk.MustExec("UPDATE t set id = 'c' where id = 'a';");
    require::Equal(1_u64, tk.Session().AffectedRows());
    tk.CheckLastMessage("Rows matched: 1  Changed: 1  Warnings: 0");
    tk.MustExec("UPDATE t set id = 'a' where id = 'a';");
    require::Equal(0_u64, tk.Session().AffectedRows());
    tk.CheckLastMessage("Rows matched: 0  Changed: 0  Warnings: 0");

    // Replace：保留 Go 中复合 SQL 和 REPLACE SELECT 的重复行消息检查。
    tk.MustExec(
        "drop table if exists t, t1;
        create table t (c1 int PRIMARY KEY, c2 int);
        create table t1 (a1 int, a2 int);",
    );
    tk.MustExec("INSERT INTO t VALUES (1,1)");
    tk.MustExec("REPLACE INTO t VALUES (2,2)");
    tk.CheckLastMessage("");
    tk.MustExec("INSERT INTO t1 VALUES (1,10), (3,30);");
    tk.CheckLastMessage("Records: 2  Duplicates: 0  Warnings: 0");
    tk.MustExec("REPLACE INTO t SELECT * from t1");
    tk.CheckLastMessage("Records: 2  Duplicates: 1  Warnings: 0");

    // CLIENT_FOUND_ROWS 会改变 affected rows 统计口径，这里只保留 Go 的 session capability 设置语义。
    tk.Session().SetClientCapability(mysql::ClientFoundRows);
    tk.MustExec(
        "drop table if exists t, t1;
        create table t (c1 int PRIMARY KEY, c2 int);
        create table t1 (a1 int, a2 int);",
    );
    tk.MustExec("INSERT INTO t1 VALUES (1, 10), (2, 2), (3, 30);");
    tk.MustExec("INSERT INTO t1 VALUES (1, 10), (2, 20), (3, 30);");
    tk.MustExec("INSERT INTO t SELECT * FROM t1 ON DUPLICATE KEY UPDATE c2=a2;");
    tk.CheckLastMessage("Records: 6  Duplicates: 3  Warnings: 0");
}

// test_query_string 对应 Go 的 TestQueryString：检查多语句和 prepared DDL 更新 session QueryString。
#[test]
fn test_query_string() {
    let store = testkit::CreateMockStore();

    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");

    tk.MustExec("create table mutil1 (a int);create table multi2 (a int)");
    let query_str = tk.Session().Value(sessionctx::QueryString);
    require::Equal("create table multi2 (a int)", query_str);

    // ExecutePreparedStmt 路径执行 DDL 时，Go 期望 QueryString 记录真实 DDL 文本。
    tk.MustExec("use test");
    tk.MustExec("CREATE TABLE t (id bigint PRIMARY KEY, age int)");
    tk.MustExec("show create table t");
    let (id, _, _, err) = tk
        .Session()
        .PrepareStmt("CREATE TABLE t2(id bigint PRIMARY KEY, age int)");
    require::NoError(err);
    let (_, err) = tk
        .Session()
        .ExecutePreparedStmt(context::Background(), id, expression::Args2Expressions4Test());
    require::NoError(err);
    let mut qs = tk.Session().Value(sessionctx::QueryString);
    require::Equal("CREATE TABLE t2(id bigint PRIMARY KEY, age int)", qs.as_string());

    // Execute 语法路径也应记录被执行的 prepared DDL。
    tk.MustExec("use test");
    tk.MustExec("drop table t2");
    tk.MustExec("prepare stmt from 'CREATE TABLE t2(id bigint PRIMARY KEY, age int)'");
    tk.MustExec("execute stmt");
    qs = tk.Session().Value(sessionctx::QueryString);
    require::Equal("CREATE TABLE t2(id bigint PRIMARY KEY, age int)", qs.as_string());
}

// test_affected_rows 对应 Go 的 TestAffectedRows：覆盖普通 DML、查询后归零、upsert 和 CLIENT_FOUND_ROWS 口径。
#[test]
fn test_affected_rows() {
    let store = testkit::CreateMockStore();

    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");

    tk.MustExec("drop table if exists t");
    tk.MustExec("create table t(id TEXT)");
    tk.MustExec(r#"INSERT INTO t VALUES ("a");"#);
    require::Equal(1, tk.Session().AffectedRows() as i32);
    tk.MustExec(r#"INSERT INTO t VALUES ("b");"#);
    require::Equal(1, tk.Session().AffectedRows() as i32);
    tk.MustExec("UPDATE t set id = 'c' where id = 'a';");
    require::Equal(1, tk.Session().AffectedRows() as i32);
    tk.MustExec("UPDATE t set id = 'a' where id = 'a';");
    require::Equal(0, tk.Session().AffectedRows() as i32);
    tk.MustQuery("SELECT * from t").Check(testkit::Rows(vec!["c", "b"]));
    require::Equal(0, tk.Session().AffectedRows() as i32);

    tk.MustExec("drop table if exists t");
    tk.MustExec("create table t (id int, data int)");
    tk.MustExec("INSERT INTO t VALUES (1, 0), (0, 0), (1, 1);");
    tk.MustExec("UPDATE t set id = 1 where data = 0;");
    require::Equal(1, tk.Session().AffectedRows() as i32);

    tk.MustExec("drop table if exists t");
    tk.MustExec("create table t (id int, c1 timestamp);");
    tk.MustExec("insert t(id) values(1);");
    tk.MustExec("UPDATE t set id = 1 where id = 1;");
    require::Equal(0, tk.Session().AffectedRows() as i32);

    // ON DUPLICATE KEY UPDATE: Go 注释说明 inserted/updated/no-op 三种行分别计为 1/2/0。
    tk.MustExec("drop table if exists t");
    tk.MustExec("create table t (c1 int PRIMARY KEY, c2 int);");
    tk.MustExec("insert t values(1, 1);");
    tk.MustExec("insert into t values (1, 1) on duplicate key update c2=2;");
    require::Equal(2, tk.Session().AffectedRows() as i32);
    tk.MustExec("insert into t values (1, 1) on duplicate key update c2=2;");
    require::Equal(0, tk.Session().AffectedRows() as i32);
    tk.MustExec("drop table if exists test");
    let create_sql = "CREATE TABLE test (
      id        VARCHAR(36) PRIMARY KEY NOT NULL,
      factor    INTEGER                 NOT NULL                   DEFAULT 2);";
    tk.MustExec(create_sql);
    let insert_sql = "INSERT INTO test(id) VALUES('id') ON DUPLICATE KEY UPDATE factor=factor+3;";
    tk.MustExec(insert_sql);
    require::Equal(1, tk.Session().AffectedRows() as i32);
    tk.MustExec(insert_sql);
    require::Equal(2, tk.Session().AffectedRows() as i32);
    tk.MustExec(insert_sql);
    require::Equal(2, tk.Session().AffectedRows() as i32);

    // CLIENT_FOUND_ROWS 下 UPDATE 命中的行数与是否实际修改分离，Go 期望 affected rows 为 2。
    tk.Session().SetClientCapability(mysql::ClientFoundRows);
    tk.MustExec("drop table if exists t");
    tk.MustExec("create table t (id int, data int)");
    tk.MustExec("INSERT INTO t VALUES (1, 0), (0, 0), (1, 1);");
    tk.MustExec("UPDATE t set id = 1 where data = 0;");
    require::Equal(2, tk.Session().AffectedRows() as i32);
}
"################;

use astersql_domain::Domain;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
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
    let (store, _domain) = CreateMockStoreAndDomain();
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
    let (store, _domain) = CreateMockStoreAndDomain();
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
