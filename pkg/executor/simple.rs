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

// Simple 语句执行器：用户/角色、事务、FLUSH、KILL、ADMIN 等非查询类语句。
//
// 对应 Go 的 `SimpleExec`。通过 `SimpleBackend` 把权限校验、密码策略、系统表写入
// 与会话状态变更从执行器迭代路径中解耦。事务（Transaction）在此处理 BEGIN/COMMIT/
// SAVEPOINT；密码历史与复用间隔用于防止短时间重复使用旧口令。

#![allow(non_camel_case_types, non_snake_case)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::time::Duration;

/// 选项未指定时的哨兵值。
const NOT_SPECIFIED: i64 = -1;
/// 资源选项计数上限（有符号 16 位）。
const MAX_INT16: i64 = i16::MAX as i64;
/// 资源选项相关无符号 16 位上限。
const MAX_UINT16: i64 = u16::MAX as i64;

#[derive(Clone, Debug, PartialEq, Eq)]
/// Simple 路径错误：后端失败、权限拒绝、无效选项、用户/角色问题等。
pub enum SimpleError {
    Backend(String),
    AccessDenied(String),
    InvalidOption(String),
    UserNotFound(String, String),
    RoleNotGranted(String),
    PasswordReuse(String),
    Unsupported(String),
}

