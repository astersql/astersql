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
// Copyright 2026 AsterSQL.

// 权限管理核心：连接鉴权、静态/动态权限校验、SSL/证书校验与 Auth Token claims。
//
// 对应 Go 的 `privileges.go`。`UserPrivileges` 持有当前会话用户身份与权限缓存
// 句柄（`Handle`），对外提供与 MySQL 兼容的鉴权/授权检查入口。
// SEM（Security Enhanced Mode，安全增强模式）开启时，对系统库/敏感表施加额外只读约束。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use sha1::{Digest, Sha1};

use crate::*;

/// 跳过权限表检查（对齐 `--skip-grant-tables`）：为 true 时几乎全部校验直接放行。
static SKIP_WITH_GRANT: AtomicBool = AtomicBool::new(false);
/// 沙箱模式：密码过期时不硬失败，而是标记需改密后继续受限登录。
static SANDBOX_MODE: AtomicBool = AtomicBool::new(false);
/// Auth Token 默认寿命（15 分钟），用于校验 JWT `iat` 是否超出生命期。
pub const defaultTokenLife: Duration = Duration::from_secs(15 * 60);

/// 设置是否跳过权限表检查。
pub fn set_skip_with_grant(skip: bool) {
    SKIP_WITH_GRANT.store(skip, Ordering::Release);
}
/// 查询是否处于 skip-grant-tables 模式。
pub fn SkipWithGrant() -> bool {
    SKIP_WITH_GRANT.load(Ordering::Acquire)
}
/// 启用/关闭密码过期沙箱模式。
pub fn set_sandbox_mode(enabled: bool) {
    SANDBOX_MODE.store(enabled, Ordering::Release);
}

