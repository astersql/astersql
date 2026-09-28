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

// `REVOKE` 语句执行器。
//
// 对应 Go 的 `RevokeExec`：在系统会话事务中回收用户权限。支持全局 / 库 / 表 / 列
// 四级权限以及动态权限（Extended）。通过 `RevokeBackend` 访问用户表、权限表、
// InfoSchema 与域通知；`RevokeSql` 枚举表达要执行的内部 SQL，由适配层负责转义执行。
#![allow(non_snake_case)]

use std::fmt::Display;

/// 权限类别：Usage 占位、动态扩展、ALL、GRANT OPTION、静态系统权限。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivilegeKind {
    /// `USAGE`：无实际权限位，撤销时直接跳过。
    Usage,
    /// 动态/扩展权限（仅全局级合法）。
    Extended,
    /// `ALL PRIVILEGES`。
    All,
    /// `GRANT OPTION`。
    Grant,
    /// 普通静态权限（Select/Insert 等）。
    Static,
}

/// 一种权限的元数据：展示名、系统表列名、作用域标志等。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrivilegeType {
    /// 权限类别。
    pub kind: PrivilegeKind,
    /// SQL 中的显示名称。
    pub display_name: String,
    /// 表/列权限 SET 串中的名字。
    pub set_name: Option<String>,
    /// mysql.* 权限表中的列名（Y/N 开关）。
    pub column_name: Option<String>,
    /// 是否可用于全局级 GRANT/REVOKE。
    pub global_scope: bool,
    /// 是否可用于库级 GRANT/REVOKE。
    pub database_scope: bool,
}

/// 列名：小写比较键与原始写法。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColumnName {
    /// 小写形式，用于匹配。
    pub lower: String,
    /// 原始列名（错误信息与写出用）。
    pub original: String,
}

/// REVOKE 列表中的一项权限（可带列清单）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrivilegeElement {
    /// 权限类型元数据。
    pub privilege: PrivilegeType,
    /// 权限名（动态权限用大写名）。
    pub name: String,
    /// 列级撤销时的目标列；空表示表级。
    pub columns: Vec<ColumnName>,
}

/// 权限作用级别。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GrantLevel {
    /// `*.*` 全局。
    Global,
    /// `db.*` 库级。
    Database,
    /// `db.tbl` 表级（可再细化到列）。
    Table,
    /// 无法识别的级别字符串。
    Unknown(String),
}

/// 带库表名的授权级别描述。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GrantLevelSpec {
    /// 级别枚举。
    pub level: GrantLevel,
    /// 目标库名；空则用当前库。
    pub database_name: String,
    /// 目标表名（表级时有效）。
    pub table_name: String,
}

/// 用户标识：用户名@主机，或 CURRENT_USER。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserIdentity {
    /// 用户名。
    pub username: String,
    /// 主机名（如 `%` / `localhost`）。
    pub hostname: String,
    /// 为真时在执行期替换为认证用户。
    pub current_user: bool,
}

/// REVOKE 目标用户规格。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserSpec {
    /// 用户身份。
    pub user: UserIdentity,
}

/// InfoSchema 解析后的列信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedColumn {
    /// 小写列名。
    pub lower_name: String,
    /// 原始列名。
    pub original_name: String,
}

/// InfoSchema 解析后的表及其列。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedTable {
    /// 表原始名。
    pub original_name: String,
    /// 表列列表。
    pub columns: Vec<ResolvedColumn>,
}

/// 解析目标表时的错误：表不存在或其他后端错误。
pub enum TargetTableError<E> {
    /// 表不存在；仍带回解析到的库名。
    TableNotExists { database: String },
    /// 其他后端错误。
    Other(E),
}

