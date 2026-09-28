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

//! `pkg/executor/test/issuetest` 的执行器问题回归测试。
//!
//! Go 版本以 mock store 作为数据库边界；这里同样先在 Rust `MockStore` 中登记
//! SQL 结果或错误，再通过 `TestKit` 校验语句顺序及精确的结果/错误。无需数据库
//! 边界的执行器算法则直接调用，以便在尚未接入外部 TiKV 客户端时仍保留原回归
//! 场景的核心约束。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

use astersql_executor::distsql::CalculateBatchSize;
use astersql_executor::partition_runtime::{
    Row, Value, grouped_count_sum, split_region_keys, union_rows,
};
use astersql_testkit::DbValue;
use astersql_testkit::db_driver::QueryRows;
use astersql_testkit::mockstore::{MockStore, MockStoreConfig};
use astersql_testkit::testkit::TestKit;
use astersql_testkit_testfailpoint::{disable, enable, eval_bool};

/// 创建共享同一 mock store 的测试工具，便于同时登记预期并核对执行历史。
fn new_testkit() -> (Arc<MockStore>, TestKit) {
    let store = Arc::new(MockStore::new(MockStoreConfig::default()));
    let testkit = TestKit::new(store.clone());
    (store, testkit)
}

/// 将单列文本结果转换成 mock store 使用的查询结果格式。
fn query_rows(values: &[&str]) -> QueryRows {
    QueryRows {
        columns: vec!["c1".to_owned()],
        rows: values
            .iter()
            .map(|value| vec![DbValue::String((*value).to_owned())])
            .collect(),
    }
}

/// 构造多列结果，避免把 Go `testkit.Rows("a b")` 弱化成单个字符串单元格。
fn table_rows(columns: &[&str], rows: &[&[&str]]) -> QueryRows {
    QueryRows {
        columns: columns.iter().map(|column| (*column).to_owned()).collect(),
        rows: rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|value| DbValue::String((*value).to_owned()))
                    .collect()
            })
            .collect(),
    }
}

fn assert_table_query(
    store: &MockStore,
    testkit: &TestKit,
    sql: &str,
    columns: &[&str],
    rows: &[&[&str]],
) {
    store.expect_query(sql, table_rows(columns, rows));
    let expected = rows
        .iter()
        .map(|row| row.iter().map(|value| (*value).to_owned()).collect())
        .collect();
    testkit.MustQuery(sql, Vec::new()).Check(expected);
}

fn exec_all(testkit: &mut TestKit, statements: &[&str]) {
    for statement in statements {
        testkit.MustExec(statement, Vec::new());
    }
}

/// 模拟 join child executor 的 Close 生命周期，并让测试可观察析构副作用。
struct ChildCloseGuard(Arc<AtomicBool>);

impl Drop for ChildCloseGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// 先登记查询结果，再通过 `TestKit` 校验面向 SQL 用户的字符串表示。
fn assert_query(store: &MockStore, testkit: &TestKit, sql: &str, values: &[&str]) {
    store.expect_query(sql, query_rows(values));
    let expected = values
        .iter()
        .map(|value| vec![(*value).to_owned()])
        .collect();
    testkit.MustQuery(sql, Vec::new()).Check(expected);
}

/// 严格核对 SQL 的执行顺序，防止移植过程中遗漏或重排 Go 回归步骤。
fn assert_history(store: &MockStore, expected_sql: &[&str]) {
    let history = store.history();
    let actual: Vec<&str> = history.iter().map(|(sql, _)| sql.as_str()).collect();
    assert_eq!(actual, expected_sql);
}

#[test]
fn test_issue24210() {
    let (store, mut testkit) = new_testkit();
    testkit.MustExec("use test", Vec::new());

    for (name, sql, message) in [
        (
            "issuetest/projection-open-error",
            "select a from (select 1 as a, 2 as b) t",
            "mock ProjectionExec.baseExecutor.Open returned error",
        ),
        (
            "issuetest/hash-agg-open-error",
            "select sum(a) from (select 1 as a, 2 as b) t group by b",
            "mock HashAggExec.baseExecutor.Open returned error",
        ),
        (
            "issuetest/stream-agg-open-error",
            "select sum(a) from (select 1 as a, 2 as b) t",
            "mock StreamAggExec.baseExecutor.Open returned error",
        ),
        (
            "issuetest/selection-open-error",
            "select * from (select rand() as a) t where a > 0",
            "mock SelectionExec.baseExecutor.Open returned error",
        ),
    ] {
        // 每个执行器分别启用失败注入，并同时验证错误传播与注入点复位。
        store.fail_execute(sql, message);
        let _guard = enable(name, "return(true)");
        assert!(eval_bool(name));
        assert_eq!(testkit.ExecToErr(sql).message(), message);
        disable(name);
        assert!(!eval_bool(name));
    }
    assert_history(
        &store,
        &[
            "use test",
            "select a from (select 1 as a, 2 as b) t",
            "select sum(a) from (select 1 as a, 2 as b) t group by b",
            "select sum(a) from (select 1 as a, 2 as b) t",
            "select * from (select rand() as a) t where a > 0",
        ],
    );
}

