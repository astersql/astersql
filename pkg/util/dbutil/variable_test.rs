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

// `ShowServerID` / `ShowGrants` 查询拼装与密码脱敏行为的单元测试。
//
// 内嵌 `GO_REFERENCE` 保留 Go 侧 sqlmock 测试原文供对照；实际可运行断言
// 用 `Fixture` 模拟 `QueryExecutor`，验证全局变量读取、GRANT 查询与
// `IDENTIFIED BY PASSWORD` 明文占位替换为 `'secret'`。

const GO_REFERENCE: &str = r################"
//

// TestShowGrants 对应 Go 测试：无 user/host 参数时查询 CURRENT_USER 并返回所有 grant 行。
#[test]
fn test_show_grants() {
    let ctx = context::Background();
    let (db, mock) = sqlmock::New().expect("sqlmock.New should succeed");

    let mock_grants = vec![
        "GRANT ALL PRIVILEGES ON *.* TO 'root'@'localhost' WITH GRANT OPTION",
        "GRANT PROXY ON ''@'' TO 'root'@'localhost' WITH GRANT OPTION",
    ];
    let mut rows = sqlmock::NewRows(vec!["Grants for root@localhost"]);
    for grant in &mock_grants {
        rows.AddRow(grant);
    }
    mock.ExpectQuery("^SHOW GRANTS FOR CURRENT_USER$").WillReturnRows(rows);

    let (grants, err) = ShowGrants(ctx, db, "", "");
    assert!(err.is_none());
    assert_eq!(mock_grants, grants);
    assert!(mock.ExpectationsWereMet().is_none());
}

// TestShowGrantsWithRoles 对应 Go 测试：先读取角色授权，再用 USING roles 重新查询完整权限。
#[test]
fn test_show_grants_with_roles() {
    let ctx = context::Background();
    let (db, mock) = sqlmock::New().expect("sqlmock.New should succeed");

    let mock_grants_without_roles = vec![
        "GRANT USAGE ON *.* TO `u1`@`localhost`",
        "GRANT `r1`@`%`,`r2`@`%` TO `u1`@`localhost`",
    ];
    let mut rows1 = sqlmock::NewRows(vec!["Grants for root@localhost"]);
    for grant in &mock_grants_without_roles {
        rows1.AddRow(grant);
    }
    mock.ExpectQuery("^SHOW GRANTS FOR CURRENT_USER$").WillReturnRows(rows1);

    let mock_grants_with_roles = vec![
        "GRANT USAGE ON *.* TO `u1`@`localhost`",
        "GRANT SELECT, INSERT, UPDATE, DELETE ON `db1`.* TO `u1`@`localhost`",
        "GRANT `r1`@`%`,`r2`@`%` TO `u1`@`localhost`",
    ];
    let mut rows2 = sqlmock::NewRows(vec!["Grants for root@localhost"]);
    for grant in &mock_grants_with_roles {
        rows2.AddRow(grant);
    }
    // Go 实现解析出 r1/r2 后拼成 USING `r1`@`%`, `r2`@`%` 再查一次。
    mock.ExpectQuery("^SHOW GRANTS FOR CURRENT_USER USING `r1`@`%`, `r2`@`%`$").WillReturnRows(rows2);

    let (grants, err) = ShowGrants(ctx, db, "", "");
    assert!(err.is_none());
    assert_eq!(mock_grants_with_roles, grants);
    assert!(mock.ExpectationsWereMet().is_none());
}

// passwordMaskCase 对应 Go 匿名结构体：记录原始 grant 和期望的密码占位替换结果。
struct PasswordMaskCase {
    original: &'static str,
    expected: &'static str,
}

// TestShowGrantsPasswordMasked 对应 Go 测试：IDENTIFIED BY PASSWORD 的明文/缺省位置统一补成 'secret'。
#[test]
fn test_show_grants_password_masked() {
    let ctx = context::Background();
    let (db, mock) = sqlmock::New().expect("sqlmock.New should succeed");

    let cases = vec![
        PasswordMaskCase {
            original: "GRANT ALL PRIVILEGES ON *.* TO 'root'@'localhost' IDENTIFIED BY PASSWORD <secret> WITH GRANT OPTION",
            expected: "GRANT ALL PRIVILEGES ON *.* TO 'root'@'localhost' IDENTIFIED BY PASSWORD 'secret' WITH GRANT OPTION",
        },
        PasswordMaskCase {
            original: "GRANT ALL PRIVILEGES ON *.* TO 'user'@'%' IDENTIFIED BY PASSWORD",
            expected: "GRANT ALL PRIVILEGES ON *.* TO 'user'@'%' IDENTIFIED BY PASSWORD 'secret'",
        },
        PasswordMaskCase {
            original: "GRANT ALL PRIVILEGES ON *.* TO 'user'@'%' IDENTIFIED BY PASSWORD WITH GRANT OPTION",
            expected: "GRANT ALL PRIVILEGES ON *.* TO 'user'@'%' IDENTIFIED BY PASSWORD 'secret' WITH GRANT OPTION",
        },
        PasswordMaskCase {
            original: "GRANT ALL PRIVILEGES ON *.* TO 'user'@'%' IDENTIFIED BY PASSWORD <secret>",
            expected: "GRANT ALL PRIVILEGES ON *.* TO 'user'@'%' IDENTIFIED BY PASSWORD 'secret'",
        },
    ];

    for case in cases {
        let rows = sqlmock::NewRows(vec!["Grants for root@localhost"]).AddRow(case.original);
        mock.ExpectQuery("^SHOW GRANTS FOR CURRENT_USER$").WillReturnRows(rows);

        let (grants, err) = ShowGrants(ctx, db, "", "");
        assert!(err.is_none());
        assert_eq!(1, grants.len());
        assert_eq!(case.expected, grants[0]);
        assert!(mock.ExpectationsWereMet().is_none());
    }
}
"################;