/// Typed forms of every SQL statement emitted by REVOKE. The production
/// adapter must render these through TiDB's identifier/value escaping layer.
///
/// REVOKE 发出的内部 SQL 类型化形式；生产适配层须经标识符/值转义后再执行。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RevokeSql {
    /// 开启内部事务。
    Begin,
    /// 提交内部事务。
    Commit,
    /// 回滚内部事务。
    Rollback,
    /// 删除动态权限；`privilege=None` 表示删除该用户全部动态权限。
    DeleteDynamicPrivilege {
        user: String,
        host: String,
        privilege: Option<String>,
    },
    /// 更新 mysql.user 全局权限列为指定值。
    UpdateGlobal {
        assignments: Vec<(String, String)>,
        user: String,
        host: String,
    },
    /// 更新 mysql.db 库级权限列。
    UpdateDatabase {
        assignments: Vec<(String, String)>,
        user: String,
        host: String,
        database: String,
    },
    /// 库级权限全为 N 时删除空授权行。
    DeleteEmptyDatabase {
        user: String,
        host: String,
        database: String,
        zero_columns: Vec<String>,
    },
    /// 更新表级权限 SET 串。
    UpdateTable {
        table_privilege: String,
        column_privilege: String,
        grantor: String,
        user: String,
        host: String,
        database: String,
        table: String,
    },
    /// 表级权限清空后删除授权行。
    DeleteTable {
        user: String,
        host: String,
        database: String,
        table: String,
    },
    /// 更新列级权限 SET 串。
    UpdateColumn {
        column_privilege: String,
        user: String,
        host: String,
        database: String,
        table: String,
        column: String,
    },
    /// 列级权限清空后删除授权行。
    DeleteColumn {
        user: String,
        host: String,
        database: String,
        table: String,
        column: String,
    },
}

/// Production boundary for transactions, system sessions, InfoSchema,
/// privilege storage, SQL execution, warnings, and domain notification.
///
/// 生产边界：事务、系统会话、InfoSchema、权限存储、内部 SQL、警告与域通知。
pub trait RevokeBackend {
    type Context;
    type Error: Display;
    type Session;

    fn error(&self, message: String) -> Self::Error;
    fn new_transaction_in_statement(&mut self, context: &Self::Context) -> Result<(), Self::Error>;
    fn set_in_transaction(&mut self, value: bool);
    fn get_system_session(&mut self) -> Result<Self::Session, Self::Error>;
    fn release_system_session(&mut self, session: Self::Session);
    fn execute_internal(
        &mut self,
        session: &mut Self::Session,
        statement: RevokeSql,
    ) -> Result<(), Self::Error>;
    fn log_rollback_error(&mut self, error: &Self::Error);

    fn authenticated_user(&self) -> (String, String);
    fn current_database(&self) -> String;
    fn current_user_string(&self) -> String;
    fn user_exists(
        &mut self,
        context: &Self::Context,
        username: &str,
        hostname: &str,
    ) -> Result<bool, Self::Error>;
    fn target_schema_name(&self, database: &str) -> String;
    fn target_schema_and_table(
        &self,
        context: &Self::Context,
        database: &str,
        table: &str,
    ) -> Result<(String, Option<ResolvedTable>), TargetTableError<Self::Error>>;
    fn database_grant_exists(
        &mut self,
        session: &mut Self::Session,
        user: &str,
        host: &str,
        database: &str,
    ) -> Result<bool, Self::Error>;
    fn table_grant_exists(
        &mut self,
        session: &mut Self::Session,
        user: &str,
        host: &str,
        database: &str,
        table: &str,
    ) -> Result<bool, Self::Error>;

    fn is_dynamic_privilege(&self, privilege: &str) -> bool;
    fn append_unregistered_dynamic_privilege_warning(&mut self, privilege: &str);
    fn table_privileges(
        &mut self,
        session: &mut Self::Session,
        user: &str,
        host: &str,
        database: &str,
        table: &str,
    ) -> Result<(String, String), Self::Error>;
    fn column_privileges(
        &mut self,
        session: &mut Self::Session,
        user: &str,
        host: &str,
        database: &str,
        table: &str,
        column: &str,
    ) -> Result<String, Self::Error>;
    fn all_database_privilege_columns(&self) -> Vec<String>;
    fn all_global_privilege_columns(&self) -> Vec<String>;
    fn grant_privilege_column(&self) -> String;
    fn grant_privilege_set_name(&self) -> String;
    fn notify_privilege_update(&mut self, users: Vec<String>) -> Result<(), Self::Error>;
}

