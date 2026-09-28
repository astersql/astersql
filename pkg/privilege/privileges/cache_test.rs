// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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
// 权限缓存单元测试：用行数据夹具驱动 `Load*Table` / 匹配 / 角色图逻辑。
//
// 对应 Go `cache_test.go`。本 crate 无 SQL 执行器，故以 `RowDataSource`
// 注入与 `INSERT INTO mysql.*` 等价的列值，再经真实 `decode*TableRow`
// 解码，覆盖用户/库/表/列权限加载、主机通配与 CIDR、角色 BFS 等。

// 权限缓存单元测试：用 `RowDataSource` 回放系统表行，覆盖加载/匹配/角色图等。
//
// 移植自 Go `cache_test.go`；因无 SQL executor，以合成 `PrivilegeRow` 驱动真实解码路径。

// Ported from cache_test.go.
//
// Go builds fixtures by running real `INSERT INTO mysql.user/db/...` SQL
// through a `testkit`-backed mockstore/session, then calls
// `MySQLPrivilege.Load*Table(sqlExecutor)`. This crate has no SQL executor or
// session layer, so `Load*Table` takes a `&dyn PrivilegeDataSource` instead of
// a SQL executor. `RowDataSource` below plays that role: each test builds
// `PrivilegeRow` maps that mirror the column values the original `INSERT`
// statements would have produced, and `RowDataSource` replays the exact same
// production `decode*TableRow` functions Go's SQL-driven path would invoke,
// so the row-decoding logic under test is fully real, not re-implemented.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::*;

// `pub(crate)` here (and on the helpers below) lets the sibling
// `privileges_test` module reuse the exact same fixture-building technique
// instead of re-implementing it: both files need to feed synthetic
// `mysql.user`/`db`/... rows through the real `decode*TableRow` functions in
// place of Go's `testkit`-driven `INSERT INTO mysql...` + `Load*Table(sqlExecutor)`.
/// 合成权限表行数据源：各字段对应 `mysql.user/db/tables_priv/...` 行集合。
/// 合成权限数据源：按表持有行，经真实 `decode*TableRow` 产出记录。
#[derive(Default, Clone)]
pub(crate) struct RowDataSource {
    /// `mysql.user` 行。
    pub(crate) user: Vec<PrivilegeRow>,
    /// `mysql.global_priv` 行（SSL/密码过期等 JSON）。
    pub(crate) global_priv: Vec<PrivilegeRow>,
    /// `mysql.global_grants` 行动态权限。
    pub(crate) dynamic_priv: Vec<PrivilegeRow>,
    /// `mysql.db` 行。
    pub(crate) db: Vec<PrivilegeRow>,
    /// `mysql.tables_priv` 行。
    pub(crate) tables_priv: Vec<PrivilegeRow>,
    /// `mysql.columns_priv` 行。
    pub(crate) columns_priv: Vec<PrivilegeRow>,
    /// `mysql.default_roles` 行。
    pub(crate) default_roles: Vec<PrivilegeRow>,
    /// `mysql.role_edges` 行（角色授予边）。
    pub(crate) role_edges: Vec<PrivilegeRow>,
}

impl PrivilegeDataSource for RowDataSource {
    fn users(&self, default_auth_plugin: &str) -> Result<Vec<UserRecord>, PrivilegeError> {
        let mut scratch = MySQLPrivilege::default();
        scratch.SetGlobalVarsAccessor(default_auth_plugin);
        for row in &self.user {
            scratch.decodeUserTableRow(row)?;
        }
        Ok(scratch.user)
    }
    fn global_privileges(&self) -> Result<Vec<globalPrivRecord>, PrivilegeError> {
        let mut scratch = MySQLPrivilege::default();
        for row in &self.global_priv {
            scratch.decodeGlobalPrivTableRow(row)?;
        }
        Ok(scratch.global_priv)
    }
    fn dynamic_privileges(&self) -> Result<Vec<dynamicPrivRecord>, PrivilegeError> {
        let mut scratch = MySQLPrivilege::default();
        for row in &self.dynamic_priv {
            scratch.decodeGlobalGrantsTableRow(row)?;
        }
        Ok(scratch.dynamic_priv)
    }
    fn databases(&self) -> Result<Vec<dbRecord>, PrivilegeError> {
        let mut scratch = MySQLPrivilege::default();
        for row in &self.db {
            scratch.decodeDBTableRow(row)?;
        }
        Ok(scratch.db)
    }
    fn tables(&self) -> Result<Vec<tablesPrivRecord>, PrivilegeError> {
        let mut scratch = MySQLPrivilege::default();
        for row in &self.tables_priv {
            scratch.decodeTablesPrivTableRow(row)?;
        }
        Ok(scratch.tables_priv)
    }
    fn columns(&self) -> Result<Vec<columnsPrivRecord>, PrivilegeError> {
        let mut scratch = MySQLPrivilege::default();
        for row in &self.columns_priv {
            scratch.decodeColumnsPrivTableRow(row)?;
        }
        Ok(scratch.columns_priv)
    }
    fn default_roles(&self) -> Result<Vec<defaultRoleRecord>, PrivilegeError> {
        let mut scratch = MySQLPrivilege::default();
        for row in &self.default_roles {
            scratch.decodeDefaultRoleTableRow(row)?;
        }
        Ok(scratch.default_roles)
    }
    fn role_graph(&self) -> Result<HashMap<RoleIdentity, roleGraphEdgesTable>, PrivilegeError> {
        let mut scratch = MySQLPrivilege::default();
        for row in &self.role_edges {
            scratch.decodeRoleEdgesTable(row)?;
        }
        Ok(scratch.role_graph)
    }
}

