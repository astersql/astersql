// Copyright 2022 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// ExecutableChecker 解析与执行路径的单元测试。
//
// `test_parse` 用真实 parser + StatementFromAst 校验表依赖清单；
// `test_execute` 用 testkit mock store 验证 SQL 能否在 test 库执行。
// DDL 指数据定义语言（CREATE/DROP/ALTER 等）。

use astersql_parser as parser;
use astersql_testkit::NewTestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_util_logutil::log::{InitLogger, LogConfig};

use super::{
    CheckerError, CheckerParser, CheckerResult, CheckerSession, ExecutableCheckerFactory,
    ExecutionContext, GetTablesNeededExist, GetTablesNeededNonExist, NewExecutableChecker,
    Statement, StatementFromAst,
};

// parseTestData 对应 Go 的测试用例结构，字段顺序保持原始声明。
/// 单条用例：SQL、是否应解析成功、存在/不存在表清单、是否应执行成功。
#[allow(non_camel_case_types)]
struct parseTestData {
    /// 待测 SQL 文本。
    sql: &'static str,
    /// 期望 Parse 成功。
    parseSucceeded: bool,
    /// 期望必须已存在的表；None 表示非 DDL（忽略清单）。
    tableNeededExist: Option<&'static [&'static str]>,
    /// 期望必须尚不存在的表；None 表示非 DDL。
    tableNeededNonExist: Option<&'static [&'static str]>,
    /// 期望在 testkit 中执行成功。
    executeSucceeded: bool,
}

// setUpTestData 对应 Go 的 list.List fixture；这里用 Vec 保留 PushBack 顺序。
/// 构造与 Go list 顺序一致的解析/执行用例集。
fn setUpTestData() -> Vec<parseTestData> {
    vec![
        parseTestData {
            sql: "drop table if exists t1,t2,t3,t4,t5;",
            parseSucceeded: true,
            tableNeededExist: Some(&["t1", "t2", "t3", "t4", "t5"]),
            tableNeededNonExist: Some(&[]),
            executeSucceeded: true,
        },
        parseTestData {
            sql: "drop database if exists mysqltest;",
            parseSucceeded: true,
            tableNeededExist: Some(&[]),
            tableNeededNonExist: Some(&[]),
            executeSucceeded: true,
        },
        parseTestData {
            sql: "create table t1 (b char(0));",
            parseSucceeded: true,
            tableNeededExist: Some(&[]),
            tableNeededNonExist: Some(&["t1"]),
            executeSucceeded: true,
        },
        parseTestData {
            sql: "insert into t1 values (''),(null);",
            parseSucceeded: true,
            tableNeededExist: None,
            tableNeededNonExist: None,
            executeSucceeded: true,
        },
        parseTestData {
            sql: "select * from t1;",
            parseSucceeded: true,
            tableNeededExist: None,
            tableNeededNonExist: None,
            executeSucceeded: true,
        },
        parseTestData {
            sql: "drop table if exists t1;",
            parseSucceeded: true,
            tableNeededExist: Some(&["t1"]),
            tableNeededNonExist: Some(&[]),
            executeSucceeded: true,
        },
        parseTestData {
            sql: "create table t1 (b char(0) not null);",
            parseSucceeded: true,
            tableNeededExist: Some(&[]),
            tableNeededNonExist: Some(&["t1"]),
            executeSucceeded: true,
        },
        parseTestData {
            sql: "create table if not exists t1 (b char(0) not null);",
            parseSucceeded: true,
            tableNeededExist: Some(&[]),
            tableNeededNonExist: Some(&["t1"]),
            executeSucceeded: true,
        },
        parseTestData {
            sql: "insert into t1 values (''),(null);",
            parseSucceeded: true,
            tableNeededExist: None,
            tableNeededNonExist: None,
            executeSucceeded: false,
        },
        parseTestData {
            sql: "select * from t1;",
            parseSucceeded: true,
            tableNeededExist: None,
            tableNeededNonExist: None,
            executeSucceeded: true,
        },
        parseTestData {
            sql: "drop table t1;",
            parseSucceeded: true,
            tableNeededExist: Some(&["t1"]),
            tableNeededNonExist: Some(&[]),
            executeSucceeded: true,
        },
        parseTestData {
            sql: "create table t(a int comment '[[range=1,10]]');",
            parseSucceeded: true,
            tableNeededExist: Some(&[]),
            tableNeededNonExist: Some(&["t"]),
            executeSucceeded: true,
        },
    ]
}

/// Real session adapter used by `NewExecutableChecker` for parse tests.
/// Charset/collation match TiDB session defaults; Execute is unused by TestParse.
/// 解析测试用的 session：utf8mb4/utf8mb4_bin；Execute 空实现。
#[derive(Default)]
struct CheckerParseSession {
    /// 是否已 close。
    closed: bool,
}

impl CheckerSession for CheckerParseSession {
    fn execute(&mut self, _context: &ExecutionContext, _sql: &str) -> CheckerResult<()> {
        Ok(())
    }

    fn charset_info(&self) -> (String, String) {
        ("utf8mb4".to_owned(), "utf8mb4_bin".to_owned())
    }

    fn close(&mut self) {
        self.closed = true;
    }
}

/// Real parser adapter: ParseOneStmt + StatementFromAst, matching Go Parse/GetTablesNeeded*.
/// A fresh parser is created per call because `astersql_parser::Parser` is not `Send`.
/// 真实解析适配：每次新建 Parser（非 Send），再映射为 Statement。
struct CheckerAstParser;