/// `REVOKE` 执行器：一次性语句，`done` 防止重复执行。
pub struct RevokeExec<B: RevokeBackend> {
    /// 后端 / 基类执行器。
    pub BaseExecutor: B,
    /// 要撤销的权限列表。
    pub Privs: Vec<PrivilegeElement>,
    /// 对象类型（表等）。
    pub ObjectType: String,
    /// 授权级别与库表名。
    pub Level: GrantLevelSpec,
    /// 目标用户列表。
    pub Users: Vec<UserSpec>,
    /// 是否已执行过（Next 只跑一次）。
    pub done: bool,
}

impl<B: RevokeBackend> RevokeExec<B> {
    /// 执行 REVOKE：开事务、逐用户撤销、提交并通知权限缓存失效。
    pub fn Next<Q>(&mut self, context: &B::Context, _request: &mut Q) -> Result<(), B::Error> {
        if self.done {
            return Ok(());
        }
        self.done = true;
        self.BaseExecutor.new_transaction_in_statement(context)?;

        // 取系统会话；失败时清除 in_transaction 标志。
        let mut internal_session = match self.BaseExecutor.get_system_session() {
            Ok(session) => session,
            Err(error) => {
                self.BaseExecutor.set_in_transaction(false);
                return Err(error);
            }
        };
        let mut committed = false;
        // 闭包内完成 Begin → 逐用户撤销 → Commit → 通知。
        let result = (|| {
            self.BaseExecutor
                .execute_internal(&mut internal_session, RevokeSql::Begin)?;

            let (auth_username, auth_hostname) = self.BaseExecutor.authenticated_user();
            // CURRENT_USER 替换为认证用户；用户必须已存在。
            for index in 0..self.Users.len() {
                if self.Users[index].user.current_user {
                    self.Users[index].user.username.clone_from(&auth_username);
                    self.Users[index].user.hostname.clone_from(&auth_hostname);
                }
                let username = self.Users[index].user.username.clone();
                let hostname = self.Users[index].user.hostname.clone();
                if !self
                    .BaseExecutor
                    .user_exists(context, &username, &hostname)?
                {
                    return Err(self
                        .BaseExecutor
                        .error(format!("Unknown user: '{}'@'{}'", username, hostname)));
                }
                self.checkDynamicPrivilegeUsage()?;
                self.revokeOneUser(context, &mut internal_session, &username, &hostname)?;
            }

            self.BaseExecutor
                .execute_internal(&mut internal_session, RevokeSql::Commit)?;
            committed = true;
            self.BaseExecutor
                .notify_privilege_update(userSpecToUserList(&self.Users))
        })();

        // 未提交则尝试回滚；回滚失败只记日志。
        if !committed
            && let Err(error) = self
                .BaseExecutor
                .execute_internal(&mut internal_session, RevokeSql::Rollback)
        {
            self.BaseExecutor.log_rollback_error(&error);
        }
        self.BaseExecutor.release_system_session(internal_session);
        self.BaseExecutor.set_in_transaction(false);
        result
    }

    /// 动态权限只能在全局级撤销。
    fn checkDynamicPrivilegeUsage(&self) -> Result<(), B::Error> {
        let dynamic_privileges: Vec<String> = self
            .Privs
            .iter()
            .filter(|privilege| privilege.privilege.kind == PrivilegeKind::Extended)
            .map(|privilege| privilege.name.to_uppercase())
            .collect();
        if !dynamic_privileges.is_empty() && self.Level.level != GrantLevel::Global {
            return Err(self.BaseExecutor.error(format!(
                "Illegal privilege level: {}",
                dynamic_privileges.join(",")
            )));
        }
        Ok(())
    }