/// 将 `(列名, JSON 值)` 对转为一条 `PrivilegeRow`。
/// 由键值对构造一行 `PrivilegeRow`。
pub(crate) fn row(pairs: &[(&str, Value)]) -> PrivilegeRow {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.clone()))
        .collect()
}

/// 构造 `mysql.user` 行；`privs` 为不加 `_priv` 后缀的小写特权名。
/// Builds a `mysql.user` row. `privs` lists the (lowercase, no `_priv` suffix)
/// privilege column names to mark `'Y'`, matching Go's `INSERT INTO mysql.user
/// (Host, User, ..., Select_priv, ...) VALUES (..., "Y", ...)` fixtures.
/// 构造 `mysql.user` 行；`privs` 为小写权限名（无 `_priv` 后缀），对应列置 `'Y'`。
pub(crate) fn user_row(host: &str, user: &str, privs: &[&str]) -> PrivilegeRow {
    let mut fields: Vec<(String, Value)> =
        vec![("host".into(), json!(host)), ("user".into(), json!(user))];
    for name in privs {
        fields.push((format!("{name}_priv"), json!("Y")));
    }
    fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// 构造 `mysql.db` 行，语义同 [`user_row`]。
/// 构造 `mysql.db` 行。
pub(crate) fn db_row(host: &str, db: &str, user: &str, privs: &[&str]) -> PrivilegeRow {
    let mut fields: Vec<(String, Value)> = vec![
        ("host".into(), json!(host)),
        ("db".into(), json!(db)),
        ("user".into(), json!(user)),
    ];
    for name in privs {
        fields.push((format!("{name}_priv"), json!("Y")));
    }
    fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// 构造 `mysql.tables_priv` 行（Table_priv/Column_priv 为逗号分隔 SET）。
/// Builds a `mysql.tables_priv` row (`Table_priv`/`Column_priv` hold
/// comma-joined SET literals, e.g. `"Select,Insert"`), matching Go's `INSERT
/// INTO mysql.tables_priv (..., Table_priv, Column_priv) VALUES (...)`.
/// 构造 `mysql.tables_priv` 行；`table_priv`/`column_priv` 为逗号分隔 SET 字面量。
pub(crate) fn tables_priv_row(
    host: &str,
    db: &str,
    user: &str,
    table: &str,
    table_priv: &str,
    column_priv: &str,
) -> PrivilegeRow {
    row(&[
        ("host", json!(host)),
        ("db", json!(db)),
        ("user", json!(user)),
        ("table_name", json!(table)),
        ("table_priv", json!(table_priv)),
        ("column_priv", json!(column_priv)),
    ])
}

/// 构造 `mysql.columns_priv` 行。
/// Builds a `mysql.columns_priv` row, matching Go's `INSERT INTO
/// mysql.columns_priv (..., Column_priv) VALUES (...)`.
/// 构造 `mysql.columns_priv` 行。
pub(crate) fn columns_priv_row(
    host: &str,
    db: &str,
    user: &str,
    table: &str,
    column: &str,
    column_priv: &str,
) -> PrivilegeRow {
    row(&[
        ("host", json!(host)),
        ("db", json!(db)),
        ("user", json!(user)),
        ("table_name", json!(table)),
        ("column_name", json!(column)),
        ("column_priv", json!(column_priv)),
    ])
}

/// 构造 `mysql.default_roles` 行。
/// Builds a `mysql.default_roles` row, matching Go's `INSERT INTO
/// mysql.default_roles` fixtures.
/// 构造 `mysql.default_roles` 行。
pub(crate) fn default_role_row(
    host: &str,
    user: &str,
    role_host: &str,
    role_user: &str,
) -> PrivilegeRow {
    row(&[
        ("host", json!(host)),
        ("user", json!(user)),
        ("default_role_host", json!(role_host)),
        ("default_role_user", json!(role_user)),
    ])
}

/// 构造 `mysql.role_edges` 行：`from_*` 为被授予角色，`to_*` 为接收方。
/// 构造角色边行（from_* 为角色，to_* 为被授予者）。
pub(crate) fn role_edge_row(
    from_host: &str,
    from_user: &str,
    to_host: &str,
    to_user: &str,
) -> PrivilegeRow {
    row(&[
        ("from_host", json!(from_host)),
        ("from_user", json!(from_user)),
        ("to_host", json!(to_host)),
        ("to_user", json!(to_user)),
    ])
}

/// 构造 `mysql.global_priv` 行（priv 为 REQUIRE/密码策略 JSON）。
/// Builds a `mysql.global_priv` row (`priv` is the JSON blob Go stores for
/// `REQUIRE SSL/X509/...` and `PASSWORD EXPIRE` clauses).
/// 构造 `mysql.global_priv` 行；`priv_json` 存 SSL/X509/密码过期等条款。
pub(crate) fn global_priv_row(host: &str, user: &str, priv_json: Value) -> PrivilegeRow {
    row(&[
        ("host", json!(host)),
        ("user", json!(user)),
        ("priv", priv_json),
    ])
}

/// 构造 `mysql.global_grants` 动态权限行。
/// Builds a `mysql.global_grants` row, matching Go's `INSERT INTO
/// mysql.global_grants (..., With_Grant_Option) VALUES (...)`.
/// 构造 `mysql.global_grants` 行动态权限；`grantable` 映射 `With_Grant_Option`。
pub(crate) fn dynamic_priv_row(
    host: &str,
    user: &str,
    priv_name: &str,
    grantable: bool,
) -> PrivilegeRow {
    row(&[
        ("host", json!(host)),
        ("user", json!(user)),
        ("priv", json!(priv_name)),
        (
            "with_grant_option",
            json!(if grantable { "Y" } else { "N" }),
        ),
    ])
}

/// 空活动角色列表。
pub(crate) fn no_roles() -> Vec<RoleIdentity> {
    Vec::new()
}

#[test]
/// 加载 user 表：特权位、邮箱、密码过期与默认插件回退。
fn test_load_user_table() {
    let mut source = RowDataSource::default();
    let mut p = NewMySQLPrivilege();
    assert!(p.LoadUserTable(&source).is_ok());
    assert_eq!(p.User().len(), 0);

    source.user = vec![
        user_row("%", "root", &["select"]),
        user_row("%", "root1", &["insert"]),
        user_row("%", "root11", &["update", "show_db", "references"]),
        user_row(
            "%",
            "root111",
            &[
                "create_user",
                "index",
                "execute",
                "create_view",
                "show_view",
                "show_db",
                "super",
                "trigger",
            ],
        ),
        row(&[
            ("host", json!("%")),
            ("user", json!("root1111")),
            (
                "user_attributes",
                json!({"metadata": {"email": "user@pingcap.com"}}),
            ),
            ("token_issuer", json!("<token-issuer>")),
        ]),
        row(&[
            ("host", json!("%")),
            ("user", json!("root2")),
            ("password_expired", json!("Y")),
            ("password_last_changed", json!(1_665_388_800_i64)),
            ("password_lifetime", json!(3)),
        ]),
        row(&[
            ("host", json!("%")),
            ("user", json!("root3")),
            ("password_expired", json!("N")),
            ("password_last_changed", json!(1_665_388_800_i64)),
        ]),
    ];

    let mut p = NewMySQLPrivilege();
    assert!(p.LoadUserTable(&source).is_ok());

    let user = p.User();
    assert_eq!(user[0].base.User, "root");
    assert_eq!(user[0].Privileges, SelectPriv);
    assert_eq!(user[1].Privileges, InsertPriv);
    assert_eq!(user[2].Privileges, UpdatePriv | ShowDBPriv | ReferencesPriv);
    assert_eq!(
        user[3].Privileges,
        CreateUserPriv
            | IndexPriv
            | ExecutePriv
            | CreateViewPriv
            | ShowViewPriv
            | ShowDBPriv
            | SuperPriv
            | TriggerPriv
    );
    assert_eq!(user[4].Email(), "user@pingcap.com");
    assert_eq!(user[4].AuthTokenIssuer, "<token-issuer>");
    assert_eq!(user[5].PasswordExpired, true);
    assert_eq!(user[5].PasswordLastChanged, 1_665_388_800);
    assert_eq!(user[5].PasswordLifeTime, 3);
    assert_eq!(user[6].PasswordExpired, false);
    assert_eq!(user[6].PasswordLastChanged, 1_665_388_800);
    assert_eq!(user[6].PasswordLifeTime, -1);

    // Switching default auth plugin.
    // 切换默认认证插件后，未显式指定插件的用户应继承该默认值。
    for plugin in [
        "mysql_native_password",
        "caching_sha2_password",
        "tidb_sm3_password",
    ] {
        let mut p = NewMySQLPrivilege();
        p.SetGlobalVarsAccessor(plugin);
        assert!(p.LoadUserTable(&source).is_ok());
        assert_eq!(p.User()[0].AuthPlugin, plugin);
    }
}

#[test]
/// 加载 global_priv：SSL/X509/SAN 解析。
fn test_load_global_priv_table() {
    let source = RowDataSource {
        global_priv: vec![row(&[
            ("host", json!("%")),
            ("user", json!("tu")),
            (
                "priv",
                json!({
                    "access": 0,
                    "plugin": "mysql_native_password",
                    "ssl_type": 3,
                    "ssl_cipher": "cipher",
                    "x509_subject": "\\C=ZH1",
                    "x509_issuer": "\\C=ZH2",
                    "san": "IP:127.0.0.1, IP:1.1.1.1, DNS:pingcap.com, URI:spiffe://mesh.pingcap.com/ns/timesh/sa/me1",
                    "password_last_changed": 1,
                }),
            ),
        ])],
        ..Default::default()
    };
    let mut p = NewMySQLPrivilege();
    assert!(p.LoadGlobalPrivTable(&source).is_ok());
    let val = p.GlobalPriv("tu")[0];
    assert_eq!(val.base.Host, "%");
    assert_eq!(val.base.User, "tu");
    assert_eq!(val.Priv.SSLType, SslTypeSpecified);
    assert_eq!(val.Priv.X509Issuer, "C=ZH2");
    assert_eq!(val.Priv.X509Subject, "C=ZH1");
    assert_eq!(
        val.Priv.SAN,
        "IP:127.0.0.1, IP:1.1.1.1, DNS:pingcap.com, URI:spiffe://mesh.pingcap.com/ns/timesh/sa/me1"
    );
    assert_eq!(val.Priv.SANs.get("IP").unwrap().len(), 2);
    assert_eq!(val.Priv.SANs.get("DNS").unwrap()[0], "pingcap.com");
    assert_eq!(
        val.Priv.SANs.get("URI").unwrap()[0],
        "spiffe://mesh.pingcap.com/ns/timesh/sa/me1"
    );
    assert!(!val.Broken);
}

#[test]
/// 加载 db 表特权位。
fn test_load_db_table() {
    let source = RowDataSource {
        db: vec![
            db_row(
                "%",
                "information_schema",
                "root",
                &["select", "insert", "update", "delete", "create"],
            ),
            db_row(
                "%",
                "mysql",
                "root1",
                &[
                    "drop",
                    "grant",
                    "index",
                    "alter",
                    "create_view",
                    "show_view",
                    "execute",
                ],
            ),
        ],
        ..Default::default()
    };
    let mut p = NewMySQLPrivilege();
    assert!(p.LoadDBTable(&source).is_ok());
    assert_eq!(
        p.DB()[0].Privileges,
        SelectPriv | InsertPriv | UpdatePriv | DeletePriv | CreatePriv
    );
    assert_eq!(
        p.DB()[1].Privileges,
        DropPriv | GrantPriv | IndexPriv | AlterPriv | CreateViewPriv | ShowViewPriv | ExecutePriv
    );
}

#[test]
fn database_grant_columns_merge_into_privilege_cache() {
    let mut privileges = NewMySQLPrivilege();
    privileges.GrantDatabasePrivilegeColumns(
        "localhost",
        "metadata_reader",
        "test",
        &["Select_priv".to_owned(), "Insert_priv".to_owned()],
    );
    privileges.GrantDatabasePrivilegeColumns(
        "localhost",
        "metadata_reader",
        "TEST",
        &["Update_priv".to_owned()],
    );

    assert_eq!(privileges.DB().len(), 1);
    assert_eq!(
        privileges.DB()[0].Privileges,
        SelectPriv | InsertPriv | UpdatePriv,
    );
}

#[test]
/// 加载 tables_priv 的 SET 特权解码。
fn test_load_tables_priv_table() {
    let source = RowDataSource {
        tables_priv: vec![row(&[
            ("host", json!("%")),
            ("db", json!("db")),
            ("user", json!("user")),
            ("table_name", json!("table")),
            ("grantor", json!("grantor")),
            ("table_priv", json!("Grant,Index,Alter")),
            ("column_priv", json!("Insert,Update")),
        ])],
        ..Default::default()
    };
    let mut p = NewMySQLPrivilege();
    assert!(p.LoadTablesPrivTable(&source).is_ok());
    let tables_priv = p.TablesPriv();
    assert_eq!(tables_priv[0].base.Host, "%");
    assert_eq!(tables_priv[0].DB, "db");
    assert_eq!(tables_priv[0].base.User, "user");
    assert_eq!(tables_priv[0].TableName, "table");
    assert_eq!(tables_priv[0].TablePriv, GrantPriv | IndexPriv | AlterPriv);
    assert_eq!(tables_priv[0].ColumnPriv, InsertPriv | UpdatePriv);
}

#[test]
/// 加载 columns_priv，并按主机特异性排序验证。
fn test_load_columns_priv_table() {
    let source = RowDataSource {
        columns_priv: vec![
            row(&[
                ("host", json!("%")),
                ("db", json!("db")),
                ("user", json!("user")),
                ("table_name", json!("table")),
                ("column_name", json!("column")),
                ("column_priv", json!("Insert,Update")),
            ]),
            row(&[
                ("host", json!("127.0.0.1")),
                ("db", json!("db")),
                ("user", json!("user")),
                ("table_name", json!("table")),
                ("column_name", json!("column")),
                ("column_priv", json!("Select")),
            ]),
        ],
        ..Default::default()
    };
    let mut p = NewMySQLPrivilege();
    assert!(p.LoadColumnsPrivTable(&source).is_ok());
    let columns_priv = p.ColumnsPriv();
    // Unlike LoadUserTable/LoadDBTable, Go's LoadColumnsPrivTable never
    // re-sorts (see cache.go); this crate stores columns_priv as a flat,
    // host-specificity-sorted Vec, so "127.0.0.1" (more specific than "%")
    // sorts first. Every field/value below is still exactly what Go's rows
    // would decode to; only the row index differs from a raw INSERT order.
    // 按主机查找行，避免依赖排序后的下标。
    let by_host = |host: &str| {
        columns_priv
            .iter()
            .find(|r| r.base.Host == host)
            .unwrap_or_else(|| panic!("no columns_priv row for host {host}"))
    };
    let wildcard = by_host("%");
    assert_eq!(wildcard.DB, "db");
    assert_eq!(wildcard.base.User, "user");
    assert_eq!(wildcard.TableName, "table");
    assert_eq!(wildcard.ColumnName, "column");
    assert_eq!(wildcard.ColumnPriv, InsertPriv | UpdatePriv);
    assert_eq!(by_host("127.0.0.1").ColumnPriv, SelectPriv);
}

#[test]
/// 列匹配与 `*`（COUNT(*)）特殊路径。
fn test_match_columns() {
    let mut source = RowDataSource {
        columns_priv: vec![
            row(&[
                ("host", json!("%")),
                ("db", json!("db")),
                ("user", json!("user")),
                ("table_name", json!("table")),
                ("column_name", json!("c1")),
                ("column_priv", json!("Insert,Update")),
            ]),
            row(&[
                ("host", json!("%")),
                ("db", json!("db")),
                ("user", json!("user")),
                ("table_name", json!("table")),
                ("column_name", json!("c2")),
                ("column_priv", json!("Select")),
            ]),
        ],
        ..Default::default()
    };
    let mut p = NewMySQLPrivilege();
    assert!(p.LoadColumnsPrivTable(&source).is_ok());
    assert!(p.MatchColumns("user", "%", "db", "table", "c1").is_some());
    // "*" is the special "any column" marker used for `SELECT COUNT(*)`; it
    // matches because c2 carries column-level SELECT.
    // `*` 表示任意列：仅当某列具备 SELECT 时匹配。
    assert!(p.MatchColumns("user", "%", "db", "table", "*").is_some());

    let mut p = NewMySQLPrivilege();
    source.columns_priv = vec![
        row(&[
            ("host", json!("%")),
            ("db", json!("db")),
            ("user", json!("user")),
            ("table_name", json!("table")),
            ("column_name", json!("c1")),
            ("column_priv", json!("Insert,Update")),
        ]),
        row(&[
            ("host", json!("%")),
            ("db", json!("db")),
            ("user", json!("user")),
            ("table_name", json!("table")),
            ("column_name", json!("c2")),
            ("column_priv", json!("References")),
        ]),
    ];
    assert!(p.LoadColumnsPrivTable(&source).is_ok());
    assert!(p.MatchColumns("user", "%", "db", "table", "c1").is_some());
    assert!(p.MatchColumns("user", "%", "db", "table", "c2").is_some());
    // Neither column carries SELECT now, so the "*" wildcard must not match.
    assert!(p.MatchColumns("user", "%", "db", "table", "*").is_none());
}

#[test]
/// 加载 default_roles。
fn test_load_default_role_table() {
    let source = RowDataSource {
        default_roles: vec![
            row(&[
                ("host", json!("%")),
                ("user", json!("test_default_roles")),
                ("default_role_host", json!("localhost")),
                ("default_role_user", json!("r_1")),
            ]),
            row(&[
                ("host", json!("%")),
                ("user", json!("test_default_roles")),
                ("default_role_host", json!("localhost")),
                ("default_role_user", json!("r_2")),
            ]),
        ],
        ..Default::default()
    };
    let mut p = NewMySQLPrivilege();
    assert!(p.LoadDefaultRoles(&source).is_ok());
    assert_eq!(p.DefaultRoles()[0].base.Host, "%");
    assert_eq!(p.DefaultRoles()[0].base.User, "test_default_roles");
    assert_eq!(p.DefaultRoles()[0].DefaultRoleHost, "localhost");
    assert_eq!(p.DefaultRoles()[0].DefaultRoleUser, "r_1");
    assert_eq!(p.DefaultRoles()[1].DefaultRoleHost, "localhost");
}

#[test]
/// 主机通配符与库名通配匹配。
fn test_pattern_match() {
    let active_roles = no_roles();

    let source = RowDataSource {
        user: vec![user_row("10.0.%", "root", &["select", "shutdown"])],
        ..Default::default()
    };
    let mut p = NewMySQLPrivilege();
    assert!(p.LoadUserTable(&source).is_ok());
    assert!(p.RequestVerification(&active_roles, "root", "10.0.1", "test", "", "", SelectPriv));
    assert!(p.RequestVerification(
        &active_roles,
        "root",
        "10.0.1.118",
        "test",
        "",
        "",
        SelectPriv
    ));
    assert!(!p.RequestVerification(
        &active_roles,
        "root",
        "localhost",
        "test",
        "",
        "",
        SelectPriv
    ));
    assert!(!p.RequestVerification(
        &active_roles,
        "root",
        "127.0.0.1",
        "test",
        "",
        "",
        SelectPriv
    ));
    assert!(!p.RequestVerification(
        &active_roles,
        "root",
        "114.114.114.114",
        "test",
        "",
        "",
        SelectPriv
    ));
    assert!(p.RequestVerification(
        &active_roles,
        "root",
        "114.114.114.114",
        "test",
        "",
        "",
        UsagePriv
    ));
    assert!(p.RequestVerification(
        &active_roles,
        "root",
        "10.0.1.118",
        "test",
        "",
        "",
        ShutdownPriv
    ));

    let source = RowDataSource {
        user: vec![user_row("", "root", &["select"])],
        ..Default::default()
    };
    let mut p = NewMySQLPrivilege();
    assert!(p.LoadUserTable(&source).is_ok());
    assert!(p.RequestVerification(&active_roles, "root", "", "test", "", "", SelectPriv));
    assert!(!p.RequestVerification(&active_roles, "root", "notnull", "test", "", "", SelectPriv));
    assert!(!p.RequestVerification(&active_roles, "root", "", "test", "", "", ShutdownPriv));

    // Pattern match for DB.
    // 库名模式 `te%` 应匹配 `test`。
    let source = RowDataSource {
        db: vec![db_row("%", "te%", "genius", &["select"])],
        ..Default::default()
    };
    let mut p = NewMySQLPrivilege();
    assert!(p.LoadDBTable(&source).is_ok());
    assert!(p.RequestVerification(
        &active_roles,
        "genius",
        "127.0.0.1",
        "test",
        "",
        "",
        SelectPriv
    ));
}

#[test]
/// IPv4 网段主机匹配及非法 mask 拒绝登录。
fn test_host_match() {
    let active_roles = no_roles();

    // Host name can be IPv4 address + netmask.
    let source = RowDataSource {
        user: vec![user_row(
            "172.0.0.0/255.0.0.0",
            "root",
            &["select", "shutdown"],
        )],
        ..Default::default()
    };
    let mut p = NewMySQLPrivilege();
    assert!(p.LoadUserTable(&source).is_ok());
    assert!(p.RequestVerification(
        &active_roles,
        "root",
        "172.0.0.1",
        "test",
        "",
        "",
        SelectPriv
    ));
    assert!(p.RequestVerification(
        &active_roles,
        "root",
        "172.1.1.1",
        "test",
        "",
        "",
        SelectPriv
    ));
    assert!(!p.RequestVerification(
        &active_roles,
        "root",
        "localhost",
        "test",
        "",
        "",
        SelectPriv
    ));
    assert!(!p.RequestVerification(
        &active_roles,
        "root",
        "127.0.0.1",
        "test",
        "",
        "",
        SelectPriv
    ));
    assert!(!p.RequestVerification(
        &active_roles,
        "root",
        "198.0.0.1",
        "test",
        "",
        "",
        SelectPriv
    ));
    assert!(p.RequestVerification(
        &active_roles,
        "root",
        "198.0.0.1",
        "test",
        "",
        "",
        UsagePriv
    ));
    assert!(p.RequestVerification(
        &active_roles,
        "root",
        "172.0.0.1",
        "test",
        "",
        "",
        ShutdownPriv
    ));

    // Invalid host name: the user can be created, but cannot log in.
    // 非法主机串仍可入库，但登录匹配失败。
    for mask in [
        "127.0.0.0/24",
        "127.0.0.1/255.0.0.0",
        "127.0.0.0/255.0.0",
        "127.0.0.0/255.0.0.0.0",
        "127%/255.0.0.0",
        "127.0.0.0/%",
        "127.0.0.%/%",
        "127%/%",
    ] {
        let source = RowDataSource {
            user: vec![user_row(mask, "root", &["select", "shutdown"])],
            ..Default::default()
        };
        let mut p = NewMySQLPrivilege();
        assert!(p.LoadUserTable(&source).is_ok());
        assert!(
            !p.RequestVerification(
                &active_roles,
                "root",
                "127.0.0.1",
                "test",
                "",
                "",
                SelectPriv
            ),
            "test case: {mask}"
        );
        assert!(
            !p.RequestVerification(
                &active_roles,
                "root",
                "127.0.0.0",
                "test",
                "",
                "",
                SelectPriv
            ),
            "test case: {mask}"
        );
        assert!(
            !p.RequestVerification(
                &active_roles,
                "root",
                "localhost",
                "test",
                "",
                "",
                ShutdownPriv
            ),
            "test case: {mask}"
        );
    }

    // Netmask notation cannot be used for IPv6 addresses.
    // IPv6 不支持掩码记法，匹配应全部失败。
    let source = RowDataSource {
        user: vec![user_row(
            "2001:db8::/ffff:ffff::",
            "root",
            &["select", "shutdown"],
        )],
        ..Default::default()
    };
    let mut p = NewMySQLPrivilege();
    assert!(p.LoadUserTable(&source).is_ok());
    assert!(!p.RequestVerification(
        &active_roles,
        "root",
        "2001:db8::1234",
        "test",
        "",
        "",
        SelectPriv
    ));
    assert!(!p.RequestVerification(
        &active_roles,
        "root",
        "2001:db8::",
        "test",
        "",
        "",
        SelectPriv
    ));
    assert!(!p.RequestVerification(
        &active_roles,
        "root",
        "localhost",
        "test",
        "",
        "",
        ShutdownPriv
    ));
}

#[test]
/// 库名大小写不敏感匹配。
fn test_case_insensitive() {
    let active_roles = no_roles();
    let source = RowDataSource {
        db: vec![db_row("127.0.0.1", "TCTrain", "genius", &["select"])],
        ..Default::default()
    };
    let mut p = NewMySQLPrivilege();
    assert!(p.LoadDBTable(&source).is_ok());
    // DB and table names are case-insensitive in MySQL.
    assert!(p.RequestVerification(
        &active_roles,
        "genius",
        "127.0.0.1",
        "TCTrain",
        "TCTrainOrder",
        "",
        SelectPriv
    ));
    assert!(p.RequestVerification(
        &active_roles,
        "genius",
        "127.0.0.1",
        "TCTRAIN",
        "TCTRAINORDER",
        "",
        SelectPriv
    ));
    assert!(p.RequestVerification(
        &active_roles,
        "genius",
        "127.0.0.1",
        "tctrain",
        "tctrainorder",
        "",
        SelectPriv
    ));
}

#[test]
/// 加载角色边图并验证邻接。
fn test_load_role_graph() {
    let source = RowDataSource {
        role_edges: vec![
            role_edge_row("%", "r_1", "%", "user2"),
            role_edge_row("%", "r_2", "%", "root"),
            role_edge_row("%", "r_3", "%", "user1"),
            role_edge_row("%", "r_4", "%", "root"),
        ],
        ..Default::default()
    };
    let mut p = NewMySQLPrivilege();
    assert!(p.LoadRoleGraph(&source).is_ok());
    let graph = p.RoleGraph();
    assert!(graph[&RoleIdentity::new("root", "%")].Find("r_2", "%"));
    assert!(graph[&RoleIdentity::new("root", "%")].Find("r_4", "%"));
    assert!(graph[&RoleIdentity::new("user2", "%")].Find("r_1", "%"));
    assert!(graph[&RoleIdentity::new("user1", "%")].Find("r_3", "%"));
    assert!(!graph.contains_key(&RoleIdentity::new("illegal", "")));
    assert!(!graph[&RoleIdentity::new("root", "%")].Find("r_1", "%"));
}

#[test]
/// 角色图 BFS 闭包（含环）。
fn test_role_graph_bfs() {
    // GRANT r_2 TO r_1; GRANT r_3 TO r_2; GRANT r_4 TO r_3; GRANT r_1 TO r_4;
    // GRANT r_5 TO r_3, r_6;
    let source = RowDataSource {
        role_edges: vec![
            role_edge_row("%", "r_2", "%", "r_1"),
            role_edge_row("%", "r_3", "%", "r_2"),
            role_edge_row("%", "r_4", "%", "r_3"),
            role_edge_row("%", "r_1", "%", "r_4"),
            role_edge_row("%", "r_5", "%", "r_3"),
            role_edge_row("%", "r_5", "%", "r_6"),
        ],
        ..Default::default()
    };
    let mut p = NewMySQLPrivilege();
    assert!(p.LoadRoleGraph(&source).is_ok());

    assert_eq!(p.FindAllRole(&[]).len(), 0);
    assert_eq!(p.FindAllRole(&[RoleIdentity::new("r_1", "%")]).len(), 5);
    assert_eq!(p.FindAllRole(&[RoleIdentity::new("r_6", "%")]).len(), 2);
    assert_eq!(
        p.FindAllRole(&[RoleIdentity::new("r_3", "%"), RoleIdentity::new("r_6", "%")])
            .len(),
        6
    );
}

#[test]
/// 有效角色：先过滤直接授予再展开；REVOKE 后失效。
fn test_find_all_user_effective_roles() {
    // GRANT r_3 TO r_1; GRANT r_4 TO r_2; GRANT r_1 TO u1; GRANT r_2 TO u1;
    let mut source = RowDataSource {
        user: ["u1", "r_1", "r_2", "r_3", "r_4"]
            .into_iter()
            .map(|user| user_row("%", user, &[]))
            .collect(),
        role_edges: vec![
            role_edge_row("%", "r_3", "%", "r_1"),
            role_edge_row("%", "r_4", "%", "r_2"),
            role_edge_row("%", "r_1", "%", "u1"),
            role_edge_row("%", "r_2", "%", "u1"),
        ],
        ..Default::default()
    };
    let mut p = NewMySQLPrivilege();
    assert!(p.LoadAll(&source).is_ok());
    let active = [RoleIdentity::new("r_1", "%"), RoleIdentity::new("r_2", "%")];
    let mut ret = p.FindAllUserEffectiveRoles("u1", "%", &active);
    ret.sort_by(|a, b| a.Username.cmp(&b.Username));
    assert_eq!(ret.len(), 4);
    let names: Vec<_> = ret.iter().map(|r| r.Username.as_str()).collect();
    assert_eq!(names, vec!["r_1", "r_2", "r_3", "r_4"]);

    // REVOKE r_2 FROM u1;
    // 撤销 r_2→u1 后，活动角色中的 r_2/r_4 不再对 u1 有效。
    source.role_edges.retain(|row| {
        !(row.get("from_user").and_then(Value::as_str) == Some("r_2")
            && row.get("to_user").and_then(Value::as_str) == Some("u1"))
    });
    assert!(p.LoadAll(&source).is_ok());
    let mut ret = p.FindAllUserEffectiveRoles("u1", "%", &active);
    ret.sort_by(|a, b| a.Username.cmp(&b.Username));
    assert_eq!(ret.len(), 2);
    let names: Vec<_> = ret.iter().map(|r| r.Username.as_str()).collect();
    assert_eq!(names, vec!["r_1", "r_3"]);
}

#[test]
/// 用户表按主机特异性排序。
fn test_sort_user_table() {
    fn names(records: &[UserRecord]) -> Vec<String> {
        records
            .iter()
            .map(|u| format!("{}@{}", u.base.User, u.base.Host))
            .collect()
    }

    let mut p = NewMySQLPrivilege();
    p.user = vec![
        NewUserRecord("%", "root"),
        NewUserRecord("localhost", "root"),
        NewUserRecord("h1.example.net", "root"),
        NewUserRecord("192.168.%", "root"),
        NewUserRecord("192.168.199.%", "root"),
    ];
    p.SortUserTable();
    assert_eq!(
        names(p.User()),
        names(&[
            NewUserRecord("h1.example.net", "root"),
            NewUserRecord("localhost", "root"),
            NewUserRecord("192.168.199.%", "root"),
            NewUserRecord("192.168.%", "root"),
            NewUserRecord("%", "root"),
        ])
    );

    p.user = vec![
        NewUserRecord("%", "root"),
        NewUserRecord("%", "jeffrey"),
        NewUserRecord("localhost", "root"),
        NewUserRecord("localhost", ""),
    ];
    p.SortUserTable();
    assert_eq!(
        names(p.User()),
        names(&[
            NewUserRecord("localhost", ""),
            NewUserRecord("localhost", "root"),
            NewUserRecord("%", "jeffrey"),
            NewUserRecord("%", "root"),
        ])
    );

    p.user = vec![
        NewUserRecord("%", "jeffrey"),
        NewUserRecord("h1.example.net", ""),
    ];
    p.SortUserTable();
    assert_eq!(
        names(p.User()),
        names(&[
            NewUserRecord("h1.example.net", ""),
            NewUserRecord("%", "jeffrey")
        ])
    );

    p.user = vec![
        NewUserRecord("192.168.%", "xxx"),
        NewUserRecord("192.168.199.%", "xxx"),
    ];
    p.SortUserTable();
    assert_eq!(
        names(p.User()),
        names(&[
            NewUserRecord("192.168.199.%", "xxx"),
            NewUserRecord("192.168.%", "xxx"),
        ])
    );
}

#[test]
fn go_host_identity_and_sorting_semantics() {
    // Go compareHost only gives special treatment to `%`, empty hosts, and
    // patterns ending in `%`; `_` and interior `%` otherwise use lexical order.
    assert_eq!(compareHost("a_", "bb"), std::cmp::Ordering::Less);
    assert_eq!(compareHost("a%b", "aab"), std::cmp::Ordering::Less);

    // Go's net.IPNet accepts the all-addresses IPv4 mask when the network is 0.
    assert!(parseHostIPNet("0.0.0.0/0.0.0.0").is_some());

    let exact = baseRecord::new("LOCALHOST", "root");
    assert!(!exact.fullyMatch("root", "localhost"));

    let mut p = NewMySQLPrivilege();
    p.user = vec![
        NewUserRecord("10.%", "root"),
        NewUserRecord("db.example", "named"),
    ];
    p.SortUserTable();
    assert!(p.connectionVerification("root", "10.%").is_some());
    assert!(p.connectionVerification("root", "10.1").is_some());
    assert!(p.matchIdentity("named", "db.example", true).is_some());
}

#[test]
/// GlobalPrivValue::RequireStr 各 SSLType 渲染。
fn test_global_priv_value_require_str() {
    let none = GlobalPrivValue {
        SSLType: SslTypeNone,
        ..Default::default()
    };
    let tls = GlobalPrivValue {
        SSLType: SslTypeAny,
        ..Default::default()
    };
    let x509 = GlobalPrivValue {
        SSLType: SslTypeX509,
        ..Default::default()
    };
    let spec = GlobalPrivValue {
        SSLType: SslTypeSpecified,
        SSLCipher: "c1".into(),
        X509Subject: "s1".into(),
        X509Issuer: "i1".into(),
        ..Default::default()
    };
    let spec2 = GlobalPrivValue {
        SSLType: SslTypeSpecified,
        X509Subject: "s1".into(),
        X509Issuer: "i1".into(),
        ..Default::default()
    };
    let spec3 = GlobalPrivValue {
        SSLType: SslTypeSpecified,
        X509Issuer: "i1".into(),
        ..Default::default()
    };
    let spec4 = GlobalPrivValue {
        SSLType: SslTypeSpecified,
        ..Default::default()
    };
    let verbatim = GlobalPrivValue {
        SSLType: SslTypeSpecified,
        SSLCipher: "a'b".into(),
        ..Default::default()
    };
    assert_eq!(none.RequireStr(), "NONE");
    assert_eq!(tls.RequireStr(), "SSL");
    assert_eq!(x509.RequireStr(), "X509");
    assert_eq!(spec.RequireStr(), "CIPHER 'c1' ISSUER 'i1' SUBJECT 's1'");
    assert_eq!(spec2.RequireStr(), "ISSUER 'i1' SUBJECT 's1'");
    assert_eq!(spec3.RequireStr(), "ISSUER 'i1'");
    assert_eq!(spec4.RequireStr(), "NONE");
    assert_eq!(verbatim.RequireStr(), "CIPHER 'a'b'");
}

#[test]
/// DBIsVisible：仅部分全局特权使库可见。
fn test_dbisvisible() {
    let mut source = RowDataSource::default();

    source.user = vec![user_row("%", "testvisdb", &["create_role", "super"])];
    let mut p = NewMySQLPrivilege();
    assert!(p.LoadUserTable(&source).is_ok());
    assert!(!p.DBIsVisible("testvisdb", "%", "visdb"));

    for (name, privs) in [
        ("testvisdb2", vec!["select"]),
        ("testvisdb3", vec!["create"]),
        ("testvisdb4", vec!["insert"]),
        ("testvisdb5", vec!["update"]),
        ("testvisdb6", vec!["create_view"]),
        ("testvisdb7", vec!["trigger"]),
        ("testvisdb8", vec!["references"]),
        ("testvisdb9", vec!["execute"]),
    ] {
        source.user = vec![user_row("%", name, &privs)];
        let mut p = NewMySQLPrivilege();
        assert!(p.LoadUserTable(&source).is_ok());
        assert!(
            p.DBIsVisible(name, "%", "visdb"),
            "privilege set: {privs:?}"
        );
    }
}

#[test]
fn persisted_scope_deltas_merge_revoke_and_drop_like_go_cache_refresh() {
    let mut privileges = NewMySQLPrivilege();
    privileges
        .user
        .push(NewUserRecord("localhost", "lifecycle"));

    privileges.GrantGlobalPrivilegeMask("localhost", "lifecycle", ProcessPriv);
    privileges.GrantDatabasePrivilegeColumns(
        "localhost",
        "lifecycle",
        "app",
        &["Select_priv".to_owned()],
    );
    privileges.GrantTablePrivilegeMask(
        "localhost",
        "lifecycle",
        "app",
        "items",
        InsertPriv,
        InsertPriv,
    );
    privileges.GrantTablePrivilegeMask(
        "localhost",
        "lifecycle",
        "app",
        "items",
        InsertPriv,
        InsertPriv,
    );
    privileges.GrantColumnPrivilegeMask(
        "localhost",
        "lifecycle",
        "app",
        "items",
        "visible",
        UpdatePriv,
    );

    assert!(
        privileges.RequestVerification(&[], "lifecycle", "localhost", "", "", "", ProcessPriv,)
    );
    assert!(privileges.RequestVerification(
        &[],
        "lifecycle",
        "localhost",
        "app",
        "items",
        "visible",
        UpdatePriv,
    ));
    assert_eq!(privileges.tables_priv.len(), 1, "duplicate GRANT merges");

    privileges.RevokePrivilegeMask(
        "localhost",
        "lifecycle",
        Some("app"),
        Some("items"),
        Some("visible"),
        UpdatePriv,
    );
    privileges.RevokePrivilegeMask(
        "localhost",
        "lifecycle",
        Some("app"),
        Some("items"),
        None,
        InsertPriv,
    );
    privileges.RevokePrivilegeMask(
        "localhost",
        "lifecycle",
        Some("app"),
        None,
        None,
        SelectPriv,
    );
    privileges.RevokePrivilegeMask("localhost", "lifecycle", None, None, None, ProcessPriv);
    assert!(privileges.db.is_empty());
    assert!(privileges.tables_priv.is_empty());
    assert!(privileges.columns_priv.is_empty());

    privileges.DropAccount("localhost", "lifecycle");
    assert!(privileges.user.is_empty());
}