impl CheckerParser for CheckerAstParser {
    fn parse_one_statement(
        &mut self,
        sql: &str,
        charset: &str,
        collation: &str,
    ) -> CheckerResult<Statement> {
        let mut parser = parser::New();
        let node = parser
            .ParseOneStmt(sql, charset, collation)
            .map_err(|error| CheckerError::new(error.to_string()))?;
        Ok(StatementFromAst(node.as_ref()))
    }
}

/// Factory matching Go `NewExecutableChecker`: InitLogger(error) + session + parser.
/// 对齐 Go 构造：error 级 logger + 解析 session + AST parser。
struct DefaultExecutableCheckerFactory;

impl ExecutableCheckerFactory for DefaultExecutableCheckerFactory {
    fn initialize_error_logger(&self) -> CheckerResult<()> {
        let config = LogConfig {
            level: "error".to_owned(),
            ..LogConfig::default()
        };
        InitLogger(&config).map_err(CheckerError::new)?;
        Ok(())
    }

    fn create_bootstrapped_session(&self) -> CheckerResult<Box<dyn CheckerSession>> {
        Ok(Box::new(CheckerParseSession::default()))
    }

    fn create_parser(&self) -> CheckerResult<Box<dyn CheckerParser>> {
        Ok(Box::new(CheckerAstParser))
    }
}

/// 将静态表名切片转为 `Option<Vec<String>>`，便于与 GetTablesNeeded* 比较。
fn expected_tables(value: Option<&'static [&'static str]>) -> Option<Vec<String>> {
    value.map(|items| items.iter().map(|item| (*item).to_owned()).collect())
}

// TestParse 对应 Go 的 TestParse：逐条解析 SQL，并检查 DDL 需要存在/不存在的表清单。
/// 逐条 Parse，断言存在/不存在表清单与 Go 用例一致。
#[test]
fn test_parse() {
    let mut ec = NewExecutableChecker(&DefaultExecutableCheckerFactory)
        .expect("NewExecutableChecker should succeed");
    let test_data = setUpTestData();
    for data in test_data {
        let stmt = match ec.Parse(data.sql) {
            Ok(stmt) => stmt,
            Err(_) => {
                assert!(
                    !data.parseSucceeded,
                    "parse unexpectedly failed for {}",
                    data.sql
                );
                continue;
            }
        };
        // Go ignores errors with `_`, leaving nil slices for non-DDL statements.
        // 非 DDL 时 GetTablesNeeded* 返回 Err，`.ok()` 得到 None，对齐 Go 的 nil slice。
        let table_needed_exist = GetTablesNeededExist(&stmt).ok();
        let table_needed_non_exist = GetTablesNeededNonExist(&stmt).ok();
        assert!(data.parseSucceeded, "parse should succeed for {}", data.sql);
        assert_eq!(
            expected_tables(data.tableNeededExist),
            table_needed_exist,
            "tableNeededExist mismatch for {}",
            data.sql
        );
        assert_eq!(
            expected_tables(data.tableNeededNonExist),
            table_needed_non_exist,
            "tableNeededNonExist mismatch for {}",
            data.sql
        );
    }
    ec.Close().expect("ExecutableChecker close should succeed");
}

/// Go 的 DDLNode 类型断言涵盖所有嵌入 ddlNode 的语句。
#[test]
fn go_ddl_node_fallback_is_classified_as_ddl() {
    let mut parser = CheckerAstParser;
    let statement = parser
        .parse_one_statement(
            "create masking policy p on t(c) as c",
            "utf8mb4",
            "utf8mb4_bin",
        )
        .expect("CREATE MASKING POLICY should parse");

    assert_eq!(Statement::OtherDdl, statement);
    assert_eq!(
        Vec::<String>::new(),
        GetTablesNeededExist(&statement).unwrap()
    );
    assert_eq!(
        Vec::<String>::new(),
        GetTablesNeededNonExist(&statement).unwrap()
    );

    let alter_database = parser
        .parse_one_statement(
            "alter database db charset = utf8mb4",
            "utf8mb4",
            "utf8mb4_bin",
        )
        .expect("ALTER DATABASE should parse");
    assert_eq!(Statement::OtherDdl, alter_database);
    assert_eq!(
        Vec::<String>::new(),
        GetTablesNeededExist(&alter_database).unwrap()
    );
    assert_eq!(
        Vec::<String>::new(),
        GetTablesNeededNonExist(&alter_database).unwrap()
    );

    use astersql_parser_ast::{
        CleanupTableLockStmt, FlashBackTableStmt, FlashBackToTimestampStmt, LockTablesStmt, Node,
        RecoverTableStmt, UnlockTablesStmt,
    };
    let fallback_nodes: [&dyn Node; 6] = [
        &LockTablesStmt::default(),
        &UnlockTablesStmt::default(),
        &CleanupTableLockStmt::default(),
        &RecoverTableStmt::default(),
        &FlashBackToTimestampStmt::default(),
        &FlashBackTableStmt::default(),
    ];
    for node in fallback_nodes {
        assert_eq!(Statement::OtherDdl, StatementFromAst(node));
    }
}

// TestExecute 对应 Go 的 TestExecute：用 testkit mock store 在 test 数据库中验证 SQL 是否可执行。
/// 在 mock store 的 test 库中逐条 Exec，断言执行成败与用例一致。
#[test]
fn test_execute() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    let test_data = setUpTestData();
    tk.MustExec("use test;", Vec::new());
    for data in test_data {
        let succeeded = tk.Exec(data.sql, Vec::new()).is_ok();
        assert_eq!(
            data.executeSucceeded, succeeded,
            "executeSucceeded mismatch for {}",
            data.sql
        );
    }
}