    /// 对单个用户按级别校验授权存在后，逐项撤销权限。
    fn revokeOneUser(
        &mut self,
        context: &B::Context,
        internal_session: &mut B::Session,
        user: &str,
        host: &str,
    ) -> Result<(), B::Error> {
        let mut database = if self.Level.database_name.is_empty() {
            self.BaseExecutor.current_database()
        } else {
            self.Level.database_name.clone()
        };
        let requested_database = database.clone();
        // 库/表级先确认授权行存在，避免静默 no-op。
        match self.Level.level {
            GrantLevel::Database => {
                database = self.BaseExecutor.target_schema_name(&database);
                if !self.BaseExecutor.database_grant_exists(
                    internal_session,
                    user,
                    host,
                    &database,
                )? {
                    return Err(self.BaseExecutor.error(format!(
                        "There is no such grant defined for user '{user}' on host '{host}' on database {requested_database}"
                    )));
                }
            }
            GrantLevel::Table => {
                let requested_table = self.Level.table_name.clone();
                let (resolved_database, table) = match self.BaseExecutor.target_schema_and_table(
                    context,
                    &database,
                    &requested_table,
                ) {
                    Ok(value) => value,
                    Err(TargetTableError::TableNotExists { database }) => (database, None),
                    Err(TargetTableError::Other(error)) => return Err(error),
                };
                database = resolved_database;
                let table_name = table
                    .map(|table| table.original_name)
                    .unwrap_or_else(|| requested_table.clone());
                if !self.BaseExecutor.table_grant_exists(
                    internal_session,
                    user,
                    host,
                    &database,
                    &table_name,
                )? {
                    return Err(self.BaseExecutor.error(format!(
                        "There is no such grant defined for user '{user}' on host '{host}' on table {requested_database}.{requested_table}"
                    )));
                }
            }
            GrantLevel::Global | GrantLevel::Unknown(_) => {}
        }

        for privilege in self.Privs.clone() {
            self.revokePriv(context, internal_session, &privilege, user, host)?;
        }
        Ok(())
    }

    /// 按级别分发到全局 / 库 / 表 / 列撤销路径；Usage 直接成功。
    fn revokePriv(
        &mut self,
        context: &B::Context,
        internal_session: &mut B::Session,
        privilege: &PrivilegeElement,
        user: &str,
        host: &str,
    ) -> Result<(), B::Error> {
        if privilege.privilege.kind == PrivilegeKind::Usage {
            return Ok(());
        }
        match self.Level.level {
            GrantLevel::Global => self.revokeGlobalPriv(internal_session, privilege, user, host),
            GrantLevel::Database => self.revokeDBPriv(internal_session, privilege, user, host),
            GrantLevel::Table if privilege.columns.is_empty() => {
                self.revokeTablePriv(context, internal_session, privilege, user, host)
            }
            GrantLevel::Table => {
                self.revokeColumnPriv(context, internal_session, privilege, user, host)
            }
            GrantLevel::Unknown(ref level) => Err(self
                .BaseExecutor
                .error(format!("Unknown revoke level: {level}"))),
        }
    }

    /// 撤销一条动态权限；未注册的权限名仅告警仍删除。
    fn revokeDynamicPriv(
        &mut self,
        internal_session: &mut B::Session,
        privilege_name: &str,
        user: &str,
        host: &str,
    ) -> Result<(), B::Error> {
        let privilege_name = privilege_name.to_uppercase();
        if !self.BaseExecutor.is_dynamic_privilege(&privilege_name) {
            self.BaseExecutor
                .append_unregistered_dynamic_privilege_warning(&privilege_name);
        }
        self.BaseExecutor.execute_internal(
            internal_session,
            RevokeSql::DeleteDynamicPrivilege {
                user: user.to_owned(),
                host: host.to_owned(),
                privilege: Some(privilege_name),
            },
        )
    }

