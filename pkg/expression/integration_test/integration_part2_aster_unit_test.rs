// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 表达式集成测试 Part2：JSON、时间转换、键编解码与系统变量。
//
// 覆盖真实 SQL 的 JSON_MERGE_PATCH、JSON 类型比较序、日期时间 CAST、
// 键编解码、安全增强模式与主键系统变量。

#![allow(non_snake_case)]

use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{Rows, TestKit};
use astersql_types::json_binary::ParseBinaryJSONFromString;

fn new_testkit() -> TestKit {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("USE test", Vec::new());
    tk
}

fn skip_if_not_starter_for_fts() -> bool {
    !astersql_config_deploymode::IsStarter()
}

#[test]
fn TestFTSParser() {
    if skip_if_not_starter_for_fts() {
        return;
    }
    let mut tk = new_testkit();
    for (definition, parser) in [
        ("FULLTEXT(a)", "STANDARD"),
        ("FULLTEXT(a) WITH PARSER standard", "STANDARD"),
        ("FULLTEXT(a) WITH PARSER multilingual", "MULTILINGUAL"),
    ] {
        tk.MustExec(&format!("CREATE TABLE tx(a TEXT,{definition})"), Vec::new());
        tk.MustQuery("SHOW CREATE TABLE tx", Vec::new())
            .Check(vec![vec![
                "tx",
                &format!(
                    "CREATE TABLE `tx` (\n  `a` text DEFAULT NULL,\n  \
                 FULLTEXT INDEX `a`(`a`) WITH PARSER {parser}\n\
                 ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"
                ),
            ]]);
        tk.MustExec("DROP TABLE tx", Vec::new());
    }
    tk.MustContainErrMsg(
        "CREATE TABLE tx(a TEXT,FULLTEXT(a) WITH PARSER abc)",
        "Unsupported parser 'abc'",
    );
}

#[test]
fn TestFTSSyntax() {
    if skip_if_not_starter_for_fts() {
        return;
    }
    let mut tk = new_testkit();
    tk.MustExec(
        "CREATE TABLE t(title TEXT,body TEXT,FULLTEXT INDEX(title))",
        Vec::new(),
    );
    tk.MustExec("ALTER TABLE t SET TIFLASH REPLICA 1", Vec::new());
    tk.MustQuery(
        "SELECT * FROM t WHERE FTS_MATCH_WORD('hello',title)",
        Vec::new(),
    );
    tk.MustQuery(
        "SELECT * FROM t WHERE FTS_MATCH_WORD('hello',title) AND body=''",
        Vec::new(),
    );
    tk.MustContainErrMsg(
        "SELECT * FROM t WHERE MATCH() AGAINST('hello')",
        "You have an error in your SQL syntax",
    );
    tk.MustExec(
        "SET @@tidb_opt_enable_alternative_logical_plans=ON",
        Vec::new(),
    );
    tk.MustQuery(
        "SELECT * FROM t WHERE MATCH(title) AGAINST('hello' IN BOOLEAN MODE)",
        Vec::new(),
    );
    tk.MustExec(
        "SET @@tidb_opt_enable_alternative_logical_plans=OFF",
        Vec::new(),
    );
    tk.MustContainErrMsg(
        "SELECT * FROM t WHERE FTS_MATCH_WORD(title,body)",
        "match against a non-constant string",
    );
    tk.MustContainErrMsg(
        "SELECT * FROM t WHERE FTS_MATCH_WORD(45.67,body)",
        "match against a non-constant string",
    );
    tk.MustContainErrMsg(
        "SELECT * FROM t WHERE FTS_MATCH_WORD('hello',title,body)",
        "Incorrect parameter count in the call to native function",
    );
}

#[test]
fn TestFTSIndexSyntax() {
    if skip_if_not_starter_for_fts() {
        return;
    }
    let mut tk = new_testkit();
    for (sql, error) in [
        (
            "CREATE TABLE t(title TEXT,body TEXT,FULLTEXT KEY(title,body))",
            "FULLTEXT index must specify one column name",
        ),
        (
            "CREATE TABLE t(title TEXT,body TEXT,FULLTEXT KEY((title)))",
            "FULLTEXT index must specify one column name",
        ),
        (
            "CREATE TABLE t(title TEXT,body TEXT,FULLTEXT KEY(title(5)))",
            "FULLTEXT index does not support prefix length",
        ),
        (
            "CREATE TABLE t(title TEXT,body TEXT,FULLTEXT KEY(title DESC))",
            "FULLTEXT index does not support DESC order",
        ),
        (
            "CREATE TABLE t(title TEXT,body TEXT,c INT,FULLTEXT KEY(c))",
            "only support string type",
        ),
        (
            "CREATE TABLE t1(title TEXT,body TEXT,FULLTEXT KEY(title) WITH PARSER ngramx)",
            "Unsupported parser",
        ),
    ] {
        tk.MustContainErrMsg(sql, error);
    }
    for (table, index_definition, index_name) in [
        ("t1", "FULLTEXT KEY(title)", "title"),
        ("t2", "FULLTEXT(title)", "title"),
        ("t3", "FULLTEXT KEY idx(title)", "idx"),
        ("t4", "FULLTEXT KEY idx(`title`)", "idx"),
        ("t5", "FULLTEXT KEY idx(title ASC)", "idx"),
        (
            "t6",
            "FULLTEXT KEY idx(title ASC) WITH PARSER standard",
            "idx",
        ),
    ] {
        tk.MustExec(
            &format!("CREATE TABLE {table}(title TEXT,body TEXT,{index_definition})"),
            Vec::new(),
        );
        tk.MustQuery(&format!("SHOW CREATE TABLE {table}"), Vec::new())
            .Check(vec![vec![
                table,
                &format!(
                    "CREATE TABLE `{table}` (\n  `title` text DEFAULT NULL,\n  \
                     `body` text DEFAULT NULL,\n  FULLTEXT INDEX `{index_name}`(`title`) \
                     WITH PARSER STANDARD\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 \
                     COLLATE=utf8mb4_bin"
                ),
            ]]);
    }
    tk.MustExec("DROP TABLE t1,t2,t3,t4,t5,t6", Vec::new());
    tk.MustExec("CREATE TABLE t1(title TEXT,body TEXT)", Vec::new());
    tk.MustContainErrMsg(
        "ALTER TABLE t1 ADD FULLTEXT INDEX(body)",
        "columnar replica must exist to create",
    );
    tk.MustExec("ALTER TABLE t1 SET TIFLASH REPLICA 1", Vec::new());
    tk.MustExec("ALTER TABLE t1 ADD FULLTEXT INDEX(body)", Vec::new());
    tk.MustQuery("SHOW CREATE TABLE t1", Vec::new())
        .Check(vec![vec![
            "t1",
            "CREATE TABLE `t1` (\n  `title` text DEFAULT NULL,\n  `body` text DEFAULT NULL,\n  \
             FULLTEXT INDEX `body`(`body`) WITH PARSER STANDARD\n) ENGINE=InnoDB DEFAULT \
             CHARSET=utf8mb4 COLLATE=utf8mb4_bin",
        ]]);
    tk.MustExec("ALTER TABLE t1 DROP INDEX body", Vec::new());
}