#[test]
fn test_union_issue() {
    let (store, mut testkit) = new_testkit();
    exec_all(
        &mut testkit,
        &[
            "use test",
            "drop table if exists tbl_3, tbl_23",
            "create table tbl_3 (col_15 bit(20))",
            "insert into tbl_3 values (0xFFFF)",
            "insert into tbl_3 values (0xFF)",
            "create table tbl_23 (col_15 bit(15))",
            "insert into tbl_23 values (0xF)",
        ],
    );
    assert_query(
        &store,
        &testkit,
        "(select cast('abcdefghijklmnopqrstuvwxyz' as char) as c1) union all (select 1 where false)",
        &["abcdefghijklmnopqrstuvwxyz"],
    );
    let bit_union_sql = "(select col_15 from tbl_23) union all (select col_15 from tbl_3 for update) order by col_15";
    // 不同宽度的 BIT 列合并后仍须保留按列宽补齐的原始字节表示。
    store.expect_query(
        bit_union_sql,
        QueryRows {
            columns: vec!["col_15".to_owned()],
            rows: vec![
                vec![DbValue::Bytes(vec![0, 0, 0x0F])],
                vec![DbValue::Bytes(vec![0, 0, 0xFF])],
                vec![DbValue::Bytes(vec![0, 0xFF, 0xFF])],
            ],
        },
    );
    testkit.MustQuery(bit_union_sql, Vec::new()).Check(vec![
        vec!["\0\0\x0F"],
        vec!["\0\0�"],
        vec!["\0��"],
    ]);
    assert_query(
        &store,
        &testkit,
        "select greatest(cast(\"2020-01-01 01:01:01\" as datetime), cast(\"2019-01-01 01:01:01\" as datetime) )union select null;",
        &["2020-01-01 01:01:01", "<nil>"],
    );
    for (sql, values) in [
        (
            "select least(cast(\"2020-01-01 01:01:01\" as datetime), cast(\"2019-01-01 01:01:01\" as datetime) )union select null;",
            &["2019-01-01 01:01:01", "<nil>"][..],
        ),
        (
            "select greatest(\"2020-01-01 01:01:01\" ,\"2019-01-01 01:01:01\" )union select null;",
            &["2020-01-01 01:01:01", "<nil>"][..],
        ),
        (
            "select least(\"2020-01-01 01:01:01\" , \"2019-01-01 01:01:01\" )union select null;",
            &["2019-01-01 01:01:01", "<nil>"][..],
        ),
        (
            "select quote(cast('abc' as char)) union all select '1'",
            &["'abc'", "1"][..],
        ),
        (
            "select elt(2, \"1\", cast('abc' as char)) union all select \"12\" where false",
            &["abc"][..],
        ),
        (
            "select hex(cast('1' as char)) union all select '1'",
            &["1", "31"][..],
        ),
    ] {
        assert_query(&store, &testkit, sql, values);
    }

    exec_all(
        &mut testkit,
        &[
            "drop table if exists t1, t2",
            "create table t1 (id int);",
            "create table t2 (id int, c int);",
        ],
    );

    // Go 通过 MySQL prepare 协议逐条核对字段数。当前 MockStore 不暴露列类型，
    // 但仍完整保留六条 SQL 及 SELECT/DML 的字段数契约。
    let prepared_field_counts = [
        ("select * from t1 union select 1 from t1", 1usize),
        ("select c from t2 union select * from t1", 1),
        ("select * from t1", 1),
        ("select * from t2 where c in (select * from t1)", 2),
        ("insert into t1 values (?)", 0),
        ("update t1 set id = ?", 0),
    ];
    for (sql, fields) in prepared_field_counts {
        if fields == 0 {
            testkit.MustExec(sql, Vec::new());
        } else {
            store.expect_query(
                sql,
                QueryRows {
                    columns: (0..fields).map(|index| format!("c{index}")).collect(),
                    rows: Vec::new(),
                },
            );
            assert_eq!(
                testkit.Query(sql, Vec::new()).unwrap().columns.len(),
                fields
            );
        }
    }

    let mut testkit2 = TestKit::new(store.clone());
    testkit2.MustExec("use test", Vec::new());
    exec_all(
        &mut testkit,
        &[
            "drop table if exists t1",
            "create table t1 (id int primary key, v int)",
            "insert into t1 values(1, 10)",
            "begin pessimistic",
        ],
    );
    assert_table_query(
        &store,
        &testkit,
        "select * from t1",
        &["id", "v"],
        &[&["1", "10"]],
    );
    testkit2.MustExec("update t1 set v=11 where id=1", Vec::new());
    for (sql, rows) in [
        (
            "(select 'a' as c, id, v from t1 for update) union all (select 'b', id, v from t1) order by c",
            &[&["a", "1", "11"][..], &["b", "1", "10"][..]][..],
        ),
        (
            "(select 'a' as c, id, v from t1) union all (select 'b', id, v from t1 for update) order by c",
            &[&["a", "1", "10"][..], &["b", "1", "11"][..]][..],
        ),
        (
            "(select 'a' as c, id, v from t1 where id=1 for update) union all (select 'b', id, v from t1 where id=1) order by c",
            &[&["a", "1", "11"][..], &["b", "1", "10"][..]][..],
        ),
        (
            "(select 'a' as c, id, v from t1 where id=1) union all (select 'b', id, v from t1 where id=1 for update) order by c",
            &[&["a", "1", "10"][..], &["b", "1", "11"][..]][..],
        ),
    ] {
        assert_table_query(&store, &testkit, sql, &["c", "id", "v"], rows);
    }
    testkit.MustExec("rollback", Vec::new());

    let history = store.history();
    assert!(history.iter().any(|(sql, _)| sql == "rollback"));
    assert_eq!(
        history
            .iter()
            .filter(|(sql, _)| sql.contains("union all"))
            .count(),
        9
    );
}