    /// 撤销全局权限：动态权限走删除表；ALL 清动态权限后再把 user 表列置 N。
    fn revokeGlobalPriv(
        &mut self,
        internal_session: &mut B::Session,
        privilege: &PrivilegeElement,
        user: &str,
        host: &str,
    ) -> Result<(), B::Error> {
        if privilege.privilege.kind == PrivilegeKind::Extended {
            return self.revokeDynamicPriv(internal_session, &privilege.name, user, host);
        }
        if privilege.privilege.kind == PrivilegeKind::All {
            self.BaseExecutor.execute_internal(
                internal_session,
                RevokeSql::DeleteDynamicPrivilege {
                    user: user.to_owned(),
                    host: host.to_owned(),
                    privilege: None,
                },
            )?;
        }
        let assignments = composeGlobalPrivUpdate(&self.BaseExecutor, &privilege.privilege, "N")?;
        self.BaseExecutor.execute_internal(
            internal_session,
            RevokeSql::UpdateGlobal {
                assignments,
                user: user.to_owned(),
                host: host.to_lowercase(),
            },
        )
    }

    /// 撤销库级权限：更新 db 表后删除全为 N 的空行。
    fn revokeDBPriv(
        &mut self,
        internal_session: &mut B::Session,
        privilege: &PrivilegeElement,
        user: &str,
        host: &str,
    ) -> Result<(), B::Error> {
        let database = if self.Level.database_name.is_empty() {
            self.BaseExecutor.current_database()
        } else {
            self.Level.database_name.clone()
        };
        let database = self.BaseExecutor.target_schema_name(&database);
        let assignments = composeDBPrivUpdate(&self.BaseExecutor, &privilege.privilege, "N")?;
        self.BaseExecutor.execute_internal(
            internal_session,
            RevokeSql::UpdateDatabase {
                assignments,
                user: user.to_owned(),
                host: host.to_owned(),
                database: database.clone(),
            },
        )?;
        let mut zero_columns = self.BaseExecutor.all_database_privilege_columns();
        zero_columns.push(self.BaseExecutor.grant_privilege_column());
        self.BaseExecutor.execute_internal(
            internal_session,
            RevokeSql::DeleteEmptyDatabase {
                user: user.to_owned(),
                host: host.to_owned(),
                database,
                zero_columns,
            },
        )
    }

    /// 撤销表级权限：更新 SET 串，空则删除授权行。
    fn revokeTablePriv(
        &mut self,
        context: &B::Context,
        internal_session: &mut B::Session,
        privilege: &PrivilegeElement,
        user: &str,
        host: &str,
    ) -> Result<(), B::Error> {
        let (database, table) = match self.BaseExecutor.target_schema_and_table(
            context,
            &self.Level.database_name,
            &self.Level.table_name,
        ) {
            Ok(value) => value,
            Err(TargetTableError::TableNotExists { database }) => (database, None),
            Err(TargetTableError::Other(error)) => return Err(error),
        };
        let table = table
            .map(|table| table.original_name)
            .unwrap_or_else(|| self.Level.table_name.clone());
        let update = composeTablePrivUpdateForRevoke(
            &mut self.BaseExecutor,
            internal_session,
            &privilege.privilege,
            user,
            host,
            &database,
            &table,
        )?;
        let delete_row = update.delete_row;
        self.BaseExecutor.execute_internal(
            internal_session,
            RevokeSql::UpdateTable {
                table_privilege: update.table_privilege,
                column_privilege: update.column_privilege,
                grantor: update.grantor,
                user: user.to_owned(),
                host: host.to_owned(),
                database: database.clone(),
                table: table.clone(),
            },
        )?;
        if delete_row {
            self.BaseExecutor.execute_internal(
                internal_session,
                RevokeSql::DeleteTable {
                    user: user.to_owned(),
                    host: host.to_owned(),
                    database,
                    table,
                },
            )?;
        }
        Ok(())
    }

