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

// `REVOKE` 执行器与辅助函数单元测试。
//
// 覆盖事务提交/回滚、动态权限校验、全局/表/列权限更新、空授权行删除、
// 未知权限错误以及用户通知列表，保持 Go `revoke.go` 的关键状态与副作用契约。

use crate::revoke::{
    ColumnName, GrantLevel, GrantLevelSpec, PrivilegeElement, PrivilegeKind, PrivilegeType,
    ResolvedColumn, ResolvedTable, RevokeBackend, RevokeExec, RevokeSql, TargetTableError,
    UserIdentity, UserSpec, composeColumnPrivUpdateForRevoke, composeTablePrivUpdateForRevoke,
    privUpdateForRevoke, userSpecToUserList,
};

#[derive(Default)]
struct MockBackend {
    statements: Vec<RevokeSql>,
    in_transaction: bool,
    released_sessions: usize,
    rollback_errors: usize,
    users_exist: bool,
    database_grant_exists: bool,
    table_grant_exists: bool,
    dynamic_privileges: Vec<String>,
    warnings: Vec<String>,
    table_privileges: (String, String),
    column_privileges: String,
    notifications: Vec<Vec<String>>,
}

impl RevokeBackend for MockBackend {
    type Context = ();
    type Error = String;
    type Session = ();

    fn error(&self, message: String) -> Self::Error {
        message
    }

    fn new_transaction_in_statement(
        &mut self,
        _context: &Self::Context,
    ) -> Result<(), Self::Error> {
        self.in_transaction = true;
        Ok(())
    }

    fn set_in_transaction(&mut self, value: bool) {
        self.in_transaction = value;
    }

    fn get_system_session(&mut self) -> Result<Self::Session, Self::Error> {
        Ok(())
    }

    fn release_system_session(&mut self, _session: Self::Session) {
        self.released_sessions += 1;
    }

    fn execute_internal(
        &mut self,
        _session: &mut Self::Session,
        statement: RevokeSql,
    ) -> Result<(), Self::Error> {
        self.statements.push(statement);
        Ok(())
    }

    fn log_rollback_error(&mut self, _error: &Self::Error) {
        self.rollback_errors += 1;
    }

    fn authenticated_user(&self) -> (String, String) {
        ("authenticated".into(), "AuthHost".into())
    }

    fn current_database(&self) -> String {
        "current_db".into()
    }

    fn current_user_string(&self) -> String {
        "grantor@host".into()
    }

    fn user_exists(
        &mut self,
        _context: &Self::Context,
        _username: &str,
        _hostname: &str,
    ) -> Result<bool, Self::Error> {
        Ok(self.users_exist)
    }

    fn target_schema_name(&self, database: &str) -> String {
        database.to_lowercase()
    }

    fn target_schema_and_table(
        &self,
        _context: &Self::Context,
        database: &str,
        table: &str,
    ) -> Result<(String, Option<ResolvedTable>), TargetTableError<Self::Error>> {
        Ok((
            database.to_lowercase(),
            Some(ResolvedTable {
                original_name: table.to_lowercase(),
                columns: vec![ResolvedColumn {
                    lower_name: "c1".into(),
                    original_name: "C1".into(),
                }],
            }),
        ))
    }

    fn database_grant_exists(
        &mut self,
        _session: &mut Self::Session,
        _user: &str,
        _host: &str,
        _database: &str,
    ) -> Result<bool, Self::Error> {
        Ok(self.database_grant_exists)
    }

    fn table_grant_exists(
        &mut self,
        _session: &mut Self::Session,
        _user: &str,
        _host: &str,
        _database: &str,
        _table: &str,
    ) -> Result<bool, Self::Error> {
        Ok(self.table_grant_exists)
    }

    fn is_dynamic_privilege(&self, privilege: &str) -> bool {
        self.dynamic_privileges.iter().any(|item| item == privilege)
    }

    fn append_unregistered_dynamic_privilege_warning(&mut self, privilege: &str) {
        self.warnings.push(privilege.into());
    }

    fn table_privileges(
        &mut self,
        _session: &mut Self::Session,
        _user: &str,
        _host: &str,
        _database: &str,
        _table: &str,
    ) -> Result<(String, String), Self::Error> {
        Ok(self.table_privileges.clone())
    }