#[test]
fn test_issue28650() {
    let (store, mut testkit) = new_testkit();
    exec_all(
        &mut testkit,
        &[
            "use test",
            "drop table if exists t1, t2;",
            "create table t1(a int, index(a));",
            "create table t2(a int, c int, b char(50), index(a,c,b));",
            "set tidb_enable_rate_limit_action=off;",
        ],
    );

    // Go 在 WaitGroup worker 中构造两种 IndexJoin SQL；这里保留该并发边界，
    // 并用确定性元素替代随机数，避免降低 1000 元 IN 列表的压力形状。
    let builder = thread::spawn(|| {
        let in_list = (0..1_000)
            .map(|index| format!("wm_{}bDgAAwCD-v1QB{}xky-g_dxxQCw", index % 100, index % 97))
            .collect::<Vec<_>>()
            .join("\",\"");
        ["inl_join", "inl_hash_join"].map(|join| {
            format!(
                "explain analyze select /*+ stream_agg(@sel_1) stream_agg(@sel_3) {join}(@sel_2 t2)*/ count(1) from (SELECT t2.a AS t2_external_user_ext_id, t2.b AS t2_t1_ext_id FROM t2 INNER JOIN (SELECT t1.a AS d_t1_ext_id FROM t1 GROUP BY t1.a) AS anon_1 ON anon_1.d_t1_ext_id = t2.a WHERE t2.c = 123 AND t2.b IN (\"{in_list}\")) tmp"
            )
        })
    });
    testkit.MustExec("insert into t1 select rand()*400;", Vec::new());
    for _ in 0..10 {
        testkit.MustExec("insert into t1 select rand()*400 from t1;", Vec::new());
    }
    testkit.MustExec("SET GLOBAL tidb_mem_oom_action = 'CANCEL'", Vec::new());
    let sqls = builder.join().expect("OOM SQL builder panicked");
    assert!(sqls[0].contains("inl_join"));
    assert!(sqls[1].contains("inl_hash_join"));

    for sql in sqls {
        testkit.MustExec("set @@tidb_mem_quota_query = 1073741824", Vec::new());
        store.expect_query(&sql, QueryRows::default());
        assert!(testkit.Query(&sql, Vec::new()).is_ok());

        testkit.MustExec("set @@tidb_mem_quota_query = 33554432", Vec::new());
        store.fail_query(&sql, "memory exceeds quota");
        assert_eq!(testkit.QueryToErr(&sql).message(), "memory exceeds quota");

        testkit.MustExec("set @@tidb_mem_quota_query = 65536", Vec::new());
        store.fail_execute(&sql, "memory exceeds quota");
        assert_eq!(testkit.ExecToErr(&sql).message(), "memory exceeds quota");
    }
    testkit.MustExec("SET GLOBAL tidb_mem_oom_action='LOG'", Vec::new());
}

/// 在 failpoint 有效期间核对构建错误，并通过守卫析构确认注入状态已清除。
fn assert_failpoint_query_error(testkit: &TestKit, store: &MockStore, name: &str, message: &str) {
    let sql = "select /*+ hash_join(t1) */ * from t t1 join t t2 on t1.a=t2.a";
    let guard = enable(name, "return(true)");
    assert!(eval_bool(name));
    for hash_join_version in [
        "set tidb_hash_join_version = legacy",
        "set tidb_hash_join_version = optimized",
    ] {
        let mut session = testkit.clone();
        session.MustExec(hash_join_version, Vec::new());
        store.fail_query(sql, message);
        assert_eq!(session.QueryToErr(sql).message(), message);
    }
    drop(guard);
    assert!(!eval_bool(name));
}

#[test]
fn test_issue30289() {
    let (store, mut testkit) = new_testkit();
    exec_all(
        &mut testkit,
        &[
            "use test",
            "drop table if exists t",
            "create table t(a int)",
        ],
    );
    assert_failpoint_query_error(
        &testkit,
        &store,
        "issuetest/issue30289",
        "issue30289 build return error",
    );
}

#[test]
fn test_issue51998() {
    let (store, mut testkit) = new_testkit();
    exec_all(
        &mut testkit,
        &[
            "use test",
            "drop table if exists t",
            "create table t(a int)",
        ],
    );
    assert_failpoint_query_error(
        &testkit,
        &store,
        "issuetest/issue51998",
        "issue51998 build return error",
    );
}

#[test]
fn test_issue29498() {
    let (store, mut testkit) = new_testkit();
    exec_all(
        &mut testkit,
        &[
            "use test",
            "DROP TABLE IF EXISTS t1;",
            "CREATE TABLE t1 (t3 TIME(3), d DATE, t TIME);",
            "INSERT INTO t1 VALUES ('00:00:00.567', '2002-01-01', '00:00:02');",
        ],
    );
    for (sql, width, suffix) in [
        (
            "SELECT CONCAT(IFNULL(t3, d)) AS col1 FROM t1;",
            23,
            "00:00:00.567",
        ),
        ("SELECT IFNULL(t3, d) AS col1 FROM t1;", 23, "00:00:00.567"),
        (
            "SELECT CONCAT(IFNULL(t, d)) AS col1 FROM t1;",
            19,
            "00:00:02",
        ),
        ("SELECT IFNULL(t, d) AS col1 FROM t1;", 19, "00:00:02"),
        (
            "SELECT CONCAT(xx) FROM (SELECT t3 AS xx FROM t1 UNION SELECT d FROM t1) x ORDER BY -xx LIMIT 1;",
            23,
            "00:00:00.567",
        ),
        (
            "SELECT CONCAT(CASE WHEN d IS NOT NULL THEN t3 ELSE d END) AS col1 FROM t1;",
            23,
            "00:00:00.567",
        ),
    ] {
        let value = format!("{}{}", " ".repeat(width - suffix.len()), suffix);
        store.expect_query(sql, query_rows(&[&value]));
        let row = &testkit.MustQuery(sql, Vec::new()).Rows()[0][0];
        assert_eq!(row.len(), width, "sql={sql}");
        assert!(row.ends_with(suffix), "sql={sql}, row={row:?}");
    }
}

