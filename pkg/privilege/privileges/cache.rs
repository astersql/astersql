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
// MySQL 权限缓存：从系统表加载 user/db/tables/columns/role 等权限并做校验。
//
// 对应 Go `privileges/cache.go`。`MySQLPrivilege` 持有内存权限表；`Handle`
// 提供并发读写包装。主机匹配支持通配符与 CIDR；角色通过 role_graph BFS 展开。
// 静态权限为位掩码，动态权限存于 `mysql.global_grants`。

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, RwLock};

use crate::PrivilegeError;

/// 静态权限位掩码类型（每一位对应一种 MySQL 特权）。
pub type PrivilegeType = u64;
// 以下常量为各类静态权限位，语义对齐 Go `mysql.PrivilegeType`。
/// USAGE：无实际特权，仅表示账户存在。
pub const UsagePriv: PrivilegeType = 0;
/// CREATE 权限。
pub const CreatePriv: PrivilegeType = 1 << 0;
/// SELECT 权限。
pub const SelectPriv: PrivilegeType = 1 << 1;
/// INSERT 权限。
pub const InsertPriv: PrivilegeType = 1 << 2;
/// UPDATE 权限。
pub const UpdatePriv: PrivilegeType = 1 << 3;
/// DELETE 权限。
pub const DeletePriv: PrivilegeType = 1 << 4;
/// SHOW DATABASES 权限。
pub const ShowDBPriv: PrivilegeType = 1 << 5;
/// DROP 权限。
pub const DropPriv: PrivilegeType = 1 << 6;
/// ALTER 权限。
pub const AlterPriv: PrivilegeType = 1 << 7;
/// INDEX 权限。
pub const IndexPriv: PrivilegeType = 1 << 8;
/// CREATE VIEW 权限。
pub const CreateViewPriv: PrivilegeType = 1 << 9;
/// SHOW VIEW 权限。
pub const ShowViewPriv: PrivilegeType = 1 << 10;
/// GRANT OPTION 权限。
pub const GrantPriv: PrivilegeType = 1 << 11;
/// TRIGGER 权限。
pub const TriggerPriv: PrivilegeType = 1 << 12;
/// REFERENCES 权限。
pub const ReferencesPriv: PrivilegeType = 1 << 13;
/// EXECUTE 权限。
pub const ExecutePriv: PrivilegeType = 1 << 14;
/// CREATE TEMPORARY TABLES 权限。
pub const CreateTMPTablePriv: PrivilegeType = 1 << 15;
/// SUPER 权限（可回退覆盖多数动态权限）。
pub const SuperPriv: PrivilegeType = 1 << 16;
/// PROCESS 权限。
pub const ProcessPriv: PrivilegeType = 1 << 17;
/// SHUTDOWN 权限。
pub const ShutdownPriv: PrivilegeType = 1 << 18;
/// CREATE USER 权限。
pub const CreateUserPriv: PrivilegeType = 1 << 19;
/// CREATE ROLE 权限。
pub const CreateRolePriv: PrivilegeType = 1 << 20;
/// DROP ROLE 权限。
pub const DropRolePriv: PrivilegeType = 1 << 21;
/// LOCK TABLES 权限。
pub const LockTablesPriv: PrivilegeType = 1 << 22;
/// CREATE ROUTINE 权限。
pub const CreateRoutinePriv: PrivilegeType = 1 << 23;
/// ALTER ROUTINE 权限。
pub const AlterRoutinePriv: PrivilegeType = 1 << 24;
/// EVENT 权限。
pub const EventPriv: PrivilegeType = 1 << 25;
/// RELOAD 权限。
pub const ReloadPriv: PrivilegeType = 1 << 26;
/// FILE 权限。
pub const FilePriv: PrivilegeType = 1 << 27;
/// CONFIG 权限。
pub const ConfigPriv: PrivilegeType = 1 << 28;
/// REPLICATION CLIENT 权限。
pub const ReplClientPriv: PrivilegeType = 1 << 29;
/// REPLICATION SLAVE 权限。
pub const ReplSlavePriv: PrivilegeType = 1 << 30;
/// OPERATE VIEW permission for materialized-view maintenance.
pub const OperateViewPriv: PrivilegeType = 1 << 31;

// 对齐 Go 的 AllGlobalPrivs/AllDBPrivs/AllTablePrivs：列表刻意不含 GrantPriv。
// Matches Go's mysql.AllGlobalPrivs/AllDBPrivs/AllTablePrivs (pkg/parser/mysql/privs.go):
// `GrantPriv` is deliberately excluded from every list below. Go's showGrants
// (cache.go) renders GRANT OPTION exclusively via the `WITH GRANT OPTION`
// suffix, never inside the comma-joined privilege list, so including it here
// would double-report it as a plain privilege name too.
/// 全局级可枚举特权列表（用于 SHOW GRANTS / ALL PRIVILEGES 判定）。
pub const ALL_GLOBAL_PRIVS: &[PrivilegeType] = &[
    SelectPriv,
    InsertPriv,
    UpdatePriv,
    DeletePriv,
    CreatePriv,
    DropPriv,
    ProcessPriv,
    ReferencesPriv,
    AlterPriv,
    ShowDBPriv,
    SuperPriv,
    ExecutePriv,
    IndexPriv,
    CreateUserPriv,
    TriggerPriv,
    CreateViewPriv,
    ShowViewPriv,
    OperateViewPriv,
    CreateRolePriv,
    DropRolePriv,
    CreateTMPTablePriv,
    LockTablesPriv,
    CreateRoutinePriv,
    AlterRoutinePriv,
    EventPriv,
    ShutdownPriv,
    ReloadPriv,
    FilePriv,
    ConfigPriv,
    ReplClientPriv,
    ReplSlavePriv,
];
/// 库级可枚举特权列表。
pub const ALL_DB_PRIVS: &[PrivilegeType] = &[
    SelectPriv,
    InsertPriv,
    UpdatePriv,
    DeletePriv,
    CreatePriv,
    DropPriv,
    ReferencesPriv,
    LockTablesPriv,
    CreateTMPTablePriv,
    EventPriv,
    CreateRoutinePriv,
    AlterRoutinePriv,
    AlterPriv,
    ExecutePriv,
    IndexPriv,
    CreateViewPriv,
    ShowViewPriv,
    OperateViewPriv,
    TriggerPriv,
];
/// 表级可枚举特权列表。
pub const ALL_TABLE_PRIVS: &[PrivilegeType] = &[
    SelectPriv,
    InsertPriv,
    UpdatePriv,
    DeletePriv,
    CreatePriv,
    DropPriv,
    IndexPriv,
    ReferencesPriv,
    AlterPriv,
    CreateViewPriv,
    ShowViewPriv,
    OperateViewPriv,
    TriggerPriv,
];

/// 将特权列表折叠为位掩码。
pub fn computePrivMask(privileges: &[PrivilegeType]) -> PrivilegeType {
    privileges
        .iter()
        .fold(0, |mask, privilege| mask | privilege)
}

/// 使库在 SHOW DATABASES 中可见的全局特权掩码。
pub const globalDBVisible: PrivilegeType = CreatePriv
    | SelectPriv
    | InsertPriv
    | UpdatePriv
    | DeletePriv
    | ShowDBPriv
    | DropPriv
    | AlterPriv
    | IndexPriv
    | CreateViewPriv
    | ShowViewPriv
    | OperateViewPriv
    | GrantPriv
    | TriggerPriv
    | ReferencesPriv
    | ExecutePriv
    | CreateTMPTablePriv;

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
/// 角色身份：用户名 + 主机名。
pub struct RoleIdentity {
    pub Username: String,
    pub Hostname: String,
}