    fn column_privileges(
        &mut self,
        _session: &mut Self::Session,
        _user: &str,
        _host: &str,
        _database: &str,
        _table: &str,
        _column: &str,
    ) -> Result<String, Self::Error> {
        Ok(self.column_privileges.clone())
    }

    fn all_database_privilege_columns(&self) -> Vec<String> {
        vec!["Select_priv".into(), "Insert_priv".into()]
    }

    fn all_global_privilege_columns(&self) -> Vec<String> {
        vec!["Select_priv".into(), "Insert_priv".into()]
    }

    fn grant_privilege_column(&self) -> String {
        "Grant_priv".into()
    }

    fn grant_privilege_set_name(&self) -> String {
        "Grant".into()
    }

    fn notify_privilege_update(&mut self, users: Vec<String>) -> Result<(), Self::Error> {
        self.notifications.push(users);
        Ok(())
    }
}

fn privilege(kind: PrivilegeKind, display_name: &str, set_name: Option<&str>) -> PrivilegeType {
    PrivilegeType {
        kind,
        display_name: display_name.into(),
        set_name: set_name.map(str::to_owned),
        column_name: Some(format!("{display_name}_priv")),
        global_scope: true,
        database_scope: true,
    }
}

fn executor(level: GrantLevel, privilege: PrivilegeElement) -> RevokeExec<MockBackend> {
    RevokeExec {
        BaseExecutor: MockBackend {
            users_exist: true,
            database_grant_exists: true,
            table_grant_exists: true,
            ..MockBackend::default()
        },
        Privs: vec![privilege],
        ObjectType: String::new(),
        Level: GrantLevelSpec {
            level,
            database_name: "TestDB".into(),
            table_name: "TestTable".into(),
        },
        Users: vec![UserSpec {
            user: UserIdentity {
                username: "alice".into(),
                hostname: "LOCALHOST".into(),
                current_user: false,
            },
        }],
        done: false,
    }
}

/// 验证用户列表只保留用户名，并按语句中的声明顺序输出。
#[test]
fn revoke_user_list_preserves_statement_order_and_names_only() {
    let users = vec![
        UserSpec {
            user: UserIdentity {
                username: "alice".into(),
                hostname: "localhost".into(),
                current_user: false,
            },
        },
        UserSpec {
            user: UserIdentity {
                username: "bob".into(),
                hostname: "%".into(),
                current_user: true,
            },
        },
    ];
    assert_eq!(
        userSpecToUserList(&users),
        vec!["alice".to_owned(), "bob".to_owned()]
    );
}

#[test]
fn revoke_global_all_matches_go_transaction_and_cleanup_order() {
    let all = PrivilegeElement {
        privilege: privilege(PrivilegeKind::All, "All", None),
        name: "ALL".into(),
        columns: vec![],
    };
    let mut revoke = executor(GrantLevel::Global, all);

    revoke.Next(&(), &mut ()).unwrap();

    assert_eq!(
        revoke.BaseExecutor.statements,
        vec![
            RevokeSql::Begin,
            RevokeSql::DeleteDynamicPrivilege {
                user: "alice".into(),
                host: "LOCALHOST".into(),
                privilege: None,
            },
            RevokeSql::UpdateGlobal {
                assignments: vec![
                    ("Select_priv".into(), "N".into()),
                    ("Insert_priv".into(), "N".into()),
                ],
                user: "alice".into(),
                host: "localhost".into(),
            },
            RevokeSql::Commit,
        ]
    );
    assert_eq!(
        revoke.BaseExecutor.notifications,
        vec![vec![String::from("alice")]]
    );
    assert_eq!(revoke.BaseExecutor.released_sessions, 1);
    assert!(!revoke.BaseExecutor.in_transaction);

    revoke.Next(&(), &mut ()).unwrap();
    assert_eq!(revoke.BaseExecutor.statements.len(), 4);
}