/// 内置动态权限名列表（备份恢复、SEM 受限权限、资源组管理等）。
fn initial_dynamic_privileges() -> Vec<String> {
    [
        "BACKUP_ADMIN",
        "RESTORE_ADMIN",
        "SYSTEM_USER",
        "SYSTEM_VARIABLES_ADMIN",
        "ROLE_ADMIN",
        "CONNECTION_ADMIN",
        "PLACEMENT_ADMIN",
        "DASHBOARD_CLIENT",
        "RESTRICTED_TABLES_ADMIN",
        "RESTRICTED_STATUS_ADMIN",
        "RESTRICTED_VARIABLES_ADMIN",
        "RESTRICTED_USER_ADMIN",
        "RESTRICTED_CONNECTION_ADMIN",
        "RESTRICTED_REPLICA_WRITER_ADMIN",
        "RESTRICTED_PRIV_ADMIN",
        "RESTRICTED_SQL_ADMIN",
        "RESOURCE_GROUP_ADMIN",
        "RESOURCE_GROUP_USER",
        "TRAFFIC_CAPTURE_ADMIN",
        "TRAFFIC_REPLAY_ADMIN",
        "APPLICATION_PASSWORD_ADMIN",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

/// 进程级动态权限注册表（可运行时增删）。
fn dynamic_privileges() -> &'static Mutex<Vec<String>> {
    static PRIVILEGES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    PRIVILEGES.get_or_init(|| Mutex::new(initial_dynamic_privileges()))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 用户身份：用户名 + 主机名（MySQL 的 `'user'@'host'`）。
pub struct UserIdentity {
    pub Username: String,
    pub Hostname: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 鉴权相关的会话变量子集（TLS 状态、默认密码寿命）。
pub struct SessionVars {
    pub tls_state: Option<TlsConnectionState>,
    pub default_password_lifetime: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 连接鉴权成功后的结果信息。
pub struct VerificationInfo {
    pub authenticated_user: String,
    pub authenticated_host: String,
    pub auth_plugin: String,
    pub password_expired: bool,
    pub resource_group_name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 客户端对等证书摘要，用于 REQUIRE X509 / SUBJECT / SAN 等 SSL 约束。
pub struct Certificate {
    pub issuer: String,
    pub subject: String,
    pub dns_names: Vec<String>,
    pub ip_addresses: Vec<String>,
    pub uris: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// TLS 连接状态：套件、证书链是否已验证、对等证书。
pub struct TlsConnectionState {
    pub cipher_suite: String,
    pub verified_chains: bool,
    pub peer_certificate: Option<Certificate>,
}

#[derive(Clone, Debug)]
/// 面向会话的权限检查器：绑定当前用户/主机与权限缓存句柄。
pub struct UserPrivileges {
    pub user: String,
    pub host: String,
    pub Handle: Handle,
}

/// 用给定权限缓存句柄构造空用户的 `UserPrivileges`。
pub fn NewUserPrivileges(handle: Handle) -> UserPrivileges {
    UserPrivileges {
        user: String::new(),
        host: String::new(),
        Handle: handle,
    }
}

impl UserPrivileges {
    /// 按用户记录的认证插件校验客户端响应（主/副密码串均可）。
    pub fn authenticateWithPlugin(
        &self,
        record: &UserRecord,
        authentication: &[u8],
        salt: &[u8],
    ) -> Result<(), PrivilegeError> {
        if checkPasswordForPlugin(
            &record.AuthPlugin,
            &record.AuthenticationString,
            salt,
            authentication,
        )? || checkPasswordForPlugin(
            &record.AuthPlugin,
            &record.AdditionalAuthString,
            salt,
            authentication,
        )? {
            Ok(())
        } else {
            Err(PrivilegeError::Authentication("password mismatch".into()))
        }
    }
    /// 检查存储的认证哈希格式是否合法（按插件类型）。
    pub fn isValidHash(&self, record: &UserRecord) -> bool {
        if record.AuthenticationString.is_empty() {
            return true;
        }
        match record.AuthPlugin.as_str() {
            "mysql_native_password" => record.AuthenticationString.len() == 41,
            "caching_sha2_password" | "tidb_sm3_password" => {
                record.AuthenticationString.len() == 70
            }
            "auth_socket"
            | "tidb_auth_token"
            | "authentication_ldap_simple"
            | "authentication_ldap_sasl" => true,
            _ => false,
        }
    }
    /// 以指定用户身份请求动态权限校验。
    pub fn RequestDynamicVerificationWithUser(
        &self,
        privilege: &str,
        grantable: bool,
        user: Option<&UserIdentity>,
    ) -> bool {
        if SkipWithGrant() {
            return true;
        }
        let Some(user) = user else { return false };
        let _ = self.Handle.ensureActiveUser(&user.Username);
        let cache = self.Handle.Get();
        let roles = cache.getDefaultRoles(&user.Username, &user.Hostname);
        cache.RequestDynamicVerification(
            &roles,
            &user.Username,
            &user.Hostname,
            privilege,
            grantable,
        )
    }
    /// 是否显式授予了指定动态权限（grantable 控制 GRANT OPTION 语义）。
    pub fn HasExplicitlyGrantedDynamicPrivilege(
        &self,
        active_roles: &[RoleIdentity],
        privilege: &str,
        grantable: bool,
    ) -> bool {
        if SkipWithGrant() {
            return true;
        }
        if self.user.is_empty() && self.host.is_empty() {
            return true;
        }
        self.Handle.Get().HasExplicitlyGrantedDynamicPrivilege(
            active_roles,
            &self.user,
            &self.host,
            privilege,
            grantable,
        )
    }
    /// 以当前会话用户请求动态权限校验。
    pub fn RequestDynamicVerification(
        &self,
        active_roles: &[RoleIdentity],
        privilege: &str,
        grantable: bool,
    ) -> bool {
        if SkipWithGrant() {
            return true;
        }
        if self.user.is_empty() && self.host.is_empty() {
            return true;
        }
        self.Handle.Get().RequestDynamicVerification(
            active_roles,
            &self.user,
            &self.host,
            privilege,
            grantable,
        )
    }
    /// 校验对 db/table/column 的静态权限；含 SEM、系统 schema 等特殊规则。
    pub fn RequestVerification(
        &self,
        active_roles: &[RoleIdentity],
        db: &str,
        table: &str,
        column: &str,
        privilege: PrivilegeType,
    ) -> bool {
        if SkipWithGrant() {
            return true;
        }
        if self.user.is_empty() && self.host.is_empty() {
            return true;
        }
        let db_lower = db.to_ascii_lowercase();
        let table_lower = table.to_ascii_lowercase();
        // If SEM is enabled and the user does not have the RESTRICTED_TABLES_ADMIN
        // privilege there are some hard rules which overwrite system tables and
        // schemas as read-only at most.
        // SEM 开启且无 RESTRICTED_TABLES_ADMIN 时，系统表/库最多只读。
        let sem_enabled = sem::IsEnabled();
        if sem_enabled
            && !self.RequestDynamicVerification(active_roles, "RESTRICTED_TABLES_ADMIN", false)
        {
            if sem::IsInvisibleTable(&db_lower, &table_lower) {
                return false;
            }
            if is_mem_or_sys_db(&db_lower) && is_sem_write_privilege(privilege) {
                return false;
            }
        }
        // 内存 schema 一律禁止写权限。
        if is_memory_schema(&db_lower) && is_write_privilege(privilege) {
            return false;
        }
        // information_schema：仅允许非写权限。
        if db_lower == "information_schema" {
            return !is_write_privilege(privilege);
        }
        // metrics_schema 的 SELECT 需要同时具备 PROCESS 权限。
        if db_lower == "metrics_schema" && privilege == SelectPriv {
            return self.Handle.Get().RequestVerification(
                active_roles,
                &self.user,
                &self.host,
                db,
                table,
                column,
                SelectPriv | ProcessPriv,
            );
        }
        if db_lower == "performance_schema"
            && table_lower.starts_with("tidb_")
            && is_write_privilege(privilege)
        {
            return false;
        }
        self.Handle.Get().RequestVerification(
            active_roles,
            &self.user,
            &self.host,
            db,
            table,
            column,
            privilege,
        )
    }
    /// 以指定用户身份做静态权限校验。
    pub fn RequestVerificationWithUser(
        &self,
        _active_roles: &[RoleIdentity],
        db: &str,
        table: &str,
        column: &str,
        privilege: PrivilegeType,
        user: Option<&UserIdentity>,
    ) -> bool {
        if SkipWithGrant() {
            return true;
        }
        let Some(user) = user else { return false };
        if user.Username.is_empty() && user.Hostname.is_empty() {
            return true;
        }
        if db.eq_ignore_ascii_case("information_schema") {
            return true;
        }
        let _ = self.Handle.ensureActiveUser(&user.Username);
        let cache = self.Handle.Get();
        let roles = cache.getDefaultRoles(&user.Username, &user.Hostname);
        cache.RequestVerification(
            &roles,
            &user.Username,
            &user.Hostname,
            db,
            table,
            column,
            privilege,
        )
    }
    /// 返回用户最大连接数（`MaxUserConnections`）。
    pub fn GetUserResources(&self, user: &str, host: &str) -> Result<i64, PrivilegeError> {
        if SkipWithGrant() {
            return Ok(0);
        }
        let _ = self.Handle.ensureActiveUser(user);
        let cache = self.Handle.Get();
        let record = cache.connectionVerification(user, host).ok_or_else(|| {
            PrivilegeError::AccessDenied {
                user: user.into(),
                host: host.into(),
            }
        })?;
        if self.isValidHash(record) {
            Ok(record.MaxUserConnections)
        } else {
            Err(PrivilegeError::Authentication(
                "Failed to get max user connections".into(),
            ))
        }
    }
    /// 查询连接应使用的认证插件名。
    pub fn GetAuthPluginForConnection(
        &self,
        user: &str,
        host: &str,
    ) -> Result<String, PrivilegeError> {
        if SkipWithGrant() {
            return Ok("mysql_native_password".into());
        }
        let _ = self.Handle.ensureActiveUser(user);
        let cache = self.Handle.Get();
        let record = cache.connectionVerification(user, host).ok_or_else(|| {
            PrivilegeError::AccessDenied {
                user: user.into(),
                host: host.into(),
            }
        })?;
        if matches!(
            record.AuthPlugin.as_str(),
            "tidb_auth_token" | "authentication_ldap_simple" | "authentication_ldap_sasl"
        ) {
            return Ok(record.AuthPlugin.clone());
        }
        if record.AuthenticationString.is_empty() && record.AuthPlugin != "auth_socket" {
            return Ok(String::new());
        }
        if self.isValidHash(record) {
            Ok(record.AuthPlugin.clone())
        } else {
            Err(PrivilegeError::Authentication(
                "Failed to get plugin for user".into(),
            ))
        }
    }
    /// 按用户名/主机匹配权限缓存中的规范身份。
    pub fn MatchIdentity(
        &mut self,
        user: &str,
        host: &str,
        skip_name_resolve: bool,
    ) -> (String, String, bool) {
        if SkipWithGrant() {
            return (user.into(), host.into(), true);
        }
        let _ = self.Handle.ensureActiveUser(user);
        if let Some(record) = self
            .Handle
            .Get()
            .matchIdentity(user, host, skip_name_resolve)
        {
            (record.base.User.clone(), record.base.Host.clone(), true)
        } else {
            (String::new(), String::new(), false)
        }
    }
    /// 按资源组名匹配用户记录，返回用户名与是否命中。
    pub fn MatchUserResourceGroupName(&self, resource_group: &str) -> (String, bool) {
        self.Handle
            .Get()
            .user
            .iter()
            .find(|r| r.ResourceGroup.eq_ignore_ascii_case(resource_group))
            .map(|r| (r.base.User.clone(), true))
            .unwrap_or_default()
    }
    /// 仅匹配身份并写入 self，不做密码校验。
    pub fn GetAuthWithoutVerification(&mut self, user: &str, host: &str) -> bool {
        if SkipWithGrant() {
            self.user = user.into();
            self.host = host.into();
            return true;
        }
        if let Some(record) = self.Handle.Get().connectionVerification(user, host) {
            self.user = user.into();
            self.host = record.base.Host.clone();
            true
        } else {
            false
        }
    }
    /// 检查密码是否过期；沙箱模式下返回 true，否则报 MustChangePassword。
    pub fn CheckPasswordExpired(
        &self,
        session: &SessionVars,
        record: &UserRecord,
    ) -> Result<bool, PrivilegeError> {
        let expired = record.PasswordExpired || {
            let life = if record.PasswordLifeTime == -1 {
                session.default_password_lifetime
            } else {
                record.PasswordLifeTime
            };
            life > 0 && record.PasswordLastChanged + life * 86400 < now_unix()
        };
        if !expired {
            return Ok(false);
        }
        if SANDBOX_MODE.load(Ordering::Acquire) {
            Ok(true)
        } else {
            Err(PrivilegeError::MustChangePassword)
        }
    }
    /// 检查账户是否因失败登录次数触发自动锁定（Password Locking）。
    pub fn VerifyAccountAutoLockInMemory(
        &self,
        user: &str,
        host: &str,
    ) -> Result<bool, PrivilegeError> {
        let cache = self.Handle.Get();
        let record = cache
            .matchUser(user, host)
            .ok_or_else(|| PrivilegeError::AccessDenied {
                user: user.into(),
                host: host.into(),
            })?;
        if !record.UserAttributesInfo.PasswordLocking.AutoAccountLocked {
            return Ok(false);
        }
        let lock = &record.UserAttributesInfo.PasswordLocking;
        if lock.PasswordLockTimeDays != -1
            && now_unix() - lock.AutoLockedLastChanged > lock.PasswordLockTimeDays * 86400
        {
            return Ok(true);
        }
        Err(GenerateAccountAutoLockErr(
            lock.FailedLoginAttempts,
            user,
            host,
            if lock.PasswordLockTimeDays == -1 {
                "unlimited"
            } else {
                "limited"
            },
            "locked",
        ))
    }
    /// 该用户是否启用了自动账户锁定策略。
    pub fn IsAccountAutoLockEnabled(&mut self, user: &str, host: &str) -> bool {
        if SkipWithGrant() {
            self.user = user.into();
            self.host = host.into();
            return false;
        }
        self.Handle.Get().matchUser(user, host).is_some_and(|r| {
            let l = &r.UserAttributesInfo.PasswordLocking;
            l.FailedLoginAttempts != 0 && l.PasswordLockTimeDays != 0
        })
    }
    /// 完整连接鉴权：账户锁定、SSL、哈希、密码、过期检查。
    pub fn ConnectionVerification(
        &mut self,
        user: &UserIdentity,
        auth_user: &str,
        auth_host: &str,
        authentication: &[u8],
        salt: &[u8],
        session: &SessionVars,
    ) -> Result<VerificationInfo, PrivilegeError> {
        if SkipWithGrant() {
            self.user = auth_user.into();
            self.host = auth_host.into();
            return Ok(VerificationInfo {
                authenticated_user: auth_user.into(),
                authenticated_host: auth_host.into(),
                ..Default::default()
            });
        }
        let cache = self.Handle.Get();
        let record = cache
            .connectionVerification(auth_user, auth_host)
            .ok_or_else(|| PrivilegeError::AccessDenied {
                user: user.Username.clone(),
                host: user.Hostname.clone(),
            })?;
        if record.AccountLocked {
            return Err(PrivilegeError::AccountLocked {
                user: auth_user.into(),
                host: auth_host.into(),
            });
        }
        if let Some(global) = cache.matchGlobalPriv(auth_user, auth_host) {
            if !self.checkSSL(global, session.tls_state.as_ref()) {
                return Err(PrivilegeError::AccessDenied {
                    user: user.Username.clone(),
                    host: user.Hostname.clone(),
                });
            }
        }
        if !self.isValidHash(record) {
            return Err(PrivilegeError::AccessDenied {
                user: user.Username.clone(),
                host: user.Hostname.clone(),
            });
        }
        // Matches Go's `else if len(pwd) > 0 || len(authentication) > 0`
        // gate: a no-password account (empty stored primary) presenting no
        // client-supplied credentials skips the password check entirely
        // (implicit success) rather than falling through to
        // `checkPasswordForPlugin`, which always reports `false` for an
        // empty stored hash.
        if !record.AuthenticationString.is_empty() || !authentication.is_empty() {
            let primary = checkPasswordForPlugin(
                &record.AuthPlugin,
                &record.AuthenticationString,
                salt,
                authentication,
            )?;
            let secondary = checkPasswordForPlugin(
                &record.AuthPlugin,
                &record.AdditionalAuthString,
                salt,
                authentication,
            )?;
            if !primary && !secondary {
                return Err(PrivilegeError::AccessDenied {
                    user: user.Username.clone(),
                    host: user.Hostname.clone(),
                });
            }
        }
        let expired = self.CheckPasswordExpired(session, record)?;
        Ok(VerificationInfo {
            authenticated_user: auth_user.into(),
            authenticated_host: record.base.Host.clone(),
            auth_plugin: record.AuthPlugin.clone(),
            password_expired: expired,
            resource_group_name: record.ResourceGroup.clone(),
        })
    }
    /// 鉴权成功后写入当前会话用户/主机。
    pub fn AuthSuccess(&mut self, auth_user: &str, auth_host: &str) {
        self.user = auth_user.into();
        self.host = auth_host.into();
    }
    /// 按全局权限记录中的 SSL/X509 要求校验 TLS 状态与证书属性。
    pub fn checkSSL(
        &self,
        privilege: &globalPrivRecord,
        state: Option<&TlsConnectionState>,
    ) -> bool {
        if privilege.Broken {
            return false;
        }
        match privilege.Priv.SSLType {
            SSLType::SslTypeNotSpecified | SSLType::SslTypeNone => true,
            SSLType::SslTypeAny => state.is_some(),
            SSLType::SslTypeX509 => state.is_some_and(|s| s.verified_chains),
            // REQUIRE CIPHER/ISSUER/SUBJECT/SAN：逐项比对证书属性。
            SSLType::SslTypeSpecified => {
                let Some(state) = state else { return false };
                let Some(cert) = &state.peer_certificate else {
                    return false;
                };
                if !state.verified_chains {
                    return false;
                }
                if !privilege.Priv.SSLCipher.is_empty()
                    && privilege.Priv.SSLCipher != state.cipher_suite
                {
                    return false;
                }
                if !privilege.Priv.X509Issuer.is_empty() && privilege.Priv.X509Issuer != cert.issuer
                {
                    return false;
                }
                if !privilege.Priv.X509Subject.is_empty()
                    && privilege.Priv.X509Subject != cert.subject
                {
                    return false;
                }
                checkCertSAN(privilege, cert, &privilege.Priv.SANs)
            }
        }
    }
    /// 判断库是否对当前用户（及激活角色）可见。
    pub fn DBIsVisible(&self, active_roles: &[RoleIdentity], db: &str) -> bool {
        if SkipWithGrant() {
            return true;
        }
        // If SEM is enabled, respect hard rules about certain schemas being
        // invisible before checking if the user has permissions granted to them.
        if sem::IsEnabled()
            && !self.RequestDynamicVerification(active_roles, "RESTRICTED_TABLES_ADMIN", false)
        {
            if sem::IsInvisibleSchema(db) {
                return false;
            }
        }
        let cache = self.Handle.Get();
        if cache.DBIsVisible(&self.user, &self.host, db) {
            return true;
        }
        cache
            .FindAllUserEffectiveRoles(&self.user, &self.host, active_roles)
            .iter()
            .any(|r| cache.DBIsVisible(&r.Username, &r.Hostname, db))
    }
    /// 构造 information_schema.USER_PRIVILEGES 风格行数据。
    pub fn UserPrivilegesTable(
        &self,
        active_roles: &[RoleIdentity],
        user: &str,
        host: &str,
    ) -> Vec<Vec<String>> {
        self.Handle
            .Get()
            .UserPrivilegesTable(active_roles, user, host)
    }
    /// 生成 SHOW GRANTS 语句列表。
    pub fn ShowGrants(
        &self,
        user: &UserIdentity,
        roles: &[RoleIdentity],
        ansi_quotes: bool,
    ) -> Vec<String> {
        self.Handle
            .Get()
            .showGrants(&user.Username, &user.Hostname, roles, ansi_quotes)
    }
    /// 校验角色列表是否均可激活；失败返回 (false, 非法角色名)。
    pub fn ActiveRoles(&self, role_list: &[RoleIdentity]) -> (bool, String) {
        if SkipWithGrant() {
            return (true, String::new());
        }
        for role in role_list {
            if !self.FindEdge(
                role,
                &UserIdentity {
                    Username: self.user.clone(),
                    Hostname: self.host.clone(),
                },
            ) {
                return (false, role.Username.clone());
            }
        }
        (true, String::new())
    }
    /// 角色图中是否存在 user -> role 边（角色授予关系）。
    pub fn FindEdge(&self, role: &RoleIdentity, user: &UserIdentity) -> bool {
        if SkipWithGrant() {
            return false;
        }
        self.Handle
            .Get()
            .FindRole(&user.Username, &user.Hostname, role)
    }
    /// 获取用户默认角色列表。
    pub fn GetDefaultRoles(&self, user: &str, host: &str) -> Vec<RoleIdentity> {
        if SkipWithGrant() {
            return Vec::new();
        }
        self.Handle.Get().getDefaultRoles(user, host)
    }
    /// 获取用户全部可授予角色。
    pub fn GetAllRoles(&self, user: &str, host: &str) -> Vec<RoleIdentity> {
        if SkipWithGrant() {
            return Vec::new();
        }
        self.Handle.Get().getAllRoles(user, host)
    }
    /// 名称是否为已注册的动态权限（大小写不敏感）。
    pub fn IsDynamicPrivilege(&self, name: &str) -> bool {
        dynamic_privileges()
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.eq_ignore_ascii_case(name))
    }
}

/// 是否为内存/虚拟系统 schema（information_schema 等）。
fn is_memory_schema(db: &str) -> bool {
    matches!(
        db,
        "information_schema" | "metrics_schema" | "performance_schema"
    )
}
// Mirrors Go's `metadef.IsMemOrSysDB`: memory schemas plus the system-related
// databases (mysql, sys, workload_schema) that SEM also treats as read-only.
/// 内存 schema 或系统相关库（mysql/sys/workload_schema），SEM 亦按只读处理。
fn is_mem_or_sys_db(db: &str) -> bool {
    is_memory_schema(db) || matches!(db, "mysql" | "sys" | "workload_schema")
}
/// 权限位是否包含任一写类权限（CREATE/ALTER/INSERT 等）。
fn is_write_privilege(p: PrivilegeType) -> bool {
    p & (CreatePriv
        | AlterPriv
        | DropPriv
        | IndexPriv
        | CreateViewPriv
        | InsertPriv
        | UpdatePriv
        | DeletePriv
        | ReferencesPriv
        | ExecutePriv
        | ShowViewPriv
        | LockTablesPriv)
        != 0
}

/// SEM 对系统库额外禁止的 DDL/DML 权限集合。
///
/// 该集合刻意比内存 schema 的通用写保护窄，与 Go 中的 switch 保持一致。
fn is_sem_write_privilege(p: PrivilegeType) -> bool {
    matches!(
        p,
        CreatePriv
            | AlterPriv
            | DropPriv
            | IndexPriv
            | CreateViewPriv
            | InsertPriv
            | UpdatePriv
            | DeletePriv
    )
}

/// 构造会话令牌插件完成签名校验后的鉴权结果。
///
/// 会话迁移按 Go 语义跳过密码过期/沙箱检查，但保留账户资源组。
pub fn VerificationInfoWithSessionToken(
    record: &UserRecord,
    token_valid: bool,
) -> Result<VerificationInfo, PrivilegeError> {
    if !token_valid {
        return Err(PrivilegeError::AccessDenied {
            user: record.base.User.clone(),
            host: record.base.Host.clone(),
        });
    }
    Ok(VerificationInfo {
        authenticated_user: record.base.User.clone(),
        authenticated_host: record.base.Host.clone(),
        auth_plugin: "tidb_session_token".into(),
        password_expired: false,
        resource_group_name: record.ResourceGroup.clone(),
    })
}

/// 当前 Unix 秒时间戳。
fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// 校验 Auth Token JWT claims：sub/email/iat/exp/iss 与用户记录一致且未过期。
pub fn checkAuthTokenClaims(
    claims: &HashMap<String, Value>,
    record: &UserRecord,
    token_life: Duration,
) -> Result<(), PrivilegeError> {
    // 辅助：claims 中取字符串字段，缺失则报 lack。
    let string = |name: &str| {
        claims
            .get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| PrivilegeError::Authentication(format!("lack '{name}'")))
    };
    if string("sub")? != record.base.User {
        return Err(PrivilegeError::Authentication("Wrong 'sub'".into()));
    }
    if string("email")? != record.Email() {
        return Err(PrivilegeError::Authentication("Wrong 'email'".into()));
    }
    let now = now_unix();
    let iat = claims
        .get("iat")
        .and_then(Value::as_i64)
        .ok_or_else(|| PrivilegeError::Authentication("lack 'iat'".into()))?;
    if now > iat + token_life.as_secs() as i64 {
        return Err(PrivilegeError::Authentication(
            "the token has been out of its life time".into(),
        ));
    }
    if now < iat {
        return Err(PrivilegeError::Authentication(
            "the token is issued at a future time".into(),
        ));
    }
    let exp = claims
        .get("exp")
        .and_then(Value::as_i64)
        .ok_or_else(|| PrivilegeError::Authentication("lack 'exp'".into()))?;
    if now > exp {
        return Err(PrivilegeError::Authentication(
            "the token has been expired".into(),
        ));
    }
    match claims.get("iss").and_then(Value::as_str) {
        Some(issuer) if issuer != record.AuthTokenIssuer => {
            Err(PrivilegeError::Authentication("Wrong 'iss'".into()))
        }
        None if !record.AuthTokenIssuer.is_empty() => {
            Err(PrivilegeError::Authentication("lack 'iss'".into()))
        }
        _ => Ok(()),
    }
}
/// 构造账户自动锁定错误（含失败次数与剩余锁定描述）。
pub fn GenerateAccountAutoLockErr(
    attempts: i64,
    user: &str,
    host: &str,
    _lock_time: &str,
    remaining: &str,
) -> PrivilegeError {
    PrivilegeError::PasswordLock {
        user: user.into(),
        host: host.into(),
        attempts,
        remaining: remaining.into(),
    }
}
/// 构造成功登录后的 Password Locking JSON（未锁定、计数清零）。
pub fn BuildSuccessPasswordLockingJSON(attempts: i64, days: i64) -> String {
    BuildPasswordLockingJSON(attempts, days, "N", 0, &now_unix().to_string())
}
/// 序列化 Password Locking 属性为 `mysql.user` 用户属性 JSON。
pub fn BuildPasswordLockingJSON(
    attempts: i64,
    days: i64,
    locked: &str,
    count: i64,
    last_changed: &str,
) -> String {
    json!({"Password_locking":{"failed_login_count":count,"failed_login_attempts":attempts,"password_lock_time_days":days,"auto_account_locked":locked,"auto_locked_last_changed":last_changed}}).to_string()
}
/// 按认证插件比对存储哈希与客户端认证响应。
pub fn checkPasswordForPlugin(
    plugin: &str,
    stored_hash: &str,
    salt: &[u8],
    authentication: &[u8],
) -> Result<bool, PrivilegeError> {
    // Matches Go's `checkPasswordForPlugin` (privileges.go): an empty stored
    // hash never authenticates on its own here. The "no-password account"
    // success path lives in `ConnectionVerification`, which skips calling
    // this function at all when both the stored hash and the client-supplied
    // `authentication` are empty.
    if stored_hash.is_empty() {
        return Ok(false);
    }
    match plugin {
        "mysql_native_password" => {
            let hex = stored_hash.strip_prefix('*').ok_or_else(|| {
                PrivilegeError::Authentication("invalid native password hash".into())
            })?;
            let stage2 =
                hex::decode(hex).map_err(|e| PrivilegeError::Authentication(e.to_string()))?;
            if stage2.len() != 20 || authentication.len() != 20 {
                return Ok(false);
            }
            // mysql_native_password：XOR(scramble, auth) 得 stage1，再 SHA1 比对 stage2。
            let mut h = Sha1::new();
            h.update(salt);
            h.update(&stage2);
            let scramble = h.finalize();
            let stage1: Vec<u8> = authentication
                .iter()
                .zip(scramble)
                .map(|(a, b)| a ^ b)
                .collect();
            Ok(Sha1::digest(stage1).as_slice() == stage2)
        }
        "caching_sha2_password" | "tidb_sm3_password" => {
            Ok(stored_hash.as_bytes() == authentication)
        }
        _ => Ok(false),
    }
}
/// 校验客户端证书 SAN（DNS/IP/URI）是否满足全局权限要求。
pub fn checkCertSAN(
    _privilege: &globalPrivRecord,
    cert: &Certificate,
    sans: &HashMap<String, Vec<String>>,
) -> bool {
    sans.iter().all(|(kind, required)| {
        let actual = match kind.to_ascii_uppercase().as_str() {
            "DNS" => &cert.dns_names,
            "IP" => &cert.ip_addresses,
            "URI" => &cert.uris,
            // Go logs and skips SAN kinds unknown to this version.
            _ => return true,
        };
        // Values for one SAN kind are alternatives, and x509 SAN values are
        // compared exactly (Go's slices.Contains), including case.
        required.iter().any(|value| actual.contains(value))
    })
}
/// 注册新的动态权限名（大写、最长 32、不可重复）。
pub fn RegisterDynamicPrivilege(name: &str) -> Result<(), PrivilegeError> {
    if name.is_empty() {
        return Err(PrivilegeError::InvalidPrivilegeType(
            "privilege name should not be empty".into(),
        ));
    }
    let upper = name.to_ascii_uppercase();
    if upper.len() > 32 {
        return Err(PrivilegeError::InvalidPrivilegeType(
            "privilege name is longer than 32 characters".into(),
        ));
    }
    let mut privileges = dynamic_privileges().lock().unwrap();
    if privileges.iter().any(|p| *p == upper) {
        return Err(PrivilegeError::InvalidPrivilegeType(
            "privilege is already registered".into(),
        ));
    }
    privileges.push(upper);
    Ok(())
}
/// 返回当前已注册动态权限列表副本。
pub fn GetDynamicPrivileges() -> Vec<String> {
    dynamic_privileges().lock().unwrap().clone()
}
/// 按名移除动态权限；存在则返回 true。
pub fn RemoveDynamicPrivilege(name: &str) -> bool {
    let mut privileges = dynamic_privileges().lock().unwrap();
    if let Some(index) = privileges.iter().position(|p| p.eq_ignore_ascii_case(name)) {
        privileges.remove(index);
        true
    } else {
        false
    }
}
/// 包初始化占位（对齐 Go `init`）。
pub fn init() {}

impl PasswordLocking {
    /// 从用户属性 JSON 解析 Password Locking 字段。
    pub fn ParseJSON(&mut self, value: &Value) -> Result<(), PrivilegeError> {
        self.FailedLoginAttempts =
            extractInt64FromJSON(value, "failed_login_attempts")?.clamp(0, i16::MAX as i64);
        self.PasswordLockTimeDays =
            extractInt64FromJSON(value, "password_lock_time_days")?.clamp(-1, i16::MAX as i64);
        self.FailedLoginCount = extractInt64FromJSON(value, "failed_login_count")?;
        self.AutoLockedLastChanged = extractTimeUnixFromJSON(value, "auto_locked_last_changed")?;
        self.AutoAccountLocked = extractBoolFromJSON(value, "auto_account_locked")?;
        Ok(())
    }
}
/// 取出 JSON 中的 `Password_locking` 对象。
fn password_locking_value<'a>(value: &'a Value) -> Option<&'a Value> {
    value.get("Password_locking")
}
/// 从 Password_locking 中提取 i64 字段，缺失则为 0。
pub fn extractInt64FromJSON(value: &Value, path: &str) -> Result<i64, PrivilegeError> {
    Ok(password_locking_value(value)
        .and_then(|v| v.get(path))
        .and_then(Value::as_i64)
        .unwrap_or(0))
}
/// 从 Password_locking 提取时间戳（支持数字或数字字符串）。
pub fn extractTimeUnixFromJSON(value: &Value, path: &str) -> Result<i64, PrivilegeError> {
    let Some(raw) = password_locking_value(value).and_then(|v| v.get(path)) else {
        return Ok(0);
    };
    if let Some(ts) = raw.as_i64() {
        return Ok(ts);
    };
    raw.as_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| PrivilegeError::InvalidJson(format!("invalid time at {path}")))
}
/// 从 Password_locking 提取布尔（`"Y"` 或 JSON true）。
pub fn extractBoolFromJSON(value: &Value, path: &str) -> Result<bool, PrivilegeError> {
    Ok(password_locking_value(value)
        .and_then(|v| v.get(path))
        .is_some_and(|v| v == "Y" || v == true))
}
