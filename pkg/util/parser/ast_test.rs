// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// `SimpleCases` 快路径集成测试。
//
// 用真实 parser AST 验证未带 schema 的表名会被补全为 `default_db.table`。

use astersql_parser as parser;

/// 单条 SimpleCases 用例：原始 SQL、默认库、期望补全结果。
struct SimpleCaseTest {
    /// 待解析的 INSERT SQL。
    sql: &'static str,
    /// 补全用的默认库名。
    db: &'static str,
    /// 期望的补全后 SQL。
    ans: &'static str,
}

/// 覆盖若干简单 INSERT：无 schema、已有 schema、带列列表、表名恰为 `value`。
#[test]
fn test_simple_cases() {
    let tests = [
        SimpleCaseTest {
            sql: "insert into t values(1, 2)",
            db: "test",
            ans: "insert into test.t values(1, 2)",
        },
        SimpleCaseTest {
            sql: "insert into mydb.t values(1, 2)",
            db: "test",
            ans: "insert into mydb.t values(1, 2)",
        },
        SimpleCaseTest {
            sql: "insert into t(a, b) values(1, 2)",
            db: "test",
            ans: "insert into test.t(a, b) values(1, 2)",
        },
        SimpleCaseTest {
            sql: "insert into value value(2, 3)",
            db: "test",
            ans: "insert into test.value value(2, 3)",
        },
    ];

    for test in tests {
        let mut p = parser::New();
        let stmt = p
            .ParseOneStmt(test.sql, "", "")
            .expect("ParseOneStmt should succeed");
        let (ans, ok) = utilparser::SimpleCases(stmt.as_ref(), test.db, test.sql);
        assert!(ok, "SimpleCases should match simple insert");
        assert_eq!(test.ans, ans);
    }
}

#[test]
fn real_ast_drives_default_db_and_restore() {
    let mut parser = parser::New();
    let implicit = parser
        .ParseOneStmt("select a from t where a = 1", "", "")
        .expect("SELECT should parse");
    assert_eq!(utilparser::GetDefaultDB(implicit.as_ref(), "test"), "test");
    assert_eq!(
        utilparser::RestoreWithDefaultDB(implicit.as_ref(), "test", "select a from t where a = 1"),
        "SELECT `a` FROM `test`.`t` WHERE `a` = 1"
    );
    assert_eq!(
        utilparser::RestoreWithoutDB(implicit.as_ref()),
        "SELECT `a` FROM `t` WHERE `a` = 1"
    );

    let explicit = parser
        .ParseOneStmt("select a from db.t", "", "")
        .expect("SELECT should parse");
    assert_eq!(utilparser::GetDefaultDB(explicit.as_ref(), "test"), "");
}

#[test]
fn default_db_walks_through_statement_wrappers_like_go_visitor() {
    let implicit = parser::New()
        .ParseOneStmt("explain select a from t", "", "")
        .expect("EXPLAIN SELECT should parse");
    assert_eq!(utilparser::GetDefaultDB(implicit.as_ref(), "test"), "test");

    let explicit = parser::New()
        .ParseOneStmt("explain select a from db.t", "", "")
        .expect("qualified EXPLAIN SELECT should parse");
    assert_eq!(utilparser::GetDefaultDB(explicit.as_ref(), "test"), "");
}

#[test]
fn real_dml_nodes_use_the_same_restore_path() {
    let cases = [
        (
            "insert into t(a) values(1) on duplicate key update a=2",
            "INSERT INTO `test`.`t` (`a`) VALUES (1) ON DUPLICATE KEY UPDATE `a` = 2",
        ),
        (
            "update t set a=1 where b=2",
            "UPDATE `test`.`t` SET `a` = 1 WHERE `b` = 2",
        ),
        (
            "delete from t where a=1",
            "DELETE FROM `test`.`t` WHERE `a` = 1",
        ),
    ];
    for (sql, expected) in cases {
        let statement = parser::New()
            .ParseOneStmt(sql, "", "")
            .expect("DML should parse");
        assert_eq!(
            utilparser::RestoreWithDefaultDB(statement.as_ref(), "test", sql),
            expected
        );
    }
}