#[test]
fn revoke_rejects_non_global_dynamic_privilege_and_rolls_back() {
    let dynamic = PrivilegeElement {
        privilege: privilege(PrivilegeKind::Extended, "Backup_Admin", None),
        name: "backup_admin".into(),
        columns: vec![],
    };
    let mut revoke = executor(GrantLevel::Database, dynamic);

    let error = revoke.Next(&(), &mut ()).unwrap_err();

    assert_eq!(error, "Illegal privilege level: BACKUP_ADMIN");
    assert_eq!(
        revoke.BaseExecutor.statements,
        vec![RevokeSql::Begin, RevokeSql::Rollback]
    );
    assert!(revoke.BaseExecutor.notifications.is_empty());
    assert_eq!(revoke.BaseExecutor.released_sessions, 1);
    assert!(!revoke.BaseExecutor.in_transaction);
}

#[test]
fn revoke_dynamic_uppercases_name_and_warns_when_unregistered() {
    let dynamic = PrivilegeElement {
        privilege: privilege(PrivilegeKind::Extended, "Backup_Admin", None),
        name: "backup_admin".into(),
        columns: vec![],
    };
    let mut revoke = executor(GrantLevel::Global, dynamic);

    revoke.Next(&(), &mut ()).unwrap();

    assert_eq!(revoke.BaseExecutor.warnings, vec!["BACKUP_ADMIN"]);
    assert!(
        revoke
            .BaseExecutor
            .statements
            .contains(&RevokeSql::DeleteDynamicPrivilege {
                user: "alice".into(),
                host: "LOCALHOST".into(),
                privilege: Some("BACKUP_ADMIN".into()),
            })
    );
}

#[test]
fn table_and_column_updates_match_go_set_semantics() {
    let select = privilege(PrivilegeKind::Static, "Select", Some("Select"));
    let mut backend = MockBackend {
        table_privileges: ("Select,Grant".into(), "Select,Update".into()),
        column_privileges: "Select,Update".into(),
        ..MockBackend::default()
    };

    let table = composeTablePrivUpdateForRevoke(
        &mut backend,
        &mut (),
        &select,
        "alice",
        "localhost",
        "test",
        "t",
    )
    .unwrap();
    assert_eq!(table.table_privilege, "Grant");
    assert_eq!(table.column_privilege, "Update");
    assert_eq!(table.grantor, "grantor@host");
    assert!(!table.delete_row);

    let column = composeColumnPrivUpdateForRevoke(
        &mut backend,
        &mut (),
        &select,
        "alice",
        "localhost",
        "test",
        "t",
        "c1",
    )
    .unwrap();
    assert_eq!(column.column_privilege, "Update");
    assert!(!column.delete_row);

    let all = privilege(PrivilegeKind::All, "All", None);
    let column = composeColumnPrivUpdateForRevoke(
        &mut backend,
        &mut (),
        &all,
        "alice",
        "localhost",
        "test",
        "t",
        "c1",
    )
    .unwrap();
    assert_eq!(column.column_privilege, "");
    assert!(column.delete_row);
}

#[test]
fn revoke_column_uses_resolved_names_and_deletes_empty_row() {
    let column_privilege = PrivilegeElement {
        privilege: privilege(PrivilegeKind::Static, "Select", Some("Select")),
        name: "SELECT".into(),
        columns: vec![ColumnName {
            lower: "c1".into(),
            original: "c1".into(),
        }],
    };
    let mut revoke = executor(GrantLevel::Table, column_privilege);
    revoke.BaseExecutor.column_privileges = "Select".into();

    revoke.Next(&(), &mut ()).unwrap();

    assert!(
        revoke
            .BaseExecutor
            .statements
            .contains(&RevokeSql::UpdateColumn {
                column_privilege: String::new(),
                user: "alice".into(),
                host: "LOCALHOST".into(),
                database: "testdb".into(),
                table: "testtable".into(),
                column: "C1".into(),
            })
    );
    assert!(
        revoke
            .BaseExecutor
            .statements
            .contains(&RevokeSql::DeleteColumn {
                user: "alice".into(),
                host: "LOCALHOST".into(),
                database: "testdb".into(),
                table: "testtable".into(),
                column: "C1".into(),
            })
    );
}

#[test]
fn unknown_privilege_is_rejected_instead_of_silently_changed() {
    let backend = MockBackend::default();
    let unknown = privilege(PrivilegeKind::Static, "Mystery", None);
    let error = privUpdateForRevoke(&backend, vec!["Select".into()], &unknown).unwrap_err();
    assert_eq!(error, "Unknown priv: Mystery");
}