impl RoleIdentity {
    /// 构造角色身份。
    pub fn new(user: impl Into<String>, host: impl Into<String>) -> Self {
        Self {
            Username: user.into(),
            Hostname: host.into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 用户元数据（来自 user_attributes.metadata）。
pub struct MetadataInfo {
    pub Email: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 密码锁定策略与失败计数（FAILED_LOGIN_ATTEMPTS 等）。
pub struct PasswordLocking {
    pub FailedLoginCount: i64,
    pub PasswordLockTimeDays: i64,
    pub AutoAccountLocked: bool,
    pub AutoLockedLastChanged: i64,
    pub FailedLoginAttempts: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// `mysql.user.user_attributes` JSON 解码结果。
pub struct UserAttributesInfo {
    pub MetadataInfo: MetadataInfo,
    pub PasswordLocking: PasswordLocking,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 权限行公共字段：Host + User。
pub struct baseRecord {
    pub Host: String,
    pub User: String,
}

impl baseRecord {
    /// 构造 Host/User 记录。
    pub fn new(host: impl Into<String>, user: impl Into<String>) -> Self {
        Self {
            Host: host.into(),
            User: user.into(),
        }
    }

    /// 主机是否匹配（支持 CIDR 与 `%`/`_` 通配）。
    pub fn hostMatch(&self, host: &str) -> bool {
        if self.Host.eq_ignore_ascii_case("localhost")
            && host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
        {
            return true;
        }
        if let Some((network, mask)) = parseHostIPNet(&self.Host) {
            return host
                .parse::<Ipv4Addr>()
                .map(|address| (u32::from(address) & mask) == network)
                .unwrap_or(false);
        }
        patternMatch(host, self.Host.as_bytes(), &[])
    }

    /// 用户名精确匹配且主机模式匹配。
    pub fn r#match(&self, user: &str, host: &str) -> bool {
        self.User == user && self.hostMatch(host)
    }

    /// 用户名与主机均区分大小写地精确匹配。
    pub fn fullyMatch(&self, user: &str, host: &str) -> bool {
        self.User == user && self.Host == host
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// `mysql.user` 表一行：认证信息与全局静态权限。
pub struct UserRecord {
    pub base: baseRecord,
    pub UserAttributesInfo: UserAttributesInfo,
    pub AuthenticationString: String,
    pub AdditionalAuthString: String,
    pub Privileges: PrivilegeType,
    pub AccountLocked: bool,
    pub AuthPlugin: String,
    pub AuthTokenIssuer: String,
    pub PasswordExpired: bool,
    pub PasswordLastChanged: i64,
    pub PasswordLifeTime: i64,
    pub MaxUserConnections: i64,
    pub ResourceGroup: String,
}

impl UserRecord {
    /// 主机模式。
    pub fn Host(&self) -> &str {
        &self.base.Host
    }
    /// 用户名。
    pub fn User(&self) -> &str {
        &self.base.User
    }
    /// 用户邮箱元数据。
    pub fn Email(&self) -> &str {
        &self.UserAttributesInfo.MetadataInfo.Email
    }
}

/// 构造空权限字段的用户记录。
pub fn NewUserRecord(host: &str, user: &str) -> UserRecord {
    UserRecord {
        base: baseRecord::new(host, user),
        ..Default::default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(i32)]
/// 连接 SSL/X509 要求类型（对应 REQUIRE 子句）。
pub enum SSLType {
    #[default]
    SslTypeNotSpecified = -1,
    SslTypeNone = 0,
    SslTypeAny = 1,
    SslTypeX509 = 2,
    SslTypeSpecified = 3,
}

pub const SslTypeNotSpecified: SSLType = SSLType::SslTypeNotSpecified;
pub const SslTypeNone: SSLType = SSLType::SslTypeNone;
pub const SslTypeAny: SSLType = SSLType::SslTypeAny;
pub const SslTypeX509: SSLType = SSLType::SslTypeX509;
pub const SslTypeSpecified: SSLType = SSLType::SslTypeSpecified;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// `mysql.global_priv.priv` JSON 中的 SSL/SAN 等要求。
pub struct GlobalPrivValue {
    pub SSLType: SSLType,
    pub SSLCipher: String,
    pub X509Issuer: String,
    pub X509Subject: String,
    pub SAN: String,
    pub SANs: HashMap<String, Vec<String>>,
}

impl GlobalPrivValue {
    /// 渲染 REQUIRE SSL/X509/CIPHER... 子句文本。
    pub fn RequireStr(&self) -> String {
        match self.SSLType {
            SSLType::SslTypeAny => "SSL".into(),
            SSLType::SslTypeX509 => "X509".into(),
            SSLType::SslTypeSpecified => {
                let mut clauses = Vec::new();
                for (name, value) in [
                    ("CIPHER", &self.SSLCipher),
                    ("ISSUER", &self.X509Issuer),
                    ("SUBJECT", &self.X509Subject),
                    ("SAN", &self.SAN),
                ] {
                    if !value.is_empty() {
                        clauses.push(format!("{name} '{value}'"));
                    }
                }
                if clauses.is_empty() {
                    "NONE".into()
                } else {
                    clauses.join(" ")
                }
            }
            _ => "NONE".into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// `mysql.global_priv` 一行。
pub struct globalPrivRecord {
    pub base: baseRecord,
    pub Priv: GlobalPrivValue,
    pub Broken: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// `mysql.global_grants` 一行（动态权限）。
pub struct dynamicPrivRecord {
    pub base: baseRecord,
    pub PrivilegeName: String,
    pub GrantOption: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// `mysql.db` 一行（库级权限）。
pub struct dbRecord {
    pub base: baseRecord,
    pub DB: String,
    pub Privileges: PrivilegeType,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// `mysql.tables_priv` 一行。
pub struct tablesPrivRecord {
    pub base: baseRecord,
    pub DB: String,
    pub TableName: String,
    pub Grantor: String,
    pub Timestamp: i64,
    pub TablePriv: PrivilegeType,
    pub ColumnPriv: PrivilegeType,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// `mysql.columns_priv` 一行。
pub struct columnsPrivRecord {
    pub base: baseRecord,
    pub DB: String,
    pub TableName: String,
    pub ColumnName: String,
    pub Timestamp: i64,
    pub ColumnPriv: PrivilegeType,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// `mysql.default_roles` 一行。
pub struct defaultRoleRecord {
    pub base: baseRecord,
    pub DefaultRoleHost: String,
    pub DefaultRoleUser: String,
}

impl dbRecord {
    fn r#match(&self, user: &str, host: &str, db: &str) -> bool {
        self.base.r#match(user, host) && pattern_match_ci(db, &self.DB)
    }
}
impl tablesPrivRecord {
    fn r#match(&self, user: &str, host: &str, db: &str, table: &str) -> bool {
        self.base.r#match(user, host)
            && self.DB.eq_ignore_ascii_case(db)
            && self.TableName.eq_ignore_ascii_case(table)
    }
}
impl columnsPrivRecord {
    fn r#match(&self, user: &str, host: &str, db: &str, table: &str, column: &str) -> bool {
        // `SELECT COUNT(*) ...` requires a column-level SELECT privilege of any column,
        // so we add a special case "*" here (see Go's columnsPrivRecord.match).
        self.base.r#match(user, host)
            && self.DB.eq_ignore_ascii_case(db)
            && self.TableName.eq_ignore_ascii_case(table)
            && (self.ColumnName.eq_ignore_ascii_case(column)
                || (column == "*" && self.ColumnPriv & SelectPriv > 0))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 角色图邻接表：某个用户/角色直接被授予的角色集合。
pub struct roleGraphEdgesTable(pub HashSet<RoleIdentity>);

impl roleGraphEdgesTable {
    /// 是否包含指定角色边。
    pub fn Find(&self, user: &str, host: &str) -> bool {
        self.0.contains(&RoleIdentity::new(user, host))
            || (host.is_empty() && self.0.contains(&RoleIdentity::new(user, "%")))
    }
}

/// 权限数据源：替代 Go 侧 SQL executor，向缓存提供各系统表行。
pub trait PrivilegeDataSource {
    // `default_auth_plugin` mirrors Go's `p.globalVars.GetGlobalSysVar(vardef.DefaultAuthPlugin)`
    // lookup inside `decodeUserTableRow`'s closure: since that lookup reads the
    // *loading* `MySQLPrivilege`'s accessor (set via `SetGlobalVarsAccessor`), the
    // data source needs it passed in rather than reading its own state.
    fn users(&self, default_auth_plugin: &str) -> Result<Vec<UserRecord>, PrivilegeError> {
        let _ = default_auth_plugin;
        Ok(Vec::new())
    }
    fn global_privileges(&self) -> Result<Vec<globalPrivRecord>, PrivilegeError> {
        Ok(Vec::new())
    }
    fn dynamic_privileges(&self) -> Result<Vec<dynamicPrivRecord>, PrivilegeError> {
        Ok(Vec::new())
    }
    fn databases(&self) -> Result<Vec<dbRecord>, PrivilegeError> {
        Ok(Vec::new())
    }
    fn tables(&self) -> Result<Vec<tablesPrivRecord>, PrivilegeError> {
        Ok(Vec::new())
    }
    fn columns(&self) -> Result<Vec<columnsPrivRecord>, PrivilegeError> {
        Ok(Vec::new())
    }
    fn default_roles(&self) -> Result<Vec<defaultRoleRecord>, PrivilegeError> {
        Ok(Vec::new())
    }
    fn role_graph(&self) -> Result<HashMap<RoleIdentity, roleGraphEdgesTable>, PrivilegeError> {
        Ok(HashMap::new())
    }
}

#[derive(Clone, Debug, Default)]
/// 内存权限缓存：各系统表记录与角色图。
pub struct MySQLPrivilege {
    pub user: Vec<UserRecord>,
    pub global_priv: Vec<globalPrivRecord>,
    pub dynamic_priv: Vec<dynamicPrivRecord>,
    pub db: Vec<dbRecord>,
    pub tables_priv: Vec<tablesPrivRecord>,
    pub columns_priv: Vec<columnsPrivRecord>,
    pub default_roles: Vec<defaultRoleRecord>,
    pub role_graph: HashMap<RoleIdentity, roleGraphEdgesTable>,
    pub ColumnsPrivMap: HashMap<String, Vec<columnsPrivRecord>>,
    pub default_auth_plugin: String,
}

/// 创建空的权限缓存。
pub fn newMySQLPrivilege() -> MySQLPrivilege {
    MySQLPrivilege::default()
}
/// `newMySQLPrivilege` 的导出别名。
pub fn NewMySQLPrivilege() -> MySQLPrivilege {
    newMySQLPrivilege()
}

impl MySQLPrivilege {
    /// 克隆当前缓存快照。
    pub fn Clone(&self) -> Self {
        self.clone()
    }
    /// 返回用户表切片。
    pub fn User(&self) -> &[UserRecord] {
        &self.user
    }
    /// 返回库权限表切片。
    pub fn DB(&self) -> &[dbRecord] {
        &self.db
    }
    /// 返回表权限表切片。
    pub fn TablesPriv(&self) -> &[tablesPrivRecord] {
        &self.tables_priv
    }
    /// 返回列权限表切片。
    pub fn ColumnsPriv(&self) -> &[columnsPrivRecord] {
        &self.columns_priv
    }
    /// 返回默认角色表切片。
    pub fn DefaultRoles(&self) -> &[defaultRoleRecord] {
        &self.default_roles
    }
    /// 按用户名筛选 global_priv 记录。
    pub fn GlobalPriv(&self, user: &str) -> Vec<&globalPrivRecord> {
        self.global_priv
            .iter()
            .filter(|record| record.base.User == user)
            .collect()
    }
    /// 返回角色图。
    pub fn RoleGraph(&self) -> &HashMap<RoleIdentity, roleGraphEdgesTable> {
        &self.role_graph
    }

    /// BFS 展开活动角色的全部可达角色。
    pub fn FindAllRole(&self, active_roles: &[RoleIdentity]) -> Vec<RoleIdentity> {
        let mut seen = HashSet::new();
        let mut queue: VecDeque<_> = active_roles.iter().cloned().collect();
        while let Some(role) = queue.pop_front() {
            if !seen.insert(role.clone()) {
                continue;
            }
            if let Some(edges) = self.role_graph.get(&role) {
                queue.extend(edges.0.iter().cloned());
            }
        }
        seen.into_iter().collect()
    }

    /// 先过滤用户当前直接拥有的活动角色，再 BFS 展开有效角色闭包。
    pub fn FindAllUserEffectiveRoles(
        &self,
        user: &str,
        host: &str,
        active_roles: &[RoleIdentity],
    ) -> Vec<RoleIdentity> {
        // Go filters `activeRoles` down to those directly granted to the user
        // *before* expanding the BFS closure (see cache.go), so roles
        // transitively reachable only through a directly-granted role are
        // still included.
        let granted: Vec<RoleIdentity> = active_roles
            .iter()
            .filter(|role| self.FindRole(user, host, role))
            .cloned()
            .collect();
        self.FindAllRole(&granted)
    }

    /// 角色图中是否存在 user@host → role 边。
    pub fn FindRole(&self, user: &str, host: &str, role: &RoleIdentity) -> bool {
        let Some(user_record) = self.matchUser(user, host) else {
            return false;
        };
        let Some(role_record) = self.matchUser(&role.Username, &role.Hostname) else {
            return false;
        };
        self.role_graph
            .get(&RoleIdentity::new(
                &user_record.base.User,
                &user_record.base.Host,
            ))
            .is_some_and(|edges| edges.Find(&role_record.base.User, &role_record.base.Host))
    }

    /// 从数据源加载全部权限系统表。
    pub fn LoadAll(&mut self, source: &dyn PrivilegeDataSource) -> Result<(), PrivilegeError> {
        self.LoadUserTable(source)?;
        self.LoadGlobalPrivTable(source)?;
        self.LoadGlobalGrantsTable(source)?;
        self.LoadDBTable(source)?;
        self.LoadTablesPrivTable(source)?;
        self.LoadColumnsPrivTable(source)?;
        self.LoadDefaultRoles(source)?;
        self.LoadRoleGraph(source)?;
        Ok(())
    }

    /// 加载 `mysql.user` 并按主机特异性排序。
    pub fn LoadUserTable(
        &mut self,
        source: &dyn PrivilegeDataSource,
    ) -> Result<(), PrivilegeError> {
        self.user = source.users(&self.default_auth_plugin)?;
        self.SortUserTable();
        Ok(())
    }
    /// 加载 `mysql.global_priv`。
    pub fn LoadGlobalPrivTable(
        &mut self,
        source: &dyn PrivilegeDataSource,
    ) -> Result<(), PrivilegeError> {
        self.global_priv = source.global_privileges()?;
        self.global_priv
            .sort_by(|a, b| compareBaseRecord(&a.base, &b.base));
        Ok(())
    }
    /// 加载 `mysql.global_grants`（动态权限）。
    pub fn LoadGlobalGrantsTable(
        &mut self,
        source: &dyn PrivilegeDataSource,
    ) -> Result<(), PrivilegeError> {
        self.dynamic_priv = source.dynamic_privileges()?;
        self.dynamic_priv
            .sort_by(|a, b| compareBaseRecord(&a.base, &b.base));
        Ok(())
    }
    /// 加载 `mysql.db`。
    pub fn LoadDBTable(&mut self, source: &dyn PrivilegeDataSource) -> Result<(), PrivilegeError> {
        self.db = source.databases()?;
        self.db.sort_by(compareDBRecord);
        Ok(())
    }
    /// 加载 `mysql.tables_priv`。
    pub fn LoadTablesPrivTable(
        &mut self,
        source: &dyn PrivilegeDataSource,
    ) -> Result<(), PrivilegeError> {
        self.tables_priv = source.tables()?;
        self.tables_priv.sort_by(compareTablesPrivRecord);
        Ok(())
    }
    /// 加载 `mysql.columns_priv` 并构建按用户索引。
    pub fn LoadColumnsPrivTable(
        &mut self,
        source: &dyn PrivilegeDataSource,
    ) -> Result<(), PrivilegeError> {
        self.columns_priv = source.columns()?;
        self.columns_priv.sort_by(compareColumnsPrivRecord);
        self.buildColumnsPrivMap();
        Ok(())
    }
    /// 加载 `mysql.default_roles`。
    pub fn LoadDefaultRoles(
        &mut self,
        source: &dyn PrivilegeDataSource,
    ) -> Result<(), PrivilegeError> {
        self.default_roles = source.default_roles()?;
        self.default_roles.sort_by(compareDefaultRoleRecord);
        Ok(())
    }
    /// 加载角色边图。
    pub fn LoadRoleGraph(
        &mut self,
        source: &dyn PrivilegeDataSource,
    ) -> Result<(), PrivilegeError> {
        self.role_graph = source.role_graph()?;
        Ok(())
    }

    /// 按主机特异性与用户名排序 user 表。
    pub fn SortUserTable(&mut self) {
        self.user.sort_by(compareUserRecord);
    }
    /// 将一次库级 GRANT 增量合并到内存权限缓存。
    pub fn GrantDatabasePrivilegeColumns(
        &mut self,
        host: &str,
        user: &str,
        database: &str,
        columns: &[String],
    ) {
        let privileges = ALL_DB_PRIVS
            .iter()
            .copied()
            .chain(std::iter::once(GrantPriv))
            .filter(|privilege| {
                let column = privilege_column_name(*privilege);
                columns
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(&column))
            })
            .fold(0, |mask, privilege| mask | privilege);
        if let Some(record) = self.db.iter_mut().find(|record| {
            record.base.Host.eq_ignore_ascii_case(host)
                && record.base.User == user
                && record.DB.eq_ignore_ascii_case(database)
        }) {
            record.Privileges |= privileges;
        } else {
            self.db.push(dbRecord {
                base: baseRecord::new(host, user),
                DB: database.to_owned(),
                Privileges: privileges,
            });
        }
        self.db.sort_by(compareDBRecord);
    }

    /// 将全局静态权限增量合并到 `mysql.user` 缓存记录。
    pub fn GrantGlobalPrivilegeMask(&mut self, host: &str, user: &str, mask: PrivilegeType) {
        if let Some(record) = self
            .user
            .iter_mut()
            .find(|record| record.base.fullyMatch(user, host))
        {
            record.Privileges |= mask;
        }
    }

    /// 将表级权限增量合并到 `mysql.tables_priv` 缓存记录。
    pub fn GrantTablePrivilegeMask(
        &mut self,
        host: &str,
        user: &str,
        database: &str,
        table: &str,
        table_mask: PrivilegeType,
        column_mask: PrivilegeType,
    ) {
        if let Some(record) = self.tables_priv.iter_mut().find(|record| {
            record.base.Host.eq_ignore_ascii_case(host)
                && record.base.User == user
                && record.DB.eq_ignore_ascii_case(database)
                && record.TableName.eq_ignore_ascii_case(table)
        }) {
            record.TablePriv |= table_mask;
            record.ColumnPriv |= column_mask;
        } else {
            self.tables_priv.push(tablesPrivRecord {
                base: baseRecord::new(host, user),
                DB: database.to_owned(),
                TableName: table.to_owned(),
                TablePriv: table_mask,
                ColumnPriv: column_mask,
                ..Default::default()
            });
        }
        self.tables_priv.sort_by(compareTablesPrivRecord);
    }

    /// 将列级权限增量合并到 `mysql.columns_priv` 缓存记录。
    pub fn GrantColumnPrivilegeMask(
        &mut self,
        host: &str,
        user: &str,
        database: &str,
        table: &str,
        column: &str,
        mask: PrivilegeType,
    ) {
        if let Some(record) = self.columns_priv.iter_mut().find(|record| {
            record.base.Host.eq_ignore_ascii_case(host)
                && record.base.User == user
                && record.DB.eq_ignore_ascii_case(database)
                && record.TableName.eq_ignore_ascii_case(table)
                && record.ColumnName.eq_ignore_ascii_case(column)
        }) {
            record.ColumnPriv |= mask;
        } else {
            self.columns_priv.push(columnsPrivRecord {
                base: baseRecord::new(host, user),
                DB: database.to_owned(),
                TableName: table.to_owned(),
                ColumnName: column.to_owned(),
                ColumnPriv: mask,
                ..Default::default()
            });
        }
        self.columns_priv.sort_by(compareColumnsPrivRecord);
        self.buildColumnsPrivMap();
    }

    /// 删除账号及其所有作用域权限，供 DROP USER 提交后原子刷新缓存。
    pub fn DropAccount(&mut self, host: &str, user: &str) {
        self.user
            .retain(|record| !record.base.fullyMatch(user, host));
        self.global_priv
            .retain(|record| !record.base.fullyMatch(user, host));
        self.dynamic_priv
            .retain(|record| !record.base.fullyMatch(user, host));
        self.db.retain(|record| !record.base.fullyMatch(user, host));
        self.tables_priv
            .retain(|record| !record.base.fullyMatch(user, host));
        self.columns_priv
            .retain(|record| !record.base.fullyMatch(user, host));
        self.default_roles
            .retain(|record| !record.base.fullyMatch(user, host));
        self.SortUserTable();
        self.buildColumnsPrivMap();
    }

    /// 在提交 REVOKE 后按精确作用域移除权限位并清理空记录。
    pub fn RevokePrivilegeMask(
        &mut self,
        host: &str,
        user: &str,
        database: Option<&str>,
        table: Option<&str>,
        column: Option<&str>,
        mask: PrivilegeType,
    ) {
        match (database, table, column) {
            (None, None, None) => {
                if let Some(record) = self
                    .user
                    .iter_mut()
                    .find(|record| record.base.fullyMatch(user, host))
                {
                    record.Privileges &= !mask;
                }
            }
            (Some(database), None, None) => {
                for record in &mut self.db {
                    if record.base.fullyMatch(user, host)
                        && record.DB.eq_ignore_ascii_case(database)
                    {
                        record.Privileges &= !mask;
                    }
                }
                self.db.retain(|record| record.Privileges != 0);
            }
            (Some(database), Some(table), None) => {
                for record in &mut self.tables_priv {
                    if record.base.fullyMatch(user, host)
                        && record.DB.eq_ignore_ascii_case(database)
                        && record.TableName.eq_ignore_ascii_case(table)
                    {
                        record.TablePriv &= !mask;
                        record.ColumnPriv &= !mask;
                    }
                }
                self.tables_priv
                    .retain(|record| record.TablePriv != 0 || record.ColumnPriv != 0);
            }
            (Some(database), Some(table), Some(column)) => {
                for record in &mut self.columns_priv {
                    if record.base.fullyMatch(user, host)
                        && record.DB.eq_ignore_ascii_case(database)
                        && record.TableName.eq_ignore_ascii_case(table)
                        && record.ColumnName.eq_ignore_ascii_case(column)
                    {
                        record.ColumnPriv &= !mask;
                    }
                }
                self.columns_priv.retain(|record| record.ColumnPriv != 0);
                self.buildColumnsPrivMap();
            }
            _ => {}
        }
    }
    /// 按用户名重建列权限索引。
    pub fn buildColumnsPrivMap(&mut self) {
        self.ColumnsPrivMap.clear();
        for record in &self.columns_priv {
            self.ColumnsPrivMap
                .entry(record.base.User.clone())
                .or_default()
                .push(record.clone());
        }
    }
    /// 设置默认认证插件（解码 user 行时回退使用）。
    pub fn SetGlobalVarsAccessor(&mut self, plugin: impl Into<String>) {
        self.default_auth_plugin = plugin.into();
    }
    /// 增量加载指定用户集合的权限并合并。
    pub fn loadSomeUsers(
        &mut self,
        source: &dyn PrivilegeDataSource,
        users: &HashSet<String>,
    ) -> Result<(), PrivilegeError> {
        let mut loaded = MySQLPrivilege::default();
        loaded.LoadAll(source)?;
        *self = self.merge(&loaded, users);
        Ok(())
    }
    /// 用 diff 中指定用户的记录替换 self 对应条目。
    pub fn merge(&self, diff: &MySQLPrivilege, users: &HashSet<String>) -> MySQLPrivilege {
        let mut result = self.clone();
        macro_rules! replace {
            ($field:ident) => {{
                result.$field.retain(|r| !users.contains(&r.base.User));
                result.$field.extend(
                    diff.$field
                        .iter()
                        .filter(|r| users.contains(&r.base.User))
                        .cloned(),
                );
            }};
        }
        replace!(user);
        replace!(db);
        replace!(tables_priv);
        replace!(columns_priv);
        replace!(default_roles);
        replace!(global_priv);
        replace!(dynamic_priv);
        result.role_graph = diff.role_graph.clone();
        result.default_auth_plugin = diff.default_auth_plugin.clone();
        result.SortUserTable();
        result.buildColumnsPrivMap();
        result
    }
    /// 按用户名与主机模式匹配身份（可选要求 host 为合法 IP）。
    pub fn matchIdentity(
        &self,
        user: &str,
        host: &str,
        _skip_name_resolve: bool,
    ) -> Option<&UserRecord> {
        // Go always checks the supplied host against cached host patterns first.
        // `skipNameResolve` only disables the reverse-DNS fallback after that
        // direct pass; this crate intentionally has no resolver boundary.
        self.user
            .iter()
            .find(|record| record.base.User == user && record.base.hostMatch(host))
    }
    /// 连接鉴权时的身份匹配（不跳过非 IP host）。
    pub fn connectionVerification(&self, user: &str, host: &str) -> Option<&UserRecord> {
        // Rust callers pass the connection host directly, whereas Go callers
        // pass the canonical host returned by MatchIdentity first.
        self.matchIdentity(user, host, false)
    }
    /// 匹配 user 表记录。
    pub fn matchUser(&self, user: &str, host: &str) -> Option<&UserRecord> {
        self.user.iter().find(|r| r.base.r#match(user, host))
    }
    /// 匹配 global_priv 记录。
    pub fn matchGlobalPriv(&self, user: &str, host: &str) -> Option<&globalPrivRecord> {
        self.global_priv.iter().find(|r| r.base.r#match(user, host))
    }
    /// 匹配库级权限记录。
    pub fn matchDB(&self, user: &str, host: &str, db: &str) -> Option<&dbRecord> {
        self.db.iter().find(|r| r.r#match(user, host, db))
    }
    /// 匹配表级权限记录。
    pub fn matchTables(
        &self,
        user: &str,
        host: &str,
        db: &str,
        table: &str,
    ) -> Option<&tablesPrivRecord> {
        self.tables_priv
            .iter()
            .find(|r| r.r#match(user, host, db, table))
    }
    /// 匹配列级权限记录（`*` 表示任意列 SELECT）。
    pub fn MatchColumns(
        &self,
        user: &str,
        host: &str,
        db: &str,
        table: &str,
        column: &str,
    ) -> Option<&columnsPrivRecord> {
        self.columns_priv
            .iter()
            .find(|r| r.r#match(user, host, db, table, column))
    }

    /// 用户或其有效角色是否被显式授予动态权限。
    pub fn HasExplicitlyGrantedDynamicPrivilege(
        &self,
        active_roles: &[RoleIdentity],
        user: &str,
        host: &str,
        privilege: &str,
        with_grant: bool,
    ) -> bool {
        let identities = std::iter::once(RoleIdentity::new(user, host)).chain(
            self.FindAllUserEffectiveRoles(user, host, active_roles)
                .into_iter(),
        );
        identities.into_iter().any(|identity| {
            self.dynamic_priv.iter().any(|record| {
                record.base.r#match(&identity.Username, &identity.Hostname)
                    && record.PrivilegeName.eq_ignore_ascii_case(privilege)
                    && (!with_grant || record.GrantOption)
            })
        })
    }
    /// 校验动态权限；非 SEM 受限时可回退 SUPER。
    pub fn RequestDynamicVerification(
        &self,
        active_roles: &[RoleIdentity],
        user: &str,
        host: &str,
        privilege: &str,
        with_grant: bool,
    ) -> bool {
        let privilege = privilege.to_ascii_uppercase();
        if self.HasExplicitlyGrantedDynamicPrivilege(
            active_roles,
            user,
            host,
            &privilege,
            with_grant,
        ) {
            return true;
        }
        // If SEM is enabled, and the privilege is of type restricted, do not fall
        // through to using SUPER as a replacement privilege.
        if sem::IsEnabled() && sem::IsRestrictedPrivilege(&privilege) {
            return false;
        }
        // For compatibility reasons, the SUPER privilege also has all DYNAMIC
        // privileges granted to it (dynamic privs are a super replacement).
        if with_grant && !self.RequestVerification(active_roles, user, host, "", "", "", GrantPriv)
        {
            return false;
        }
        self.RequestVerification(active_roles, user, host, "", "", "", SuperPriv)
    }
    /// 校验静态权限：全局 → 库 → 表 → 列，并纳入有效角色。
    pub fn RequestVerification(
        &self,
        active_roles: &[RoleIdentity],
        user: &str,
        host: &str,
        db: &str,
        table: &str,
        column: &str,
        privilege: PrivilegeType,
    ) -> bool {
        if privilege == UsagePriv {
            return true;
        }
        // Matches Go's `FindAllUserEffectiveRoles(user, host, activeRoles)`:
        // re-validates that every caller-supplied active role is *currently*
        // granted to `user`@`host` before expanding the BFS closure, so a
        // stale/sticky session role list (e.g. after `REVOKE role FROM
        // user`) can no longer grant privileges through a revoked role.
        let identities = std::iter::once(RoleIdentity::new(user, host)).chain(
            self.FindAllUserEffectiveRoles(user, host, active_roles)
                .into_iter(),
        );
        for identity in identities {
            let u = &identity.Username;
            let h = &identity.Hostname;
            if self
                .matchUser(u, h)
                .is_some_and(|r| r.Privileges & privilege != 0)
            {
                return true;
            }
            if !db.is_empty()
                && self
                    .matchDB(u, h, db)
                    .is_some_and(|r| r.Privileges & privilege != 0)
            {
                return true;
            }
            if !table.is_empty()
                && self.matchTables(u, h, db, table).is_some_and(|r| {
                    r.TablePriv & privilege != 0
                        || (!column.is_empty() && r.ColumnPriv & privilege != 0)
                })
            {
                return true;
            }
            if !column.is_empty()
                && self
                    .MatchColumns(u, h, db, table, column)
                    .is_some_and(|r| r.ColumnPriv & privilege != 0)
            {
                return true;
            }
        }
        false
    }
    /// 判断库对用户是否可见（全局可见特权或任一级非空授权）。
    pub fn DBIsVisible(&self, user: &str, host: &str, db: &str) -> bool {
        if db.eq_ignore_ascii_case("information_schema") {
            return true;
        }
        if self.matchUser(user, host).is_some_and(|record| {
            record.Privileges & globalDBVisible != 0
                || (db.eq_ignore_ascii_case("metrics_schema")
                    && record.Privileges & ProcessPriv != 0)
        }) {
            return true;
        }
        self.db
            .iter()
            .any(|r| r.r#match(user, host, db) && r.Privileges != 0)
            || self.tables_priv.iter().any(|r| {
                r.base.r#match(user, host)
                    && r.DB.eq_ignore_ascii_case(db)
                    && (r.TablePriv | r.ColumnPriv) != 0
            })
            || self.columns_priv.iter().any(|r| {
                r.base.r#match(user, host) && r.DB.eq_ignore_ascii_case(db) && r.ColumnPriv != 0
            })
    }
    // 移植 Go showGrants：按全局/库/表/列/角色/动态权限顺序生成 GRANT 文本。
    // Ports Go's `showGrants` (cache.go) verbatim: global -> db -> table ->
    // column -> role -> dynamic-privilege lines, each scope (other than the
    // single global line) sorted independently, mirroring `SHOW GRANTS`
    // output order. `ansi_quotes` mirrors `sqlMode&mysql.ModeANSIQuotes`
    // (stringutil.Escape) for identifier quoting.
    /// 生成 SHOW GRANTS 语句列表。
    pub fn showGrants(
        &self,
        user: &str,
        host: &str,
        roles: &[RoleIdentity],
        ansi_quotes: bool,
    ) -> Vec<String> {
        let mut gs: Vec<String> = Vec::new();
        let all_roles = self.FindAllUserEffectiveRoles(user, host, roles);

        let mut has_global_grant = false;
        let mut current_priv: PrivilegeType = 0;
        let same_username_exists = self.user.iter().any(|r| r.base.User == user);
        if same_username_exists {
            let mut user_exists = false;
            for record in self.user.iter().filter(|r| r.base.User == user) {
                if record.base.fullyMatch(user, host) {
                    user_exists = true;
                    has_global_grant = true;
                    current_priv |= record.Privileges;
                    break;
                }
            }
            if !user_exists {
                return gs;
            }
        }
        for role in &all_roles {
            for record in self.user.iter().filter(|r| r.base.User == role.Username) {
                if record.base.fullyMatch(&role.Username, &role.Hostname) {
                    has_global_grant = true;
                    current_priv |= record.Privileges;
                }
            }
        }
        let g = userPrivToString(current_priv);
        if !g.is_empty() {
            gs.push(if current_priv & GrantPriv != 0 {
                format!("GRANT {g} ON *.* TO '{user}'@'{host}' WITH GRANT OPTION")
            } else {
                format!("GRANT {g} ON *.* TO '{user}'@'{host}'")
            });
        }
        // This is a mysql convention: a user with any global grant (even none
        // of the enumerable ones, e.g. only GrantPriv) still gets a USAGE line.
        if gs.is_empty() && has_global_grant {
            gs.push(if current_priv & GrantPriv != 0 {
                format!("GRANT USAGE ON *.* TO '{user}'@'{host}' WITH GRANT OPTION")
            } else {
                format!("GRANT USAGE ON *.* TO '{user}'@'{host}'")
            });
        }

        // Db scope grants.
        let mut sort_from = gs.len();
        let mut db_priv_table: HashMap<String, PrivilegeType> = HashMap::new();
        for record in &self.db {
            if record.base.fullyMatch(user, host) {
                *db_priv_table.entry(record.DB.clone()).or_default() |= record.Privileges;
            } else {
                for role in &all_roles {
                    if record.base.r#match(&role.Username, &role.Hostname) {
                        *db_priv_table.entry(record.DB.clone()).or_default() |= record.Privileges;
                    }
                }
            }
        }
        for (db_name, p) in &db_priv_table {
            let escaped = escape_identifier(db_name, ansi_quotes);
            let g = dbPrivToString(*p);
            if !g.is_empty() {
                gs.push(if p & GrantPriv != 0 {
                    format!("GRANT {g} ON {escaped}.* TO '{user}'@'{host}' WITH GRANT OPTION")
                } else {
                    format!("GRANT {g} ON {escaped}.* TO '{user}'@'{host}'")
                });
            } else if p & GrantPriv != 0 {
                gs.push(format!(
                    "GRANT USAGE ON {escaped}.* TO '{user}'@'{host}' WITH GRANT OPTION"
                ));
            }
        }
        gs[sort_from..].sort();

        // Table scope grants.
        sort_from = gs.len();
        let mut table_priv_table: HashMap<String, PrivilegeType> = HashMap::new();
        for record in &self.tables_priv {
            let key = format!(
                "{}.{}",
                escape_identifier(&record.DB, ansi_quotes),
                escape_identifier(&record.TableName, ansi_quotes)
            );
            if record.base.User == user && record.base.Host == host {
                *table_priv_table.entry(key).or_default() |= record.TablePriv;
            } else {
                for role in &all_roles {
                    if record.base.r#match(&role.Username, &role.Hostname) {
                        *table_priv_table.entry(key.clone()).or_default() |= record.TablePriv;
                    }
                }
            }
        }
        for (key, p) in &table_priv_table {
            let g = tablePrivToString(*p);
            if !g.is_empty() {
                gs.push(if p & GrantPriv != 0 {
                    format!("GRANT {g} ON {key} TO '{user}'@'{host}' WITH GRANT OPTION")
                } else {
                    format!("GRANT {g} ON {key} TO '{user}'@'{host}'")
                });
            } else if p & GrantPriv != 0 {
                gs.push(format!(
                    "GRANT USAGE ON {key} TO '{user}'@'{host}' WITH GRANT OPTION"
                ));
            }
        }
        gs[sort_from..].sort();

        // Column scope grants (column and table are combined into one line).
        sort_from = gs.len();
        let mut column_priv_table: HashMap<String, HashMap<PrivilegeType, Vec<String>>> =
            HashMap::new();
        for record in &self.columns_priv {
            if !collectColumnGrant(record, user, host, &mut column_priv_table) {
                for role in &all_roles {
                    collectColumnGrant(
                        record,
                        &role.Username,
                        &role.Hostname,
                        &mut column_priv_table,
                    );
                }
            }
        }
        for (key, v) in &column_priv_table {
            let priv_cols = privOnColumnsToString(v);
            gs.push(format!("GRANT {priv_cols} ON {key} TO '{user}'@'{host}'"));
        }
        gs[sort_from..].sort();

        // Role grants.
        if let Some(edges) = self.role_graph.get(&RoleIdentity::new(user, host)) {
            let mut sorted: Vec<String> = edges
                .0
                .iter()
                .map(|k| format!("'{}'@'{}'", k.Username, k.Hostname))
                .collect();
            sorted.sort();
            gs.push(format!("GRANT {} TO '{user}'@'{host}'", sorted.join(", ")));
        }

        // If the SHOW GRANTS is for the current user, there might be
        // activeRoles (allRoles); merge the Dynamic privileges assigned to
        // the user with inherited dynamic privileges from those roles.
        let mut dynamic_privs_map: HashMap<String, bool> = HashMap::new();
        for record in self.dynamic_priv.iter().filter(|r| r.base.User == user) {
            if record.base.fullyMatch(user, host) {
                dynamic_privs_map.insert(record.PrivilegeName.clone(), record.GrantOption);
            }
        }
        for role in &all_roles {
            for record in self
                .dynamic_priv
                .iter()
                .filter(|r| r.base.User == role.Username)
            {
                if record.base.fullyMatch(&role.Username, &role.Hostname) {
                    // Skip if the map already has a grantable entry: don't
                    // clobber an existing privilege with a non-grantable one
                    // inherited from a role.
                    if dynamic_privs_map.get(&record.PrivilegeName) == Some(&true) {
                        continue;
                    }
                    dynamic_privs_map.insert(record.PrivilegeName.clone(), record.GrantOption);
                }
            }
        }
        let mut dynamic_privs: Vec<String> = Vec::new();
        let mut grantable_dynamic_privs: Vec<String> = Vec::new();
        for (priv_name, grantable) in &dynamic_privs_map {
            if *grantable {
                grantable_dynamic_privs.push(priv_name.clone());
            } else {
                dynamic_privs.push(priv_name.clone());
            }
        }
        if !dynamic_privs.is_empty() {
            dynamic_privs.sort();
            gs.push(format!(
                "GRANT {} ON *.* TO '{user}'@'{host}'",
                dynamic_privs.join(",")
            ));
        }
        if !grantable_dynamic_privs.is_empty() {
            grantable_dynamic_privs.sort();
            gs.push(format!(
                "GRANT {} ON *.* TO '{user}'@'{host}' WITH GRANT OPTION",
                grantable_dynamic_privs.join(",")
            ));
        }
        gs
    }
    /// 构造 USER_PRIVILEGES 风格结果行。
    pub fn UserPrivilegesTable(
        &self,
        active_roles: &[RoleIdentity],
        user: &str,
        host: &str,
    ) -> Vec<Vec<String>> {
        // Seeing all users requires SELECT on mysql.*. SUPER and unrelated
        // dynamic privileges do not grant this visibility.
        let show_other_users =
            self.RequestVerification(active_roles, user, host, "mysql", "", "", SelectPriv);
        let mut rows = Vec::new();
        for record in &self.user {
            if !show_other_users && !record.base.r#match(user, host) {
                continue;
            }
            let grantee = format!("'{}'@'{}'", record.base.User, record.base.Host);
            let grantable = if record.Privileges & GrantPriv != 0 {
                "YES"
            } else {
                "NO"
            };
            let mut appended = false;
            for privilege in ALL_GLOBAL_PRIVS {
                if record.Privileges & *privilege == 0 {
                    continue;
                }
                rows.push(vec![
                    grantee.clone(),
                    "def".into(),
                    privilege_name(*privilege).into(),
                    grantable.into(),
                ]);
                appended = true;
            }
            if !appended {
                rows.push(vec![grantee, "def".into(), "USAGE".into(), "NO".into()]);
            }
        }
        for record in &self.dynamic_priv {
            if !show_other_users && !record.base.r#match(user, host) {
                continue;
            }
            rows.push(vec![
                format!("'{}'@'{}'", record.base.User, record.base.Host),
                "def".into(),
                record.PrivilegeName.clone(),
                if record.GrantOption { "YES" } else { "NO" }.into(),
            ]);
        }
        rows
    }
    /// 获取用户默认角色。
    pub fn getDefaultRoles(&self, user: &str, host: &str) -> Vec<RoleIdentity> {
        self.default_roles
            .iter()
            .filter(|r| r.base.r#match(user, host))
            .map(|r| RoleIdentity::new(&r.DefaultRoleUser, &r.DefaultRoleHost))
            .collect()
    }
    /// Replace one account's default-role rows while preserving all other accounts.
    pub fn setDefaultRoles(&mut self, user: &str, host: &str, roles: &[RoleIdentity]) {
        self.default_roles
            .retain(|record| !record.base.fullyMatch(user, host));
        self.default_roles
            .extend(roles.iter().map(|role| defaultRoleRecord {
                base: baseRecord::new(host, user),
                DefaultRoleHost: role.Hostname.clone(),
                DefaultRoleUser: role.Username.clone(),
            }));
        self.default_roles.sort_by(compareDefaultRoleRecord);
    }
    /// 获取用户直接被授予的全部角色。
    pub fn getAllRoles(&self, user: &str, host: &str) -> Vec<RoleIdentity> {
        self.role_graph
            .get(&RoleIdentity::new(user, host))
            .map(|e| e.0.iter().cloned().collect())
            .unwrap_or_default()
    }
}

/// 比较 baseRecord：先主机特异性，再用户名。
pub fn compareBaseRecord(x: &baseRecord, y: &baseRecord) -> Ordering {
    // Go compares by host specificity first, then by user name (see
    // cache.go's compareBaseRecord); host must stay the primary key.
    compareHost(&x.Host, &y.Host).then_with(|| x.User.cmp(&y.User))
}
/// 比较 UserRecord（委托 compareBaseRecord）。
pub fn compareUserRecord(x: &UserRecord, y: &UserRecord) -> Ordering {
    compareBaseRecord(&x.base, &y.base)
}
/// 比较 defaultRoleRecord。
pub fn compareDefaultRoleRecord(x: &defaultRoleRecord, y: &defaultRoleRecord) -> Ordering {
    compareBaseRecord(&x.base, &y.base).then_with(|| x.DefaultRoleUser.cmp(&y.DefaultRoleUser))
}
/// 比较 globalPrivRecord。
pub fn compareGlobalPrivRecord(x: &globalPrivRecord, y: &globalPrivRecord) -> Ordering {
    compareBaseRecord(&x.base, &y.base)
}
/// 比较 dynamicPrivRecord。
pub fn compareDynamicPrivRecord(x: &dynamicPrivRecord, y: &dynamicPrivRecord) -> Ordering {
    compareBaseRecord(&x.base, &y.base).then_with(|| x.PrivilegeName.cmp(&y.PrivilegeName))
}
/// 比较 columnsPrivRecord。
pub fn compareColumnsPrivRecord(x: &columnsPrivRecord, y: &columnsPrivRecord) -> Ordering {
    compareBaseRecord(&x.base, &y.base)
        .then_with(|| x.DB.cmp(&y.DB))
        .then_with(|| x.TableName.cmp(&y.TableName))
        .then_with(|| x.ColumnName.cmp(&y.ColumnName))
}
/// 比较 dbRecord。
pub fn compareDBRecord(x: &dbRecord, y: &dbRecord) -> Ordering {
    compareBaseRecord(&x.base, &y.base).then_with(|| x.DB.cmp(&y.DB))
}
/// 比较 tablesPrivRecord。
pub fn compareTablesPrivRecord(x: &tablesPrivRecord, y: &tablesPrivRecord) -> Ordering {
    compareBaseRecord(&x.base, &y.base)
        .then_with(|| x.DB.cmp(&y.DB))
        .then_with(|| x.TableName.cmp(&y.TableName))
}

/// 按 Go 的历史规则比较主机特异性。
pub fn compareHost(x: &str, y: &str) -> Ordering {
    // Keep this deliberately aligned with Go's historical ordering rules.
    if x == "%" || y == "%" {
        return match (x == "%", y == "%") {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => unreachable!(),
        };
    }
    if x.is_empty() || y.is_empty() {
        return match (x.is_empty(), y.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => unreachable!(),
        };
    }
    let x_ends = x.ends_with('%');
    let y_ends = y.ends_with('%');
    if x_ends || y_ends {
        return match (x_ends, y_ends) {
            (false, true) => Ordering::Less,
            (true, false) => Ordering::Greater,
            (true, true) => y.len().cmp(&x.len()),
            (false, false) => unreachable!(),
        };
    }
    x.cmp(y)
}
/// 解析 `ip/mask` 形式的主机网段。
pub fn parseHostIPNet(value: &str) -> Option<(u32, u32)> {
    let (network, mask) = value.split_once('/')?;
    let network = u32::from(network.parse::<Ipv4Addr>().ok()?);
    let mask = u32::from(mask.parse::<Ipv4Addr>().ok()?);
    if network & mask != network {
        return None;
    }
    Some((network, mask))
}
/// 主机通配符匹配（`%`/`_`/`\`）。
pub fn patternMatch(value: &str, pattern_chars: &[u8], _pattern_types: &[u8]) -> bool {
    wildcard_match(value.as_bytes(), pattern_chars, false)
}
/// 忽略大小写的通配符匹配（用于库名）。
fn pattern_match_ci(value: &str, pattern: &str) -> bool {
    wildcard_match(
        value.to_ascii_lowercase().as_bytes(),
        pattern.to_ascii_lowercase().as_bytes(),
        false,
    )
}
/// 递归实现 `%`/`_` 通配与反斜杠转义。
fn wildcard_match(value: &[u8], pattern: &[u8], escaped: bool) -> bool {
    if pattern.is_empty() {
        return value.is_empty();
    }
    if !escaped && pattern[0] == b'\\' {
        return wildcard_match(value, &pattern[1..], true);
    }
    if !escaped && pattern[0] == b'%' {
        return wildcard_match(value, &pattern[1..], false)
            || (!value.is_empty() && wildcard_match(&value[1..], pattern, false));
    }
    if !value.is_empty()
        && ((!escaped && pattern[0] == b'_') || value[0].eq_ignore_ascii_case(&pattern[0]))
    {
        return wildcard_match(&value[1..], &pattern[1..], false);
    }
    false
}

/// 权限位到 SHOW GRANTS 显示名。
pub(crate) fn privilege_name(privilege: PrivilegeType) -> &'static str {
    match privilege {
        SelectPriv => "SELECT",
        InsertPriv => "INSERT",
        UpdatePriv => "UPDATE",
        DeletePriv => "DELETE",
        CreatePriv => "CREATE",
        DropPriv => "DROP",
        GrantPriv => "GRANT OPTION",
        IndexPriv => "INDEX",
        AlterPriv => "ALTER",
        CreateViewPriv => "CREATE VIEW",
        ShowViewPriv => "SHOW VIEW",
        OperateViewPriv => "OPERATE VIEW",
        TriggerPriv => "TRIGGER",
        ReferencesPriv => "REFERENCES",
        ExecutePriv => "EXECUTE",
        CreateTMPTablePriv => "CREATE TEMPORARY TABLES",
        SuperPriv => "SUPER",
        ProcessPriv => "PROCESS",
        ShutdownPriv => "SHUTDOWN",
        CreateUserPriv => "CREATE USER",
        ShowDBPriv => "SHOW DATABASES",
        CreateRolePriv => "CREATE ROLE",
        DropRolePriv => "DROP ROLE",
        LockTablesPriv => "LOCK TABLES",
        CreateRoutinePriv => "CREATE ROUTINE",
        AlterRoutinePriv => "ALTER ROUTINE",
        EventPriv => "EVENT",
        ReloadPriv => "RELOAD",
        FilePriv => "FILE",
        ConfigPriv => "CONFIG",
        ReplClientPriv => "REPLICATION CLIENT",
        ReplSlavePriv => "REPLICATION SLAVE",
        _ => "USAGE",
    }
}
/// 权限位到 `mysql.user`/`db` 列名（如 `Select_priv`）。
pub(crate) fn privilege_column_name(privilege: PrivilegeType) -> String {
    match privilege {
        LockTablesPriv => "Lock_tables_priv".into(),
        CreateTMPTablePriv => "Create_tmp_table_priv".into(),
        ReplClientPriv => "Repl_client_priv".into(),
        ReplSlavePriv => "Repl_slave_priv".into(),
        // These two diverge from `privilege_name`'s SHOW GRANTS display text
        // ("GRANT OPTION" / "SHOW DATABASES"): Go's mysql.Priv2UserCol maps
        // them to "Grant_priv" / "Show_db_priv" column names.
        GrantPriv => "Grant_priv".into(),
        ShowDBPriv => "Show_db_priv".into(),
        _ => format!(
            "{}_priv",
            privilege_name(privilege)
                .to_ascii_lowercase()
                .replace(' ', "_")
        ),
    }
}
// Matches Go's `PrivToString` (cache.go): unlike the earlier draft, this does
// NOT special-case zero privileges as "USAGE" -- callers (showGrants et al.)
// decide the "GRANT USAGE ..." fallback line themselves based on whether the
// joined string is empty, exactly like cache.go's showGrants does.
/// 移植 Go stringutil.Escape：按 ANSI_QUOTES 选择引号并转义标识符。
/// Ports Go's `stringutil.Escape` (identifier quoting for `SHOW GRANTS`
/// output): backtick-quoted by default, double-quoted under
/// `sql_mode & ANSI_QUOTES`, with the quote character doubled if present in
/// the identifier itself.
pub fn escape_identifier(name: &str, ansi_quotes: bool) -> String {
    let quote = if ansi_quotes { '"' } else { '`' };
    format!(
        "{quote}{}{quote}",
        name.replace(quote, &format!("{quote}{quote}"))
    )
}
/// 将权限掩码格式化为逗号分隔的特权名列表。
pub fn PrivToString(
    privileges: PrivilegeType,
    all_privileges: &[PrivilegeType],
    _names: &HashMap<PrivilegeType, String>,
) -> String {
    all_privileges
        .iter()
        .filter(|p| privileges & **p != 0)
        .map(|p| privilege_name(*p))
        .collect::<Vec<_>>()
        .join(",")
}
/// 计算特权列表的完整掩码。
fn priv_mask(all_privileges: &[PrivilegeType]) -> PrivilegeType {
    all_privileges.iter().fold(0, |mask, p| mask | *p)
}
/// SHOW GRANTS 中表示全部特权的字面量。
pub const AllPrivilegeLiteral: &str = "ALL PRIVILEGES";
/// 全局特权掩码转字符串（齐全则输出 ALL PRIVILEGES）。
pub fn userPrivToString(privileges: PrivilegeType) -> String {
    if privileges & !GrantPriv == priv_mask(ALL_GLOBAL_PRIVS) {
        return AllPrivilegeLiteral.into();
    }
    PrivToString(privileges, ALL_GLOBAL_PRIVS, &HashMap::new())
}
/// 库级特权掩码转字符串。
pub fn dbPrivToString(privileges: PrivilegeType) -> String {
    if privileges & !GrantPriv == priv_mask(ALL_DB_PRIVS) {
        return AllPrivilegeLiteral.into();
    }
    PrivToString(privileges, ALL_DB_PRIVS, &HashMap::new())
}
/// 表级特权掩码转字符串。
pub fn tablePrivToString(privileges: PrivilegeType) -> String {
    if privileges & !GrantPriv == priv_mask(ALL_TABLE_PRIVS) {
        return AllPrivilegeLiteral.into();
    }
    PrivToString(privileges, ALL_TABLE_PRIVS, &HashMap::new())
}
/// 列权限映射转 `PRIV (col,...)` 片段。
pub fn privOnColumnsToString(privileges: &HashMap<PrivilegeType, Vec<String>>) -> String {
    let mut entries: Vec<_> = privileges
        .iter()
        .map(|(p, c)| {
            format!(
                "{} ({})",
                privilege_name(*p),
                c.iter()
                    .map(|v| format!("`{v}`"))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .collect();
    entries.sort();
    entries.join(",")
}

#[derive(Clone, Debug)]
/// 权限缓存句柄：以读写锁保护 `MySQLPrivilege`。
pub struct Handle {
    data: Arc<RwLock<MySQLPrivilege>>,
    active_users: Arc<RwLock<HashSet<String>>>,
    full_data: Arc<AtomicBool>,
    source_data: Arc<RwLock<MySQLPrivilege>>,
}
impl Handle {
    /// 创建空句柄。
    pub fn New() -> Self {
        Self {
            data: Arc::new(RwLock::new(MySQLPrivilege::default())),
            active_users: Arc::new(RwLock::new(HashSet::new())),
            full_data: Arc::new(AtomicBool::new(false)),
            source_data: Arc::new(RwLock::new(MySQLPrivilege::default())),
        }
    }
    /// 克隆当前缓存快照。
    pub fn Get(&self) -> MySQLPrivilege {
        self.data.read().unwrap().clone()
    }
    /// 从数据源全量刷新缓存。
    pub fn UpdateAll(&self, source: &dyn PrivilegeDataSource) -> Result<(), PrivilegeError> {
        let mut next = MySQLPrivilege::default();
        next.LoadAll(source)?;
        *self.source_data.write().unwrap() = next.clone();
        *self.data.write().unwrap() = next;
        self.full_data.store(true, AtomicOrdering::Release);
        Ok(())
    }
    /// 刷新全部活动用户的权限，并切换到按需加载模式。
    pub fn UpdateAllActive(&self, source: &dyn PrivilegeDataSource) -> Result<(), PrivilegeError> {
        let mut next = MySQLPrivilege::default();
        next.LoadAll(source)?;
        *self.source_data.write().unwrap() = next.clone();
        self.full_data.store(false, AtomicOrdering::Release);
        let active: Vec<String> = self.active_users.read().unwrap().iter().cloned().collect();
        let users = findUserAndAllRoles(&active, &next.role_graph);
        let merged = self.data.read().unwrap().merge(&next, &users);
        *self.data.write().unwrap() = merged;
        Ok(())
    }
    /// 仅当列表包含活动用户时刷新该用户及其角色权限。
    pub fn Update(
        &self,
        users: &[String],
        source: &dyn PrivilegeDataSource,
    ) -> Result<(), PrivilegeError> {
        let mut next = MySQLPrivilege::default();
        next.LoadAll(source)?;
        *self.source_data.write().unwrap() = next.clone();
        self.full_data.store(false, AtomicOrdering::Release);
        let active = self.active_users.read().unwrap();
        if !users.iter().any(|user| active.contains(user)) {
            return Ok(());
        }
        drop(active);
        let users = findUserAndAllRoles(users, &next.role_graph);
        let merged = self.data.read().unwrap().merge(&next, &users);
        *self.data.write().unwrap() = merged;
        Ok(())
    }
    /// `Update` 的别名入口。
    pub fn updateUsers(
        &self,
        users: &[String],
        source: &dyn PrivilegeDataSource,
    ) -> Result<(), PrivilegeError> {
        self.Update(users, source)
    }
    /// 直接替换内部缓存数据。
    pub fn merge(&self, data: MySQLPrivilege) {
        *self.source_data.write().unwrap() = data.clone();
        *self.data.write().unwrap() = data;
        self.full_data.store(true, AtomicOrdering::Release);
    }
    /// 确保用户及其全部角色权限已按需加载。
    pub fn ensureActiveUser(&self, user: &str) -> Result<(), PrivilegeError> {
        if self.full_data.load(AtomicOrdering::Acquire)
            || self.active_users.read().unwrap().contains(user)
        {
            return Ok(());
        }
        let source = self.source_data.read().unwrap().clone();
        let users = findUserAndAllRoles(&[user.to_string()], &source.role_graph);
        let merged = self.data.read().unwrap().merge(&source, &users);
        *self.data.write().unwrap() = merged;
        self.active_users.write().unwrap().insert(user.to_string());
        Ok(())
    }

    /// 测试/诊断：当前是否缓存了全部用户。
    pub fn CheckFullData(&self) -> bool {
        self.full_data.load(AtomicOrdering::Acquire)
    }
}
impl Default for Handle {
    fn default() -> Self {
        Self::New()
    }
}
/// 创建权限缓存句柄。
pub fn NewHandle() -> Handle {
    Handle::New()
}

/// 为加载 SQL 追加 `WHERE User IN (...)` 过滤。
pub fn addUserFilterCondition(sql: &str, users: &HashSet<String>) -> String {
    if users.is_empty() {
        return sql.into();
    }
    let mut users: Vec<_> = users
        .iter()
        .map(|u| format!("'{}'", u.replace('\'', "''")))
        .collect();
    users.sort();
    format!("{sql} WHERE User IN ({})", users.join(","))
}
/// 从角色图收集用户及其全部间接角色用户名。
pub fn findUserAndAllRoles(
    users: &[String],
    graph: &HashMap<RoleIdentity, roleGraphEdgesTable>,
) -> HashSet<String> {
    let mut out: HashSet<_> = users.iter().cloned().collect();
    let mut queue: VecDeque<_> = graph
        .iter()
        .filter(|(id, _)| out.contains(&id.Username))
        .flat_map(|(_, e)| e.0.iter().cloned())
        .collect();
    while let Some(role) = queue.pop_front() {
        if out.insert(role.Username.clone()) {
            if let Some(edges) = graph.get(&role) {
                queue.extend(edges.0.iter().cloned());
            }
        }
    }
    out
}
/// 判断错误是否为权限系统表不存在。
pub fn noSuchTable(error: &PrivilegeError) -> bool {
    matches!(error, PrivilegeError::NoSuchTable(_))
}
/// 将 Table_priv/Column_priv 的 SET 枚举字面量映射为权限位。
/// setStrToPrivilege maps a Table_priv/Column_priv SET-enum literal (Go's
/// `mysql.SetStr2Priv`) to a privilege bit. This is intentionally distinct
/// from `privilege_name`, which returns the SHOW GRANTS display text (e.g.
/// "GRANT OPTION" vs. the SET literal "Grant").
fn setStrToPrivilege(name: &str) -> PrivilegeType {
    match name {
        "Create" => CreatePriv,
        "Select" => SelectPriv,
        "Insert" => InsertPriv,
        "Update" => UpdatePriv,
        "Delete" => DeletePriv,
        "Drop" => DropPriv,
        "Grant" => GrantPriv,
        "References" => ReferencesPriv,
        "Lock Tables" => LockTablesPriv,
        "Create Temporary Tables" => CreateTMPTablePriv,
        "Event" => EventPriv,
        "Create Routine" => CreateRoutinePriv,
        "Alter Routine" => AlterRoutinePriv,
        "Alter" => AlterPriv,
        "Execute" => ExecutePriv,
        "Index" => IndexPriv,
        "Create View" => CreateViewPriv,
        "Show View" => ShowViewPriv,
        "Operate View" => OperateViewPriv,
        "Trigger" => TriggerPriv,
        _ => 0,
    }
}
/// 将 SET 名列表折叠为权限掩码。
pub fn decodeSetToPrivilege(names: &[String]) -> PrivilegeType {
    names
        .iter()
        .fold(0, |mask, name| mask | setStrToPrivilege(name))
}

/// 将缓存权限位按 MySQL SET 列的规范顺序编码为枚举名称。
pub fn EncodePrivilegeSet(mask: PrivilegeType, allowed: &[PrivilegeType]) -> Vec<String> {
    allowed
        .iter()
        .copied()
        .filter(|privilege| mask & privilege != 0)
        .filter_map(|privilege| {
            [
                "Select",
                "Insert",
                "Update",
                "Delete",
                "Create",
                "Drop",
                "Grant",
                "References",
                "Lock Tables",
                "Create Temporary Tables",
                "Event",
                "Create Routine",
                "Alter Routine",
                "Alter",
                "Execute",
                "Index",
                "Create View",
                "Show View",
                "Trigger",
            ]
            .into_iter()
            .find(|name| setStrToPrivilege(name) == privilege)
            .map(str::to_owned)
        })
        .collect()
}

/// 通过句柄查询角色边是否存在。
pub fn findRole(handle: &Handle, user: &str, host: &str, role: &RoleIdentity) -> bool {
    handle.Get().FindRole(user, host, role)
}

/// 权限系统表一行的列名→JSON 值映射（测试/加载用）。
pub type PrivilegeRow = HashMap<String, serde_json::Value>;

impl baseRecord {
    /// 从行中填充 User/Host 字段。
    pub fn assignUserOrHost(&mut self, row: &PrivilegeRow) {
        if let Some(user) = row.get("user").and_then(serde_json::Value::as_str) {
            self.User = user.into();
        }
        if let Some(host) = row.get("host").and_then(serde_json::Value::as_str) {
            self.Host = host.into();
        }
    }
}

impl MySQLPrivilege {
    /// 解码 `mysql.user` 一行并追加到缓存。
    pub fn decodeUserTableRow(&mut self, row: &PrivilegeRow) -> Result<(), PrivilegeError> {
        let mut record = NewUserRecord("", "");
        record.base.assignUserOrHost(row);
        record.AuthenticationString = row_text(row, "authentication_string");
        record.AuthPlugin = {
            let p = row_text(row, "plugin");
            if p.is_empty() {
                if self.default_auth_plugin.is_empty() {
                    "mysql_native_password".into()
                } else {
                    self.default_auth_plugin.clone()
                }
            } else {
                p
            }
        };
        record.AccountLocked = row_yes(row, "account_locked");
        record.PasswordExpired = row_yes(row, "password_expired");
        record.AuthTokenIssuer = row_text(row, "token_issuer");
        record.MaxUserConnections = row_int(row, "max_user_connections");
        record.PasswordLastChanged = row_int(row, "password_last_changed");
        record.PasswordLifeTime = match row.get("password_lifetime") {
            None => -1,
            Some(value) => value.as_i64().unwrap_or(-1),
        };
        record.Privileges = privilege_columns(row);
        // Go decodes email/resource_group/additional_password/password-locking from the
        // `user_attributes` JSON column (see cache.go decodeUserTableRow); our row carries
        // that same JSON blob under the "user_attributes" key.
        if let Some(attributes) = row.get("user_attributes") {
            if let Some(email) = attributes
                .get("metadata")
                .and_then(|metadata| metadata.get("email"))
                .and_then(serde_json::Value::as_str)
            {
                record.UserAttributesInfo.MetadataInfo.Email = email.into();
            }
            if let Some(group) = attributes
                .get("resource_group")
                .and_then(serde_json::Value::as_str)
            {
                record.ResourceGroup = group.into();
            }
            if let Some(additional) = attributes
                .get("additional_password")
                .and_then(serde_json::Value::as_str)
            {
                record.AdditionalAuthString = additional.into();
            }
            record
                .UserAttributesInfo
                .PasswordLocking
                .ParseJSON(attributes)?;
        }
        self.user.push(record);
        Ok(())
    }
    /// 解码 `mysql.global_priv` 一行。
    pub fn decodeGlobalPrivTableRow(&mut self, row: &PrivilegeRow) -> Result<(), PrivilegeError> {
        let mut base = baseRecord::default();
        base.assignUserOrHost(row);
        let mut broken = false;
        let mut priv_value = GlobalPrivValue::default();
        match row.get("priv") {
            None => {}
            Some(value) if value.is_object() => {
                priv_value.SSLType = match value
                    .get("ssl_type")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(-1)
                {
                    0 => SslTypeNone,
                    1 => SslTypeAny,
                    2 => SslTypeX509,
                    3 => SslTypeSpecified,
                    _ => SslTypeNotSpecified,
                };
                priv_value.SSLCipher = json_text(value, "ssl_cipher");
                priv_value.X509Issuer = json_text(value, "x509_issuer")
                    .trim_start_matches('\\')
                    .into();
                priv_value.X509Subject = json_text(value, "x509_subject")
                    .trim_start_matches('\\')
                    .into();
                priv_value.SAN = json_text(value, "san");
                if !priv_value.SAN.is_empty() {
                    match astersql_util::misc::ParseAndCheckSAN(&priv_value.SAN) {
                        Ok(sans) => priv_value.SANs = sans,
                        Err(_) => broken = true,
                    }
                }
            }
            Some(_) => broken = true,
        }
        self.global_priv.push(globalPrivRecord {
            base,
            Priv: priv_value,
            Broken: broken,
        });
        Ok(())
    }
    /// 解码 `mysql.global_grants` 一行。
    pub fn decodeGlobalGrantsTableRow(&mut self, row: &PrivilegeRow) -> Result<(), PrivilegeError> {
        let mut base = baseRecord::default();
        base.assignUserOrHost(row);
        self.dynamic_priv.push(dynamicPrivRecord {
            base,
            PrivilegeName: row_text(row, "priv").to_ascii_uppercase(),
            GrantOption: row_yes(row, "with_grant_option"),
        });
        Ok(())
    }
    /// 解码 `mysql.db` 一行。
    pub fn decodeDBTableRow(&mut self, row: &PrivilegeRow) -> Result<(), PrivilegeError> {
        let mut base = baseRecord::default();
        base.assignUserOrHost(row);
        self.db.push(dbRecord {
            base,
            DB: row_text(row, "db"),
            Privileges: privilege_columns(row),
        });
        Ok(())
    }
    /// 解码 `mysql.tables_priv` 一行。
    pub fn decodeTablesPrivTableRow(&mut self, row: &PrivilegeRow) -> Result<(), PrivilegeError> {
        let mut base = baseRecord::default();
        base.assignUserOrHost(row);
        self.tables_priv.push(tablesPrivRecord {
            base,
            DB: row_text(row, "db"),
            TableName: row_text(row, "table_name"),
            Grantor: row_text(row, "grantor"),
            TablePriv: set_privileges(row.get("table_priv")),
            ColumnPriv: set_privileges(row.get("column_priv")),
            ..Default::default()
        });
        Ok(())
    }
    /// 解码角色边并写入角色图。
    pub fn decodeRoleEdgesTable(&mut self, row: &PrivilegeRow) -> Result<(), PrivilegeError> {
        let user = RoleIdentity::new(row_text(row, "to_user"), row_text(row, "to_host"));
        let role = RoleIdentity::new(row_text(row, "from_user"), row_text(row, "from_host"));
        self.role_graph.entry(user).or_default().0.insert(role);
        Ok(())
    }
    /// 解码 `mysql.default_roles` 一行。
    pub fn decodeDefaultRoleTableRow(&mut self, row: &PrivilegeRow) -> Result<(), PrivilegeError> {
        let mut base = baseRecord::default();
        base.assignUserOrHost(row);
        self.default_roles.push(defaultRoleRecord {
            base,
            DefaultRoleHost: row_text(row, "default_role_host"),
            DefaultRoleUser: row_text(row, "default_role_user"),
        });
        Ok(())
    }
    /// 解码 `mysql.columns_priv` 一行。
    pub fn decodeColumnsPrivTableRow(&mut self, row: &PrivilegeRow) -> Result<(), PrivilegeError> {
        let mut base = baseRecord::default();
        base.assignUserOrHost(row);
        self.columns_priv.push(columnsPrivRecord {
            base,
            DB: row_text(row, "db"),
            TableName: row_text(row, "table_name"),
            ColumnName: row_text(row, "column_name"),
            ColumnPriv: set_privileges(row.get("column_priv")),
            ..Default::default()
        });
        Ok(())
    }
}

/// 读取行中字符串列，缺失则为空串。
fn row_text(row: &PrivilegeRow, key: &str) -> String {
    row.get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .into()
}
/// 读取行中整型列，缺失则为 0。
fn row_int(row: &PrivilegeRow, key: &str) -> i64 {
    row.get(key)
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0)
}
/// 读取 `'Y'`/true 布尔列。
fn row_yes(row: &PrivilegeRow, key: &str) -> bool {
    row.get(key).is_some_and(|v| v == "Y" || v == true)
}
/// 从 JSON 对象读取字符串字段。
fn json_text(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .into()
}
/// 解析逗号分隔的 SET 特权字符串。
fn set_privileges(value: Option<&serde_json::Value>) -> PrivilegeType {
    value
        .and_then(serde_json::Value::as_str)
        .map(|s| {
            decodeSetToPrivilege(
                &s.split(',')
                    .map(|v| v.trim().to_owned())
                    .collect::<Vec<_>>(),
            )
        })
        .unwrap_or(0)
}
/// 扫描 user/db 行中全部 `*_priv` 列（含 GrantPriv）。
/// Column-scan superset for `mysql.user`/`mysql.db` `*_priv` fields. Matches
/// Go's `mysql.Priv2UserCol` (privs.go), which is `ALL_GLOBAL_PRIVS` plus
/// `GrantPriv` (`GrantPriv` is excluded from `ALL_GLOBAL_PRIVS` itself since
/// that list drives `SHOW GRANTS` display text, where GRANT OPTION is shown
/// via a `WITH GRANT OPTION` suffix rather than as a plain privilege name).
fn privilege_scan_columns() -> impl Iterator<Item = PrivilegeType> {
    ALL_GLOBAL_PRIVS
        .iter()
        .copied()
        .chain(std::iter::once(GrantPriv))
}
/// 从行的 `*_priv='Y'` 列合成权限掩码。
fn privilege_columns(row: &PrivilegeRow) -> PrivilegeType {
    privilege_scan_columns()
        .filter(|p| row_yes(row, &privilege_column_name(*p).to_ascii_lowercase()))
        .fold(0, |a, p| a | p)
}

/// 将 `mysql.user`/`mysql.db` 权限列名转换为缓存使用的静态权限位。
pub fn DecodePrivilegeColumns(columns: &[String]) -> PrivilegeType {
    privilege_scan_columns()
        .filter(|privilege| {
            let expected = privilege_column_name(*privilege);
            columns
                .iter()
                .any(|column| column.eq_ignore_ascii_case(&expected))
        })
        .fold(0, |mask, privilege| mask | privilege)
}

/// 对多行依次调用解码闭包。
pub fn loadTable(
    rows: &[PrivilegeRow],
    mut decode: impl FnMut(&PrivilegeRow) -> Result<(), PrivilegeError>,
) -> Result<(), PrivilegeError> {
    for row in rows {
        decode(row)?
    }
    Ok(())
}
/// 字典序比较用户项（测试辅助）。
pub fn compareItemUser(a: &str, b: &str) -> bool {
    a < b
}
/// 字典序比较库项（测试辅助）。
pub fn compareItemDB(a: &str, b: &str) -> bool {
    a < b
}
/// 字典序比较表权限项（测试辅助）。
pub fn compareItemTablesPriv(a: &str, b: &str) -> bool {
    a < b
}
/// 字典序比较列权限项（测试辅助）。
pub fn compareItemColumnsPriv(a: &str, b: &str) -> bool {
    a < b
}
/// 字典序比较默认角色项（测试辅助）。
pub fn compareItemDefaultRole(a: &str, b: &str) -> bool {
    a < b
}
/// 字典序比较 global_priv 项（测试辅助）。
pub fn compareItemGlobalPriv(a: &str, b: &str) -> bool {
    a < b
}
/// 字典序比较动态权限项（测试辅助）。
pub fn compareItemDynamicPriv(a: &str, b: &str) -> bool {
    a < b
}
/// 若记录匹配用户，则把列权限归并到 SHOW GRANTS 表映射。
pub fn collectColumnGrant(
    record: &columnsPrivRecord,
    user: &str,
    host: &str,
    table: &mut HashMap<String, HashMap<PrivilegeType, Vec<String>>>,
) -> bool {
    if !record.base.r#match(user, host) {
        return false;
    }
    let entry = table
        .entry(format!("{}.{}", record.DB, record.TableName))
        .or_default();
    for privilege in ALL_TABLE_PRIVS
        .iter()
        .copied()
        .filter(|p| record.ColumnPriv & *p != 0)
    {
        entry
            .entry(privilege)
            .or_default()
            .push(record.ColumnName.clone());
    }
    true
}
/// 追加动态权限到 USER_PRIVILEGES 风格行集。
pub fn appendDynamicPrivRecord(
    mut rows: Vec<Vec<String>>,
    record: &dynamicPrivRecord,
) -> Vec<Vec<String>> {
    rows.push(vec![
        format!("'{}'@'{}'", record.base.User, record.base.Host),
        "def".into(),
        record.PrivilegeName.clone(),
        if record.GrantOption { "YES" } else { "NO" }.into(),
    ]);
    rows
}
/// 将用户静态权限展开为 USER_PRIVILEGES 行。
pub fn appendUserPrivilegesTableRow(
    mut rows: Vec<Vec<String>>,
    record: &UserRecord,
) -> Vec<Vec<String>> {
    let grantable = if record.Privileges & GrantPriv != 0 {
        "YES"
    } else {
        "NO"
    };
    if record.Privileges == 0 {
        rows.push(vec![
            format!("'{}'@'{}'", record.base.User, record.base.Host),
            "def".into(),
            "USAGE".into(),
            "NO".into(),
        ])
    } else {
        for privilege in ALL_GLOBAL_PRIVS
            .iter()
            .copied()
            .filter(|p| record.Privileges & *p != 0)
        {
            rows.push(vec![
                format!("'{}'@'{}'", record.base.User, record.base.Host),
                "def".into(),
                privilege_name(privilege).into(),
                grantable.into(),
            ])
        }
    }
    rows
}