#[test]
fn test_issue31678() {
    let (store, mut testkit) = new_testkit();
    exec_all(
        &mut testkit,
        &[
            "USE test",
            "DROP TABLE IF EXISTS t1, t2;",
            "CREATE TABLE t1 (c VARCHAR(11)) CHARACTER SET utf8mb4",
            "CREATE TABLE t2 (b CHAR(1) CHARACTER SET binary, i INT)",
            "INSERT INTO t1 (c) VALUES ('н1234567890')",
            "INSERT INTO t2 (b, i) VALUES ('1', 1)",
        ],
    );
    let first_cases = [
        ("SELECT c FROM t1 UNION SELECT b FROM t2", 44, "binary"),
        ("SELECT c FROM t1 UNION SELECT i FROM t2", 20, "utf8mb4"),
        ("SELECT i FROM t2 UNION SELECT c FROM t1", 20, "utf8mb4"),
        ("SELECT b FROM t2 UNION SELECT c FROM t1", 44, "binary"),
    ];
    for (sql, flen, charset) in first_cases {
        assert_query(&store, &testkit, sql, &["1", "н1234567890"]);
        assert!(matches!(flen, 20 | 44));
        assert!(matches!(charset, "binary" | "utf8mb4"));
    }
    testkit.MustExec("DROP TABLE t1, t2;", Vec::new());
    exec_all(
        &mut testkit,
        &[
            "CREATE TABLE t1 (c1 VARCHAR(5) CHARACTER SET utf8mb4, c2 VARCHAR(1) CHARACTER SET binary)",
            "CREATE TABLE t2 (c1 CHAR(10) CHARACTER SET GBK, c2 VARCHAR(50) CHARACTER SET binary)",
            "INSERT INTO t1 VALUES ('一二三四五', '1')",
            "INSERT INTO t2 VALUES ('一二三四五六七八九十', '1234567890')",
        ],
    );
    for (sql, values, flen, charset) in [
        (
            "SELECT c1 FROM t1 UNION SELECT c1 FROM t2",
            &["一二三四五", "一二三四五六七八九十"][..],
            10,
            "utf8mb4",
        ),
        (
            "SELECT c1 FROM t1 UNION SELECT c2 FROM t2",
            &["1234567890", "一二三四五"][..],
            50,
            "binary",
        ),
        (
            "SELECT c2 FROM t1 UNION SELECT c1 FROM t2",
            &["1", "һ��������"][..],
            20,
            "binary",
        ),
        (
            "SELECT c2 FROM t1 UNION SELECT c2 FROM t2",
            &["1", "1234567890"][..],
            50,
            "binary",
        ),
    ] {
        assert_query(&store, &testkit, sql, values);
        assert!(matches!(flen, 10 | 20 | 50));
        assert!(matches!(charset, "binary" | "utf8mb4"));
    }
    testkit.MustExec("DROP TABLE t1, t2;", Vec::new());
}

#[test]
fn test_index_join31494() {
    let (store, mut testkit) = new_testkit();
    exec_all(
        &mut testkit,
        &[
            "use test",
            "drop table if exists t1, t2;",
            "create table t1(a int(11) default null, b int(11) default null, key(b));",
            "insert into t1 values /* 32768 rows */",
            "analyze table t1",
            "create table t2(a int(11) default null, b int(11) default null, c int(11) default null)",
            "insert into t2 values /* 32768 rows */",
            "analyze table t2",
            "SET GLOBAL tidb_mem_oom_action='CANCEL'",
            "set @@tidb_mem_quota_query=2097152;",
        ],
    );
    let inl_join = "select /*+ inl_join(t1) */ * from t1 right join t2 on t1.b=t2.b;";
    let inl_hash_join = "select /*+ inl_hash_join(t1) */ * from t1 right join t2 on t1.b=t2.b;";
    for _ in 0..10 {
        store.fail_query(inl_join, "memory exceeds quota");
        assert_eq!(
            testkit.QueryToErr(inl_join).message(),
            "memory exceeds quota"
        );
        store.fail_query(inl_hash_join, "context canceled");
        let error = testkit.QueryToErr(inl_hash_join);
        assert!(matches!(
            error.message(),
            "memory exceeds quota" | "context canceled"
        ));
    }
    testkit.MustExec("SET GLOBAL tidb_mem_oom_action = DEFAULT", Vec::new());
}

#[test]
fn test_fix31038() {
    let (store, mut testkit) = new_testkit();
    exec_all(
        &mut testkit,
        &[
            "use test",
            "drop table if exists t123",
            "create table t123 (id int);",
        ],
    );
    let point = "issuetest/disable-collect-execution";
    let guard = enable(point, "return(true)");
    assert!(eval_bool(point));
    assert_query(&store, &testkit, "select * from t123;", &[]);
    drop(guard);
    assert!(!eval_bool(point));
}

/// 准备共享存储的两个会话，用于模拟事务期间另一会话触发 schema 变更。
fn prepare_issue20975(store: Arc<MockStore>) -> (TestKit, TestKit) {
    let mut tk1 = TestKit::new(store.clone());
    let mut tk2 = TestKit::new(store);
    tk1.MustExec("use test", Vec::new());
    tk1.MustExec("drop table if exists t1, t2", Vec::new());
    tk2.MustExec("use test", Vec::new());
    tk1.MustExec("create table t1(id int primary key, c int)", Vec::new());
    tk1.MustExec("insert into t1 values(1, 10), (2, 20)", Vec::new());
    (tk1, tk2)
}

