// Copyright 2026 AsterSQL.
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

// 权限管理接口：定义会话绑定的 `Manager` trait 与校验信息结构。
//
// 对应 Go 的 `privilege` 包入口。会话通过 `BindPrivilegeManager` /
// `GetPrivilegeManager` 挂载实现；上层在登录、鉴权、SHOW GRANTS、库可见性等
// 路径调用本接口，而不直接依赖具体缓存实现（如 `privileges` crate）。

#![allow(dead_code, non_snake_case, non_upper_case_globals, unused_variables)]

use std::fmt;
use std::sync::Arc;

pub use crate::parser::auth::{RoleIdentity, UserIdentity};
pub use crate::parser::mysql::PrivilegeType;
pub use crate::privilege::conn::AuthConn;
pub use crate::sessionctx::ExecutionContext as Context;
pub use crate::sessionctx::variable::SessionVars;
pub use crate::types::Datum;
pub use crate::util::sqlexec::{
    GoError as PrivilegeError, RestrictedSQLExecutor as RestrictedSqlExecutor,
};

/// 会话上下文中存放权限 Manager 的键类型。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct KeyType(i32);

impl KeyType {
    /// 返回会话键的字符串表示（固定为 `"privilege-key"`）。
    pub fn String(self) -> &'static str {
        "privilege-key"
    }

    /// `String` 的小写别名，兼容 Go 命名习惯。
    pub fn string(self) -> &'static str {
        self.String()
    }
}

impl fmt::Display for KeyType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.String())
    }
}

/// 连接校验结果：是否沙箱模式、是否因密码错误失败，以及匹配到的资源组名。
/// Information returned by [`Manager::ConnectionVerification`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VerificationInfo {
    /// 是否进入沙箱模式（例如要求先改密码）。
    pub InSandBoxMode: bool,
    /// 校验失败是否因密码错误。
    pub FailedDueToWrongPassword: bool,
    /// 用户绑定的资源组名称。
    pub ResourceGroupName: String,
}

