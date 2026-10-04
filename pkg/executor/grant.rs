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

// `GRANT` 语句执行器：在 mysql 系统权限表中写入用户权限与 TLS 要求。
//
// 覆盖全局 / 库 / 表 / 列级静态权限与动态权限；可按 SQL mode
// 自动创建不存在的用户，并处理 REQUIRE SSL/X509 等选项。
// 通过 `GrantDependencies` / `SystemSession` 注入元数据与系统表访问。

#![allow(dead_code, non_snake_case)]

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

/// GRANT 路径统一结果别名。
pub type GrantResult<T = ()> = Result<T, GrantError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// GRANT 错误分类。
pub enum GrantErrorKind {
    General,
    TableNotExists,
    IllegalPrivilegeLevel,
    DynamicPrivilegeNotRegistered,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 带分类的 GRANT 错误。
pub struct GrantError {
    pub kind: GrantErrorKind,
    pub message: String,
}

impl GrantError {
    /// 构造通用错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            kind: GrantErrorKind::General,
            message: message.into(),
        }
    }

    /// 构造带分类的错误。
    pub fn with_kind(kind: GrantErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for GrantError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for GrantError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 授权作用域：全局、库或表。
pub enum GrantLevelKind {
    Global,
    Database,
    Table,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// GRANT 目标级别及库表名。
pub struct GrantLevel {
    pub level: GrantLevelKind,
    pub db_name: String,
    pub table_name: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 授权对象类型（表/函数/过程等）。
pub enum ObjectType {
    None,
    Table,
    Function,
    Procedure,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 静态权限元数据：系统表列名、SET 名与适用级别标志。
pub struct StaticPrivilege {
    pub name: String,
    pub column_name: String,
    pub set_name: String,
    pub global: bool,
    pub database: bool,
    pub table: bool,
    pub column: bool,
    pub global_only: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 权限种类：ALL/USAGE/GRANT/CREATE、动态或静态权限。
pub enum PrivilegeType {
    All,
    Usage,
    Grant,
    Create,
    Extended(String),
    Static(StaticPrivilege),
}

impl PrivilegeType {
    /// 是否为动态（Extended）权限。
    fn is_dynamic(&self) -> bool {
        matches!(self, Self::Extended(_))
    }

    /// 是否为 CREATE 类权限。
    fn is_create(&self) -> bool {
        matches!(self, Self::Create)
            || matches!(self, Self::Static(privilege) if privilege.name.eq_ignore_ascii_case("CREATE"))
    }

    /// 对应 mysql.user 等表中的权限列名。
    fn column_name(&self) -> Option<&str> {
        match self {
            Self::Grant => Some("Grant_priv"),
            Self::Create => Some("Create_priv"),
            Self::Static(privilege) => Some(&privilege.column_name),
            Self::All | Self::Usage | Self::Extended(_) => None,
        }
    }

    /// 对应 tables_priv/columns_priv 中 SET 元素名。
    fn set_name(&self) -> Option<&str> {
        match self {
            Self::Grant => Some("Grant"),
            Self::Create => Some("Create"),
            Self::Static(privilege) => Some(&privilege.set_name),
            Self::All | Self::Usage | Self::Extended(_) => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 一条授予项：权限及可选列列表。
pub struct PrivilegeElement {
    pub privilege: PrivilegeType,
    pub columns: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 用户身份 user@host；`current_user` 表示 CURRENT_USER。
pub struct UserIdentity {
    pub username: String,
    pub hostname: String,
    pub current_user: bool,
}

impl UserIdentity {
    /// 格式化为 `user@host`。
    fn display_name(&self) -> String {
        format!("{}@{}", self.username, self.hostname)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 创建用户时的认证插件与认证串。
pub struct AuthOption {
    pub auth_plugin: String,
    pub auth_string: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// GRANT 目标用户及可选认证选项。
pub struct UserSpec {
    pub user: UserIdentity,
    pub auth_option: Option<AuthOption>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
/// REQUIRE 子句选项类型（TLS/X509/密码令牌签发者等）。
pub enum AuthTokenOrTlsOptionType {
    TlsNone,
    Ssl,
    X509,
    Cipher,
    Issuer,
    Subject,
    San,
    TokenIssuer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 单条 REQUIRE 选项及其值。
pub struct AuthTokenOrTlsOption {
    pub option_type: AuthTokenOrTlsOptionType,
    pub value: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 全局权限 JSON 中记录的 SSL 要求类型。
pub enum SslType {
    NotSpecified,
    None,
    Any,
    X509,
    Specified,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 写入 mysql.global_priv 的 TLS/X509 约束。
pub struct GlobalPrivValue {
    pub ssl_type: SslType,
    pub ssl_cipher: String,
    pub x509_issuer: String,
    pub x509_subject: String,
    pub san: String,
}

impl Default for GlobalPrivValue {
    fn default() -> Self {
        Self {
            ssl_type: SslType::NotSpecified,
            ssl_cipher: String::new(),
            x509_issuer: String::new(),
            x509_subject: String::new(),
            san: String::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 目标表元数据（用于列名校验）。
pub struct TableMetadata {
    pub schema_name: String,
    pub table_name: String,
    pub columns: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 各级别 ALL 展开时包含的静态权限列表。
pub struct PrivilegeCatalog {
    pub all_global: Vec<StaticPrivilege>,
    pub all_database: Vec<StaticPrivilege>,
    pub all_table: Vec<StaticPrivilege>,
    pub all_column: Vec<StaticPrivilege>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 系统表 UPDATE 的列赋值。
pub struct Assignment {
    pub column: String,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 对权限系统表的写操作抽象。
pub enum SystemMutation {
    CreateUser {
        host: String,
        user: String,
        authentication_string: String,
        plugin: String,
    },
    InsertGlobalPriv {
        host: String,
        user: String,
        value: String,
    },
    InsertDatabasePriv {
        host: String,
        user: String,
        database: String,
    },
    InsertTablePriv {
        host: String,
        user: String,
        database: String,
        table: String,
    },
    InsertColumnPriv {
        host: String,
        user: String,
        database: String,
        table: String,
        column: String,
    },
    UpdateGlobalPriv {
        host: String,
        user: String,
        value: Option<String>,
    },
    ReplaceDynamicGrant {
        host: String,
        user: String,
        privilege: String,
        with_grant_option: bool,
    },
    UpdateGlobalLevel {
        host: String,
        user: String,
        assignments: Vec<Assignment>,
    },
    UpdateDatabaseLevel {
        host: String,
        user: String,
        database: String,
        assignments: Vec<Assignment>,
    },
    UpdateTableLevel {
        host: String,
        user: String,
        database: String,
        table: String,
        table_privileges: String,
        column_privileges: String,
        grantor: String,
    },
    UpdateColumnLevel {
        host: String,
        user: String,
        database: String,
        table: String,
        column: String,
        column_privileges: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 对权限系统表的读查询抽象。
pub enum SystemQuery {
    GlobalPrivExists {
        user: String,
        host: String,
    },
    DatabasePrivExists {
        user: String,
        host: String,
        database: String,
    },
    TablePrivExists {
        user: String,
        host: String,
        database: String,
        table: String,
    },
    ColumnPrivExists {
        user: String,
        host: String,
        database: String,
        table: String,
        column: String,
    },
    TablePrivileges {
        user: String,
        host: String,
        database: String,
        table: String,
    },
    ColumnPrivileges {
        user: String,
        host: String,
        database: String,
        table: String,
        column: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 结果集单元格：NULL、文本或 SET。
pub enum CellValue {
    Null,
    Text(String),
    Set(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 一行查询结果。
pub struct Row(pub Vec<CellValue>);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 结果列类型；SET 用于解析权限集合字符串。
pub enum FieldType {
    Other,
    Set,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 结果集字段描述。
pub struct ResultField {
    pub field_type: FieldType,
}

/// 系统表查询结果游标。
pub trait RecordSet: Send {
    fn next(&mut self, maximum_rows: usize) -> GrantResult<Vec<Row>>;
    fn fields(&self) -> Vec<ResultField>;
    fn close(&mut self) -> GrantResult;
}

/// 操作 mysql 权限表的内部会话。
pub trait SystemSession: Send {
    fn set_user(&mut self, user: &UserIdentity) -> GrantResult;
    fn begin(&mut self) -> GrantResult;
    fn commit(&mut self) -> GrantResult;
    fn rollback(&mut self) -> GrantResult;
    fn execute(&mut self, mutation: SystemMutation) -> GrantResult;
    fn query(&mut self, query: SystemQuery) -> GrantResult<Box<dyn RecordSet>>;
    fn grantor(&self) -> GrantResult<String>;
    fn maximum_chunk_size(&self) -> GrantResult<usize>;
}

/// GRANT 执行对外依赖：会话、用户、元数据与通知。
pub trait GrantDependencies: Send + Sync {
    fn current_database(&self) -> GrantResult<String>;
    fn current_user(&self) -> GrantResult<UserIdentity>;
    fn no_auto_create_user(&self) -> GrantResult<bool>;
    fn new_transaction_in_statement(&self) -> GrantResult;
    fn set_in_transaction(&self, in_transaction: bool) -> GrantResult;
    fn acquire_system_session(&self) -> GrantResult<Box<dyn SystemSession>>;
    fn release_system_session(&self, session: Box<dyn SystemSession>) -> GrantResult;
    fn default_auth_plugin(&self) -> GrantResult<String>;
    fn user_exists_with_retry_variants(
        &self,
        username: &mut String,
        hostname: &str,
    ) -> GrantResult<bool>;
    fn validate_username(&self, username: &str) -> GrantResult;
    fn encode_password(
        &self,
        user: &UserSpec,
        auth_plugin: &str,
        default_auth_plugin: &str,
    ) -> GrantResult<String>;
    fn canonical_schema_name(&self, database: &str) -> GrantResult<Option<String>>;
    fn table_by_name(&self, database: &str, table: &str) -> GrantResult<Option<TableMetadata>>;
    fn is_dynamic_privilege(&self, privilege: &str) -> GrantResult<bool>;
    fn is_supported_cipher(&self, cipher: &str) -> GrantResult<bool>;
    fn validate_x509_name(&self, name: &str) -> GrantResult;
    fn validate_san(&self, san: &str) -> GrantResult;
    fn notify_update_privilege(&self, users: &[UserIdentity]) -> GrantResult;
}

/// `GRANT` 主执行器状态。
pub struct GrantExec {
    pub privileges: Vec<PrivilegeElement>,
    pub object_type: ObjectType,
    pub level: GrantLevel,
    pub users: Vec<UserSpec>,
    pub auth_token_or_tls_options: Option<Vec<AuthTokenOrTlsOption>>,
    pub with_grant: bool,
    pub done: bool,
    pub catalog: PrivilegeCatalog,
    pub dependencies: Arc<dyn GrantDependencies>,
}

impl GrantExec {
    /// 解析目标库表、在内部事务中应用授权并清理事务标志。
    pub fn Next(&mut self) -> GrantResult {
        if self.done {
            return Ok(());
        }
        self.done = true;

        let mut database = if self.level.db_name.is_empty() {
            self.dependencies.current_database()?
        } else {
            self.level.db_name.clone()
        };
        if self.level.level == GrantLevelKind::Database {
            database = getTargetSchemaName(self.dependencies.as_ref(), &database)?;
        }
        if self.level.level == GrantLevelKind::Table {
            database = self.validate_table_grant(&database)?;
        }

        self.dependencies.new_transaction_in_statement()?;
        let transaction_result = self.run_internal_transaction(&database);
        let clear_result = self.dependencies.set_in_transaction(false);
        // 优先返回授权错误；事务标志清理失败次之。
        match (transaction_result, clear_result) {
            (Err(error), _) => Err(error),
            (Ok(()), Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    /// 校验表/列级权限合法性，解析规范库名与表名。
    fn validate_table_grant(&mut self, database: &str) -> GrantResult<String> {
        for privilege in &self.privileges {
            if privilege.columns.is_empty() {
                let valid = matches!(
                    privilege.privilege,
                    PrivilegeType::All
                        | PrivilegeType::Usage
                        | PrivilegeType::Grant
                        | PrivilegeType::Extended(_)
                ) || privilege.privilege.is_create()
                    || is_table_privilege(&privilege.privilege);
                if !valid {
                    return Err(GrantError::new("illegal GRANT privilege for table"));
                }
            } else {
                let valid = matches!(
                    privilege.privilege,
                    PrivilegeType::All | PrivilegeType::Usage
                ) || is_column_privilege(&privilege.privilege);
                if !valid {
                    return Err(GrantError::new(
                        "wrong usage: COLUMN GRANT with non-column privileges",
                    ));
                }
            }
        }

        let table = self
            .dependencies
            .table_by_name(database, &self.level.table_name)?;
        if table.is_none()
            && !self.privileges.iter().any(|privilege| {
                matches!(privilege.privilege, PrivilegeType::All) || privilege.privilege.is_create()
            })
        {
            return Err(table_not_exists(database, &self.level.table_name));
        }

        if let Some(table) = table {
            if !table
                .table_name
                .eq_ignore_ascii_case(&self.level.table_name)
            {
                return Err(table_not_exists(database, &self.level.table_name));
            }
            self.level.table_name = table.table_name;
        }

        let canonical_database = self.dependencies.canonical_schema_name(database)?;
        if !self.level.db_name.is_empty() && canonical_database.is_none() {
            return Err(table_not_exists(database, &self.level.table_name));
        }
        Ok(canonical_database.unwrap_or_else(|| database.to_owned()))
    }

    /// 获取系统会话、事务内 apply_grant，失败回滚并释放会话。
    fn run_internal_transaction(&mut self, database: &str) -> GrantResult {
        let current_user = self.dependencies.current_user()?;
        let mut session = self.dependencies.acquire_system_session()?;
        let mut committed = false;

        let result = (|| {
            session.set_user(&current_user)?;
            session.begin()?;
            self.apply_grant(database, session.as_mut())?;
            session.commit()?;
            committed = true;
            let users = self
                .users
                .iter()
                .map(|user| user.user.clone())
                .collect::<Vec<_>>();
            self.dependencies.notify_update_privilege(&users)
        })();

        if result.is_err() && !committed {
            let _rollback_error = session.rollback().err();
        }
        let release_result = self.dependencies.release_system_session(session);
        match (result, release_result) {
            (Err(error), _) => Err(error),
            (Ok(()), Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    /// 确保用户存在、初始化权限行，再按级别写入权限与 TLS。
    fn apply_grant(&mut self, database: &str, session: &mut dyn SystemSession) -> GrantResult {
        let default_auth_plugin = self.dependencies.default_auth_plugin()?;
        let authenticated_user = self.dependencies.current_user()?;
        for user in &mut self.users {
            if user.user.current_user {
                user.user.username = authenticated_user.username.clone();
                user.user.hostname = authenticated_user.hostname.clone();
            }
            let exists = self
                .dependencies
                .user_exists_with_retry_variants(&mut user.user.username, &user.user.hostname)?;
            // NO_AUTO_CREATE_USER 开启时禁止 GRANT 隐式建用户。
            if !exists {
                if self.dependencies.no_auto_create_user()? {
                    return Err(GrantError::new(
                        "GRANT cannot create user in current SQL mode",
                    ));
                }
                self.dependencies.validate_username(&user.user.username)?;
                let auth_plugin = user
                    .auth_option
                    .as_ref()
                    .map(|option| option.auth_plugin.as_str())
                    .filter(|plugin| !plugin.is_empty())
                    .unwrap_or(&default_auth_plugin)
                    .to_owned();
                let password =
                    self.dependencies
                        .encode_password(user, &auth_plugin, &default_auth_plugin)?;
                session.execute(SystemMutation::CreateUser {
                    host: user.user.hostname.clone(),
                    user: user.user.username.clone(),
                    authentication_string: password,
                    plugin: auth_plugin,
                })?;
            }
        }

        for index in 0..self.users.len() {
            let user = self.users[index].clone();
            if self.auth_token_or_tls_options.is_some() {
                checkAndInitGlobalPriv(session, &user.user.username, &user.user.hostname)?;
            }
            match self.level.level {
                GrantLevelKind::Global => {}
                GrantLevelKind::Database => {
                    checkAndInitDBPriv(session, database, &user.user.username, &user.user.hostname)?
                }
                GrantLevelKind::Table => checkAndInitTablePriv(
                    session,
                    database,
                    &self.level.table_name,
                    &user.user.username,
                    &user.user.hostname,
                )?,
            }

            let mut privileges = self.privileges.clone();
            // WITH GRANT OPTION：对非动态权限附加 Grant_priv。
            if self.with_grant && containsNonDynamicPriv(&privileges) {
                privileges.push(PrivilegeElement {
                    privilege: PrivilegeType::Grant,
                    columns: Vec::new(),
                });
            }

            self.grantGlobalPriv(session, &user)?;
            for privilege in &privileges {
                if !privilege.columns.is_empty() {
                    self.checkAndInitColumnPriv(&user.user, &privilege.columns, session)?;
                }
                self.grantLevelPriv(privilege, &user, session)?;
            }
        }
        Ok(())
    }

    /// 确保列权限行存在，必要时插入空条目。
    fn checkAndInitColumnPriv(
        &self,
        user: &UserIdentity,
        columns: &[String],
        session: &mut dyn SystemSession,
    ) -> GrantResult {
        let (database, table) = getTargetSchemaAndTable(
            self.dependencies.as_ref(),
            &self.level.db_name,
            &self.level.table_name,
        )?;
        for column in columns {
            let canonical_column = find_column(&table, column)
                .ok_or_else(|| GrantError::new(format!("unknown column: {column}")))?;
            if !columnPrivEntryExists(
                session,
                &user.username,
                &user.hostname,
                &database,
                &table.table_name,
                canonical_column,
            )? {
                initColumnPrivEntry(
                    session,
                    &user.username,
                    &user.hostname,
                    &database,
                    &table.table_name,
                    canonical_column,
                )?;
            }
        }
        Ok(())
    }

    /// 将 REQUIRE 选项写入 global_priv。
    fn grantGlobalPriv(&self, session: &mut dyn SystemSession, user: &UserSpec) -> GrantResult {
        let options = match &self.auth_token_or_tls_options {
            Some(options) if !options.is_empty() => options,
            _ => return Ok(()),
        };
        let value = tlsOption2GlobalPriv(self.dependencies.as_ref(), options)?;
        session.execute(SystemMutation::UpdateGlobalPriv {
            host: user.user.hostname.clone(),
            user: user.user.username.clone(),
            value,
        })
    }

    /// 按权限类型与级别分发到动态/全局/库/表/列授权。
    fn grantLevelPriv(
        &self,
        privilege: &PrivilegeElement,
        user: &UserSpec,
        session: &mut dyn SystemSession,
    ) -> GrantResult {
        match &privilege.privilege {
            PrivilegeType::Extended(name) => self.grantDynamicPriv(name, user, session),
            PrivilegeType::Usage => Ok(()),
            _ => match self.level.level {
                GrantLevelKind::Global => self.grantGlobalLevel(privilege, user, session),
                GrantLevelKind::Database => self.grantDBLevel(privilege, user, session),
                GrantLevelKind::Table if privilege.columns.is_empty() => {
                    self.grantTableLevel(privilege, user, session)
                }
                GrantLevelKind::Table => self.grantColumnLevel(privilege, user, session),
            },
        }
    }

    /// 仅允许在全局级别注册并写入动态权限。
    fn grantDynamicPriv(
        &self,
        privilege_name: &str,
        user: &UserSpec,
        session: &mut dyn SystemSession,
    ) -> GrantResult {
        let privilege_name = privilege_name.to_uppercase();
        if self.level.level != GrantLevelKind::Global {
            return Err(GrantError::with_kind(
                GrantErrorKind::IllegalPrivilegeLevel,
                format!("illegal privilege level for {privilege_name}"),
            ));
        }
        if !self.dependencies.is_dynamic_privilege(&privilege_name)? {
            return Err(GrantError::with_kind(
                GrantErrorKind::DynamicPrivilegeNotRegistered,
                format!("dynamic privilege is not registered: {privilege_name}"),
            ));
        }
        session.execute(SystemMutation::ReplaceDynamicGrant {
            host: user.user.hostname.clone(),
            user: user.user.username.clone(),
            privilege: privilege_name,
            with_grant_option: self.with_grant,
        })
    }

    /// 更新 mysql.user 全局权限列。
    fn grantGlobalLevel(
        &self,
        privilege: &PrivilegeElement,
        user: &UserSpec,
        session: &mut dyn SystemSession,
    ) -> GrantResult {
        let assignments = composeGlobalPrivUpdate(&self.catalog, &privilege.privilege, "Y")?;
        session.execute(SystemMutation::UpdateGlobalLevel {
            host: user.user.hostname.clone(),
            user: user.user.username.clone(),
            assignments,
        })
    }

    /// 更新 mysql.db 库级权限列。
    fn grantDBLevel(
        &self,
        privilege: &PrivilegeElement,
        user: &UserSpec,
        session: &mut dyn SystemSession,
    ) -> GrantResult {
        if is_global_only_privilege(&privilege.privilege) {
            return Err(GrantError::new(
                "wrong usage: DB GRANT with global privileges",
            ));
        }
        let database = getTargetSchemaName(self.dependencies.as_ref(), &self.level.db_name)?;
        let assignments = composeDBPrivUpdate(&self.catalog, &privilege.privilege, "Y")?;
        session.execute(SystemMutation::UpdateDatabaseLevel {
            host: user.user.hostname.clone(),
            user: user.user.username.clone(),
            database,
            assignments,
        })
    }

    /// 合并并写回 tables_priv 的表/列权限 SET。
    fn grantTableLevel(
        &self,
        privilege: &PrivilegeElement,
        user: &UserSpec,
        session: &mut dyn SystemSession,
    ) -> GrantResult {
        let (database, table_name) = match getTargetSchemaAndTable(
            self.dependencies.as_ref(),
            &self.level.db_name,
            &self.level.table_name,
        ) {
            Ok((database, table)) => (database, table.table_name),
            Err(error) if error.kind == GrantErrorKind::TableNotExists => {
                let database =
                    getTargetSchemaName(self.dependencies.as_ref(), &self.level.db_name)?;
                (database, self.level.table_name.clone())
            }
            Err(error) => return Err(error),
        };
        let update = composeTablePrivUpdateForGrant(
            &self.catalog,
            session,
            &privilege.privilege,
            &user.user.username,
            &user.user.hostname,
            &database,
            &table_name,
        )?;
        session.execute(SystemMutation::UpdateTableLevel {
            host: user.user.hostname.clone(),
            user: user.user.username.clone(),
            database,
            table: table_name,
            table_privileges: update.table_privileges,
            column_privileges: update.column_privileges,
            grantor: update.grantor,
        })
    }

    /// 逐列合并并写回 columns_priv。
    fn grantColumnLevel(
        &self,
        privilege: &PrivilegeElement,
        user: &UserSpec,
        session: &mut dyn SystemSession,
    ) -> GrantResult {
        let (database, table) = getTargetSchemaAndTable(
            self.dependencies.as_ref(),
            &self.level.db_name,
            &self.level.table_name,
        )?;
        for column in &privilege.columns {
            let canonical_column = find_column(&table, column)
                .ok_or_else(|| GrantError::new(format!("unknown column: {column}")))?;
            let column_privileges = composeColumnPrivUpdateForGrant(
                &self.catalog,
                session,
                &privilege.privilege,
                &user.user.username,
                &user.user.hostname,
                &database,
                &table.table_name,
                canonical_column,
            )?;
            session.execute(SystemMutation::UpdateColumnLevel {
                host: user.user.hostname.clone(),
                user: user.user.username.clone(),
                database: database.clone(),
                table: table.table_name.clone(),
                column: canonical_column.to_owned(),
                column_privileges,
            })?;
        }
        Ok(())
    }
}

/// 权限列表是否含非动态权限（决定是否附加 GRANT OPTION）。
fn containsNonDynamicPriv(privileges: &[PrivilegeElement]) -> bool {
    privileges
        .iter()
        .any(|privilege| !privilege.privilege.is_dynamic())
}

/// 确保 global_priv 行存在。
fn checkAndInitGlobalPriv(session: &mut dyn SystemSession, user: &str, host: &str) -> GrantResult {
    if globalPrivEntryExists(session, user, host)? {
        return Ok(());
    }
    initGlobalPrivEntry(session, user, host)
}

/// 确保 db 权限行存在。
fn checkAndInitDBPriv(
    session: &mut dyn SystemSession,
    database: &str,
    user: &str,
    host: &str,
) -> GrantResult {
    if dbUserExists(session, user, host, database)? {
        return Ok(());
    }
    initDBPrivEntry(session, user, host, database)
}

/// 确保 tables_priv 行存在。
fn checkAndInitTablePriv(
    session: &mut dyn SystemSession,
    database: &str,
    table: &str,
    user: &str,
    host: &str,
) -> GrantResult {
    if tableUserExists(session, user, host, database, table)? {
        return Ok(());
    }
    initTablePrivEntry(session, user, host, database, table)
}

/// 插入空的 global_priv JSON 对象。
fn initGlobalPrivEntry(session: &mut dyn SystemSession, user: &str, host: &str) -> GrantResult {
    session.execute(SystemMutation::InsertGlobalPriv {
        host: host.to_owned(),
        user: user.to_owned(),
        value: "{}".to_owned(),
    })
}

/// 插入空的 db 权限行。
fn initDBPrivEntry(
    session: &mut dyn SystemSession,
    user: &str,
    host: &str,
    database: &str,
) -> GrantResult {
    session.execute(SystemMutation::InsertDatabasePriv {
        host: host.to_owned(),
        user: user.to_owned(),
        database: database.to_owned(),
    })
}

/// 插入空的 tables_priv 行。
fn initTablePrivEntry(
    session: &mut dyn SystemSession,
    user: &str,
    host: &str,
    database: &str,
    table: &str,
) -> GrantResult {
    session.execute(SystemMutation::InsertTablePriv {
        host: host.to_owned(),
        user: user.to_owned(),
        database: database.to_owned(),
        table: table.to_owned(),
    })
}

/// 插入空的 columns_priv 行。
fn initColumnPrivEntry(
    session: &mut dyn SystemSession,
    user: &str,
    host: &str,
    database: &str,
    table: &str,
    column: &str,
) -> GrantResult {
    session.execute(SystemMutation::InsertColumnPriv {
        host: host.to_owned(),
        user: user.to_owned(),
        database: database.to_owned(),
        table: table.to_owned(),
        column: column.to_owned(),
    })
}

/// 将 REQUIRE 选项校验并序列化为 global_priv JSON；无有效选项返回 None。
fn tlsOption2GlobalPriv(
    dependencies: &dyn GrantDependencies,
    options: &[AuthTokenOrTlsOption],
) -> GrantResult<Option<String>> {
    tls_options_to_global_priv_with_validation(
        options,
        |cipher| dependencies.is_supported_cipher(cipher),
        |name| dependencies.validate_x509_name(name),
        |san| dependencies.validate_san(san),
    )
}

/// Convert CREATE/ALTER USER REQUIRE options using the same conversion as GRANT.
/// An omitted clause yields `{}`, whereas a token-issuer-only clause has no TLS value.
pub fn account_tls_options_to_global_priv(
    options: &[astersql_parser_ast::AuthTokenOrTLSOption],
) -> GrantResult<Option<String>> {
    use astersql_parser_ast::AuthTokenOrTLSOptionType as AstOption;
    let options = options
        .iter()
        .map(|option| AuthTokenOrTlsOption {
            option_type: match option.Type {
                AstOption::TlsNone => AuthTokenOrTlsOptionType::TlsNone,
                AstOption::Ssl => AuthTokenOrTlsOptionType::Ssl,
                AstOption::X509 => AuthTokenOrTlsOptionType::X509,
                AstOption::Cipher => AuthTokenOrTlsOptionType::Cipher,
                AstOption::Issuer => AuthTokenOrTlsOptionType::Issuer,
                AstOption::Subject => AuthTokenOrTlsOptionType::Subject,
                AstOption::SAN => AuthTokenOrTlsOptionType::San,
                AstOption::TokenIssuer => AuthTokenOrTlsOptionType::TokenIssuer,
            },
            value: option.Value.clone(),
        })
        .collect::<Vec<_>>();
    tls_options_to_global_priv_with_validation(
        &options,
        |cipher| Ok(astersql_util_tls::SupportCipher.contains(cipher)),
        |name| {
            astersql_util::misc::CheckSupportX509NameOneline(name)
                .map_err(|error| GrantError::new(error.to_string()))
        },
        |san| {
            astersql_util::misc::ParseAndCheckSAN(san)
                .map(|_| ())
                .map_err(|error| GrantError::new(error.to_string()))
        },
    )
}

fn tls_options_to_global_priv_with_validation(
    options: &[AuthTokenOrTlsOption],
    supported_cipher: impl Fn(&str) -> GrantResult<bool>,
    validate_x509_name: impl Fn(&str) -> GrantResult,
    validate_san: impl Fn(&str) -> GrantResult,
) -> GrantResult<Option<String>> {
    if options.is_empty() {
        return Ok(Some("{}".to_owned()));
    }
    let mut seen = BTreeSet::new();
    for option in options {
        // REQUIRE 子句同类选项不可重复。
        if !seen.insert(option.option_type) {
            let type_name = match option.option_type {
                AuthTokenOrTlsOptionType::Cipher => "CIPHER",
                AuthTokenOrTlsOptionType::Issuer => "ISSUER",
                AuthTokenOrTlsOptionType::Subject => "SUBJECT",
                AuthTokenOrTlsOptionType::San => "SAN",
                AuthTokenOrTlsOptionType::TokenIssuer => "",
                AuthTokenOrTlsOptionType::TlsNone
                | AuthTokenOrTlsOptionType::Ssl
                | AuthTokenOrTlsOptionType::X509 => "",
            };
            return Err(GrantError::new(format!(
                "Duplicate require {type_name} clause"
            )));
        }
    }

    let mut global = GlobalPrivValue::default();
    for option in options {
        match option.option_type {
            AuthTokenOrTlsOptionType::TlsNone => global.ssl_type = SslType::None,
            AuthTokenOrTlsOptionType::Ssl => global.ssl_type = SslType::Any,
            AuthTokenOrTlsOptionType::X509 => global.ssl_type = SslType::X509,
            AuthTokenOrTlsOptionType::Cipher => {
                global.ssl_type = SslType::Specified;
                if !option.value.is_empty() {
                    if !supported_cipher(&option.value)? {
                        return Err(GrantError::new(format!(
                            "Unsupported cipher suite: {}",
                            option.value
                        )));
                    }
                    global.ssl_cipher = option.value.clone();
                }
            }
            AuthTokenOrTlsOptionType::Issuer => {
                validate_x509_name(&option.value)?;
                global.ssl_type = SslType::Specified;
                global.x509_issuer = option.value.clone();
            }
            AuthTokenOrTlsOptionType::Subject => {
                validate_x509_name(&option.value)?;
                global.ssl_type = SslType::Specified;
                global.x509_subject = option.value.clone();
            }
            AuthTokenOrTlsOptionType::San => {
                validate_san(&option.value)?;
                global.ssl_type = SslType::Specified;
                global.san = option.value.clone();
            }
            AuthTokenOrTlsOptionType::TokenIssuer => {}
        }
    }
    if global.ssl_type == SslType::NotSpecified
        && global.ssl_cipher.is_empty()
        && global.x509_issuer.is_empty()
        && global.x509_subject.is_empty()
        && global.san.is_empty()
    {
        return Ok(None);
    }
    Ok(Some(serialize_global_priv(&global)))
}

/// 构造全局权限列 UPDATE 赋值列表。
fn composeGlobalPrivUpdate(
    catalog: &PrivilegeCatalog,
    privilege: &PrivilegeType,
    value: &str,
) -> GrantResult<Vec<Assignment>> {
    if !matches!(privilege, PrivilegeType::All) {
        let valid = matches!(privilege, PrivilegeType::Grant) || is_global_privilege(privilege);
        if !valid {
            return Err(GrantError::new(
                "wrong usage: GLOBAL GRANT with non-global privileges",
            ));
        }
        return Ok(vec![Assignment {
            column: privilege_column_name(privilege)?.to_owned(),
            value: value.to_owned(),
        }]);
    }
    Ok(catalog
        .all_global
        .iter()
        .map(|privilege| Assignment {
            column: privilege.column_name.clone(),
            value: value.to_owned(),
        })
        .collect())
}

/// 构造库级权限列 UPDATE 赋值列表。
fn composeDBPrivUpdate(
    catalog: &PrivilegeCatalog,
    privilege: &PrivilegeType,
    value: &str,
) -> GrantResult<Vec<Assignment>> {
    if !matches!(privilege, PrivilegeType::All) {
        let valid = matches!(privilege, PrivilegeType::Grant) || is_database_privilege(privilege);
        if !valid {
            return Err(GrantError::new(
                "wrong usage: DB GRANT with non-database privileges",
            ));
        }
        return Ok(vec![Assignment {
            column: privilege_column_name(privilege)?.to_owned(),
            value: value.to_owned(),
        }]);
    }
    Ok(catalog
        .all_database
        .iter()
        .map(|privilege| Assignment {
            column: privilege.column_name.clone(),
            value: value.to_owned(),
        })
        .collect())
}

/// tables_priv 一次更新所需的 SET 串与 grantor。
struct TablePrivilegeUpdate {
    table_privileges: String,
    column_privileges: String,
    grantor: String,
}

/// 合并已有/ALL 展开的表权限 SET，供 GRANT 写回。
fn composeTablePrivUpdateForGrant(
    catalog: &PrivilegeCatalog,
    session: &mut dyn SystemSession,
    privilege: &PrivilegeType,
    user: &str,
    host: &str,
    database: &str,
    table: &str,
) -> GrantResult<TablePrivilegeUpdate> {
    let (mut table_privileges, mut column_privileges) = if matches!(privilege, PrivilegeType::All) {
        (
            catalog
                .all_table
                .iter()
                .map(|privilege| privilege.set_name.clone())
                .collect(),
            catalog
                .all_column
                .iter()
                .map(|privilege| privilege.set_name.clone())
                .collect(),
        )
    } else {
        let (table_privileges, column_privileges) =
            getTablePriv(session, user, host, database, table)?;
        (
            set_from_string(&table_privileges),
            set_from_string(&column_privileges),
        )
    };
    if !matches!(privilege, PrivilegeType::All) {
        add_to_set(&mut table_privileges, privilege_set_name(privilege)?);
        if is_column_privilege(privilege) {
            add_to_set(&mut column_privileges, privilege_set_name(privilege)?);
        }
    }
    Ok(TablePrivilegeUpdate {
        table_privileges: set_to_string(table_privileges),
        column_privileges: set_to_string(column_privileges),
        grantor: session.grantor()?,
    })
}

/// 合并已有/ALL 展开的列权限 SET。
fn composeColumnPrivUpdateForGrant(
    catalog: &PrivilegeCatalog,
    session: &mut dyn SystemSession,
    privilege: &PrivilegeType,
    user: &str,
    host: &str,
    database: &str,
    table: &str,
    column: &str,
) -> GrantResult<String> {
    let mut privileges = if matches!(privilege, PrivilegeType::All) {
        catalog
            .all_column
            .iter()
            .map(|privilege| privilege.set_name.clone())
            .collect()
    } else {
        set_from_string(&getColumnPriv(
            session, user, host, database, table, column,
        )?)
    };
    if !matches!(privilege, PrivilegeType::All) {
        add_to_set(&mut privileges, privilege_set_name(privilege)?);
    }
    Ok(set_to_string(privileges))
}

/// 查询是否至少返回一行。
fn recordExists(session: &mut dyn SystemSession, query: SystemQuery) -> GrantResult<bool> {
    let record_set = session.query(query)?;
    let (rows, _) = getRowsAndFields(session, record_set)?;
    Ok(!rows.is_empty())
}

/// global_priv 是否已有该用户条目。
fn globalPrivEntryExists(
    session: &mut dyn SystemSession,
    user: &str,
    host: &str,
) -> GrantResult<bool> {
    recordExists(
        session,
        SystemQuery::GlobalPrivExists {
            user: user.to_owned(),
            host: host.to_owned(),
        },
    )
}

/// mysql.db 是否已有该用户库条目。
fn dbUserExists(
    session: &mut dyn SystemSession,
    user: &str,
    host: &str,
    database: &str,
) -> GrantResult<bool> {
    recordExists(
        session,
        SystemQuery::DatabasePrivExists {
            user: user.to_owned(),
            host: host.to_owned(),
            database: database.to_owned(),
        },
    )
}

/// tables_priv 是否已有该用户表条目。
fn tableUserExists(
    session: &mut dyn SystemSession,
    user: &str,
    host: &str,
    database: &str,
    table: &str,
) -> GrantResult<bool> {
    recordExists(
        session,
        SystemQuery::TablePrivExists {
            user: user.to_owned(),
            host: host.to_owned(),
            database: database.to_owned(),
            table: table.to_owned(),
        },
    )
}

/// columns_priv 是否已有该用户列条目。
fn columnPrivEntryExists(
    session: &mut dyn SystemSession,
    user: &str,
    host: &str,
    database: &str,
    table: &str,
    column: &str,
) -> GrantResult<bool> {
    recordExists(
        session,
        SystemQuery::ColumnPrivExists {
            user: user.to_owned(),
            host: host.to_owned(),
            database: database.to_owned(),
            table: table.to_owned(),
            column: column.to_owned(),
        },
    )
}

/// 读取 tables_priv 中表权限与列权限 SET 串。
fn getTablePriv(
    session: &mut dyn SystemSession,
    user: &str,
    host: &str,
    database: &str,
    table: &str,
) -> GrantResult<(String, String)> {
    let record_set = session.query(SystemQuery::TablePrivileges {
        user: user.to_owned(),
        host: host.to_owned(),
        database: database.to_owned(),
        table: table.to_owned(),
    })?;
    let (rows, fields) = getRowsAndFields(session, record_set).map_err(|error| {
        GrantError::new(format!(
            "get table privilege failed for {user} {host} {database} {table}: {error}"
        ))
    })?;
    let row = rows.first().ok_or_else(|| {
        GrantError::new(format!(
            "get table privilege failed for {user} {host} {database} {table}"
        ))
    })?;
    let table_privilege = set_cell(row, fields.first(), 0);
    let column_privilege = set_cell(row, fields.get(1), 1);
    Ok((table_privilege, column_privilege))
}

/// 读取 columns_priv 中列权限 SET 串。
fn getColumnPriv(
    session: &mut dyn SystemSession,
    user: &str,
    host: &str,
    database: &str,
    table: &str,
    column: &str,
) -> GrantResult<String> {
    let record_set = session.query(SystemQuery::ColumnPrivileges {
        user: user.to_owned(),
        host: host.to_owned(),
        database: database.to_owned(),
        table: table.to_owned(),
        column: column.to_owned(),
    })?;
    let (rows, fields) = getRowsAndFields(session, record_set).map_err(|error| {
        GrantError::new(format!(
            "get column privilege failed for {user} {host} {database} {table}: {error}"
        ))
    })?;
    let row = rows.first().ok_or_else(|| {
        GrantError::new(format!(
            "get column privilege failed for {user} {host} {database} {table} {column}"
        ))
    })?;
    Ok(set_cell(row, fields.first(), 0))
}

/// 解析目标库名（空则用当前库）并规范化。
fn getTargetSchemaName(
    dependencies: &dyn GrantDependencies,
    database: &str,
) -> GrantResult<String> {
    let database = if database.is_empty() {
        dependencies.current_database()?
    } else {
        database.to_owned()
    };
    if database.is_empty() {
        return Ok(String::new());
    }
    Ok(dependencies
        .canonical_schema_name(&database)?
        .unwrap_or(database))
}

/// 解析目标库并加载表元数据；表不存在则报错。
fn getTargetSchemaAndTable(
    dependencies: &dyn GrantDependencies,
    database: &str,
    table: &str,
) -> GrantResult<(String, TableMetadata)> {
    let database = getTargetSchemaName(dependencies, database)?;
    if database.is_empty() {
        return Err(GrantError::new("missing database name for GRANT privilege"));
    }
    let metadata = dependencies
        .table_by_name(&database, table)?
        .ok_or_else(|| table_not_exists(&database, table))?;
    Ok((database, metadata))
}

/// 读尽 RecordSet 并关闭，返回行与字段。
fn getRowsAndFields(
    session: &mut dyn SystemSession,
    mut record_set: Box<dyn RecordSet>,
) -> GrantResult<(Vec<Row>, Vec<ResultField>)> {
    let maximum_chunk_size = session.maximum_chunk_size()?;
    let rows = getRowFromRecordSet(record_set.as_mut(), maximum_chunk_size)?;
    record_set.close()?;
    Ok((rows, record_set.fields()))
}

/// 按 chunk 拉取直至空。
fn getRowFromRecordSet(
    record_set: &mut dyn RecordSet,
    maximum_chunk_size: usize,
) -> GrantResult<Vec<Row>> {
    let mut rows = Vec::new();
    loop {
        let mut chunk = record_set.next(maximum_chunk_size)?;
        if chunk.is_empty() {
            return Ok(rows);
        }
        rows.append(&mut chunk);
    }
}

/// 忽略大小写查找列名。
fn find_column<'a>(table: &'a TableMetadata, column: &str) -> Option<&'a str> {
    table
        .columns
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(column))
        .map(String::as_str)
}

/// 构造表不存在错误。
fn table_not_exists(database: &str, table: &str) -> GrantError {
    GrantError::with_kind(
        GrantErrorKind::TableNotExists,
        format!("table does not exist: {database}.{table}"),
    )
}

/// 权限是否允许授予到全局级别。
fn is_global_privilege(privilege: &PrivilegeType) -> bool {
    matches!(privilege, PrivilegeType::Create)
        || matches!(privilege, PrivilegeType::Static(value) if value.global)
}

/// 权限是否允许授予到库级别。
fn is_database_privilege(privilege: &PrivilegeType) -> bool {
    matches!(privilege, PrivilegeType::Create)
        || matches!(privilege, PrivilegeType::Static(value) if value.database)
}

/// 权限是否允许授予到表级别。
fn is_table_privilege(privilege: &PrivilegeType) -> bool {
    matches!(privilege, PrivilegeType::Create)
        || matches!(privilege, PrivilegeType::Static(value) if value.table)
}

/// 权限是否允许授予到列级别。
fn is_column_privilege(privilege: &PrivilegeType) -> bool {
    matches!(privilege, PrivilegeType::Static(value) if value.column)
}

/// 是否为仅能在全局授予的权限。
fn is_global_only_privilege(privilege: &PrivilegeType) -> bool {
    matches!(privilege, PrivilegeType::Static(value) if value.global_only)
}

/// 取系统表权限列名。
fn privilege_column_name(privilege: &PrivilegeType) -> GrantResult<&str> {
    privilege
        .column_name()
        .ok_or_else(|| GrantError::new("privilege has no system-table column"))
}

/// 取 SET 类型权限元素名。
fn privilege_set_name(privilege: &PrivilegeType) -> GrantResult<&str> {
    privilege
        .set_name()
        .ok_or_else(|| GrantError::new("privilege has no SET representation"))
}

/// 将逗号分隔 SET 串拆成列表。
fn set_from_string(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

/// 向 SET 列表追加不重复元素。
fn add_to_set(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_owned());
    }
}

/// 将 SET 列表拼回逗号串。
fn set_to_string(values: Vec<String>) -> String {
    values.join(",")
}

/// 仅当字段为 SET 类型时取出单元格文本。
fn set_cell(row: &Row, field: Option<&ResultField>, index: usize) -> String {
    if !matches!(field.map(|field| field.field_type), Some(FieldType::Set)) {
        return String::new();
    }
    match row.0.get(index) {
        Some(CellValue::Set(value)) => value.clone(),
        _ => String::new(),
    }
}

/// 将 GlobalPrivValue 序列化为 mysql.global_priv 使用的 JSON。
fn serialize_global_priv(global: &GlobalPrivValue) -> String {
    // MySQL SSLType values and Go's json.Marshal `omitempty` representation.
    let ssl_type = match global.ssl_type {
        SslType::NotSpecified => -1,
        SslType::None => 0,
        SslType::Any => 1,
        SslType::X509 => 2,
        SslType::Specified => 3,
    };
    let mut fields = Vec::new();
    if ssl_type != 0 {
        fields.push(format!("\"ssl_type\":{ssl_type}"));
    }
    for (name, value) in [
        ("ssl_cipher", &global.ssl_cipher),
        ("x509_issuer", &global.x509_issuer),
        ("x509_subject", &global.x509_subject),
        ("san", &global.san),
    ] {
        if !value.is_empty() {
            fields.push(format!("\"{name}\":\"{}\"", json_escape(value)));
        }
    }
    format!("{{{}}}", fields.join(","))
}

/// JSON 字符串转义。
fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                escaped.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => escaped.push(character),
        }
    }
    escaped
}

/// 保留的用户展示辅助（与 Go 侧对称）。
fn _user_identity_display(identity: &UserIdentity) -> String {
    identity.display_name()
}