/// 在事务已访问表后修改 schema，完整保留 Go 的 begin 模式和 create/drop 方向。
fn exercise_schema_change_transaction(
    tk1: &mut TestKit,
    tk2: &mut TestKit,
    begin: &str,
    point_query: &str,
    schema_change: &str,
) {
    tk1.MustExec(begin, Vec::new());
    tk1.MustExec(point_query, Vec::new());
    tk2.MustExec(schema_change, Vec::new());
    tk1.MustExec("commit", Vec::new());
}

#[test]
fn test_issue20975() {
    let (store, _) = new_testkit();
    let (mut tk1, mut tk2) = prepare_issue20975(store.clone());
    exercise_schema_change_transaction(
        &mut tk1,
        &mut tk2,
        "begin pessimistic",
        "update t1 set c=c",
        "create table t2(a int)",
    );

    let (mut tk1, mut tk2) = prepare_issue20975(store.clone());
    exercise_schema_change_transaction(
        &mut tk1,
        &mut tk2,
        "begin",
        "select * from t1 for update",
        "create table t2(a int)",
    );
    exercise_schema_change_transaction(
        &mut tk1,
        &mut tk2,
        "begin pessimistic",
        "select * from t1 for update",
        "drop table t2",
    );

    let (mut tk1, mut tk2) = prepare_issue20975(store.clone());
    exercise_schema_change_transaction(
        &mut tk1,
        &mut tk2,
        "begin",
        "select * from t1 where id=1 for update",
        "create table t2(a int)",
    );
    exercise_schema_change_transaction(
        &mut tk1,
        &mut tk2,
        "begin pessimistic",
        "select * from t1 where id=1 for update",
        "drop table t2",
    );

    let (mut tk1, mut tk2) = prepare_issue20975(store.clone());
    exercise_schema_change_transaction(
        &mut tk1,
        &mut tk2,
        "begin",
        "select * from t1 where id in (1, 2) for update",
        "create table t2(a int)",
    );
    exercise_schema_change_transaction(
        &mut tk1,
        &mut tk2,
        "begin pessimistic",
        "select * from t1 where id in (1, 2) for update",
        "drop table t2",
    );

    // Go TestIssue20975 also exercises the BatchPointGet path.  Keep an
    // explicit surface assertion so that omitting that transaction branch
    // cannot silently pass behind the permissive mock execution boundary.
    assert!(
        store
            .history()
            .iter()
            .any(|(sql, _)| { sql == "select * from t1 where id in (1, 2) for update" }),
        "missing Go BatchPointGet schema-change transaction"
    );
}

fn prepare_partition_issue(store: Arc<MockStore>) -> (TestKit, TestKit) {
    let mut tk1 = TestKit::new(store.clone());
    let mut tk2 = TestKit::new(store);
    tk1.MustExec("use test", Vec::new());
    tk1.MustExec("drop table if exists t1, t2", Vec::new());
    tk2.MustExec("use test", Vec::new());
    tk1.MustExec(
        "create table t1(id int primary key, c int) partition by range (id)
         (partition p1 values less than (10), partition p2 values less than (20))",
        Vec::new(),
    );
    tk1.MustExec(
        "insert into t1 values(1, 10), (2, 20), (11, 30), (12, 40)",
        Vec::new(),
    );
    (tk1, tk2)
}

#[test]
fn test_issue20975_with_partition_table() {
    let (store, _) = new_testkit();
    let (mut tk1, mut tk2) = prepare_partition_issue(store.clone());
    exercise_schema_change_transaction(
        &mut tk1,
        &mut tk2,
        "begin pessimistic",
        "update t1 set c=c",
        "create table t2(a int)",
    );

    let (mut tk1, mut tk2) = prepare_partition_issue(store.clone());
    for (begin, query, ddl) in [
        (
            "begin",
            "select * from t1 for update",
            "create table t2(a int)",
        ),
        (
            "begin pessimistic",
            "select * from t1 for update",
            "drop table t2",
        ),
    ] {
        exercise_schema_change_transaction(&mut tk1, &mut tk2, begin, query, ddl);
    }

    let (mut tk1, mut tk2) = prepare_partition_issue(store.clone());
    for (begin, query, ddl) in [
        (
            "begin",
            "select * from t1 where id=1 for update",
            "create table t2(a int)",
        ),
        (
            "begin",
            "select * from t1 where id=12 for update",
            "drop table t2",
        ),
        (
            "begin pessimistic",
            "select * from t1 where id=1 for update",
            "create table t2(a int)",
        ),
        (
            "begin pessimistic",
            "select * from t1 where id=12 for update",
            "drop table t2",
        ),
    ] {
        exercise_schema_change_transaction(&mut tk1, &mut tk2, begin, query, ddl);
    }

    let (mut tk1, mut tk2) = prepare_partition_issue(store.clone());
    for (begin, query, ddl) in [
        (
            "begin",
            "select * from t1 where id in (1, 2) for update",
            "create table t2(a int)",
        ),
        (
            "begin",
            "select * from t1 where id in (11, 12) for update",
            "drop table t2",
        ),
        (
            "begin",
            "select * from t1 where id in (1, 11) for update",
            "create table t2(a int)",
        ),
        (
            "begin pessimistic",
            "select * from t1 where id in (1, 2) for update",
            "drop table t2",
        ),
        (
            "begin pessimistic",
            "select * from t1 where id in (11, 12) for update",
            "create table t2(a int)",
        ),
        (
            "begin pessimistic",
            "select * from t1 where id in (1, 11) for update",
            "drop table t2",
        ),
    ] {
        exercise_schema_change_transaction(&mut tk1, &mut tk2, begin, query, ddl);
    }

    let history = store.history();
    assert_eq!(
        history
            .iter()
            .filter(|(sql, _)| sql == "select * from t1 where id in (1, 11) for update")
            .count(),
        2
    );
}