use crate::variable::{ShowGrants, ShowServerID, ShowVersion};
use crate::{DbError, QueryExecutor, QueryResult, Value};
use std::sync::Mutex;

/// 记录收到的 SQL，并按查询前缀返回固定结果，供隔离断言。
struct Fixture {
    /// 按调用顺序记录传入的查询文本。
    queries: Mutex<Vec<String>>,
}
impl QueryExecutor for Fixture {
    fn QueryContext(&self, query: &str, _args: &[Value]) -> Result<QueryResult, DbError> {
        self.queries.lock().unwrap().push(query.to_owned());
        // 全局变量路径：返回 server_id=42，供 ShowServerID 解析。
        if query.starts_with("SHOW GLOBAL VARIABLES") {
            return Ok(QueryResult {
                columns: vec!["Variable_name".into(), "Value".into()],
                rows: vec![vec!["server_id".into(), "42".into()]],
            });
        }
        // 其余视为 SHOW GRANTS：返回缺省密码占位，供脱敏逻辑补全 'secret'。
        Ok(QueryResult {
            columns: vec!["Grants".into()],
            rows: vec![vec![
                "GRANT ALL ON *.* TO 'root'@'localhost' IDENTIFIED BY PASSWORD".into(),
            ]],
        })
    }
}

/// 断言 ShowServerID / ShowGrants 使用规范 SQL，且密码占位被脱敏为 'secret'。
#[test]
fn variables_and_grants_use_canonical_queries_and_password_masking() {
    let fixture = Fixture {
        queries: Mutex::new(Vec::new()),
    };
    assert_eq!(ShowServerID(&fixture).unwrap(), 42);
    let grants = ShowGrants(&fixture, "", "").unwrap();
    assert_eq!(
        grants[0],
        "GRANT ALL ON *.* TO 'root'@'localhost' IDENTIFIED BY PASSWORD 'secret'"
    );
    assert!(fixture.queries.lock().unwrap()[1].starts_with("SHOW GRANTS FOR CURRENT_USER"));
}

#[test]
fn variable_wrappers_and_explicit_user_query_match_go() {
    let fixture = Fixture {
        queries: Mutex::new(Vec::new()),
    };
    assert_eq!(ShowVersion(&fixture).unwrap(), "42");
    assert_eq!(crate::variable::ShowLogBin(&fixture).unwrap(), "42");
    assert_eq!(crate::variable::ShowBinlogFormat(&fixture).unwrap(), "42");
    assert_eq!(crate::variable::ShowBinlogRowImage(&fixture).unwrap(), "42");
    assert_eq!(
        crate::variable::ShowMySQLVariable(&fixture, "server_id").unwrap(),
        "42"
    );
    let grants = ShowGrants(&fixture, "alice", "localhost").unwrap();
    assert_eq!(grants.len(), 1);
    let queries = fixture.queries.lock().unwrap();
    assert!(
        queries
            .iter()
            .any(|query| query == "SHOW GRANTS FOR 'alice'@'localhost'")
    );
}

struct NullGrantFixture;

impl QueryExecutor for NullGrantFixture {
    fn QueryContext(&self, _query: &str, _args: &[Value]) -> Result<QueryResult, DbError> {
        Ok(QueryResult {
            columns: vec!["Grants".into()],
            rows: vec![vec![Value::Null]],
        })
    }
}

#[test]
fn show_grants_does_not_silently_drop_null_rows() {
    assert!(ShowGrants(&NullGrantFixture, "", "").is_err());
}

/// Go uses the supplied identifiers verbatim when constructing SHOW statements.
#[test]
fn show_queries_preserve_go_identifier_interpolation() {
    let fixture = Fixture {
        queries: Mutex::new(Vec::new()),
    };

    ShowVersion(&fixture).unwrap();
    ShowGrants(&fixture, "o'reilly", "local'host").unwrap();

    assert_eq!(
        fixture.queries.lock().unwrap().as_slice(),
        [
            "SHOW GLOBAL VARIABLES LIKE 'version';",
            "SHOW GRANTS FOR 'o'reilly'@'local'host'",
        ]
    );
}

struct RoleFixture {
    queries: Mutex<Vec<String>>,
}

impl QueryExecutor for RoleFixture {
    fn QueryContext(&self, query: &str, _args: &[Value]) -> Result<QueryResult, DbError> {
        self.queries.lock().unwrap().push(query.to_owned());
        let grants = if query.contains(" USING ") {
            vec!["GRANT SELECT ON *.* TO `u1`@`localhost`".into()]
        } else {
            vec!["GRANT `role ON call`@`%` TO `u1`@`localhost`".into()]
        };
        Ok(QueryResult {
            columns: vec!["Grants for u1@localhost".into()],
            rows: grants.into_iter().map(|grant| vec![grant]).collect(),
        })
    }
}

/// Role identifiers may contain the token ` ON ` inside quotes; Go's parser still
/// recognizes the GrantRoleStmt and performs the second query.
#[test]
fn quoted_role_containing_on_is_expanded() {
    let fixture = RoleFixture {
        queries: Mutex::new(Vec::new()),
    };

    assert_eq!(
        ShowGrants(&fixture, "", "").unwrap(),
        ["GRANT SELECT ON *.* TO `u1`@`localhost`"]
    );
    assert_eq!(
        fixture.queries.lock().unwrap().as_slice(),
        [
            "SHOW GRANTS FOR CURRENT_USER",
            "SHOW GRANTS FOR CURRENT_USER USING `role ON call`@`%`",
        ]
    );
}