/// 权限 Manager：SHOW GRANTS、静态/动态权限校验、登录鉴权、角色图查询等。
/// Provides all privilege operations exposed by the Go `Manager` interface.
pub trait Manager {
    /// 生成指定用户/角色的 GRANT 语句列表。
    fn ShowGrants(
        &self,
        ctx: &Context,
        sctx: &dyn SessionContext,
        user: &UserIdentity,
        roles: &[RoleIdentity],
    ) -> Result<Vec<String>, PrivilegeError>;
    /// 校验当前活动角色是否具备指定静态权限（库/表/列级）。
    fn RequestVerification(
        &self,
        active_role: &[RoleIdentity],
        db: &str,
        table: &str,
        column: &str,
        priv_type: PrivilegeType,
    ) -> bool;
    /// 以显式用户身份校验静态权限。
    fn RequestVerificationWithUser(
        &self,
        ctx: &Context,
        db: &str,
        table: &str,
        column: &str,
        priv_type: PrivilegeType,
        user: &UserIdentity,
    ) -> bool;
    /// 是否被显式授予指定动态权限（不含 SUPER 回退）。
    fn HasExplicitlyGrantedDynamicPrivilege(
        &self,
        active_roles: &[RoleIdentity],
        priv_name: &str,
        grantable: bool,
    ) -> bool;
    /// 校验动态权限；必要时可回退到 SUPER。
    fn RequestDynamicVerification(
        &self,
        active_roles: &[RoleIdentity],
        priv_name: &str,
        grantable: bool,
    ) -> bool;
    /// 以显式用户身份校验动态权限。
    fn RequestDynamicVerificationWithUser(
        &self,
        ctx: &Context,
        priv_name: &str,
        grantable: bool,
        user: &UserIdentity,
    ) -> bool;
    /// 检查内存中的自动账户锁定状态。
    fn VerifyAccountAutoLockInMemory(&self, user: &str, host: &str)
    -> Result<bool, PrivilegeError>;
    /// 账户是否启用了自动锁定策略。
    fn IsAccountAutoLockEnabled(&self, user: &str, host: &str) -> bool;
    /// 连接登录鉴权，返回沙箱/资源组等校验信息。
    fn ConnectionVerification(
        &self,
        user: &UserIdentity,
        auth_user: &str,
        auth_host: &str,
        auth: &[u8],
        salt: &[u8],
        session_vars: &SessionVars,
        auth_conn: &mut dyn AuthConn<Context = Context, Error = PrivilegeError>,
    ) -> Result<VerificationInfo, PrivilegeError>;
    /// 鉴权成功后的回调（清除失败计数等）。
    fn AuthSuccess(&self, auth_user: &str, auth_host: &str);
    /// 是否允许跳过密码校验的认证路径。
    fn GetAuthWithoutVerification(&self, user: &str, host: &str) -> bool;
    /// 按用户名与主机模式匹配身份，返回规范化的 user/host。
    fn MatchIdentity(
        &self,
        ctx: &Context,
        user: &str,
        host: &str,
        skip_name_resolve: bool,
    ) -> (String, String, bool);
    /// 匹配用户资源组名称是否存在。
    fn MatchUserResourceGroupName(
        &self,
        exec: &mut dyn RestrictedSqlExecutor,
        resource_group_name: &str,
    ) -> (String, bool);
    /// 判断活动角色下指定库是否可见。
    fn DBIsVisible(&self, active_role: &[RoleIdentity], db: &str) -> bool;
    /// 构造 INFORMATION_SCHEMA.USER_PRIVILEGES 风格行。
    fn UserPrivilegesTable(
        &self,
        active_roles: &[RoleIdentity],
        user: &str,
        host: &str,
    ) -> Vec<Vec<Datum>>;
    /// 激活角色列表，返回是否成功与错误信息。
    fn ActiveRoles(
        &self,
        ctx: &Context,
        sctx: &dyn SessionContext,
        role_list: &[RoleIdentity],
    ) -> (bool, String);
    /// 角色图中是否存在 user→role 边。
    fn FindEdge(&self, ctx: &Context, role: &RoleIdentity, user: &UserIdentity) -> bool;
    /// 获取用户的默认角色列表。
    fn GetDefaultRoles(&self, ctx: &Context, user: &str, host: &str) -> Vec<RoleIdentity>;
    /// 获取用户被授予的全部角色。
    fn GetAllRoles(&self, user: &str, host: &str) -> Vec<RoleIdentity>;
    /// 判断权限名是否为动态权限。
    fn IsDynamicPrivilege(&self, priv_name: &str) -> bool;
    /// 获取连接应使用的认证插件名。
    fn GetAuthPluginForConnection(
        &self,
        ctx: &Context,
        user: &str,
        host: &str,
    ) -> Result<String, PrivilegeError>;
    /// 获取用户的最大连接数等资源限制。
    fn GetUserResources(&self, user: &str, host: &str) -> Result<i64, PrivilegeError>;
}

/// 会话上下文中权限 Manager 的默认键值。
pub const KEY: KeyType = KeyType(0);

/// 将权限 Manager 绑定到会话上下文（可传 `None` 清除）。
/// Binds a privilege manager to a session context.
pub fn BindPrivilegeManager(ctx: &mut dyn SessionContext, manager: Option<Arc<dyn Manager>>) {
    ctx.set_value(KEY, manager);
}

/// 按键读取会话中保存的权限 Manager。
/// Provides the value stored for the package-private privilege key.
pub trait PrivilegeManagerKeyProvider {
    fn value(&self, key: KeyType) -> Option<Arc<dyn Manager>>;
}

/// 会话上下文：支持按键写入权限 Manager。
/// Session context operations required by manager binding and lookup.
pub trait SessionContext: PrivilegeManagerKeyProvider {
    fn set_value(&mut self, key: KeyType, value: Option<Arc<dyn Manager>>);
}

/// 从会话上下文取出已绑定的权限 Manager。
/// Gets the privilege manager bound to a session context.
pub fn GetPrivilegeManager(ctx: &dyn PrivilegeManagerKeyProvider) -> Option<Arc<dyn Manager>> {
    ctx.value(KEY)
}