impl Display for SimpleError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for SimpleError {}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
/// 用户标识：用户名 + 主机名；`current_user` 表示 CURRENT_USER()。
pub struct UserIdentity {
    pub username: String,
    pub hostname: String,
    pub current_user: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// CREATE/ALTER USER 规格：认证插件、口令、双密码保留/丢弃标志。
pub struct UserSpec {
    pub user: UserIdentity,
    pub password: String,
    pub auth_plugin: String,
    pub auth_string: String,
    pub retain_current_password: bool,
    pub discard_old_password: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 每小时查询/更新/连接数及最大用户连接等资源限额种类。
pub enum ResourceOptionKind {
    MaxQueriesPerHour,
    MaxUpdatesPerHour,
    MaxConnectionsPerHour,
    MaxUserConnections,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 单条资源限额选项。
pub struct ResourceOption {
    pub kind: ResourceOptionKind,
    pub count: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 密码过期、账户锁定、失败登录次数、密码历史与复用间隔等选项种类。
pub enum PasswordOptionKind {
    Expire,
    ExpireDefault,
    ExpireNever,
    ExpireInterval,
    Lock,
    Unlock,
    FailedLoginAttempts,
    PasswordLockTime,
    PasswordLockTimeUnbounded,
    PasswordHistory,
    PasswordHistoryDefault,
    PasswordReuseInterval,
    PasswordReuseDefault,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 单条密码/锁定选项及其计数值。
pub struct PasswordOption {
    pub kind: PasswordOptionKind,
    pub count: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// SET ROLE / SET DEFAULT ROLE 的模式：显式列表、ALL、ALL EXCEPT、DEFAULT、NONE。
pub enum RoleMode {
    Regular,
    All,
    AllExcept,
    Default,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// ADMIN 子命令种类（重载统计、刷计划缓存、BDR 角色等）。
pub enum AdminKind {
    ReloadStatistics,
    FlushPlanCache,
    SetBdrRole,
    UnsetBdrRole,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 语句作用域：SESSION / INSTANCE / GLOBAL。
pub enum StatementScope {
    Session,
    Instance,
    Global,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 可执行的 Simple 语句 AST 摘要（授权、用户、事务、FLUSH、KILL 等）。
pub enum Statement {
    GrantRole {
        roles: Vec<UserIdentity>,
        users: Vec<UserIdentity>,
    },
    RevokeRole {
        roles: Vec<UserIdentity>,
        users: Vec<UserIdentity>,
    },
    SetRole {
        mode: RoleMode,
        roles: Vec<UserIdentity>,
    },
    SetDefaultRole {
        mode: RoleMode,
        roles: Vec<UserIdentity>,
        users: Vec<UserIdentity>,
    },
    Use {
        database: String,
    },
    Flush {
        sql: String,
        stats_delta: bool,
    },
    AlterInstance {
        reload_tls: bool,
        no_rollback_on_error: bool,
    },
    Begin {
        read_only: bool,
        stale_start_ts: Option<u64>,
    },
    Commit,
    Savepoint {
        name: String,
    },
    ReleaseSavepoint {
        name: String,
    },
    Rollback {
        savepoint: Option<String>,
    },
    CreateUser {
        specs: Vec<UserSpec>,
        if_not_exists: bool,
        is_role: bool,
    },
    AlterUser {
        specs: Vec<UserSpec>,
        if_exists: bool,
        privileged_options: bool,
    },
    DropUser {
        users: Vec<UserIdentity>,
        if_exists: bool,
        is_role: bool,
    },
    RenameUser {
        pairs: Vec<(UserIdentity, UserIdentity)>,
    },
    SetPassword {
        user: Option<UserIdentity>,
        password: String,
    },
    SetSessionStates {
        json: String,
    },
    Kill {
        connection_id: u64,
        query: bool,
    },
    RefreshStats {
        sql: String,
        current_instance: bool,
    },
    DropStats {
        table_ids: Vec<i64>,
    },
    Shutdown,
    Admin {
        kind: AdminKind,
        scope: StatementScope,
        value: String,
    },
    SetResourceGroup {
        name: String,
    },
    AlterRange {
        range: String,
        policy: String,
    },
    DropQueryWatch {
        id: u64,
    },
    Binlog,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 发给 Backend 的具体操作（SQL、通知权限、事务控制、远程 KILL 等）。
pub enum Operation {
    Sql(String),
    NotifyPrivileges(Vec<String>),
    SetDatabase(String),
    SetRoles(Vec<UserIdentity>),
    Begin {
        read_only: bool,
        stale_start_ts: Option<u64>,
    },
    Commit,
    Rollback,
    Savepoint(String),
    ReleaseSavepoint(String),
    RollbackToSavepoint(String),
    Kill {
        connection_id: u64,
        query: bool,
        remote: bool,
    },
    RefreshStats(String),
    FlushStatsDelta(String),
    DropStats(Vec<i64>),
    ReloadTls {
        no_rollback_on_error: bool,
    },
    Shutdown {
        delay: Duration,
    },
    DecodeSessionStates(String),
    FlushPlanCache(StatementScope),
    SetBdrRole(String),
    UnsetBdrRole,
    SetResourceGroup(String),
    AlterRange {
        range: String,
        policy: String,
    },
    DropQueryWatch(u64),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 系统表中的用户记录：认证串、密码历史、属性与锁定/过期标志。
pub struct UserRecord {
    pub identity: UserIdentity,
    pub auth_plugin: String,
    pub auth_string: String,
    pub password_history: Vec<(String, i64)>,
    pub attributes: BTreeMap<String, String>,
    pub account_locked: bool,
    pub password_expired: bool,
}

/// Simple 执行后端：执行操作、查用户、校验权限/密码、会话与部署标志。
pub trait SimpleBackend: Send + Sync + 'static {
    fn execute(&self, operation: Operation) -> Result<(), SimpleError>;
    fn query_user(
        &self,
        user: &UserIdentity,
        for_update: bool,
    ) -> Result<Option<UserRecord>, SimpleError>;
    fn username_variants(&self, name: &str) -> Vec<String>;
    fn validate_username(&self, name: &str) -> Result<(), SimpleError>;
    fn validate_username_format(&self, name: &str) -> bool;
    fn verify_privilege(&self, privilege: &str) -> bool;
    fn verify_role_edge(
        &self,
        role: &UserIdentity,
        user: &UserIdentity,
    ) -> Result<bool, SimpleError>;
    fn validate_password(&self, user: &UserIdentity, password: &str) -> Result<(), SimpleError>;
    fn hash_password(&self, plugin: &str, password: &str) -> Result<String, SimpleError>;
    fn check_hashing_password(
        &self,
        hash: &str,
        password: &str,
        plugin: &str,
    ) -> Result<bool, SimpleError>;
    fn auth_plugin_clear_text(&self, plugin: &str) -> bool;
    fn default_auth_plugin(&self) -> Result<String, SimpleError>;
    fn global_password_history(&self) -> i64;
    fn global_password_reuse_interval(&self) -> i64;
    fn now_unix(&self) -> i64;
    fn current_user(&self) -> Option<UserIdentity>;
    fn active_roles(&self) -> Vec<UserIdentity>;
    fn set_current_user(&self, users: &[UserIdentity]);
    fn in_transaction(&self) -> bool;
    fn pessimistic_transaction(&self) -> bool;
    fn restricted_read_only(&self) -> bool;
    fn is_starter_deployment(&self) -> bool;
    fn skip_grant_table(&self) -> bool;
    fn validate_password_enabled(&self) -> bool;
    fn resource_group_exists(&self, name: &str) -> bool;
    fn placement_policy_exists(&self, name: &str) -> bool;
    fn broadcast(&self, sql: &str) -> Result<(), SimpleError>;
    fn warn(&self, warning: SimpleError);
}

#[derive(Clone, Debug)]
/// Simple 执行器状态：当前语句、解析上下文、是否来自远程及完成标记。
pub struct SimpleExec<B: SimpleBackend> {
    pub backend: Arc<B>,
    pub Statement: Statement,
    pub ResolveCtx: Option<String>,
    pub IsFromRemote: bool,
    pub done: bool,
    pub staleTxnStartTS: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 从 AST 解析出的资源限额聚合。
pub struct resourceOptionsInfo {
    pub maxQueriesPerHour: i64,
    pub maxUpdatesPerHour: i64,
    pub maxConnectionsPerHour: i64,
    pub maxUserConnections: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 从 AST 解析出的密码过期/锁定/历史等选项聚合。
pub struct passwordOrLockOptionsInfo {
    pub lockAccount: String,
    pub passwordExpired: String,
    pub passwordLifetime: Option<i64>,
    pub passwordHistory: i64,
    pub passwordHistoryChange: bool,
    pub passwordReuseInterval: i64,
    pub passwordReuseIntervalChange: bool,
    pub failedLoginAttempts: i64,
    pub passwordLockTime: i64,
    pub failedLoginAttemptsChange: bool,
    pub passwordLockTimeChange: bool,
}

impl Default for passwordOrLockOptionsInfo {
    fn default() -> Self {
        Self {
            lockAccount: String::new(),
            passwordExpired: String::new(),
            passwordLifetime: None,
            passwordHistory: NOT_SPECIFIED,
            passwordHistoryChange: false,
            passwordReuseInterval: NOT_SPECIFIED,
            passwordReuseIntervalChange: false,
            failedLoginAttempts: 0,
            passwordLockTime: 0,
            failedLoginAttemptsChange: false,
            passwordLockTimeChange: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 密码历史条数与复用时间间隔（秒/天语义由调用方解释）。
pub struct passwordReuseInfo {
    pub passwordHistory: i64,
    pub passwordReuseInterval: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 变更用户时携带的主机、用户名、选项与口令/认证串。
pub struct userInfo {
    pub host: String,
    pub user: String,
    pub pLI: Option<passwordOrLockOptionsInfo>,
    pub pwd: String,
    pub authString: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// ALTER USER 密码锁定相关字段及“是否显式出现”标记。
pub struct alterUserPasswordLocking {
    pub failedLoginAttempts: i64,
    pub passwordLockTime: i64,
    pub failedLoginAttemptsNotFound: bool,
    pub passwordLockTimeChangeNotFound: bool,
    pub containsNoOthers: bool,
}

impl resourceOptionsInfo {
    /// 加载并钳制资源选项到合法范围。
    pub fn loadResourceOptions(&mut self, options: &[ResourceOption]) -> Result<(), SimpleError> {
        for option in options {
            let count = option.count.clamp(0, MAX_INT16);
            match option.kind {
                ResourceOptionKind::MaxQueriesPerHour => self.maxQueriesPerHour = count,
                ResourceOptionKind::MaxUpdatesPerHour => self.maxUpdatesPerHour = count,
                ResourceOptionKind::MaxConnectionsPerHour => self.maxConnectionsPerHour = count,
                ResourceOptionKind::MaxUserConnections => self.maxUserConnections = count,
            }
        }
        Ok(())
    }
}

impl passwordOrLockOptionsInfo {
    /// 解析密码/锁定选项，记录哪些字段被显式修改。
    pub fn loadOptions(&mut self, options: &[PasswordOption]) -> Result<(), SimpleError> {
        for option in options.iter().rev() {
            match option.kind {
                PasswordOptionKind::Expire => {
                    self.passwordExpired = "Y".into();
                    break;
                }
                PasswordOptionKind::ExpireDefault => {
                    self.passwordLifetime = None;
                    break;
                }
                PasswordOptionKind::ExpireNever => {
                    self.passwordLifetime = Some(0);
                    break;
                }
                PasswordOptionKind::ExpireInterval => {
                    if !(1..=MAX_UINT16).contains(&option.count) {
                        return Err(SimpleError::InvalidOption(format!(
                            "invalid password lifetime {}",
                            option.count
                        )));
                    }
                    self.passwordLifetime = Some(option.count);
                    break;
                }
                _ => {}
            }
        }
        for option in options {
            match option.kind {
                PasswordOptionKind::Lock => self.lockAccount = "Y".into(),
                PasswordOptionKind::Unlock => self.lockAccount = "N".into(),
                PasswordOptionKind::FailedLoginAttempts => {
                    self.failedLoginAttempts = option.count.clamp(0, MAX_INT16);
                    self.failedLoginAttemptsChange = true;
                }
                PasswordOptionKind::PasswordLockTime => {
                    self.passwordLockTime = option.count.clamp(0, MAX_INT16);
                    self.passwordLockTimeChange = true;
                }
                PasswordOptionKind::PasswordLockTimeUnbounded => {
                    self.passwordLockTime = -1;
                    self.passwordLockTimeChange = true;
                }
                PasswordOptionKind::PasswordHistory => {
                    self.passwordHistory = option.count.clamp(0, MAX_UINT16);
                    self.passwordHistoryChange = true;
                }
                PasswordOptionKind::PasswordHistoryDefault => {
                    self.passwordHistory = NOT_SPECIFIED;
                    self.passwordHistoryChange = true;
                }
                PasswordOptionKind::PasswordReuseInterval => {
                    self.passwordReuseInterval = option.count.clamp(0, MAX_UINT16);
                    self.passwordReuseIntervalChange = true;
                }
                PasswordOptionKind::PasswordReuseDefault => {
                    self.passwordReuseInterval = NOT_SPECIFIED;
                    self.passwordReuseIntervalChange = true;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// 判断当前认证插件与策略下是否需要写入密码历史。
pub fn whetherSavePasswordHistory<B: SimpleBackend>(
    backend: &B,
    options: &passwordOrLockOptionsInfo,
) -> bool {
    let count = if options.passwordHistoryChange && options.passwordHistory != NOT_SPECIFIED {
        options.passwordHistory
    } else {
        backend.global_password_history()
    };
    let days =
        if options.passwordReuseIntervalChange && options.passwordReuseInterval != NOT_SPECIFIED {
            options.passwordReuseInterval
        } else {
            backend.global_password_reuse_interval()
        };
    count > 0 || days > 0
}

/// 生成 CREATE USER 失败登录锁定属性的 JSON。
pub fn createUserFailedLoginJSON(info: &passwordOrLockOptionsInfo) -> String {
    if (info.failedLoginAttemptsChange && info.failedLoginAttempts != 0)
        || (info.passwordLockTimeChange && info.passwordLockTime != 0)
    {
        format!(
            "\"Password_locking\": {{\"failed_login_attempts\": {},\"password_lock_time_days\": {}}}",
            info.failedLoginAttempts, info.passwordLockTime
        )
    } else {
        String::new()
    }
}

/// 生成 ALTER USER 失败登录/锁定属性的 JSON。
pub fn alterUserFailedLoginJSON(info: &alterUserPasswordLocking, lock_account: &str) -> String {
    let mut fields = Vec::new();
    if info.failedLoginAttempts != 0 || info.passwordLockTime != 0 {
        if lock_account == "N" {
            fields.extend([
                "\"auto_account_locked\": \"N\"".into(),
                "\"failed_login_count\": 0".into(),
            ]);
        }
        fields.push(format!(
            "\"failed_login_attempts\": {}",
            info.failedLoginAttempts
        ));
        fields.push(format!(
            "\"password_lock_time_days\": {}",
            info.passwordLockTime
        ));
    }
    if fields.is_empty() {
        String::new()
    } else {
        format!("\"Password_locking\": {{{}}}", fields.join(","))
    }
}

/// 从用户属性 JSON 读取失败登录与锁定时间信息。
pub fn readPasswordLockingInfo(
    record: Option<&UserRecord>,
    options: &passwordOrLockOptionsInfo,
) -> Result<alterUserPasswordLocking, SimpleError> {
    let mut result = alterUserPasswordLocking::default();
    let attrs = record.map(|value| &value.attributes);
    if options.failedLoginAttemptsChange {
        result.failedLoginAttempts = options.failedLoginAttempts;
    } else if let Some(value) = attrs.and_then(|map| map.get("failed_login_attempts")) {
        result.failedLoginAttempts = value
            .parse::<i64>()
            .map_err(|error| SimpleError::InvalidOption(error.to_string()))?
            .clamp(0, MAX_INT16);
    } else {
        result.failedLoginAttemptsNotFound = true;
    }
    if options.passwordLockTimeChange {
        result.passwordLockTime = options.passwordLockTime;
    } else if let Some(value) = attrs.and_then(|map| map.get("password_lock_time_days")) {
        result.passwordLockTime = value
            .parse::<i64>()
            .map_err(|error| SimpleError::InvalidOption(error.to_string()))?
            .clamp(-1, MAX_INT16);
    } else {
        result.passwordLockTimeChangeNotFound = true;
    }
    result.containsNoOthers = attrs.is_none_or(|map| {
        map.keys()
            .all(|key| key.starts_with("password_") || key.starts_with("failed_"))
    });
    Ok(result)
}

/// 删除用户属性中的密码锁定相关键。
pub fn deletePasswordLockingAttribute<B: SimpleBackend>(
    backend: &B,
    user: &UserIdentity,
    info: &alterUserPasswordLocking,
) -> Result<(), SimpleError> {
    if (info.failedLoginAttemptsNotFound && info.passwordLockTimeChangeNotFound)
        || info.failedLoginAttempts != 0
        || info.passwordLockTime != 0
    {
        return Ok(());
    }
    let expression = if info.containsNoOthers {
        "NULL"
    } else {
        "JSON_REMOVE(user_attributes, '$.Password_locking')"
    };
    backend.execute(Operation::Sql(format!(
        "UPDATE mysql.user SET user_attributes={expression} WHERE Host='{}' AND User='{}'",
        user.hostname.to_lowercase(),
        user.username
    )))
}

/// 将用户标识列表格式化为 `user@host` 字符串列表。
pub fn userIdentityToUserList(specs: &[UserIdentity]) -> Vec<String> {
    specs
        .iter()
        .map(|user| format!("'{}'@'{}'", user.username, user.hostname))
        .collect()
}

/// 返回 (retain_current, discard_old) 双密码选项。
pub fn dualPasswordOption(spec: &UserSpec) -> (bool, bool) {
    (spec.retain_current_password, spec.discard_old_password)
}
/// 任一规格请求了双密码相关选项则为 true。
pub fn dualPasswordRequested(specs: &[UserSpec]) -> bool {
    specs
        .iter()
        .any(|spec| spec.retain_current_password || spec.discard_old_password)
}
/// 检查 ALTER USER 是否包含需要特权的语句级选项。
pub fn alterUserHasPrivilegedOptions(stmt: &astersql_parser_ast::AlterUserStmt) -> bool {
    !stmt.AuthTokenOrTLSOptions.is_empty()
        || !stmt.ResourceOptions.is_empty()
        || !stmt.PasswordOrLockOptions.is_empty()
        || stmt.CommentOrAttributeOption.is_some()
        || stmt.ResourceGroupNameOption.is_some()
}

/// 取出已认证用户的用户名与主机。
pub fn authenticatedUserNameAndHost(user: &UserIdentity) -> (String, String) {
    (user.username.clone(), user.hostname.clone())
}

/// 是否跳过精确用户名查找（走变体匹配）。
pub fn skipExactUsernameLookup<B: SimpleBackend>(backend: &B, name: &str) -> bool {
    backend.validate_username(name).is_err() && backend.validate_username_format(name)
}

/// 认证插件是否支持双密码。
pub fn isDualPasswordCapablePlugin<B: SimpleBackend>(backend: &B, plugin: &str) -> bool {
    backend.auth_plugin_clear_text(plugin)
}

/// 空插件名时回落到默认认证插件。
pub fn effectiveAuthPlugin(plugin: &str, default_plugin: &str) -> String {
    if !plugin.is_empty() {
        plugin.into()
    } else if default_plugin.is_empty() {
        "mysql_native_password".into()
    } else {
        default_plugin.into()
    }
}

/// 构造双密码场景下的附加口令历史条目。
pub fn buildAdditionalPasswordEntry(
    old_password: &str,
    name: &str,
    host: &str,
) -> Result<String, SimpleError> {
    if old_password.is_empty() {
        return Err(SimpleError::InvalidOption(format!(
            "second password for '{name}'@'{host}' cannot be empty"
        )));
    }
    let escaped = old_password.replace('\\', "\\\\").replace('"', "\\\"");
    Ok(format!("\"additional_password\": \"{escaped}\""))
}

/// 根据复用间隔计算历史口令仍有效的最早时间戳。
pub fn getValidTime<B: SimpleBackend>(backend: &B, reuse: &passwordReuseInfo) -> i64 {
    backend
        .now_unix()
        .saturating_sub(reuse.passwordReuseInterval.saturating_mul(86_400))
        .max(0)
}

/// 收集 FLUSH 统计增量涉及的目标表 ID。
pub fn appendStatsDeltaTargetTableIDs(
    target_ids: &mut Vec<i64>,
    table_id: i64,
    partition_ids: &[i64],
) -> Vec<i64> {
    target_ids.push(table_id);
    target_ids.extend_from_slice(partition_ids);
    target_ids.clone()
}

/// 规范化/还原 REFRESH STATS 语句文本。
pub fn restoreRefreshStatsSQL(sql: &str) -> Result<String, SimpleError> {
    if sql.trim().is_empty() {
        Err(SimpleError::InvalidOption("empty refresh stats SQL".into()))
    } else {
        Ok(sql.into())
    }
}
/// 规范化/还原 FLUSH STATS_DELTA 语句文本。
pub fn restoreFlushStatsDeltaSQL(sql: &str) -> Result<String, SimpleError> {
    if sql.trim().is_empty() {
        Err(SimpleError::InvalidOption("empty flush stats SQL".into()))
    } else {
        Ok(sql.into())
    }
}

/// 异步延迟后触发关闭（给客户端时间收到响应）。
pub fn asyncDelayShutdown<B: SimpleBackend>(
    backend: Arc<B>,
    delay: Duration,
) -> Result<(), SimpleError> {
    backend.execute(Operation::Shutdown { delay })
}

impl<B: SimpleBackend> SimpleExec<B> {
    /// 执行一次 Simple 语句（完成后置 done，重复调用直接返回）。
    pub fn Next(&mut self) -> Result<(), SimpleError> {
        // 语句只需执行一次；后续 Next 为空操作。
        if self.done {
            return Ok(());
        }
        if self.autoNewTxn() {
            self.backend.execute(Operation::Commit)?;
        }
        let statement = self.Statement.clone();
        let result = match statement {
            Statement::GrantRole { roles, users } => self.executeGrantRole(&roles, &users),
            Statement::RevokeRole { roles, users } => self.executeRevokeRole(&roles, &users),
            Statement::SetRole { mode, roles } => self.executeSetRole(mode, &roles),
            Statement::SetDefaultRole { mode, roles, users } => {
                self.executeSetDefaultRole(mode, &roles, &users)
            }
            Statement::Use { database } => self.executeUse(&database),
            Statement::Flush { sql, stats_delta } => self.executeFlush(&sql, stats_delta),
            Statement::AlterInstance {
                reload_tls,
                no_rollback_on_error,
            } => self.executeAlterInstance(reload_tls, no_rollback_on_error),
            Statement::Begin {
                read_only,
                stale_start_ts,
            } => self.executeBegin(read_only, stale_start_ts),
            Statement::Commit => {
                self.executeCommit();
                Ok(())
            }
            Statement::Savepoint { name } => self.executeSavepoint(&name),
            Statement::ReleaseSavepoint { name } => self.executeReleaseSavepoint(&name),
            Statement::Rollback { savepoint } => self.executeRollback(savepoint.as_deref()),
            Statement::CreateUser {
                specs,
                if_not_exists,
                is_role,
            } => self.executeCreateUser(&specs, if_not_exists, is_role),
            Statement::AlterUser {
                specs,
                if_exists,
                privileged_options,
            } => self.executeAlterUser(&specs, if_exists, privileged_options),
            Statement::DropUser {
                users,
                if_exists,
                is_role,
            } => self.executeDropUser(&users, if_exists, is_role),
            Statement::RenameUser { pairs } => self.executeRenameUser(&pairs),
            Statement::SetPassword { user, password } => {
                self.executeSetPwd(user.as_ref(), &password)
            }
            Statement::SetSessionStates { json } => self.executeSetSessionStates(&json),
            Statement::Kill {
                connection_id,
                query,
            } => self.executeKillStmt(connection_id, query),
            Statement::RefreshStats {
                sql,
                current_instance,
            } => self.executeRefreshStats(&sql, current_instance),
            Statement::DropStats { table_ids } => self.executeDropStats(&table_ids),
            Statement::Shutdown => self.executeShutdown(),
            Statement::Admin { kind, scope, value } => self.executeAdmin(kind, scope, &value),
            Statement::SetResourceGroup { name } => self.executeSetResourceGroupName(&name),
            Statement::AlterRange { range, policy } => self.executeAlterRange(&range, &policy),
            Statement::DropQueryWatch { id } => self.executeDropQueryWatch(id),
            Statement::Binlog => Ok(()),
        };
        self.done = true;
        result
    }

    /// SET DEFAULT ROLE NONE。
    pub fn setDefaultRoleNone(&mut self, users: &[UserIdentity]) -> Result<(), SimpleError> {
        self.set_default_roles(&[], users)
    }
    /// SET DEFAULT ROLE 显式角色列表。
    pub fn setDefaultRoleRegular(
        &mut self,
        roles: &[UserIdentity],
        users: &[UserIdentity],
    ) -> Result<(), SimpleError> {
        for user in users {
            for role in roles {
                if !self.backend.verify_role_edge(role, user)? {
                    return Err(SimpleError::RoleNotGranted(format!(
                        "{}@{}",
                        role.username, role.hostname
                    )));
                }
            }
        }
        self.set_default_roles(roles, users)
    }
    /// SET DEFAULT ROLE ALL。
    pub fn setDefaultRoleAll(&mut self, users: &[UserIdentity]) -> Result<(), SimpleError> {
        for user in users {
            self.require_user(user)?;
        }
        self.backend.execute(Operation::Sql(format!(
            "REPLACE mysql.default_roles FROM mysql.role_edges FOR {}",
            userIdentityToUserList(users).join(",")
        )))?;
        self.backend
            .execute(Operation::NotifyPrivileges(userIdentityToUserList(users)))
    }
    /// 为当前用户设置默认角色。
    pub fn setDefaultRoleForCurrentUser(
        &mut self,
        roles: &[UserIdentity],
    ) -> Result<(), SimpleError> {
        let user = self
            .backend
            .current_user()
            .ok_or_else(|| SimpleError::AccessDenied("missing current user".into()))?;
        self.setDefaultRoleRegular(roles, &[user])
    }
    /// 分派 SET DEFAULT ROLE 各模式。
    pub fn executeSetDefaultRole(
        &mut self,
        mode: RoleMode,
        roles: &[UserIdentity],
        users: &[UserIdentity],
    ) -> Result<(), SimpleError> {
        match mode {
            RoleMode::None => self.setDefaultRoleNone(users),
            RoleMode::All => self.setDefaultRoleAll(users),
            RoleMode::Regular => self.setDefaultRoleRegular(roles, users),
            RoleMode::Default | RoleMode::AllExcept => {
                Err(SimpleError::Unsupported("default-role mode".into()))
            }
        }
    }
    /// SET ROLE 显式列表。
    pub fn setRoleRegular(&mut self, roles: &[UserIdentity]) -> Result<(), SimpleError> {
        let user = self
            .backend
            .current_user()
            .ok_or_else(|| SimpleError::AccessDenied("missing current user".into()))?;
        for role in roles {
            if !self.backend.verify_role_edge(role, &user)? {
                return Err(SimpleError::RoleNotGranted(role.username.clone()));
            }
        }
        self.backend.execute(Operation::SetRoles(roles.to_vec()))
    }
    /// SET ROLE ALL。
    pub fn setRoleAll(&mut self) -> Result<(), SimpleError> {
        self.backend
            .execute(Operation::SetRoles(self.backend.active_roles()))
    }
    /// SET ROLE ALL EXCEPT ...。
    pub fn setRoleAllExcept(&mut self, excluded: &[UserIdentity]) -> Result<(), SimpleError> {
        let excluded: BTreeSet<_> = excluded.iter().cloned().collect();
        let roles = self
            .backend
            .active_roles()
            .into_iter()
            .filter(|role| !excluded.contains(role))
            .collect();
        self.backend.execute(Operation::SetRoles(roles))
    }
    /// SET ROLE DEFAULT。
    pub fn setRoleDefault(&mut self) -> Result<(), SimpleError> {
        self.setRoleAll()
    }
    /// SET ROLE NONE。
    pub fn setRoleNone(&mut self) -> Result<(), SimpleError> {
        self.backend.execute(Operation::SetRoles(Vec::new()))
    }
    /// 分派 SET ROLE 各模式。
    pub fn executeSetRole(
        &mut self,
        mode: RoleMode,
        roles: &[UserIdentity],
    ) -> Result<(), SimpleError> {
        match mode {
            RoleMode::Regular => self.setRoleRegular(roles),
            RoleMode::All => self.setRoleAll(),
            RoleMode::AllExcept => self.setRoleAllExcept(roles),
            RoleMode::Default => self.setRoleDefault(),
            RoleMode::None => self.setRoleNone(),
        }
    }
    /// 构造库访问被拒错误。
    pub fn dbAccessDenied(&mut self, database: &str) -> SimpleError {
        SimpleError::AccessDenied(format!("database access denied: {database}"))
    }
    /// USE db：切换当前库。
    pub fn executeUse(&mut self, database: &str) -> Result<(), SimpleError> {
        if !self.backend.verify_privilege("USE") {
            return Err(self.dbAccessDenied(database));
        }
        self.backend
            .execute(Operation::SetDatabase(database.into()))
    }
    /// BEGIN：[只读]/stale_read 起始 TS。
    pub fn executeBegin(
        &mut self,
        read_only: bool,
        stale_start_ts: Option<u64>,
    ) -> Result<(), SimpleError> {
        self.staleTxnStartTS = stale_start_ts.unwrap_or(0);
        self.backend.execute(Operation::Begin {
            read_only,
            stale_start_ts,
        })
    }
    /// SAVEPOINT name。
    pub fn executeSavepoint(&mut self, name: &str) -> Result<(), SimpleError> {
        if self.backend.in_transaction() {
            self.backend.execute(Operation::Savepoint(name.into()))
        } else {
            Ok(())
        }
    }
    /// RELEASE SAVEPOINT name。
    pub fn executeReleaseSavepoint(&mut self, name: &str) -> Result<(), SimpleError> {
        self.backend
            .execute(Operation::ReleaseSavepoint(name.into()))
    }
    /// 更新会话当前用户集合。
    pub fn setCurrentUser(&mut self, users: &[UserIdentity]) {
        self.backend.set_current_user(users);
    }
    /// REVOKE role FROM user。
    pub fn executeRevokeRole(
        &mut self,
        roles: &[UserIdentity],
        users: &[UserIdentity],
    ) -> Result<(), SimpleError> {
        self.require_privilege("REVOKE ROLE")?;
        self.backend.execute(Operation::Sql(format!(
            "DELETE mysql.role_edges roles={} users={}",
            userIdentityToUserList(roles).join(","),
            userIdentityToUserList(users).join(",")
        )))?;
        self.setCurrentUser(users);
        self.backend
            .execute(Operation::NotifyPrivileges(userIdentityToUserList(users)))
    }
    /// COMMIT。
    pub fn executeCommit(&mut self) {
        if let Err(error) = self.backend.execute(Operation::Commit) {
            self.backend.warn(error);
        }
    }
    /// ROLLBACK 或 ROLLBACK TO SAVEPOINT。
    pub fn executeRollback(&mut self, savepoint: Option<&str>) -> Result<(), SimpleError> {
        match savepoint {
            Some(name) => self
                .backend
                .execute(Operation::RollbackToSavepoint(name.into())),
            None => self.backend.execute(Operation::Rollback),
        }
    }
    /// 是否启用密码强度校验插件策略。
    pub fn isValidatePasswordEnabled(&mut self) -> bool {
        self.backend.validate_password_enabled()
    }

    /// CREATE USER / CREATE ROLE：校验、哈希口令、写系统表并通知权限。
    pub fn executeCreateUser(
        &mut self,
        specs: &[UserSpec],
        if_not_exists: bool,
        is_role: bool,
    ) -> Result<(), SimpleError> {
        self.require_privilege(if is_role {
            "CREATE ROLE"
        } else {
            "CREATE USER"
        })?;
        self.backend
            .execute(Operation::Sql("BEGIN PESSIMISTIC".into()))?;
        let result = (|| {
            for spec in specs {
                if self.backend.query_user(&spec.user, true)?.is_some() {
                    if if_not_exists {
                        self.backend.warn(SimpleError::InvalidOption(format!(
                            "user {} exists",
                            spec.user.username
                        )));
                        continue;
                    }
                    return Err(SimpleError::InvalidOption(format!(
                        "user {} exists",
                        spec.user.username
                    )));
                }
                self.backend.validate_username(&spec.user.username)?;
                if self.isValidatePasswordEnabled() && !is_role {
                    self.backend.validate_password(&spec.user, &spec.password)?;
                }
                let plugin =
                    effectiveAuthPlugin(&spec.auth_plugin, &self.backend.default_auth_plugin()?);
                let password = if is_role {
                    String::new()
                } else {
                    self.backend.hash_password(&plugin, &spec.password)?
                };
                self.backend.execute(Operation::Sql(format!(
                    "INSERT mysql.user USER='{}' HOST='{}' PLUGIN='{}' AUTH='{}' ROLE={is_role}",
                    spec.user.username,
                    spec.user.hostname.to_lowercase(),
                    plugin,
                    password
                )))?;
            }
            Ok(())
        })();
        self.finish_transaction(result)?;
        self.backend
            .execute(Operation::NotifyPrivileges(userIdentityToUserList(
                &specs.iter().map(|s| s.user.clone()).collect::<Vec<_>>(),
            )))
    }

    /// ALTER USER：改口令/选项，处理双密码与锁定属性。
    pub fn executeAlterUser(
        &mut self,
        specs: &[UserSpec],
        if_exists: bool,
        privileged_options: bool,
    ) -> Result<(), SimpleError> {
        let self_service = specs
            .iter()
            .all(|spec| self.backend.current_user().as_ref() == Some(&spec.user));
        if !self_service || privileged_options {
            self.require_privilege("ALTER USER")?;
        }
        self.checkSandboxMode(specs)?;
        self.backend
            .execute(Operation::Sql("BEGIN PESSIMISTIC".into()))?;
        let result = (|| {
            for spec in specs {
                let Some(record) = self.backend.query_user(&spec.user, true)? else {
                    if if_exists {
                        self.backend.warn(SimpleError::UserNotFound(
                            spec.user.username.clone(),
                            spec.user.hostname.clone(),
                        ));
                        continue;
                    }
                    return Err(SimpleError::UserNotFound(
                        spec.user.username.clone(),
                        spec.user.hostname.clone(),
                    ));
                };
                let default_plugin = self.backend.default_auth_plugin()?;
                let old_plugin = effectiveAuthPlugin(&record.auth_plugin, &default_plugin);
                let new_plugin = effectiveAuthPlugin(&spec.auth_plugin, &default_plugin);
                if spec.retain_current_password
                    && !isDualPasswordCapablePlugin(self.backend.as_ref(), &old_plugin)
                {
                    return Err(SimpleError::InvalidOption(
                        "auth plugin cannot retain password".into(),
                    ));
                }
                if spec.retain_current_password && old_plugin != new_plugin {
                    return Err(SimpleError::InvalidOption(
                        "cannot change plugin while retaining password".into(),
                    ));
                }
                if self.isValidatePasswordEnabled() {
                    self.backend.validate_password(&spec.user, &spec.password)?;
                }
                let hashed = self.backend.hash_password(&new_plugin, &spec.password)?;
                self.checkPasswordReusePolicy(
                    &userInfo {
                        host: spec.user.hostname.clone(),
                        user: spec.user.username.clone(),
                        pwd: hashed.clone(),
                        authString: record.auth_string.clone(),
                        pLI: None,
                    },
                    &new_plugin,
                )?;
                let additional = if spec.retain_current_password {
                    Some(buildAdditionalPasswordEntry(
                        &record.auth_string,
                        &spec.user.username,
                        &spec.user.hostname,
                    )?)
                } else {
                    None
                };
                self.backend.execute(Operation::Sql(format!("UPDATE mysql.user USER='{}' HOST='{}' PLUGIN='{}' AUTH='{}' ADDITIONAL={additional:?} DISCARD={}", spec.user.username, spec.user.hostname.to_lowercase(), new_plugin, hashed, spec.discard_old_password)))?;
            }
            Ok(())
        })();
        self.finish_transaction(result)?;
        self.backend
            .execute(Operation::NotifyPrivileges(userIdentityToUserList(
                &specs.iter().map(|s| s.user.clone()).collect::<Vec<_>>(),
            )))
    }

    /// 沙箱/受限模式下禁止的用户操作检查。
    pub fn checkSandboxMode(&mut self, specs: &[UserSpec]) -> Result<(), SimpleError> {
        if self.backend.restricted_read_only()
            && !specs
                .iter()
                .all(|spec| self.backend.current_user().as_ref() == Some(&spec.user))
        {
            Err(SimpleError::AccessDenied(
                "sandbox mode only permits own password".into(),
            ))
        } else {
            Ok(())
        }
    }

    /// GRANT role TO user。
    pub fn executeGrantRole(
        &mut self,
        roles: &[UserIdentity],
        users: &[UserIdentity],
    ) -> Result<(), SimpleError> {
        self.require_privilege("GRANT ROLE")?;
        for role in roles {
            if !isRole(self.backend.as_ref(), role)? {
                return Err(SimpleError::InvalidOption(format!(
                    "{} is not a role",
                    role.username
                )));
            }
        }
        for user in users {
            self.require_user(user)?;
        }
        self.backend.execute(Operation::Sql(format!(
            "INSERT mysql.role_edges roles={} users={}",
            userIdentityToUserList(roles).join(","),
            userIdentityToUserList(users).join(",")
        )))?;
        self.backend
            .execute(Operation::NotifyPrivileges(userIdentityToUserList(users)))
    }

    /// RENAME USER：更新系统表中的用户@主机。
    pub fn executeRenameUser(
        &mut self,
        pairs: &[(UserIdentity, UserIdentity)],
    ) -> Result<(), SimpleError> {
        self.require_privilege("CREATE USER")?;
        self.backend
            .execute(Operation::Sql("BEGIN PESSIMISTIC".into()))?;
        let result = (|| {
            for (old, new) in pairs {
                self.require_user(old)?;
                if self.backend.query_user(new, true)?.is_some() {
                    return Err(SimpleError::InvalidOption(format!(
                        "target {} exists",
                        new.username
                    )));
                }
                for table in [
                    "user",
                    "db",
                    "tables_priv",
                    "columns_priv",
                    "global_grants",
                    "default_roles",
                    "role_edges",
                    "password_history",
                ] {
                    renameUserHostInSystemTable(
                        self.backend.as_ref(),
                        table,
                        "User",
                        "Host",
                        old,
                        new,
                    )?;
                }
            }
            Ok(())
        })();
        self.finish_transaction(result)?;
        let users = pairs
            .iter()
            .flat_map(|(old, new)| [old.clone(), new.clone()])
            .collect::<Vec<_>>();
        self.backend
            .execute(Operation::NotifyPrivileges(userIdentityToUserList(&users)))
    }
    /// 删除指定 query watch。
    pub fn executeDropQueryWatch(&mut self, id: u64) -> Result<(), SimpleError> {
        self.backend.execute(Operation::DropQueryWatch(id))
    }
    /// DROP USER / DROP ROLE。
    pub fn executeDropUser(
        &mut self,
        users: &[UserIdentity],
        if_exists: bool,
        is_role: bool,
    ) -> Result<(), SimpleError> {
        self.require_privilege(if is_role { "DROP ROLE" } else { "DROP USER" })?;
        self.backend
            .execute(Operation::Sql("BEGIN PESSIMISTIC".into()))?;
        let result = (|| {
            for user in users {
                if self.backend.query_user(user, true)?.is_none() {
                    if if_exists {
                        self.backend.warn(SimpleError::UserNotFound(
                            user.username.clone(),
                            user.hostname.clone(),
                        ));
                        continue;
                    }
                    return Err(SimpleError::UserNotFound(
                        user.username.clone(),
                        user.hostname.clone(),
                    ));
                }
                self.backend.execute(Operation::Sql(format!(
                    "DELETE ACCOUNT '{}@{}'",
                    user.username, user.hostname
                )))?;
            }
            Ok(())
        })();
        self.finish_transaction(result)?;
        self.backend
            .execute(Operation::NotifyPrivileges(userIdentityToUserList(users)))
    }
    /// SET PASSWORD：校验复用策略后更新认证串。
    pub fn executeSetPwd(
        &mut self,
        user: Option<&UserIdentity>,
        password: &str,
    ) -> Result<(), SimpleError> {
        let target = user
            .cloned()
            .or_else(|| self.backend.current_user())
            .ok_or_else(|| SimpleError::AccessDenied("missing current user".into()))?;
        let self_service = self.backend.current_user().as_ref() == Some(&target);
        if !self_service {
            self.require_privilege("UPDATE mysql.user")?;
        }
        self.backend.validate_password(&target, password)?;
        let record = self.backend.query_user(&target, true)?.ok_or_else(|| {
            SimpleError::UserNotFound(target.username.clone(), target.hostname.clone())
        })?;
        let plugin = effectiveAuthPlugin(&record.auth_plugin, &self.backend.default_auth_plugin()?);
        let hashed = self.backend.hash_password(&plugin, password)?;
        self.checkPasswordReusePolicy(
            &userInfo {
                host: target.hostname.clone(),
                user: target.username.clone(),
                pwd: hashed.clone(),
                authString: record.auth_string,
                pLI: None,
            },
            &plugin,
        )?;
        self.backend.execute(Operation::Sql(format!(
            "UPDATE mysql.user SET authentication_string='{hashed}' WHERE User='{}' AND Host='{}'",
            target.username,
            target.hostname.to_lowercase()
        )))?;
        self.backend
            .execute(Operation::NotifyPrivileges(userIdentityToUserList(&[
                target,
            ])))
    }
    /// KILL [QUERY] connection_id（本地或广播远程）。
    pub fn executeKillStmt(&mut self, connection_id: u64, query: bool) -> Result<(), SimpleError> {
        killBySQLStmt(
            self.backend.as_ref(),
            connection_id,
            query,
            self.IsFromRemote,
        )
    }
    /// REFRESH STATS（可限当前实例）。
    pub fn executeRefreshStats(
        &mut self,
        sql: &str,
        current_instance: bool,
    ) -> Result<(), SimpleError> {
        if current_instance {
            self.executeRefreshStatsOnCurrentInstance(sql)
        } else {
            broadcast(self.backend.as_ref(), &restoreRefreshStatsSQL(sql)?)
        }
    }
    /// 仅在当前实例执行 REFRESH STATS。
    pub fn executeRefreshStatsOnCurrentInstance(&mut self, sql: &str) -> Result<(), SimpleError> {
        self.backend
            .execute(Operation::RefreshStats(restoreRefreshStatsSQL(sql)?))
    }
    /// FLUSH STATS_DELTA 并可选广播。
    pub fn executeFlushStatsDelta(&mut self, sql: &str) -> Result<(), SimpleError> {
        broadcast(self.backend.as_ref(), &restoreFlushStatsDeltaSQL(sql)?)
    }
    /// 仅当前实例刷统计增量。
    pub fn executeFlushStatsDeltaOnCurrentInstance(
        &mut self,
        sql: &str,
    ) -> Result<(), SimpleError> {
        self.backend
            .execute(Operation::FlushStatsDelta(restoreFlushStatsDeltaSQL(sql)?))
    }
    /// FLUSH 通用入口（含 stats_delta 分支）。
    pub fn executeFlush(&mut self, sql: &str, stats_delta: bool) -> Result<(), SimpleError> {
        if stats_delta {
            self.executeFlushStatsDelta(sql)
        } else {
            self.backend.broadcast(sql)
        }
    }
    /// ALTER INSTANCE：如 reload TLS。
    pub fn executeAlterInstance(
        &mut self,
        reload_tls: bool,
        no_rollback_on_error: bool,
    ) -> Result<(), SimpleError> {
        if reload_tls {
            self.backend.execute(Operation::ReloadTls {
                no_rollback_on_error,
            })
        } else {
            Err(SimpleError::Unsupported("alter instance operation".into()))
        }
    }
    /// DROP STATS 指定表。
    pub fn executeDropStats(&mut self, table_ids: &[i64]) -> Result<(), SimpleError> {
        self.backend
            .execute(Operation::DropStats(table_ids.to_vec()))
    }
    /// 某些语句是否应自动开启新事务。
    pub fn autoNewTxn(&mut self) -> bool {
        matches!(
            self.Statement,
            Statement::CreateUser { .. }
                | Statement::AlterUser { .. }
                | Statement::DropUser { .. }
                | Statement::RenameUser { .. }
                | Statement::RevokeRole { .. }
                | Statement::GrantRole { .. }
                | Statement::Flush { .. }
        )
    }
    /// SHUTDOWN：延迟后关闭实例。
    pub fn executeShutdown(&mut self) -> Result<(), SimpleError> {
        asyncDelayShutdown(Arc::clone(&self.backend), Duration::from_secs(1))
    }
    /// 从 JSON 恢复会话状态。
    pub fn executeSetSessionStates(&mut self, json: &str) -> Result<(), SimpleError> {
        if !(json.trim_start().starts_with('{') && json.trim_end().ends_with('}')) {
            return Err(SimpleError::InvalidOption(
                "invalid session states JSON".into(),
            ));
        }
        self.backend
            .execute(Operation::DecodeSessionStates(json.into()))
    }
    /// 分派 ADMIN 子命令。
    pub fn executeAdmin(
        &mut self,
        kind: AdminKind,
        scope: StatementScope,
        value: &str,
    ) -> Result<(), SimpleError> {
        match kind {
            AdminKind::ReloadStatistics => self.executeAdminReloadStatistics(),
            AdminKind::FlushPlanCache => self.executeAdminFlushPlanCache(scope),
            AdminKind::SetBdrRole => self.executeAdminSetBDRRole(value),
            AdminKind::UnsetBdrRole => self.executeAdminUnsetBDRRole(),
        }
    }
    /// ADMIN RELOAD STATISTICS。
    pub fn executeAdminReloadStatistics(&mut self) -> Result<(), SimpleError> {
        Err(SimpleError::Unsupported(
            "extended statistics feature has been removed".into(),
        ))
    }
    /// ADMIN FLUSH [SESSION|INSTANCE|GLOBAL] PLAN_CACHE。
    pub fn executeAdminFlushPlanCache(&mut self, scope: StatementScope) -> Result<(), SimpleError> {
        if scope == StatementScope::Global {
            Err(SimpleError::Unsupported("global plan-cache flush".into()))
        } else {
            self.backend.execute(Operation::FlushPlanCache(scope))
        }
    }
    /// ADMIN SET BDR ROLE。
    pub fn executeAdminSetBDRRole(&mut self, role: &str) -> Result<(), SimpleError> {
        self.backend.execute(Operation::SetBdrRole(role.into()))
    }
    /// ADMIN UNSET BDR ROLE。
    pub fn executeAdminUnsetBDRRole(&mut self) -> Result<(), SimpleError> {
        self.backend.execute(Operation::UnsetBdrRole)
    }
    /// SET RESOURCE GROUP。
    pub fn executeSetResourceGroupName(&mut self, name: &str) -> Result<(), SimpleError> {
        let resolved = if name.is_empty() { "default" } else { name };
        if !self.backend.resource_group_exists(resolved) {
            return Err(SimpleError::InvalidOption(format!(
                "resource group {resolved} does not exist"
            )));
        }
        self.backend
            .execute(Operation::SetResourceGroup(resolved.into()))
    }
    /// ALTER RANGE ... PLACEMENT POLICY。
    pub fn executeAlterRange(&mut self, range: &str, policy: &str) -> Result<(), SimpleError> {
        if !matches!(range, "global" | "meta") {
            return Err(SimpleError::Unsupported("range name".into()));
        }
        if policy != "default" && !self.backend.placement_policy_exists(policy) {
            return Err(SimpleError::InvalidOption(format!(
                "placement policy {policy} does not exist"
            )));
        }
        self.backend.execute(Operation::AlterRange {
            range: range.into(),
            policy: policy.into(),
        })
    }

    fn set_default_roles(
        &self,
        roles: &[UserIdentity],
        users: &[UserIdentity],
    ) -> Result<(), SimpleError> {
        for user in users {
            self.require_user(user)?;
        }
        self.backend.execute(Operation::Sql(format!(
            "REPLACE mysql.default_roles roles={} users={}",
            userIdentityToUserList(roles).join(","),
            userIdentityToUserList(users).join(",")
        )))?;
        self.backend
            .execute(Operation::NotifyPrivileges(userIdentityToUserList(users)))
    }
    /// 缺少特权则返回 AccessDenied。
    fn require_privilege(&self, privilege: &str) -> Result<(), SimpleError> {
        if self.backend.skip_grant_table() || self.backend.verify_privilege(privilege) {
            Ok(())
        } else {
            Err(SimpleError::AccessDenied(privilege.into()))
        }
    }
    /// 用户必须存在，否则 UserNotFound。
    fn require_user(&self, user: &UserIdentity) -> Result<UserRecord, SimpleError> {
        self.backend
            .query_user(user, true)?
            .ok_or_else(|| SimpleError::UserNotFound(user.username.clone(), user.hostname.clone()))
    }
    /// 根据结果提交或回滚自动事务。
    fn finish_transaction(&self, result: Result<(), SimpleError>) -> Result<(), SimpleError> {
        match result {
            Ok(()) => self.backend.execute(Operation::Commit),
            Err(error) => {
                let _ = self.backend.execute(Operation::Rollback);
                Err(error)
            }
        }
    }

    pub fn getUserPasswordLimit(
        &mut self,
        user: &UserIdentity,
        options: &passwordOrLockOptionsInfo,
    ) -> Result<passwordReuseInfo, SimpleError> {
        getUserPasswordLimit(self.backend.as_ref(), user, options)
    }
    pub fn deleteHistoricalData(
        &mut self,
        user: &userInfo,
        max_rows: i64,
        reuse: &passwordReuseInfo,
    ) -> Result<(), SimpleError> {
        deleteHistoricalData(self.backend.as_ref(), user, max_rows, reuse)
    }
    pub fn addHistoricalData(
        &mut self,
        user: &userInfo,
        reuse: &passwordReuseInfo,
    ) -> Result<(), SimpleError> {
        addHistoricalData(self.backend.as_ref(), user, reuse)
    }
    pub fn getUserPasswordNum(&mut self, user: &userInfo) -> Result<i64, SimpleError> {
        getUserPasswordNum(self.backend.as_ref(), user)
    }
    pub fn fullRecordCheck(&mut self, user: &userInfo, plugin: &str) -> Result<bool, SimpleError> {
        fullRecordCheck(self.backend.as_ref(), user, plugin)
    }
    pub fn checkPasswordHistoryRule(
        &mut self,
        user: &userInfo,
        reuse: &passwordReuseInfo,
        plugin: &str,
    ) -> Result<bool, SimpleError> {
        checkPasswordHistoryRule(self.backend.as_ref(), user, reuse, plugin)
    }
    pub fn checkPasswordTimeRule(
        &mut self,
        user: &userInfo,
        reuse: &passwordReuseInfo,
        plugin: &str,
    ) -> Result<bool, SimpleError> {
        checkPasswordTimeRule(self.backend.as_ref(), user, reuse, plugin)
    }
    pub fn passwordVerification(
        &mut self,
        user: &userInfo,
        reuse: &passwordReuseInfo,
        plugin: &str,
    ) -> Result<(bool, i64), SimpleError> {
        passwordVerification(self.backend.as_ref(), user, reuse, plugin)
    }
    pub fn checkPasswordReusePolicy(
        &mut self,
        user: &userInfo,
        plugin: &str,
    ) -> Result<(), SimpleError> {
        checkPasswordReusePolicy(self.backend.as_ref(), user, plugin)
    }
}

/// 判断标识是否为角色（而非普通用户）。
pub fn isRole<B: SimpleBackend>(backend: &B, user: &UserIdentity) -> Result<bool, SimpleError> {
    Ok(backend
        .query_user(user, false)?
        .is_some_and(|row| row.account_locked && row.password_expired))
}

/// 读取用户或全局的密码历史条数上限。
pub fn getUserPasswordLimit<B: SimpleBackend>(
    backend: &B,
    user: &UserIdentity,
    options: &passwordOrLockOptionsInfo,
) -> Result<passwordReuseInfo, SimpleError> {
    let record = backend
        .query_user(user, false)?
        .ok_or_else(|| SimpleError::UserNotFound(user.username.clone(), user.hostname.clone()))?;
    let stored_history = record
        .attributes
        .get("password_reuse_history")
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| backend.global_password_history());
    let stored_interval = record
        .attributes
        .get("password_reuse_time")
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| backend.global_password_reuse_interval());
    Ok(passwordReuseInfo {
        passwordHistory: if options.passwordHistoryChange {
            if options.passwordHistory == NOT_SPECIFIED {
                backend.global_password_history()
            } else {
                options.passwordHistory
            }
        } else {
            stored_history
        },
        passwordReuseInterval: if options.passwordReuseIntervalChange {
            if options.passwordReuseInterval == NOT_SPECIFIED {
                backend.global_password_reuse_interval()
            } else {
                options.passwordReuseInterval
            }
        } else {
            stored_interval
        },
    })
}

/// 按条数/时间裁剪密码历史。
pub fn deleteHistoricalData<B: SimpleBackend>(
    backend: &B,
    user: &userInfo,
    max_rows: i64,
    reuse: &passwordReuseInfo,
) -> Result<(), SimpleError> {
    if reuse.passwordReuseInterval > i32::MAX as i64 || max_rows == 0 {
        return Ok(());
    }
    let time_clause = if reuse.passwordReuseInterval == 0 {
        String::new()
    } else {
        format!(" AND Password_timestamp < {}", getValidTime(backend, reuse))
    };
    backend.execute(Operation::Sql(format!("DELETE mysql.password_history WHERE User='{}' AND Host='{}'{time_clause} ORDER BY Password_timestamp LIMIT {max_rows}", user.user, user.host.to_lowercase())))
}

/// 追加一条密码历史记录。
pub fn addHistoricalData<B: SimpleBackend>(
    backend: &B,
    user: &userInfo,
    reuse: &passwordReuseInfo,
) -> Result<(), SimpleError> {
    if reuse.passwordHistory <= 0 && reuse.passwordReuseInterval <= 0 {
        return Ok(());
    }
    backend.execute(Operation::Sql(format!(
        "INSERT mysql.password_history Host='{}' User='{}' Password='{}'",
        user.host.to_lowercase(),
        user.user,
        user.pwd
    )))
}

/// 用插件校验明文是否匹配历史哈希。
pub fn checkPasswordsMatch<B: SimpleBackend>(
    backend: &B,
    rows: &[(String, i64)],
    old_password: &str,
    plugin: &str,
) -> Result<bool, SimpleError> {
    for (password, _) in rows {
        if backend.check_hashing_password(password, old_password, plugin)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// 当前已保存的历史口令条数。
pub fn getUserPasswordNum<B: SimpleBackend>(
    backend: &B,
    user: &userInfo,
) -> Result<i64, SimpleError> {
    let identity = UserIdentity {
        username: user.user.clone(),
        hostname: user.host.clone(),
        current_user: false,
    };
    Ok(backend
        .query_user(&identity, false)?
        .map_or(0, |record| record.password_history.len() as i64))
}

/// 历史已满时的完整记录检查。
pub fn fullRecordCheck<B: SimpleBackend>(
    backend: &B,
    user: &userInfo,
    plugin: &str,
) -> Result<bool, SimpleError> {
    let identity = UserIdentity {
        username: user.user.clone(),
        hostname: user.host.clone(),
        current_user: false,
    };
    let rows = backend
        .query_user(&identity, false)?
        .map_or_else(Vec::new, |record| record.password_history);
    if plugin.is_empty() || plugin == "mysql_native_password" {
        Ok(rows.iter().all(|(password, _)| password != &user.pwd))
    } else {
        checkPasswordsMatch(backend, &rows, &user.authString, plugin)
    }
}

/// 密码历史条数规则：禁止复用最近 N 次口令。
pub fn checkPasswordHistoryRule<B: SimpleBackend>(
    backend: &B,
    user: &userInfo,
    reuse: &passwordReuseInfo,
    plugin: &str,
) -> Result<bool, SimpleError> {
    if reuse.passwordHistory <= 0 {
        return Ok(true);
    }
    let identity = UserIdentity {
        username: user.user.clone(),
        hostname: user.host.clone(),
        current_user: false,
    };
    let mut rows = backend
        .query_user(&identity, false)?
        .map_or_else(Vec::new, |record| record.password_history);
    rows.sort_by_key(|(_, timestamp)| -*timestamp);
    rows.truncate(reuse.passwordHistory as usize);
    if plugin.is_empty() || plugin == "mysql_native_password" {
        Ok(rows.iter().all(|(password, _)| password != &user.pwd))
    } else {
        checkPasswordsMatch(backend, &rows, &user.authString, plugin)
    }
}

/// 密码复用时间规则：禁止在间隔内复用。
pub fn checkPasswordTimeRule<B: SimpleBackend>(
    backend: &B,
    user: &userInfo,
    reuse: &passwordReuseInfo,
    plugin: &str,
) -> Result<bool, SimpleError> {
    if reuse.passwordReuseInterval <= 0 {
        return Ok(true);
    }
    let valid_after = getValidTime(backend, reuse);
    let identity = UserIdentity {
        username: user.user.clone(),
        hostname: user.host.clone(),
        current_user: false,
    };
    let rows = backend
        .query_user(&identity, false)?
        .map_or_else(Vec::new, |record| {
            record
                .password_history
                .into_iter()
                .filter(|(_, timestamp)| *timestamp >= valid_after)
                .collect()
        });
    if plugin.is_empty() || plugin == "mysql_native_password" {
        Ok(rows.iter().all(|(password, _)| password != &user.pwd))
    } else {
        checkPasswordsMatch(backend, &rows, &user.authString, plugin)
    }
}

/// 综合历史与时间规则做口令校验。
pub fn passwordVerification<B: SimpleBackend>(
    backend: &B,
    user: &userInfo,
    reuse: &passwordReuseInfo,
    plugin: &str,
) -> Result<(bool, i64), SimpleError> {
    let count = getUserPasswordNum(backend, user)?;
    let delete_count = count
        .saturating_sub(reuse.passwordHistory)
        .saturating_add(1)
        .max(0);
    if reuse.passwordHistory <= 0 && reuse.passwordReuseInterval <= 0 {
        return Ok((true, delete_count));
    }
    if count <= reuse.passwordHistory || reuse.passwordReuseInterval > i32::MAX as i64 {
        return Ok((fullRecordCheck(backend, user, plugin)?, delete_count));
    }
    if reuse.passwordHistory > 0 && !checkPasswordHistoryRule(backend, user, reuse, plugin)? {
        return Ok((false, 0));
    }
    if reuse.passwordReuseInterval > 0 && !checkPasswordTimeRule(backend, user, reuse, plugin)? {
        return Ok((false, 0));
    }
    Ok((true, delete_count))
}

/// 对外入口：按策略检查新口令是否可接受。
pub fn checkPasswordReusePolicy<B: SimpleBackend>(
    backend: &B,
    user: &userInfo,
    plugin: &str,
) -> Result<(), SimpleError> {
    let identity = UserIdentity {
        username: user.user.clone(),
        hostname: user.host.clone(),
        current_user: false,
    };
    let options = user.pLI.clone().unwrap_or_default();
    let reuse = getUserPasswordLimit(backend, &identity, &options)?;
    let (allowed, delete_count) = passwordVerification(backend, user, &reuse, plugin)?;
    if !allowed {
        return Err(SimpleError::PasswordReuse(format!(
            "password reuse denied for '{}@{}'",
            user.user, user.host
        )));
    }
    deleteHistoricalData(backend, user, delete_count, &reuse)?;
    addHistoricalData(backend, user, &reuse)
}

/// 用户是否存在（精确匹配）。
pub fn userExists<B: SimpleBackend>(
    backend: &B,
    name: &str,
    host: &str,
) -> Result<bool, SimpleError> {
    backend
        .query_user(
            &UserIdentity {
                username: name.into(),
                hostname: host.into(),
                current_user: false,
            },
            false,
        )
        .map(|row| row.is_some())
}

/// 用户是否存在（含用户名变体重试）。
pub fn userExistsWithRetryVariants<B: SimpleBackend>(
    backend: &B,
    name: &mut String,
    host: &str,
) -> Result<bool, SimpleError> {
    for variant in backend.username_variants(name) {
        if userExists(backend, &variant, host)? {
            *name = variant;
            return Ok(true);
        }
    }
    if skipExactUsernameLookup(backend, name) {
        Ok(false)
    } else {
        userExists(backend, name, host)
    }
}

/// 内部：变体重试查找用户。
pub fn userExistsInternalWithRetryVariants<B: SimpleBackend>(
    backend: &B,
    name: &mut String,
    host: &str,
) -> Result<(bool, String, String), SimpleError> {
    for variant in backend.username_variants(name) {
        let result = userExistsInternal(backend, &variant, host)?;
        if result.0 {
            *name = variant;
            return Ok(result);
        }
    }
    if skipExactUsernameLookup(backend, name) {
        Ok((false, String::new(), String::new()))
    } else {
        userExistsInternal(backend, name, host)
    }
}

/// 内部：精确查找用户记录。
pub fn userExistsInternal<B: SimpleBackend>(
    backend: &B,
    name: &str,
    host: &str,
) -> Result<(bool, String, String), SimpleError> {
    let identity = UserIdentity {
        username: name.into(),
        hostname: host.into(),
        current_user: false,
    };
    Ok(match backend.query_user(&identity, true)? {
        Some(row) => (true, row.auth_plugin, row.auth_string),
        None => (false, String::new(), String::new()),
    })
}

/// 在系统表中重命名 user@host。
pub fn renameUserHostInSystemTable<B: SimpleBackend>(
    backend: &B,
    table: &str,
    username_column: &str,
    host_column: &str,
    old: &UserIdentity,
    new: &UserIdentity,
) -> Result<(), SimpleError> {
    backend.execute(Operation::Sql(format!("UPDATE mysql.{table} SET {username_column}='{}', {host_column}='{}' WHERE {username_column}='{}' AND {host_column}='{}'", new.username, new.hostname.to_lowercase(), old.username, old.hostname.to_lowercase())))
}

/// 通过 SQL 语句路径执行 KILL。
pub fn killBySQLStmt<B: SimpleBackend>(
    backend: &B,
    connection_id: u64,
    query: bool,
    from_remote: bool,
) -> Result<(), SimpleError> {
    backend.execute(Operation::Kill {
        connection_id,
        query,
        remote: from_remote,
    })
}

/// 向其他 TiDB 实例广播远程 KILL。
pub fn killRemoteConn<B: SimpleBackend>(
    backend: &B,
    connection_id: u64,
    query: bool,
) -> Result<(), SimpleError> {
    backend.execute(Operation::Kill {
        connection_id,
        query,
        remote: true,
    })
}

/// 将 SQL 广播到集群其他节点。
pub fn broadcast<B: SimpleBackend>(backend: &B, sql: &str) -> Result<(), SimpleError> {
    backend.broadcast(sql)
}