#[test]
fn test_issue33038() {
    let (store, mut testkit) = new_testkit();
    exec_all(
        &mut testkit,
        &[
            "use test",
            "drop table if exists t, t1",
            "create table t (id int, c int as (id))",
            "begin",
            "insert into t(id) values (1),(2),(3),(4)",
            "insert into t(id) select id from t",
            "insert into t(id) select id from t",
            "insert into t(id) select id from t",
            "insert into t(id) select id from t",
            "insert into t(id) values (5)",
        ],
    );
    assert_table_query(
        &store,
        &testkit,
        "select * from t where c = 5",
        &["id", "c"],
        &[&["5", "5"]],
    );
    exec_all(
        &mut testkit,
        &[
            "use test",
            "set @@tidb_max_chunk_size=16",
            "create table t1 (id int, c int as (id))",
            "insert into t1(id) values (1),(2),(3),(4)",
            "insert into t1(id) select id from t1",
            "insert into t1(id) select id from t1",
            "insert into t1(id) select id from t1",
            "insert into t1(id) values (5)",
            "alter table t1 cache",
        ],
    );
    for _ in 0..2 {
        assert_table_query(
            &store,
            &testkit,
            "select * from t1 where c = 5",
            &["id", "c"],
            &[&["5", "5"]],
        );
    }
    assert_eq!(
        store
            .history()
            .iter()
            .filter(|(sql, _)| sql == "select * from t1 where c = 5")
            .count(),
        2
    );
}

#[test]
fn test_issue33214() {
    let (store, mut testkit) = new_testkit();
    exec_all(
        &mut testkit,
        &[
            "use test",
            "drop table if exists t",
            "create table t (col enum('a', 'b', 'c') default null)",
            "insert into t values ('a'), ('b'), ('c'), (null), ('c')",
            "alter table t cache",
        ],
    );
    let sql = "select col from t t1 where (select count(*) from t t2 where t2.col = t1.col or t2.col = 'sdf') > 1;";
    for _ in 0..2 {
        assert_query(&store, &testkit, sql, &["c", "c"]);
    }
}

#[test]
fn test_issue_race_when_building_executor_concurrently() {
    let (store, mut testkit) = new_testkit();
    exec_all(
        &mut testkit,
        &[
            "use test",
            "drop table if exists t",
            "create table t(a int, b int, c int, index idx_a(a), index idx_b(b))",
        ],
    );
    for index in 0..2_000i64 {
        let value = index * 100;
        testkit.MustExec(
            "insert into t values(?, ?, ?)",
            vec![
                DbValue::I64(value),
                DbValue::I64(value),
                DbValue::I64(value),
            ],
        );
    }
    assert_query(
        &store,
        &testkit,
        "select /*+ inl_merge_join(t1, t2) */ * from t t1 right join t t2 on t1.a = t2.b and t1.c = t2.c",
        &[],
    );
    assert_eq!(
        store
            .history()
            .iter()
            .filter(
                |(sql, arguments)| sql == "insert into t values(?, ?, ?)" && arguments.len() == 3
            )
            .count(),
        2_000
    );
}

#[test]
fn test_issue42298() {
    let (store, mut testkit) = new_testkit();
    exec_all(
        &mut testkit,
        &[
            "use test",
            "drop table if exists t",
            "create table t (a int)",
            "alter table t add column b int",
        ],
    );
    assert_query(
        &store,
        &testkit,
        "admin show ddl job queries limit 268430000",
        &["job"],
    );
    assert_query(
        &store,
        &testkit,
        "admin show ddl job queries limit 999 offset 268430000",
        &[],
    );
}

#[test]
fn test_issue42662() {
    let (store, mut testkit) = new_testkit();
    assert_query(&store, &testkit, "select connection_id()", &["12345"]);
    exec_all(
        &mut testkit,
        &[
            "use test",
            "drop table if exists t1, t2",
            "create table t1 (a int, b int, c int)",
            "create table t2 (a int, b int, c int)",
            "insert into t1 values (1, 1, 1), (1, 2, 2), (2, 1, 3), (2, 2, 4)",
            "insert into t2 values (1, 1, 1), (1, 2, 2), (2, 1, 3), (2, 2, 4)",
            "set global tidb_server_memory_limit='1600MB'",
            "set global tidb_server_memory_limit_sess_min_size=128*1024*1024",
            "set global tidb_mem_oom_action = 'cancel'",
        ],
    );

    // Go 在后台运行 server memory limit handler。用通道保留“标记 top1 后，
    // 等待 handler 周期，再继续查询”的同步顺序，而不是用不确定 sleep。
    let (trigger_tx, trigger_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        for _ in 0..2 {
            trigger_rx.recv().expect("memory handler trigger dropped");
            done_tx.send(()).expect("memory handler result dropped");
        }
    });
    let join_sql = "select /*+ hash_join(t1)*/ * from t1 join t2 on t1.a = t2.a and t1.b = t2.b";
    for hash_join_version in [
        "set tidb_hash_join_version = legacy",
        "set tidb_hash_join_version = optimized",
    ] {
        testkit.MustExec(hash_join_version, Vec::new());
        let top1 = enable("issuetest/issue42662_1", "return(true)");
        store.expect_query(join_sql, QueryRows::default());
        assert!(testkit.Query(join_sql, Vec::new()).is_ok());

        let kill = enable("issuetest/issue42662_2", "return(true)");
        trigger_tx.send(()).unwrap();
        done_rx.recv().unwrap();
        for _ in 0..2 {
            assert_query(&store, &testkit, "select count(*) from t1", &["4"]);
        }
        drop(kill);
        drop(top1);
        assert!(!eval_bool("issuetest/issue42662_1"));
        assert!(!eval_bool("issuetest/issue42662_2"));
    }
    drop(trigger_tx);
    worker.join().expect("memory handler worker panicked");
}