#[test]
/// GET_LOCK/RELEASE_LOCK 覆盖参数、名称、引用计数、批量释放与跨会话竞争。
fn TestGetLock() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store.clone());
    let tk2 = TestKit::new(store);

    assert!(
        tk.QueryToErr("SELECT GET_LOCK('testlock')")
            .message()
            .contains("Incorrect parameter count")
    );
    tk.MustQuery("SELECT GET_LOCK('testlock1',0)", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery("SELECT GET_LOCK('testlock2',-10)", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery("SHOW WARNINGS", Vec::new()).Check(vec![vec![
        "Warning",
        "1292",
        "Truncated incorrect get_lock value: '-10'",
    ]]);
    tk.MustQuery(
        "SELECT RELEASE_LOCK('testlock1'),RELEASE_LOCK('testlock2')",
        Vec::new(),
    )
    .Check(Rows(&["1 1"]));
    tk.MustQuery("SELECT RELEASE_ALL_LOCKS()", Vec::new())
        .Check(Rows(&["0"]));

    for sql in [
        "SELECT GET_LOCK('',10)",
        "SELECT GET_LOCK(NULL,10)",
        "SELECT RELEASE_LOCK('')",
        "SELECT RELEASE_LOCK(NULL)",
        "SELECT GET_LOCK(REPEAT('a',65),10)",
        "SELECT RELEASE_LOCK(REPEAT('a',65))",
    ] {
        assert!(tk.Exec(sql, Vec::new()).is_err(), "sql={sql}");
    }
    tk.MustQuery("SELECT GET_LOCK('aaa',NULL)", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery("SELECT RELEASE_LOCK('aaa')", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery("SELECT GET_LOCK('aBC',-10)", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery("SELECT RELEASE_LOCK('AbC')", Vec::new())
        .Check(Rows(&["1"]));
    for name in ["randombytes", "abc"] {
        tk.MustQuery(&format!("SELECT RELEASE_LOCK('{name}')"), Vec::new())
            .Check(Rows(&["0"]));
    }
    tk.MustQuery("SELECT GET_LOCK(1234,10)", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery("SELECT GET_LOCK(REPEAT('a',64),10)", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery(
        "SELECT RELEASE_LOCK(1234),RELEASE_LOCK(REPEAT('aa',32))",
        Vec::new(),
    )
    .Check(Rows(&["1 1"]));
    tk.MustQuery("SELECT RELEASE_ALL_LOCKS()", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustQuery("SELECT GET_LOCK(REPEAT(UNHEX('C3A4'),33),10)", Vec::new());
    tk.MustQuery("SELECT RELEASE_LOCK(REPEAT(UNHEX('C3A4'),33))", Vec::new());
    tk.MustQuery("SELECT GET_LOCK('nnn',1.2)", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery("SELECT RELEASE_LOCK('nnn')", Vec::new())
        .Check(Rows(&["1"]));

    tk.MustQuery(
        "SELECT GET_LOCK('a1',1.2),GET_LOCK('a2',1.2),\
         GET_LOCK('a3',1.2),GET_LOCK('a4',1.2)",
        Vec::new(),
    )
    .Check(Rows(&["1 1 1 1"]));
    tk.MustQuery(
        "SELECT RELEASE_LOCK('a1'),RELEASE_LOCK('a2'),RELEASE_LOCK('a3'),\
         RELEASE_LOCK('random'),RELEASE_LOCK('a4')",
        Vec::new(),
    )
    .Check(Rows(&["1 1 1 0 1"]));
    tk.MustQuery("SELECT RELEASE_ALL_LOCKS()", Vec::new())
        .Check(Rows(&["0"]));

    tk.MustQuery(
        "SELECT GET_LOCK('a1',1.2),GET_LOCK('a2',1.2),\
         GET_LOCK('a3',1.2),GET_LOCK('a4',1.2)",
        Vec::new(),
    )
    .Check(Rows(&["1 1 1 1"]));
    tk.MustQuery("SELECT RELEASE_ALL_LOCKS()", Vec::new())
        .Check(Rows(&["4"]));
    tk.MustQuery("SELECT RELEASE_LOCK('a1')", Vec::new())
        .Check(Rows(&["0"]));

    tk.MustQuery(
        "SELECT GET_LOCK('a1',1.2),GET_LOCK('a2',1.2),\
         GET_LOCK('a3',1.2),GET_LOCK('a4',1.2)",
        Vec::new(),
    )
    .Check(Rows(&["1 1 1 1"]));
    tk.MustQuery(
        "SELECT GET_LOCK('a1',1.2),GET_LOCK('a2',1.2),GET_LOCK('a5',1.2)",
        Vec::new(),
    )
    .Check(Rows(&["1 1 1"]));
    tk.MustQuery("SELECT RELEASE_ALL_LOCKS()", Vec::new())
        .Check(Rows(&["7"]));
    for name in ["a1", "a5"] {
        tk.MustQuery(&format!("SELECT RELEASE_LOCK('{name}')"), Vec::new())
            .Check(Rows(&["0"]));
    }
    tk.MustQuery("SELECT RELEASE_ALL_LOCKS()", Vec::new())
        .Check(Rows(&["0"]));

    tk.MustQuery("SELECT GET_LOCK('mygloballock',1)", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery("SELECT RELEASE_LOCK('mygloballock')", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery("SELECT RELEASE_LOCK('mygloballock')", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustQuery("SELECT GET_LOCK('mygloballock',1)", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery("SELECT GET_LOCK('mygloballock',1)", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery("SELECT RELEASE_LOCK('mygloballock')", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery("SELECT RELEASE_LOCK('mygloballock')", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery("SELECT RELEASE_LOCK('mygloballock')", Vec::new())
        .Check(Rows(&["0"]));

    tk2.MustQuery("SELECT GET_LOCK('mygloballock',1)", Vec::new())
        .Check(Rows(&["1"]));
    for timeout in [1, 0] {
        tk.MustQuery(
            &format!("SELECT GET_LOCK('mygloballock',{timeout})"),
            Vec::new(),
        )
        .Check(Rows(&["0"]));
        tk.MustQuery("SELECT RELEASE_LOCK('mygloballock')", Vec::new())
            .Check(Rows(&["0"]));
    }
    tk2.MustQuery("SELECT RELEASE_LOCK('mygloballock')", Vec::new())
        .Check(Rows(&["1"]));
    tk2.MustQuery("SELECT RELEASE_ALL_LOCKS()", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustQuery("SELECT RELEASE_ALL_LOCKS()", Vec::new())
        .Check(Rows(&["0"]));
}

#[test]
/// INFORMATION 内置函数覆盖持久 last-id、结果行数、影响行数与会话身份。
fn TestInfoBuiltin() {
    let mut tk = new_testkit();

    tk.MustExec("DROP TABLE IF EXISTS t", Vec::new());
    tk.MustExec(
        "CREATE TABLE t(id INT AUTO_INCREMENT, a INT, PRIMARY KEY(id))",
        Vec::new(),
    );
    tk.MustExec("INSERT INTO t(a) VALUES(1)", Vec::new());
    tk.MustQuery("SELECT LAST_INSERT_ID()", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustExec("INSERT INTO t VALUES(2,1)", Vec::new());
    tk.MustQuery("SELECT LAST_INSERT_ID()", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustExec("INSERT INTO t(a) VALUES(1)", Vec::new());
    tk.MustQuery("SELECT LAST_INSERT_ID()", Vec::new())
        .Check(Rows(&["3"]));
    tk.MustQuery("SELECT LAST_INSERT_ID(5)", Vec::new())
        .Check(Rows(&["5"]));
    tk.MustQuery("SELECT LAST_INSERT_ID()", Vec::new())
        .Check(Rows(&["5"]));

    tk.MustExec("DROP TABLE t", Vec::new());
    tk.MustExec("CREATE TABLE t(a INT)", Vec::new());
    tk.MustQuery("SELECT * FROM t", Vec::new());
    tk.MustQuery("SELECT FOUND_ROWS()", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustQuery("SELECT FOUND_ROWS()", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustExec("INSERT INTO t VALUES(1),(2),(2)", Vec::new());
    for (sql, expected) in [
        ("SELECT * FROM t", "3"),
        ("SELECT * FROM t WHERE a=0", "0"),
        ("SELECT * FROM t WHERE a=1", "1"),
        ("SELECT * FROM t WHERE a LIKE '2'", "2"),
        ("SHOW TABLES LIKE 't'", "1"),
        ("SELECT COUNT(*) FROM t", "1"),
    ] {
        tk.AddComment(format!("FOUND_ROWS source: {sql}"));
        tk.MustQuery(sql, Vec::new());
        let found = tk.MustQuery("SELECT FOUND_ROWS()", Vec::new());
        assert_eq!(found.Rows(), Rows(&[expected]), "FOUND_ROWS source: {sql}");
        tk.ClearComment();
    }

    tk.MustQuery("SELECT DATABASE()", Vec::new())
        .Check(Rows(&["test"]));
    tk.MustExec("DROP DATABASE test", Vec::new());
    tk.MustQuery("SELECT DATABASE()", Vec::new())
        .Check(Rows(&["<nil>"]));
    tk.MustExec("CREATE DATABASE test", Vec::new());
    tk.MustExec("USE test", Vec::new());

    tk.MustQuery("SELECT CURRENT_USER()", Vec::new())
        .Check(Rows(&["root@%"]));
    tk.MustQuery("SELECT USER()", Vec::new())
        .Check(Rows(&["root@localhost"]));
    assert!(!tk.MustQuery("SELECT CONNECTION_ID()", Vec::new()).Rows()[0][0].is_empty());
    assert!(!tk.MustQuery("SELECT VERSION()", Vec::new()).Rows()[0][0].is_empty());
    let tidb_version = tk.MustQuery("SELECT TIDB_VERSION()", Vec::new()).Rows()[0][0].clone();
    assert!(tidb_version.contains("Release Version:"));
    assert!(tidb_version.contains("Edition:"));

    tk.MustExec("DROP TABLE IF EXISTS t", Vec::new());
    tk.MustExec("CREATE TABLE t(a INT PRIMARY KEY,b INT)", Vec::new());
    tk.MustQuery("SELECT ROW_COUNT()", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustExec("INSERT INTO t VALUES(1,11),(2,22),(3,33)", Vec::new());
    tk.MustQuery("SELECT ROW_COUNT()", Vec::new())
        .Check(Rows(&["3"]));
    tk.MustQuery("SELECT * FROM t", Vec::new());
    tk.MustQuery("SELECT ROW_COUNT()", Vec::new())
        .Check(Rows(&["-1"]));
    tk.MustExec("UPDATE t SET b=22 WHERE a=1", Vec::new());
    tk.MustQuery("SELECT ROW_COUNT()", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustExec("UPDATE t SET b=22 WHERE a=1", Vec::new());
    tk.MustQuery("SELECT ROW_COUNT()", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustExec("DELETE FROM t WHERE a=2", Vec::new());
    tk.MustQuery("SELECT ROW_COUNT()", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery("SELECT ROW_COUNT()", Vec::new())
        .Check(Rows(&["-1"]));

    tk.MustQuery("SELECT BENCHMARK(3,BENCHMARK(2,LENGTH('abc')))", Vec::new())
        .Check(Rows(&["0"]));
    assert!(
        tk.Exec("SELECT BENCHMARK(3,LENGTH('a','b'))", Vec::new())
            .is_err()
    );
    tk.MustQuery("SELECT TIDB_IS_DDL_OWNER()", Vec::new())
        .Check(Rows(&["1"]));
}

#[test]
/// TiFlash 隔离读下仍可规划含 TIDB_SHARD 表达式索引的表；禁用 MPP 后拒绝。
fn TestShardIndexOnTiFlash() {
    let mut tk = new_testkit();
    tk.MustExec("DROP TABLE IF EXISTS t", Vec::new());
    tk.MustExec(
        "CREATE TABLE t(
            id INT PRIMARY KEY CLUSTERED,
            a INT,
            b INT,
            UNIQUE KEY uk_expr((TIDB_SHARD(a)),a)
        )",
        Vec::new(),
    );
    tk.MustExec("ALTER TABLE t SET TIFLASH REPLICA 1", Vec::new());
    tk.MustExec(
        "SET @@SESSION.tidb_isolation_read_engines='tiflash'",
        Vec::new(),
    );
    tk.MustExec("SET @@SESSION.tidb_enforce_mpp=1", Vec::new());
    for row in tk
        .MustQuery("EXPLAIN SELECT MAX(b) FROM t", Vec::new())
        .Rows()
    {
        let line = row.join(" ");
        if line.contains("TableFullScan") {
            assert!(line.contains("tiflash"), "{line}");
        }
    }
    tk.MustExec("SET @@SESSION.tidb_enforce_mpp=0", Vec::new());
    tk.MustExec("SET @@SESSION.tidb_allow_mpp=0", Vec::new());
    let error = tk.QueryToErr("EXPLAIN SELECT MAX(b) FROM t");
    assert_eq!(
        error.message(),
        "[planner:1815]Internal : Can't find a proper physical plan for this query"
    );
}

#[test]
/// mysql.expr_pushdown_blacklist reload 后按 TiKV/TiFlash 分配残余过滤。
fn TestExprPushdownBlacklist() {
    let mut tk = new_testkit();
    tk.MustExec("DROP TABLE IF EXISTS t", Vec::new());
    tk.MustExec("CREATE TABLE t(a INT,b DATE)", Vec::new());
    tk.MustExec("SET @@SESSION.tidb_allow_tiflash_cop=ON", Vec::new());
    tk.MustExec("ALTER TABLE t SET TIFLASH REPLICA 1", Vec::new());
    tk.MustExec(
        "INSERT INTO mysql.expr_pushdown_blacklist VALUES
        ('<','tikv,tiflash,tidb','for test'),
        ('cast','tiflash','for test'),
        ('date_format','tikv','for test'),
        ('Cast.CastTimeAsDuration','tikv','for test')",
        Vec::new(),
    );
    tk.MustExec("ADMIN RELOAD EXPR_PUSHDOWN_BLACKLIST", Vec::new());
    tk.MustExec(
        "SET @@SESSION.tidb_isolation_read_engines='tiflash'",
        Vec::new(),
    );
    tk.MustExec(
        "SET @@SESSION.tidb_opt_enable_late_materialization=OFF",
        Vec::new(),
    );
    let sql = "EXPLAIN FORMAT='brief' SELECT * FROM test.t
        WHERE b>DATE'1988-01-01' AND b<DATE'1994-01-01'
          AND CAST(a AS DECIMAL(10,2))>10.10
          AND DATE_FORMAT(b,'%m')='11'";
    let rows = tk.MustQuery(sql, Vec::new()).Rows();
    assert_eq!(
        rows[0][4],
        "gt(cast(test.t.a, decimal(10,2) BINARY), 10.10), lt(test.t.b, 1994-01-01)"
    );
    assert_eq!(
        rows[2][4],
        "eq(date_format(test.t.b, \"%m\"), \"11\"), gt(test.t.b, 1988-01-01)"
    );

    tk.MustExec(
        "SET @@SESSION.tidb_isolation_read_engines='tikv'",
        Vec::new(),
    );
    let rows = tk.MustQuery(sql, Vec::new()).Rows();
    assert_eq!(
        rows[0][4],
        "eq(date_format(test.t.b, \"%m\"), \"11\"), lt(test.t.b, 1994-01-01)"
    );
    assert_eq!(
        rows[2][4],
        "gt(cast(test.t.a, decimal(10,2) BINARY), 10.10), gt(test.t.b, 1988-01-01)"
    );

    let rows = tk
        .MustQuery(
            "EXPLAIN FORMAT='brief' SELECT * FROM t WHERE CAST(b AS CHAR)='10:00:00'",
            Vec::new(),
        )
        .Rows();
    assert_eq!(rows[1][2], "cop[tikv]");
    assert_eq!(
        rows[1][4],
        "eq(cast(test.t.b, var_string(10)), \"10:00:00\")"
    );
    let rows = tk
        .MustQuery(
            "EXPLAIN FORMAT='brief' SELECT * FROM test.t WHERE HOUR(b)>10",
            Vec::new(),
        )
        .Rows();
    assert_eq!(rows[0][2], "root");
    assert_eq!(rows[0][4], "gt(hour(cast(test.t.b, time)), 10)");

    tk.MustExec("DROP TABLE IF EXISTS t0", Vec::new());
    tk.MustExec("CREATE TABLE t0(c0 DOUBLE,PRIMARY KEY(c0))", Vec::new());
    tk.MustExec("INSERT INTO t0 VALUES(1)", Vec::new());
    let with_pk = tk
        .MustQuery(
            "SELECT c0 FROM t0 WHERE ATAN2((t0.c0 IS NULL),-('a'))",
            Vec::new(),
        )
        .Rows();
    assert_eq!(with_pk, Rows(&["1"]));
    tk.MustExec("DROP TABLE t0", Vec::new());
    tk.MustExec("CREATE TABLE t0(c0 DOUBLE)", Vec::new());
    tk.MustExec("INSERT INTO t0 VALUES(1)", Vec::new());
    let without_pk = tk
        .MustQuery(
            "SELECT c0 FROM t0 WHERE ATAN2((t0.c0 IS NULL),-('a'))",
            Vec::new(),
        )
        .Rows();
    assert_eq!(without_pk, with_pk);

    tk.MustExec(
        "DELETE FROM mysql.expr_pushdown_blacklist WHERE reason='for test'",
        Vec::new(),
    );
    tk.MustExec("ADMIN RELOAD EXPR_PUSHDOWN_BLACKLIST", Vec::new());
}

#[test]
/// 从 DNF 每个分支抽取共同 CNF 项，保留不能约简的余项。
fn TestFilterExtractFromDNF() {
    use astersql_expression as expression;

    fn column(id: i64, name: &str) -> expression::ExprBox {
        let mut column = expression::Column::new(
            *expression::types::NewFieldType(expression::mysql::TypeLonglong),
            id,
            id,
            id as isize,
        );
        column.OrigName = format!("test.t.{name}");
        Box::new(column)
    }
    fn comparison(
        context: &astersql_expression_exprstatic::ExprContext,
        operator: &str,
        id: i64,
        name: &str,
        value: i64,
    ) -> expression::ExprBox {
        expression::NewFunctionBase(
            context,
            operator,
            *expression::types::NewFieldType(expression::mysql::TypeLonglong),
            vec![column(id, name), Box::new(expression::NewInt64Const(value))],
        )
        .unwrap()
    }
    fn and(
        context: &astersql_expression_exprstatic::ExprContext,
        expressions: Vec<expression::ExprBox>,
    ) -> expression::ExprBox {
        expression::ComposeCNFCondition(context, &expressions).unwrap()
    }
    fn or(
        context: &astersql_expression_exprstatic::ExprContext,
        expressions: Vec<expression::ExprBox>,
    ) -> expression::ExprBox {
        expression::ComposeDNFCondition(context, &expressions).unwrap()
    }
    fn or_left(
        context: &astersql_expression_exprstatic::ExprContext,
        expressions: Vec<expression::ExprBox>,
    ) -> expression::ExprBox {
        expressions
            .into_iter()
            .reduce(|left, right| {
                expression::NewFunctionBase(
                    context,
                    expression::ast::LogicOr,
                    *expression::types::NewFieldType(expression::mysql::TypeLonglong),
                    vec![left, right],
                )
                .unwrap()
            })
            .unwrap()
    }

    let mut context = astersql_expression_exprstatic::NewExprContext(Vec::new());
    let eq = |context: &astersql_expression_exprstatic::ExprContext, id, name, value| {
        comparison(context, expression::ast::EQ, id, name, value)
    };
    let gt = |context: &astersql_expression_exprstatic::ExprContext, id, name, value| {
        comparison(context, expression::ast::GT, id, name, value)
    };
    let lt = |context: &astersql_expression_exprstatic::ExprContext, id, name, value| {
        comparison(context, expression::ast::LT, id, name, value)
    };

    let cases = vec![
        (
            or_left(
                &context,
                vec![
                    eq(&context, 1, "a", 1),
                    eq(&context, 1, "a", 1),
                    eq(&context, 1, "a", 1),
                ],
            ),
            "[eq(test.t.a, 1)]",
        ),
        (
            or(
                &context,
                vec![
                    eq(&context, 1, "a", 1),
                    eq(&context, 1, "a", 1),
                    and(
                        &context,
                        vec![eq(&context, 1, "a", 1), eq(&context, 2, "b", 1)],
                    ),
                ],
            ),
            "[eq(test.t.a, 1)]",
        ),
        (
            or_left(
                &context,
                vec![
                    and(
                        &context,
                        vec![eq(&context, 1, "a", 1), eq(&context, 1, "a", 1)],
                    ),
                    eq(&context, 1, "a", 1),
                    eq(&context, 2, "b", 1),
                ],
            ),
            "[or(or(and(eq(test.t.a, 1), eq(test.t.a, 1)), eq(test.t.a, 1)), \
             eq(test.t.b, 1))]",
        ),
        (
            or(
                &context,
                vec![
                    and(
                        &context,
                        vec![eq(&context, 1, "a", 1), eq(&context, 2, "b", 2)],
                    ),
                    and(
                        &context,
                        vec![eq(&context, 1, "a", 1), eq(&context, 2, "b", 3)],
                    ),
                    and(
                        &context,
                        vec![eq(&context, 1, "a", 1), eq(&context, 2, "b", 4)],
                    ),
                ],
            ),
            "[eq(test.t.a, 1) or(eq(test.t.b, 2), or(eq(test.t.b, 3), \
             eq(test.t.b, 4)))]",
        ),
        (
            or(
                &context,
                vec![
                    and(
                        &context,
                        vec![
                            eq(&context, 1, "a", 1),
                            eq(&context, 2, "b", 1),
                            eq(&context, 3, "c", 1),
                        ],
                    ),
                    and(
                        &context,
                        vec![eq(&context, 1, "a", 1), eq(&context, 2, "b", 1)],
                    ),
                    and(
                        &context,
                        vec![
                            eq(&context, 1, "a", 1),
                            eq(&context, 2, "b", 1),
                            gt(&context, 3, "c", 2),
                            lt(&context, 3, "c", 3),
                        ],
                    ),
                ],
            ),
            "[eq(test.t.a, 1) eq(test.t.b, 1)]",
        ),
    ];
    for (condition, expected) in cases {
        let mut extracted = expression::ExtractFiltersFromDNFs(&mut context, vec![condition]);
        extracted.sort_by_key(|item| item.HashCode().to_vec());
        assert_eq!(
            expression::StringifyExpressionsWithCtx(context.GetEvalCtx(), &extracted),
            expected
        );
    }
}

#[test]
/// TIDB_DECODE_KEY 覆盖行键、索引键、表前缀、无效键告警与回归键。
fn TestTiDBDecodeKeyFunc() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("USE test", Vec::new());
    for (key, expected) in [
        (
            "74800000000000002B5F72800000000000A5D3",
            r#"{"_tidb_rowid":42451,"table_id":"43"}"#,
        ),
        (
            "74800000000000ffff5f7205bff199999999999a013131000000000000f9",
            r#"{"handle":"{1.1, 11}","table_id":65535}"#,
        ),
        (
            "74800000000000019B5F698000000000000001015257303100000000FB013736383232313130FF3900000000000000F8010000000000000000F7",
            r#"{"index_id":1,"index_vals":"RW01, 768221109, ","table_id":411}"#,
        ),
        (
            "7480000000000000695F698000000000000001038000000000004E20",
            r#"{"index_id":1,"index_vals":"20000","table_id":105}"#,
        ),
        ("7480000000000000FF4700000000000000F8", r#"{"table_id":71}"#),
        (
            "74800000000000012B5F72800000000000A5D3",
            r#"{"_tidb_rowid":42451,"table_id":"299"}"#,
        ),
    ] {
        tk.MustQuery(&format!("SELECT TIDB_DECODE_KEY('{key}')"), Vec::new())
            .Check(vec![vec![expected]]);
    }
    let invalid = "7480000000000000FF2E5F728000000011FFE1A3000000000000";
    tk.MustQuery(&format!("SELECT TIDB_DECODE_KEY('{invalid}')"), Vec::new())
        .Check(vec![vec![invalid]]);
    tk.MustQuery("SHOW WARNINGS", Vec::new())
        .CheckContain(&format!("invalid key: {invalid}"));
    let regression = "7480000000000100375F69800000000000000103800000000001D4C1023B6458";
    tk.MustQuery(
        &format!("SELECT TIDB_DECODE_KEY('{regression}')"),
        Vec::new(),
    )
    .Check(vec![vec![regression]]);

    tk.MustExec(
        "CREATE TABLE decoded(a INT PRIMARY KEY CLUSTERED,b INT)",
        Vec::new(),
    );
    let table_id = domain.stats_table("test", "decoded").unwrap().1.ID;
    assert!(
        domain.stats_table("test", "decoded").unwrap().1.PKIsHandle,
        "explicit CLUSTERED integer primary key must be the row handle"
    );
    let key = tk
        .MustQuery(
            "SELECT TIDB_ENCODE_RECORD_KEY('test','decoded',10)",
            Vec::new(),
        )
        .Rows()[0][0]
        .clone();
    tk.MustQuery(&format!("SELECT TIDB_DECODE_KEY('{key}')"), Vec::new())
        .Check(vec![vec![format!(r#"{{"a":10,"table_id":"{table_id}"}}"#)]]);

    tk.MustExec(
        "CREATE TABLE decoded_nc(a INT PRIMARY KEY NONCLUSTERED,b INT)",
        Vec::new(),
    );
    let nonclustered = domain.stats_table("test", "decoded_nc").unwrap().1;
    assert!(!nonclustered.PKIsHandle);
    let key = tk
        .MustQuery(
            "SELECT TIDB_ENCODE_RECORD_KEY('test','decoded_nc',10)",
            Vec::new(),
        )
        .Rows()[0][0]
        .clone();
    tk.MustQuery(&format!("SELECT TIDB_DECODE_KEY('{key}')"), Vec::new())
        .Check(vec![vec![format!(
            r#"{{"_tidb_rowid":10,"table_id":"{}"}}"#,
            nonclustered.ID
        )]]);

    tk.MustExec(
        "CREATE TABLE decoded_p(a INT PRIMARY KEY CLUSTERED,b INT,KEY bk(b)) \
         PARTITION BY RANGE(a)(PARTITION p0 VALUES LESS THAN(10), \
         PARTITION p1 VALUES LESS THAN(20))",
        Vec::new(),
    );
    let table = domain.stats_table("test", "decoded_p").unwrap().1;
    let partition_id = table.GetPartitionInfo().unwrap().Definitions[0].ID;
    let key = tk
        .MustQuery(
            "SELECT TIDB_ENCODE_RECORD_KEY('test','decoded_p(p0)',1)",
            Vec::new(),
        )
        .Rows()[0][0]
        .clone();
    tk.MustQuery(&format!("SELECT TIDB_DECODE_KEY('{key}')"), Vec::new())
        .Check(vec![vec![format!(
            r#"{{"a":1,"partition_id":{partition_id},"table_id":"{}"}}"#,
            table.ID
        )]]);
    let prefix = format!("74{:016x}", (partition_id as u64) ^ (1_u64 << 63));
    tk.MustQuery(&format!("SELECT TIDB_DECODE_KEY('{prefix}')"), Vec::new())
        .Check(vec![vec![format!(
            r#"{{"partition_id":{partition_id},"table_id":{}}}"#,
            table.ID
        )]]);
    let key = tk
        .MustQuery(
            "SELECT TIDB_ENCODE_INDEX_KEY('test','decoded_p(p0)','bk',100)",
            Vec::new(),
        )
        .Rows()[0][0]
        .clone();
    tk.MustQuery(
        &format!("SELECT TIDB_DECODE_KEY('{key}')"),
        Vec::new(),
    )
    .Check(vec![vec![format!(
        r#"{{"index_id":1,"index_vals":{{"b":"100"}},"partition_id":{partition_id},"table_id":{}}}"#,
        table.ID
    )]]);
}

#[test]
/// TIDB_ENCODE_RECORD/INDEX_KEY 使用真实表、索引和 HASH 分区物理 ID。
fn TestTiDBEncodeKey() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("USE test", Vec::new());
    tk.MustExec("CREATE TABLE t(a INT PRIMARY KEY,b INT)", Vec::new());
    tk.MustExec("INSERT INTO t VALUES(1,1)", Vec::new());
    let table_id = domain.stats_table("test", "t").unwrap().1.ID;
    tk.MustContainErrMsg(
        "SELECT TIDB_ENCODE_RECORD_KEY('test','t1',0)",
        "doesn't exist",
    );
    tk.MustQuery("SELECT TIDB_ENCODE_RECORD_KEY('test','t',1)", Vec::new())
        .Check(vec![vec![format!(
            "74{:016x}5f728000000000000001",
            (table_id as u64) ^ (1_u64 << 63)
        )]]);
    tk.MustExec("ALTER TABLE t ADD INDEX i(b)", Vec::new());
    tk.MustContainErrMsg(
        "SELECT TIDB_ENCODE_INDEX_KEY('test','t','i1',1)",
        "index not found",
    );
    tk.MustQuery(
        "SELECT TIDB_ENCODE_INDEX_KEY('test','t','i',1,1)",
        Vec::new(),
    )
    .Check(vec![vec![format!(
        "74{:016x}5f698000000000000001038000000000000001038000000000000001",
        (table_id as u64) ^ (1_u64 << 63)
    )]]);

    tk.MustExec(
        "CREATE TABLE t1(a INT PRIMARY KEY,b INT) PARTITION BY HASH(a) PARTITIONS 4",
        Vec::new(),
    );
    tk.MustExec("INSERT INTO t1 VALUES(1,1)", Vec::new());
    let table = domain.stats_table("test", "t1").unwrap().1;
    let partition_id = table.GetPartitionInfo().unwrap().Definitions[1].ID;
    let key = format!(
        "74{:016x}5f728000000000000001",
        (partition_id as u64) ^ (1_u64 << 63)
    );
    tk.MustQuery(
        "SELECT TIDB_ENCODE_RECORD_KEY('test','t1(p1)',1)",
        Vec::new(),
    )
    .Check(vec![vec![key.clone()]]);
    assert_ne!(
        tk.MustQuery(&format!("SELECT TIDB_MVCC_INFO('{key}')"), Vec::new())
            .Rows()[0][0],
        r#"{"info":{}}"#
    );
}

#[test]
/// JSON/VARCHAR 列上的 RFC 7396 矩阵及非法 JSON 错误。
fn TestBuiltinFuncJSONMergePatch_InColumn() {
    let mut tk = new_testkit();
    tk.MustExec(
        "CREATE TABLE t(
            id INT NOT NULL AUTO_INCREMENT,
            j JSON NULL,
            vc VARCHAR(5000) NULL,
            PRIMARY KEY(id)
        )",
        Vec::new(),
    );
    let cases = [
        (
            Some(r#"{"a":"b"}"#),
            Some(r#"{"a":"c"}"#),
            Some(r#"{"a":"c"}"#),
        ),
        (
            Some(r#"{"a":"b"}"#),
            Some(r#"{"b":"c"}"#),
            Some(r#"{"a":"b","b":"c"}"#),
        ),
        (Some(r#"{"a":"b"}"#), Some(r#"{"a":null}"#), Some("{}")),
        (
            Some(r#"{"a":"b","b":"c"}"#),
            Some(r#"{"a":null}"#),
            Some(r#"{"b":"c"}"#),
        ),
        (
            Some(r#"{"a":["b"]}"#),
            Some(r#"{"a":"c"}"#),
            Some(r#"{"a":"c"}"#),
        ),
        (
            Some(r#"{"a":"c"}"#),
            Some(r#"{"a":["b"]}"#),
            Some(r#"{"a":["b"]}"#),
        ),
        (
            Some(r#"{"a":{"b":"c"}}"#),
            Some(r#"{"a":{"b":"d","c":null}}"#),
            Some(r#"{"a":{"b":"d"}}"#),
        ),
        (
            Some(r#"{"a":[{"b":"c"}]}"#),
            Some(r#"{"a":[1]}"#),
            Some(r#"{"a":[1]}"#),
        ),
        (
            Some(r#"["a","b"]"#),
            Some(r#"["c","d"]"#),
            Some(r#"["c","d"]"#),
        ),
        (Some(r#"{"a":"b"}"#), Some(r#"["c"]"#), Some(r#"["c"]"#)),
        (Some(r#"{"a":"foo"}"#), Some("null"), Some("null")),
        (Some(r#"{"a":"foo"}"#), Some(r#""bar""#), Some(r#""bar""#)),
        (
            Some(r#"{"e":null}"#),
            Some(r#"{"a":1}"#),
            Some(r#"{"e":null,"a":1}"#),
        ),
        (
            Some(r#"[1,2]"#),
            Some(r#"{"a":"b","c":null}"#),
            Some(r#"{"a":"b"}"#),
        ),
        (
            Some("{}"),
            Some(r#"{"a":{"bb":{"ccc":null}}}"#),
            Some(r#"{"a":{"bb":{}}}"#),
        ),
        (
            Some(
                r#"{"title":"Goodbye!","author":{"givenName":"John","familyName":"Doe"},"tags":["example","sample"],"content":"This will be unchanged"}"#,
            ),
            Some(
                r#"{"title":"Hello!","phoneNumber":"+01-123-456-7890","author":{"familyName":null},"tags":["example"]}"#,
            ),
            Some(
                r#"{"title":"Hello!","author":{"givenName":"John"},"tags":["example"],"content":"This will be unchanged","phoneNumber":"+01-123-456-7890"}"#,
            ),
        ),
        (None, Some(r#"{"a":1}"#), None),
        (Some(r#"{"a":1}"#), None, None),
        (Some(r#"{"a":"foo"}"#), Some("true"), Some("true")),
        (Some(r#"{"a":"foo"}"#), Some("false"), Some("false")),
        (Some(r#"{"a":"foo"}"#), Some("123"), Some("123")),
        (Some(r#"{"a":"foo"}"#), Some("123.1"), Some("123.1")),
        (Some(r#"{"a":"foo"}"#), Some("[1,2,3]"), Some("[1,2,3]")),
        (Some("null"), Some(r#"{"a":1}"#), Some(r#"{"a":1}"#)),
        (Some(r#"{"a":1}"#), Some("null"), Some("null")),
    ];
    for (index, (target, patch, expected)) in cases.into_iter().enumerate() {
        let literal = |value: Option<&str>| {
            value.map_or_else(
                || "NULL".to_owned(),
                |value| format!("'{}'", value.replace('\'', "''")),
            )
        };
        tk.MustExec(
            &format!(
                "INSERT INTO t VALUES({},{},{})",
                index + 1,
                literal(target),
                literal(patch)
            ),
            Vec::new(),
        );
        let result = tk.MustQuery(
            &format!(
                "SELECT JSON_MERGE_PATCH(j,vc) FROM t WHERE id={}",
                index + 1
            ),
            Vec::new(),
        );
        if let Some(expected) = expected {
            let canonical = ParseBinaryJSONFromString(expected).unwrap().String();
            result.Check(vec![vec![canonical]]);
        } else {
            result.Check(Rows(&["<nil>"]));
        }
    }
    tk.MustExec(r#"INSERT INTO t VALUES(100,'{"a":1}','[1]}')"#, Vec::new());
    tk.MustContainErrMsg(
        "SELECT JSON_MERGE_PATCH(j,vc) FROM t WHERE id=100",
        "Invalid JSON text",
    );
}

#[test]
/// 常量表达式 JSON_MERGE_PATCH 覆盖多参数、NULL、标量替换与非法文本。
fn TestBuiltinFuncJSONMergePatch_InExpression() {
    let tk = new_testkit();
    let cases: &[(&[Option<&str>], Option<&str>)] = &[
        (
            &[Some(r#"{"a":"b"}"#), Some(r#"{"a":"c"}"#)],
            Some(r#"{"a":"c"}"#),
        ),
        (
            &[Some(r#"{"a":"b"}"#), Some(r#"{"b":"c"}"#)],
            Some(r#"{"a":"b","b":"c"}"#),
        ),
        (&[Some(r#"{"a":"b"}"#), Some(r#"{"a":null}"#)], Some("{}")),
        (
            &[
                Some(r#"{"a":{"b":"c"}}"#),
                Some(r#"{"a":{"b":"d","c":null}}"#),
            ],
            Some(r#"{"a":{"b":"d"}}"#),
        ),
        (&[None, Some("1")], Some("1")),
        (&[Some("1"), None], None),
        (
            &[
                Some(r#"{"a":"foo"}"#),
                Some(r#"{"a":null}"#),
                Some(r#"{"b":"123"}"#),
                Some(r#"{"c":1}"#),
            ],
            Some(r#"{"b":"123","c":1}"#),
        ),
        (
            &[
                Some(r#"{"a":"foo"}"#),
                Some(r#"{"a":null}"#),
                Some(r#"{"c":1}"#),
            ],
            Some(r#"{"c":1}"#),
        ),
        (
            &[Some(r#"{"a":"foo"}"#), Some(r#"{"a":null}"#), Some("true")],
            Some("true"),
        ),
        (
            &[Some("null"), Some("true"), Some("[1,2,3]")],
            Some("[1,2,3]"),
        ),
        (
            &[
                Some("true"),
                Some("false"),
                Some("[]"),
                Some("{}"),
                Some("null"),
            ],
            Some("null"),
        ),
        (
            &[
                Some("false"),
                Some("[]"),
                Some("{}"),
                Some("null"),
                Some("true"),
            ],
            Some("true"),
        ),
        (
            &[
                Some("true"),
                Some("[]"),
                Some("{}"),
                Some("null"),
                Some("false"),
            ],
            Some("false"),
        ),
        (
            &[
                Some("true"),
                Some("false"),
                Some("{}"),
                Some("null"),
                Some("[]"),
            ],
            Some("[]"),
        ),
        (
            &[
                Some("true"),
                Some("false"),
                Some("{}"),
                Some("null"),
                Some("1"),
            ],
            Some("1"),
        ),
        (
            &[
                Some("true"),
                Some("false"),
                Some("{}"),
                Some("null"),
                Some("1.8"),
            ],
            Some("1.8"),
        ),
        (
            &[
                Some("true"),
                Some("false"),
                Some("{}"),
                Some("null"),
                Some(r#""112""#),
            ],
            Some(r#""112""#),
        ),
        (&[Some(r#"{"a":"foo"}"#), Some("123.1")], Some("123.1")),
        (
            &[None, Some("null"), Some("[1,2,3]"), Some(r#"{"a":1}"#)],
            Some(r#"{"a":1}"#),
        ),
        (
            &[Some("null"), None, Some("[1,2,3]"), Some(r#"{"a":1}"#)],
            Some(r#"{"a":1}"#),
        ),
        (
            &[Some("null"), Some("[1,2,3]"), None, Some(r#"{"a":1}"#)],
            None,
        ),
        (
            &[Some("null"), Some("[1,2,3]"), Some(r#"{"a":1}"#), None],
            None,
        ),
        (
            &[None, Some("null"), Some(r#"{"a":1}"#), Some("[1,2,3]")],
            Some("[1,2,3]"),
        ),
        (
            &[Some("null"), Some(r#"{"a":1}"#), None, Some("[1,2,3]")],
            Some("[1,2,3]"),
        ),
        (
            &[Some("null"), Some(r#"{"a":1}"#), Some("[1,2,3]"), None],
            None,
        ),
        (&[Some(r#"{"a":"foo"}"#), Some("false")], Some("false")),
        (&[Some(r#"{"a":"foo"}"#), Some("123")], Some("123")),
        (&[Some(r#"{"a":"foo"}"#), Some("[1,2,3]")], Some("[1,2,3]")),
        (&[Some("null"), Some(r#"{"a":1}"#)], Some(r#"{"a":1}"#)),
    ];
    for (arguments, expected) in cases {
        let arguments = arguments
            .iter()
            .map(|value| {
                value.map_or_else(
                    || "NULL".to_owned(),
                    |value| format!("'{}'", value.replace('\'', "''")),
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let result = tk.MustQuery(&format!("SELECT JSON_MERGE_PATCH({arguments})"), Vec::new());
        if let Some(expected) = expected {
            let canonical = ParseBinaryJSONFromString(expected).unwrap().String();
            result.Check(vec![vec![canonical]]);
        } else {
            result.Check(Rows(&["<nil>"]));
        }
    }
    assert!(
        tk.QueryToErr(r#"SELECT JSON_MERGE_PATCH('{"a":1}','jjj','null')"#)
            .message()
            .contains("Invalid JSON text")
    );
}

#[test]
fn TestCompareBuiltin() {
    let mut tk = new_testkit();
    tk.MustExec(
        "CREATE TABLE t(pk INT NOT NULL PRIMARY KEY AUTO_INCREMENT, i INT, j JSON)",
        Vec::new(),
    );
    for (i, json) in [
        (0, "NULL"),
        (1, r#"'{"a":2}'"#),
        (2, "'[1,2]'"),
        (3, r#"'\"scalar string\"'"#),
        (4, "'true'"),
        (5, "'false'"),
        (6, "'null'"),
        (7, "'-1'"),
        (8, "'32768'"),
        (9, "'3.14'"),
        (10, "'{}'"),
        (11, "'[]'"),
    ] {
        tk.MustExec(
            &format!("INSERT INTO t(i,j) VALUES({i},{json})"),
            Vec::new(),
        );
    }
    tk.MustQuery(
        "SELECT i, j = j, j <=> NULL, j = NULL FROM t ORDER BY i",
        Vec::new(),
    )
    .Check(Rows(&[
        "0 <nil> 1 <nil>",
        "1 1 0 <nil>",
        "2 1 0 <nil>",
        "3 1 0 <nil>",
        "4 1 0 <nil>",
        "5 1 0 <nil>",
        "6 1 0 <nil>",
        "7 1 0 <nil>",
        "8 1 0 <nil>",
        "9 1 0 <nil>",
        "10 1 0 <nil>",
        "11 1 0 <nil>",
    ]));

    tk.MustQuery(
        "SELECT COALESCE(NULL), COALESCE(NULL,NULL), COALESCE(NULL,1)",
        Vec::new(),
    )
    .Check(Rows(&["<nil> <nil> 1"]));
    tk.MustQuery(
        "SELECT COALESCE(CAST(1 AS JSON),CAST(2 AS JSON)), \
         COALESCE(NULL,CAST(2 AS JSON)), COALESCE(CAST(1 AS JSON),NULL)",
        Vec::new(),
    )
    .Check(Rows(&["1 2 1"]));
    tk.MustQuery(
        "SELECT NULLIF(NULL,1), NULLIF(1,NULL), NULLIF(1,1), \
         NULLIF(NULL,NULL), NULLIF(1,1.0), NULLIF(1,'1.0'), \
         NULLIF('abc',1), NULLIF(1+2,1), NULLIF(1,1+2), \
         NULLIF(2+3,1+2), HEX(NULLIF('abc',1))",
        Vec::new(),
    )
    .Check(Rows(&["<nil> 1 <nil> <nil> <nil> <nil> abc 3 1 5 616263"]));

    tk.MustQuery(
        "SELECT INTERVAL(NULL,1,2), INTERVAL(1,2,3), INTERVAL(2,1,3), \
         INTERVAL(3,1,2), INTERVAL(0,'b','1','2'), INTERVAL('a','b','1','2')",
        Vec::new(),
    )
    .Check(Rows(&["-1 0 1 2 1 1"]));
    tk.MustQuery(
        "SELECT INTERVAL(23,1,23,23,23,30,44,200), \
         INTERVAL(23,1.7,15.3,23.1,30,44,200), \
         INTERVAL(9007199254740992,9007199254740993), \
         INTERVAL(100,NULL,NULL,NULL,NULL,NULL,100)",
        Vec::new(),
    )
    .Check(Rows(&["4 2 0 6"]));
    tk.MustQuery(
        "SELECT GREATEST(1,2,3), GREATEST('a','b','c'), \
         GREATEST(1.1,1.2,1.3), GREATEST('123a',1,2), \
         LEAST(1,2,3), LEAST('a','b','c'), LEAST(1.1,1.2,1.3), \
         LEAST('123a',1,2)",
        Vec::new(),
    )
    .Check(Rows(&["3 c 1.3 2 1 a 1.1 1"]));
    tk.MustQuery(
        "SELECT 1 < 17666000000000000000, \
         1 > 17666000000000000000, 1 = 17666000000000000000",
        Vec::new(),
    )
    .Check(Rows(&["1 0 0"]));
    tk.MustQuery(
        "SELECT ROW(1,2,3)=ROW(1,2,3), ROW(1,2,3)=ROW(1+3,2,3), \
         ROW(1,2,3)<>ROW(1,2,3), ROW(1,2,3)<>ROW(1+3,2,3), \
         ROW(1+3,2,3)<>ROW(1+3,2,3)",
        Vec::new(),
    )
    .Check(Rows(&["1 0 0 1 0"]));
}

#[test]
fn TestCastJSONTimeDuration() {
    let mut tk = new_testkit();
    tk.MustExec("CREATE TABLE t(i INT, j JSON)", Vec::new());
    for sql in [
        "INSERT INTO t VALUES (0, DATE('1998-06-13'))",
        "INSERT INTO t VALUES (1, CAST('1998-06-13 12:12:12' AS DATETIME))",
        "INSERT INTO t VALUES (2, DATE('1596-03-31'))",
        "INSERT INTO t VALUES (3, CAST('1596-03-31 12:12:12' AS DATETIME))",
        r#"INSERT INTO t VALUES (4, '"1596-03-31 12:12:12"')"#,
        r#"INSERT INTO t VALUES (5, '"12:12:12"')"#,
        "INSERT INTO t VALUES (6, CAST('12:12:12' AS TIME))",
    ] {
        tk.MustExec(sql, Vec::new());
    }
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let today_datetime = format!("{today} 12:12:12");
    tk.MustQuery(
        "SELECT i, CAST(j AS DATE), CAST(j AS DATETIME), CAST(j AS TIME), JSON_TYPE(j) \
         FROM t ORDER BY i",
        Vec::new(),
    )
    .Check(vec![
        vec!["0", "1998-06-13", "1998-06-13 00:00:00", "00:00:00", "DATE"],
        vec![
            "1",
            "1998-06-13",
            "1998-06-13 12:12:12",
            "12:12:12",
            "DATETIME",
        ],
        vec!["2", "1596-03-31", "1596-03-31 00:00:00", "00:00:00", "DATE"],
        vec![
            "3",
            "1596-03-31",
            "1596-03-31 12:12:12",
            "12:12:12",
            "DATETIME",
        ],
        vec![
            "4",
            "1596-03-31",
            "1596-03-31 12:12:12",
            "12:12:12",
            "STRING",
        ],
        vec![
            "5",
            "2012-12-12",
            "2012-12-12 00:00:00",
            "12:12:12",
            "STRING",
        ],
        vec![
            "6",
            today.as_str(),
            today_datetime.as_str(),
            "12:12:12",
            "TIME",
        ],
    ]);
}

#[test]
fn TestTimestamp() {
    let mut tk = new_testkit();
    tk.MustExec("SET time_zone = '+00:00'", Vec::new());
    let timestamp1 = tk.MustQuery("SELECT @@timestamp", Vec::new()).Rows()[0][0]
        .parse::<f64>()
        .unwrap();
    let now1 = tk.MustQuery("SELECT NOW(6)", Vec::new()).Rows()[0][0].clone();
    tk.MustExec("SET @@timestamp = 12345", Vec::new());
    tk.MustQuery("SELECT @@timestamp", Vec::new())
        .Check(Rows(&["12345"]));
    tk.MustQuery("SELECT NOW()", Vec::new())
        .Check(vec![vec!["1970-01-01 03:25:45"]]);
    tk.MustExec("SET @@timestamp = DEFAULT", Vec::new());
    std::thread::sleep(std::time::Duration::from_micros(2));
    let timestamp2 = tk.MustQuery("SELECT @@timestamp", Vec::new()).Rows()[0][0]
        .parse::<f64>()
        .unwrap();
    let now2 = tk.MustQuery("SELECT NOW(6)", Vec::new()).Rows()[0][0].clone();
    assert!(timestamp1 < timestamp2);
    assert!(now1 < now2);

    tk.MustExec("SET @@timestamp = 12345", Vec::new());
    tk.MustQuery("SELECT NOW()", Vec::new())
        .Check(vec![vec!["1970-01-01 03:25:45"]]);
    tk.MustExec("SET @@timestamp = 0", Vec::new());
    std::thread::sleep(std::time::Duration::from_micros(2));
    let timestamp3 = tk.MustQuery("SELECT @@timestamp", Vec::new()).Rows()[0][0]
        .parse::<f64>()
        .unwrap();
    assert!(timestamp2 < timestamp3);
}

#[test]
fn TestIssue9710() {
    let mut tk = new_testkit();
    let row = tk
        .MustQuery(
            "SELECT NOW(), NOW(6), UNIX_TIMESTAMP(), UNIX_TIMESTAMP(NOW())",
            Vec::new(),
        )
        .Rows()
        .remove(0);
    assert_eq!(&row[0][..19], &row[1][..19]);
    assert_eq!(row[2], row[3]);
}

#[test]
fn TestEnumIndex() {
    let mut tk = new_testkit();
    tk.MustExec("CREATE TABLE t(e ENUM('c','a','b'))", Vec::new());
    tk.MustExec(
        "CREATE TABLE tidx(e ENUM('c','a','b'), INDEX idx(e))",
        Vec::new(),
    );
    let values = (0..50)
        .map(|index| format!("({})", index % 3 + 1))
        .collect::<Vec<_>>()
        .join(",");
    tk.MustExec(&format!("INSERT INTO t VALUES {values}"), Vec::new());
    tk.MustExec(&format!("INSERT INTO tidx VALUES {values}"), Vec::new());
    for operator in ["=", "!=", ">", ">=", "<", "<="] {
        for operand in [
            "'a'", "'b'", "'c'", "'d'", "''", "1", "2", "3", "4", "0", "-1",
        ] {
            let condition = format!("e {operator} {operand}");
            let expected = tk
                .MustQuery(&format!("SELECT * FROM t WHERE {condition}"), Vec::new())
                .Sort()
                .Rows();
            tk.MustQuery(&format!("SELECT * FROM tidx WHERE {condition}"), Vec::new())
                .Sort()
                .Check(expected);
        }
    }

    tk.MustExec("DROP TABLE t", Vec::new());
    tk.MustExec(
        "CREATE TABLE t(e ENUM('d','c','b','a'), a INT, INDEX idx(e))",
        Vec::new(),
    );
    tk.MustExec("INSERT INTO t VALUES(1,1),(2,2),(3,3),(4,4)", Vec::new());
    tk.MustQuery(
        "SELECT /*+ USE_INDEX(t, idx) */ * FROM t \
         WHERE e NOT IN ('a','d') AND a = 2",
        Vec::new(),
    )
    .Check(Rows(&["c 2"]));

    tk.MustExec("DROP TABLE t", Vec::new());
    tk.MustExec(
        "CREATE TABLE t(col1 ENUM('a','b','c'), col2 ENUM('a','b','c'), \
         col3 INT, INDEX idx(col1,col2))",
        Vec::new(),
    );
    tk.MustExec("INSERT INTO t VALUES(1,1,1),(2,2,2),(3,3,3)", Vec::new());
    for predicate in ["col2 BETWEEN 'b' AND 'b'", "col2 = 'b'"] {
        tk.MustQuery(
            &format!(
                "SELECT /*+ USE_INDEX(t,idx) */ col3 FROM t \
                 WHERE {predicate} AND col1 IS NOT NULL"
            ),
            Vec::new(),
        )
        .Check(Rows(&["2"]));
    }

    tk.MustExec("DROP TABLE t", Vec::new());
    tk.MustExec(
        "CREATE TABLE t(e ENUM('a','b','c'), INDEX idx(e))",
        Vec::new(),
    );
    tk.MustExec("INSERT IGNORE INTO t VALUES(0),(1),(2),(3)", Vec::new());
    tk.MustQuery("SELECT * FROM t WHERE e = ''", Vec::new())
        .Check(vec![vec![""]]);
    tk.MustQuery("SELECT * FROM t WHERE e != 'a'", Vec::new())
        .Sort()
        .Check(vec![vec![""], vec!["b"], vec!["c"]]);
    tk.MustExec("ALTER TABLE t DROP INDEX idx", Vec::new());
    tk.MustQuery("SELECT * FROM t WHERE e != 'a'", Vec::new())
        .Sort()
        .Check(vec![vec![""], vec!["b"], vec!["c"]]);
}

#[test]
/// 小初始 Chunk、单并发扫描下反复复用输出列，200 行 NULL 位图和值均正确。
fn TestDecodetoChunkReuse() {
    let mut tk = new_testkit();
    tk.MustExec("CREATE TABLE chk(a INT,b VARCHAR(20))", Vec::new());
    for index in 0..200 {
        if index % 5 == 0 {
            tk.MustExec("INSERT INTO chk VALUES(NULL,NULL)", Vec::new());
        } else {
            tk.MustExec(
                &format!("INSERT INTO chk VALUES({index},'{index}')"),
                Vec::new(),
            );
        }
    }
    tk.MustExec("SET tidb_distsql_scan_concurrency=1", Vec::new());
    tk.MustExec("SET tidb_init_chunk_size=2", Vec::new());
    tk.MustExec("SET tidb_max_chunk_size=32", Vec::new());
    let rows = tk.MustQuery("SELECT * FROM chk", Vec::new()).Rows();
    assert_eq!(rows.len(), 200);
    for (index, row) in rows.iter().enumerate() {
        if index % 5 == 0 {
            assert_eq!(row, &vec!["<nil>".to_owned(), "<nil>".to_owned()]);
        } else {
            assert_eq!(row, &vec![index.to_string(), index.to_string()]);
        }
    }
}

#[test]
/// SEM 对所有用户禁止 SELECT INTO OUTFILE。
fn TestSecurityEnhancedMode() {
    use astersql_sessionctx_vardef as vardef;
    use astersql_sessionctx_variable as variable;

    if variable::GetSysVar(vardef::Hostname).is_none() {
        variable::RegisterSysVar(variable::SysVar {
            Scope: vardef::ScopeNone,
            Name: vardef::Hostname.into(),
            Value: vardef::DefHostname.into(),
            ..variable::SysVar::default()
        });
    }
    if variable::GetSysVar(vardef::TiDBEnableEnhancedSecurity).is_none() {
        variable::RegisterSysVar(variable::SysVar {
            Scope: vardef::ScopeNone,
            Name: vardef::TiDBEnableEnhancedSecurity.into(),
            Value: vardef::Off.into(),
            Type: vardef::TypeStr,
            ..variable::SysVar::default()
        });
    }
    let restore = astersql_util_sem_compat::SwitchToSEMForTest(astersql_util_sem_compat::V1);
    let mut tk = new_testkit();
    tk.MustGetErrMsg(
        "SELECT 1 INTO OUTFILE '/tmp/aaaa'",
        "[planner:8132]Feature 'SELECT INTO' is not supported when security enhanced mode is enabled",
    );
    restore();
}

#[test]
/// sql_require_primary_key 对 CREATE 与删除主键的 DDL 门禁。
fn TestPrimaryKeyRequiredSysvar() {
    let mut tk = new_testkit();
    tk.MustExec("CREATE TABLE t(name VARCHAR(60),age INT)", Vec::new());
    tk.MustExec("DROP TABLE t", Vec::new());
    tk.MustExec("SET @@sql_require_primary_key=TRUE", Vec::new());
    tk.MustContainErrMsg(
        "CREATE TABLE t(name VARCHAR(60),age INT)",
        "without a primary key",
    );
    tk.MustExec(
        "CREATE TABLE t(
            id BIGINT NOT NULL PRIMARY KEY AUTO_RANDOM,
            name VARCHAR(60),
            age INT
        )",
        Vec::new(),
    );
    tk.MustGetErrMsg(
        "ALTER TABLE t DROP COLUMN id",
        "[ddl:8200]Unsupported drop integer primary key",
    );
    tk.MustExec(
        "CREATE TABLE t2(
            id INT NOT NULL,
            c1 INT DEFAULT NULL,
            PRIMARY KEY(id) NONCLUSTERED
        )",
        Vec::new(),
    );
    tk.MustContainErrMsg("ALTER TABLE t2 DROP COLUMN id", "Primary Key covered");
    tk.MustContainErrMsg("ALTER TABLE t2 DROP PRIMARY KEY", "without a primary key");
}