    /// 撤销列级权限：逐列更新；删除行后可提前结束。
    fn revokeColumnPriv(
        &mut self,
        context: &B::Context,
        internal_session: &mut B::Session,
        privilege: &PrivilegeElement,
        user: &str,
        host: &str,
    ) -> Result<(), B::Error> {
        let (database, table) = self
            .BaseExecutor
            .target_schema_and_table(context, &self.Level.database_name, &self.Level.table_name)
            .map_err(|error| match error {
                TargetTableError::TableNotExists { .. } => self.BaseExecutor.error(format!(
                    "table not found: {}.{}",
                    self.Level.database_name, self.Level.table_name
                )),
                TargetTableError::Other(error) => error,
            })?;
        let table = table.expect("successful column-level resolution returns a table");
        for column in &privilege.columns {
            let resolved = table
                .columns
                .iter()
                .find(|candidate| candidate.lower_name == column.lower)
                .ok_or_else(|| {
                    self.BaseExecutor
                        .error(format!("Unknown column: {}", column.original))
                })?;
            let update = composeColumnPrivUpdateForRevoke(
                &mut self.BaseExecutor,
                internal_session,
                &privilege.privilege,
                user,
                host,
                &database,
                &table.original_name,
                &resolved.original_name,
            )?;
            let delete_row = update.delete_row;
            self.BaseExecutor.execute_internal(
                internal_session,
                RevokeSql::UpdateColumn {
                    column_privilege: update.column_privilege,
                    user: user.to_owned(),
                    host: host.to_owned(),
                    database: database.clone(),
                    table: table.original_name.clone(),
                    column: resolved.original_name.clone(),
                },
            )?;
            if delete_row {
                self.BaseExecutor.execute_internal(
                    internal_session,
                    RevokeSql::DeleteColumn {
                        user: user.to_owned(),
                        host: host.to_owned(),
                        database: database.clone(),
                        table: table.original_name.clone(),
                        column: resolved.original_name.clone(),
                    },
                )?;
                break;
            }
        }
        Ok(())
    }
}

/// 从用户规格提取用户名列表（保持顺序，供权限缓存失效通知）。
pub fn userSpecToUserList(specifications: &[UserSpec]) -> Vec<String> {
    specifications
        .iter()
        .map(|specification| specification.user.username.clone())
        .collect()
}

/// 从当前权限 SET 列表中移除目标权限名。
pub fn privUpdateForRevoke<B: RevokeBackend>(
    backend: &B,
    mut current: Vec<String>,
    privilege: &PrivilegeType,
) -> Result<Vec<String>, B::Error> {
    let set_name = privilege
        .set_name
        .as_ref()
        .ok_or_else(|| backend.error(format!("Unknown priv: {}", privilege.display_name)))?;
    if let Some(index) = current.iter().position(|entry| entry == set_name) {
        current.remove(index);
    }
    Ok(current)
}

/// 表级撤销后的更新载荷；`delete_row` 表示权限已空应删行。
pub struct TablePrivilegeUpdate {
    /// 新的表权限 SET 串。
    pub table_privilege: String,
    /// 新的列权限 SET 串。
    pub column_privilege: String,
    /// 授权者字符串。
    pub grantor: String,
    /// 是否应删除整行授权。
    pub delete_row: bool,
}