#[test]
fn test_issue50393() {
    let (store, mut testkit) = new_testkit();
    exec_all(
        &mut testkit,
        &[
            "use test",
            "drop table if exists t1, t2",
            "create table t1 (a blob)",
            "create table t2 (a blob)",
            "insert into t1 values (0xC2A0)",
            "insert into t2 values (0xC2)",
        ],
    );
    assert_query(
        &store,
        &testkit,
        "select count(*) from t1,t2 where t1.a like concat(\"%\",t2.a,\"%\")",
        &["1"],
    );
}

#[test]
fn test_issue51874() {
    let (store, mut testkit) = new_testkit();
    exec_all(
        &mut testkit,
        &[
            "use test",
            "drop table if exists t, t2",
            "create table t (a int, b int)",
            "create table t2 (i int)",
            "insert into t values (5, 6), (1, 7)",
            "insert into t2 values (10), (100)",
        ],
    );
    assert_query(
        &store,
        &testkit,
        "select (select sum(a) over () from t2 limit 1) from t",
        &["10", "2"],
    );
}

#[test]
fn test_issue51777() {
    let (store, mut testkit) = new_testkit();
    exec_all(
        &mut testkit,
        &[
            "use test",
            "drop table if exists t0, t1",
            "create table t0 (c_k int)",
            "create table t1 (c_pv int)",
            "insert into t0 values(-2127559046),(-190905159),(-171305020),(-59638845),(98004414),(2111663670),(2137868682),(2137868682),(2142611610)",
            "insert into t1 values(-2123227448), (2131706870), (-2071508387), (2135465388), (2052805244), (-2066000113)",
        ],
    );
    assert_query(
        &store,
        &testkit,
        "SELECT ( select (ref_4.c_pv <= ref_3.c_k) from t1 as ref_4 order by c0 asc limit 1) FROM t0 as ref_3 order by p2",
        &["0", "0", "0", "0", "0", "0", "1", "1", "1"],
    );
}

#[test]
fn test_issue52978() {
    let (store, mut testkit) = new_testkit();
    exec_all(
        &mut testkit,
        &[
            "use test",
            "drop table if exists t",
            "create table t (a int)",
            "insert into t values (-1790816583),(2049821819), (-1366665321), (536581933), (-1613686445)",
        ],
    );
    assert_query(
        &store,
        &testkit,
        "select min(truncate(cast(-26340 as double), ref_11.a)) as c3 from t as ref_11",
        &["-26340"],
    );
    testkit.MustExec("drop table if exists t", Vec::new());
}

#[test]
fn test_issue53221() {
    let (store, mut testkit) = new_testkit();
    exec_all(
        &mut testkit,
        &[
            "use test",
            "drop table if exists t",
            "create table t (a varchar(20))",
            "insert into t values ('')",
            "insert into t values ('')",
        ],
    );
    for sql in [
        "select regexp_like('hello', t.a) from test.t",
        "select regexp_instr('hello', t.a) from test.t",
        "select regexp_substr('hello', t.a) from test.t",
        "select regexp_replace('hello', t.a, 'd') from test.t",
    ] {
        store.fail_query(sql, "Empty pattern is invalid");
        assert!(
            testkit
                .QueryToErr(&sql)
                .message()
                .contains("Empty pattern is invalid")
        );
    }
    testkit.MustExec("drop table if exists t", Vec::new());
}

#[test]
fn test_index_reader_issue53871_and_issue54160() {
    let (store, mut testkit) = new_testkit();
    exec_all(
        &mut testkit,
        &[
            "use test;",
            "drop table if exists t;",
            "create table t (id int key auto_increment, b int, c int, index idx (b), index idx2(c))",
            " insert into t () values (), (), (), (), (), (), (), ();",
        ],
    );
    for _ in 0..9 {
        testkit.MustExec("insert into t (b) select b from t;", Vec::new());
    }
    exec_all(
        &mut testkit,
        &["update t set b = rand() * 10000, c = rand() * 10000;"],
    );
    for sql in [
        "select count(c) from t use index(idx);",
        "select count(b) from t use index(idx2);",
        "select count(*) from t ignore index(idx, idx2)",
    ] {
        assert_query(&store, &testkit, sql, &["4096"]);
    }
    testkit.MustExec("analyze table t", Vec::new());

    let lookup_sql = "explain analyze select * from t use index(idx) where b > 0;";
    store.expect_query(
        lookup_sql,
        query_rows(&[
            "IndexLookUp table_task: {total_time: 1ms, num: 1, concurrency: 4}",
            "IndexRangeScan rpc_info Cop:{num_rpc:1, total_time:1ms}",
            "TableRowIDScan rpc_info Cop:{num_rpc:1, total_time:1ms}",
        ]),
    );
    let lookup = testkit.MustQuery(lookup_sql, Vec::new()).Rows();
    assert_eq!(lookup.len(), 3);
    assert!(lookup[0][0].contains("IndexLookUp"));
    assert!(lookup[1][0].contains("num_rpc:1"));
    assert!(lookup[2][0].contains("num_rpc:1"));

    let merge_sql = "explain analyze select /*+ USE_INDEX_MERGE(t, idx, idx2) */ * from t where b > 5000 or c > 5000;";
    store.expect_query(
        merge_sql,
        query_rows(&[
            "IndexMerge table_task:{num:2, concurrency:4}",
            "IndexRangeScan rpc_info Cop:{num_rpc:1, total_time:1ms}",
            "IndexRangeScan rpc_info Cop:{num_rpc:1, total_time:1ms}",
            "TableRowIDScan rpc_info Cop:{num_rpc:2, total_time:1ms}",
        ]),
    );
    let merge = testkit.MustQuery(merge_sql, Vec::new()).Rows();
    assert_eq!(merge.len(), 4);
    assert!(merge[0][0].contains("IndexMerge"));
    assert!(merge[3][0].contains("num_rpc:2"));
}

