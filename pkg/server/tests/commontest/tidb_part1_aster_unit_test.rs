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

// TiDB server 公共协议回归测试（part1）。
//
// 覆盖 Regression 场景下的 DDL/DML/查询顺序，以及 MySQL DSN 对库名转义、
// sql mode 参数与游标 fetchSize 的编码行为。

use astersql_server_internal_testserverclient::{
    ExecuteResult, QueryResult, SqlExecutor, SqlValue, TestServerClient,
};

/// 内存版 `SqlExecutor`：复现 Go `RunTestRegression` 触达的 SQL 语义。
#[derive(Default)]
struct ProtocolExecutor {
    /// 已执行/查询过的 SQL 文本序列。
    statements: Vec<String>,
    /// 模拟表 `test` 中的行值。
    rows: Vec<i64>,
}

impl SqlExecutor for ProtocolExecutor {
    /// 执行写语句并保持 Go 用例断言的 affected rows 与参数契约。
    fn execute(&mut self, sql: &str, parameters: &[SqlValue]) -> Result<ExecuteResult, String> {
        self.statements.push(sql.to_owned());
        let rows_affected = match sql {
            "CREATE TABLE test (val TINYINT)" | "select user()" => 0,
            "INSERT INTO test VALUES (1)" => {
                self.rows.push(1);
                1
            }
            "UPDATE test SET val = 0 WHERE val = ?" => {
                if parameters != [SqlValue::Signed(1)] {
                    return Err(format!("unexpected UPDATE parameters: {parameters:?}"));
                }
                let affected = self.rows.iter().filter(|value| **value == 1).count() as u64;
                self.rows.iter_mut().for_each(|value| {
                    if *value == 1 {
                        *value = 0;
                    }
                });
                affected
            }
            "DELETE FROM test WHERE val = 0" => {
                let before = self.rows.len();
                self.rows.retain(|value| *value != 0);
                (before - self.rows.len()) as u64
            }
            "DELETE FROM test" => {
                let affected = self.rows.len() as u64;
                self.rows.clear();
                affected
            }
            _ => return Err(format!("unexpected execute SQL: {sql}")),
        };
        Ok(ExecuteResult {
            rows_affected,
            last_insert_id: 0,
        })
    }

    /// 执行查询，覆盖空表、布尔值、常量与非 nil 空字节参数回显。
    fn query(&mut self, sql: &str, parameters: &[SqlValue]) -> Result<QueryResult, String> {
        self.statements.push(sql.to_owned());
        let rows = match sql {
            "select user()" => vec![vec![SqlValue::Text("root@localhost".to_owned())]],
            "SELECT * FROM test" | "SELECT val FROM test" => self
                .rows
                .iter()
                .copied()
                .map(|value| vec![SqlValue::Signed(value)])
                .collect(),
            "SELECT 1" => vec![vec![SqlValue::Signed(1)]],
            "SELECT ?" if parameters == [SqlValue::Bytes(Vec::new())] => {
                vec![vec![SqlValue::Bytes(Vec::new())]]
            }
            "SELECT ?" => return Err(format!("unexpected echo parameters: {parameters:?}")),
            _ => return Err(format!("unexpected query SQL: {sql}")),
        };
        Ok(QueryResult {
            columns: vec!["val".to_owned()],
            rows,
        })
    }
}

/// 验证 Go `TestRegression` 的完整执行、查询、更新、删除与二进制回显契约。
#[test]
fn regression_scenario_matches_go_run_test_regression_contract() {
    let client = TestServerClient::new();
    let mut executor = ProtocolExecutor::default();
    client
        .run_test_regression(&mut executor)
        .expect("regression protocol scenario");

    assert!(executor.rows.is_empty());
    assert_eq!(
        executor.statements,
        vec![
            "select user()",
            "CREATE TABLE test (val TINYINT)",
            "SELECT * FROM test",
            "INSERT INTO test VALUES (1)",
            "SELECT val FROM test",
            "UPDATE test SET val = 0 WHERE val = ?",
            "SELECT val FROM test",
            "DELETE FROM test WHERE val = 0",
            "DELETE FROM test",
            "SELECT 1",
            "SELECT ?",
        ]
    );
}

/// 验证 DSN 对含 `/` 的库名与带空格的参数做百分号编码，且游标 DSN 附带 fetchSize。
#[test]
fn mysql_dsn_preserves_cursor_and_escaped_database_options() {
    let mut client = TestServerClient::new();
    client.port = 4000;
    // 覆盖库名与 sql mode，检查特殊字符在 DSN query 中的转义。
    let override_database =
        |config: &mut astersql_server_internal_testserverclient::MysqlConfig| {
            config.database = "db/name".to_owned();
            config
                .parameters
                .insert("sql mode".to_owned(), "STRICT_ALL_TABLES".to_owned());
        };
    let dsn = client.get_dsn(&[&override_database]);
    assert_eq!(
        dsn,
        "root@tcp(127.0.0.1:4000)/db%2Fname?sql+mode=STRICT_ALL_TABLES"
    );
    assert_eq!(
        client.get_dsn_with_cursor(128),
        "root@tcp(127.0.0.1:4000)/test?fetchSize=128"
    );
}