/// 计算表级撤销后的 SET 串；ALL 仅保留 GRANT OPTION。
pub fn composeTablePrivUpdateForRevoke<B: RevokeBackend>(
    backend: &mut B,
    session: &mut B::Session,
    privilege: &PrivilegeType,
    name: &str,
    host: &str,
    database: &str,
    table: &str,
) -> Result<TablePrivilegeUpdate, B::Error> {
    let (current_table, current_column) =
        backend.table_privileges(session, name, host, database, table)?;
    let (new_table, new_column) = if privilege.kind == PrivilegeKind::All {
        let grant = backend.grant_privilege_set_name();
        let new_table = SetFromString(&current_table)
            .into_iter()
            .filter(|entry| entry == &grant)
            .collect();
        (new_table, Vec::new())
    } else {
        (
            privUpdateForRevoke(backend, SetFromString(&current_table), privilege)?,
            privUpdateForRevoke(backend, SetFromString(&current_column), privilege)?,
        )
    };
    Ok(TablePrivilegeUpdate {
        table_privilege: new_table.join(","),
        column_privilege: new_column.join(","),
        grantor: backend.current_user_string(),
        delete_row: new_table.is_empty(),
    })
}

/// 列级撤销后的更新载荷。
pub struct ColumnPrivilegeUpdate {
    /// 新的列权限 SET 串。
    pub column_privilege: String,
    /// 是否应删除该列授权行。
    pub delete_row: bool,
}

#[allow(clippy::too_many_arguments)]
/// 计算列级撤销后的 SET 串；ALL 清空全部列权限。
pub fn composeColumnPrivUpdateForRevoke<B: RevokeBackend>(
    backend: &mut B,
    session: &mut B::Session,
    privilege: &PrivilegeType,
    name: &str,
    host: &str,
    database: &str,
    table: &str,
    column: &str,
) -> Result<ColumnPrivilegeUpdate, B::Error> {
    let new_column = if privilege.kind == PrivilegeKind::All {
        Vec::new()
    } else {
        let current = backend.column_privileges(session, name, host, database, table, column)?;
        privUpdateForRevoke(backend, SetFromString(&current), privilege)?
    };
    Ok(ColumnPrivilegeUpdate {
        column_privilege: new_column.join(","),
        delete_row: new_column.is_empty(),
    })
}

/// 组装全局权限列赋值（值为 `Y`/`N`）。
fn composeGlobalPrivUpdate<B: RevokeBackend>(
    backend: &B,
    privilege: &PrivilegeType,
    value: &str,
) -> Result<Vec<(String, String)>, B::Error> {
    composeScopePrivilegeUpdate(backend, privilege, value, true)
}

/// 组装库级权限列赋值。
fn composeDBPrivUpdate<B: RevokeBackend>(
    backend: &B,
    privilege: &PrivilegeType,
    value: &str,
) -> Result<Vec<(String, String)>, B::Error> {
    composeScopePrivilegeUpdate(backend, privilege, value, false)
}

/// 按作用域校验权限合法性，并生成列赋值列表；ALL 覆盖全部列。
fn composeScopePrivilegeUpdate<B: RevokeBackend>(
    backend: &B,
    privilege: &PrivilegeType,
    value: &str,
    global: bool,
) -> Result<Vec<(String, String)>, B::Error> {
    if privilege.kind != PrivilegeKind::All {
        let supported = privilege.kind == PrivilegeKind::Grant
            || if global {
                privilege.global_scope
            } else {
                privilege.database_scope
            };
        if !supported {
            return Err(backend.error(if global {
                "Wrong usage: GLOBAL GRANT with NON-GLOBAL PRIVILEGES".to_owned()
            } else {
                "Wrong usage: DB GRANT with NON-DB PRIVILEGES".to_owned()
            }));
        }
        let column = privilege
            .column_name
            .clone()
            .ok_or_else(|| backend.error(format!("Unknown priv: {}", privilege.display_name)))?;
        return Ok(vec![(column, value.to_owned())]);
    }
    let columns = if global {
        backend.all_global_privilege_columns()
    } else {
        backend.all_database_privilege_columns()
    };
    Ok(columns
        .into_iter()
        .map(|column| (column, value.to_owned()))
        .collect())
}

/// 将逗号分隔的权限 SET 串拆成列表；空串得空列表。
fn SetFromString(value: &str) -> Vec<String> {
    if value.is_empty() {
        Vec::new()
    } else {
        value.split(',').map(str::to_owned).collect()
    }
}
