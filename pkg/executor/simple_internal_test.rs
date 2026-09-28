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

// Simple 执行器内部单元测试。

use crate::simple::alterUserHasPrivilegedOptions;
use astersql_parser::New;
use astersql_parser_ast::AlterUserStmt;

#[test]
fn alter_user_has_privileged_options() {
    let cases = [
        ("ALTER USER u IDENTIFIED BY 'x'", false),
        (
            "ALTER USER u IDENTIFIED BY 'x' RETAIN CURRENT PASSWORD",
            false,
        ),
        ("ALTER USER u DISCARD OLD PASSWORD", false),
        (
            "ALTER USER USER() IDENTIFIED BY 'x' RETAIN CURRENT PASSWORD",
            false,
        ),
        ("ALTER USER u REQUIRE SSL", true),
        ("ALTER USER u REQUIRE NONE", true),
        ("ALTER USER u WITH MAX_USER_CONNECTIONS 10", true),
        ("ALTER USER u PASSWORD EXPIRE", true),
        ("ALTER USER u ACCOUNT LOCK", true),
        ("ALTER USER u FAILED_LOGIN_ATTEMPTS 3", true),
        ("ALTER USER u PASSWORD HISTORY 5", true),
        ("ALTER USER u COMMENT 'c'", true),
        ("ALTER USER u ATTRIBUTE '{\"k\": \"v\"}'", true),
        ("ALTER USER u RESOURCE GROUP rg1", true),
        ("ALTER USER u DISCARD OLD PASSWORD COMMENT 'c'", true),
        (
            "ALTER USER u IDENTIFIED BY 'x' RETAIN CURRENT PASSWORD ACCOUNT LOCK",
            true,
        ),
    ];

    let mut parser = New();
    for (sql, privileged) in cases {
        let stmt = parser
            .ParseOneStmt(sql, "", "")
            .unwrap_or_else(|error| panic!("{sql}: {error}"));
        let alter_stmt = stmt
            .as_any()
            .downcast_ref::<AlterUserStmt>()
            .unwrap_or_else(|| panic!("{sql}: expected AlterUserStmt"));
        assert_eq!(
            alterUserHasPrivilegedOptions(alter_stmt),
            privileged,
            "{sql}"
        );
    }
}