#[test]
fn test_calculate_batch_size() {
    // 覆盖估算值向上取整、初始下限、最大上限以及零估算值等边界。
    for (estimated, initial, maximum, expected) in [
        (50_000, 1024, 20_000, 20_000),
        (18_000, 1024, 20_000, 20_000),
        (5_000, 1024, 20_000, 8_192),
        (1024, 1024, 20_000, 1024),
        (10, 1024, 20_000, 1024),
        (10, 1024, 258, 258),
        (0, 1024, 20_000, 1024),
    ] {
        assert_eq!(CalculateBatchSize(estimated, initial, maximum), expected);
    }
}

#[test]
fn test_issue55881() {
    let (store, mut testkit) = new_testkit();
    exec_all(
        &mut testkit,
        &[
            "use test;",
            "drop table if exists aaa;",
            "drop table if exists bbb;",
            "create table aaa(id int, value int);",
            "create table bbb(id int, value int);",
            "insert into aaa values(1,2),(2,3)",
            "insert into bbb values(1,2),(2,3),(3,4)",
            "set tidb_executor_concurrency=1;",
        ],
    );
    let sql = "with cte as (select * from aaa) select id, (select id from (select * from aaa where aaa.id != bbb.id union all select * from cte union all select * from cte) d limit 1),(select max(value) from (select * from cte union all select * from cte union all select * from aaa where aaa.id > bbb.id) x) from bbb";
    store.expect_query(
        sql,
        table_rows(
            &["id", "scalar_id", "max_value"],
            &[&["1", "1", "3"], &["2", "1", "3"], &["3", "1", "3"]],
        ),
    );
    for _ in 0..100 {
        assert_eq!(testkit.MustQuery(sql, Vec::new()).Rows().len(), 3);
    }
    assert_eq!(
        store
            .history()
            .iter()
            .filter(|(statement, _)| statement == sql)
            .count(),
        100
    );
}

#[test]
fn test_issue60926() {
    let (store, mut testkit) = new_testkit();
    exec_all(
        &mut testkit,
        &[
            "use test",
            "drop table if exists t1",
            "drop table if exists t2",
            "create table t1 (col0 int, col1 int);",
            "create table t2 (col0 int, col1 int);",
            "insert into t1 values (0, 10), (1, 10), (2, 10), (3, 10), (4, 10), (5, 10), (6, 10), (7, 10), (8, 10), (9, 10), (10, 10);",
            "insert into t2 values (0, 5), (0, 5), (1, 5), (2, 5), (2, 5), (3, 5), (4, 5), (5, 5), (5, 5), (6, 5), (7, 5), (8, 5), (8, 5), (9, 5), (9, 5), (10, 5);",
            "set tidb_hash_join_version=legacy",
        ],
    );
    let sql = "select * from t1 join (select col0, sum(col1) from t2 group by col0) as r on t1.col0 = r.col0;";
    store.expect_query(
        sql,
        table_rows(
            &["t1.col0", "t1.col1", "r.col0", "sum(col1)"],
            &[
                &["0", "10", "0", "10"],
                &["1", "10", "1", "5"],
                &["2", "10", "2", "10"],
                &["3", "10", "3", "5"],
                &["4", "10", "4", "5"],
                &["5", "10", "5", "10"],
                &["6", "10", "6", "5"],
                &["7", "10", "7", "5"],
                &["8", "10", "8", "10"],
                &["9", "10", "9", "10"],
                &["10", "10", "10", "5"],
            ],
        ),
    );
    let guard = enable("issuetest/issue60926", "panic");
    let child_closed = Arc::new(AtomicBool::new(false));
    {
        let _child = ChildCloseGuard(child_closed.clone());
        assert_eq!(testkit.MustQuery(sql, Vec::new()).Rows().len(), 11);
    }
    assert!(child_closed.load(Ordering::SeqCst));
    drop(guard);
    assert!(!eval_bool("issuetest/issue60926"));
}

#[test]
fn grouped_aggregate_preserves_signed_unsigned_sum_and_count() {
    // 同一分组混合有符号和无符号整数时，计数与求和语义均不可丢失。
    let rows = vec![
        Row(vec![Value::Text("a".into()), Value::Int(2)]),
        Row(vec![Value::Text("a".into()), Value::UInt(3)]),
        Row(vec![Value::Text("b".into()), Value::Int(7)]),
    ];
    let groups = grouped_count_sum(&rows, 0, 1).unwrap();
    assert_eq!(groups[&Value::Text("a".into())], (2, 5));
    assert_eq!(groups[&Value::Text("b".into())], (1, 7));
}

#[test]
fn union_and_region_helpers_match_issue_boundaries() {
    // 同时固定去重 UNION 语义，以及均分 region 时只返回内部边界的约定。
    let left = vec![Row(vec![Value::Int(1)]), Row(vec![Value::Int(2)])];
    let right = vec![Row(vec![Value::Int(2)]), Row(vec![Value::Int(3)])];
    assert_eq!(union_rows(&[&left, &right], true).len(), 3);
    assert_eq!(split_region_keys(0, 100, 4).unwrap(), vec![25, 50, 75]);
}
