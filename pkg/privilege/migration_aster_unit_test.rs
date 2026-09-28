// Copyright 2026 AsterSQL.

// 权限 Manager 绑定与 VerificationInfo 零值的迁移对照单元测试。
//
// 用最小 `TestManager` / `TestSessionContext` 桩实现校验：
// KEY 字符串化、VerificationInfo 默认值，以及 Bind/Get/替换/解绑往返。

use std::collections::HashMap;
use std::sync::Arc;

use astersql_privilege::{
    BindPrivilegeManager, Context, Datum, GetPrivilegeManager, KEY, KeyType, Manager,
    PrivilegeError, PrivilegeManagerKeyProvider, PrivilegeType, RestrictedSqlExecutor,
    RoleIdentity, SessionContext, SessionVars, UserIdentity, VerificationInfo,
};

/// 可配置 allow 标志的 Manager 桩，多数方法原样返回该标志或空集合。
struct TestManager {
    allow: bool,
}

impl Manager for TestManager {
    fn ShowGrants(
        &self,
        _ctx: &Context,
        _sctx: &dyn SessionContext,
        _user: &UserIdentity,
        _roles: &[RoleIdentity],
    ) -> Result<Vec<String>, PrivilegeError> {
        Ok(Vec::new())
    }

    fn RequestVerification(
        &self,
        _active_role: &[RoleIdentity],
        _db: &str,
        _table: &str,
        _column: &str,
        _priv_type: PrivilegeType,
    ) -> bool {
        self.allow
    }

    fn RequestVerificationWithUser(
        &self,
        _ctx: &Context,
        _db: &str,
        _table: &str,
        _column: &str,
        _priv_type: PrivilegeType,
        _user: &UserIdentity,
    ) -> bool {
        self.allow
    }

    fn HasExplicitlyGrantedDynamicPrivilege(
        &self,
        _active_roles: &[RoleIdentity],
        _priv_name: &str,
        _grantable: bool,
    ) -> bool {
        self.allow
    }

    fn RequestDynamicVerification(
        &self,
        _active_roles: &[RoleIdentity],
        _priv_name: &str,
        _grantable: bool,
    ) -> bool {
        self.allow
    }

    fn RequestDynamicVerificationWithUser(
        &self,
        _ctx: &Context,
        _priv_name: &str,
        _grantable: bool,
        _user: &UserIdentity,
    ) -> bool {
        self.allow
    }

    fn VerifyAccountAutoLockInMemory(
        &self,
        _user: &str,
        _host: &str,
    ) -> Result<bool, PrivilegeError> {
        Ok(self.allow)
    }

    fn IsAccountAutoLockEnabled(&self, _user: &str, _host: &str) -> bool {
        self.allow
    }

    fn ConnectionVerification(
        &self,
        _user: &UserIdentity,
        _auth_user: &str,
        _auth_host: &str,
        _auth: &[u8],
        _salt: &[u8],
        _session_vars: &SessionVars,
        _auth_conn: &mut dyn astersql_privilege::AuthConn<Context = Context, Error = PrivilegeError>,
    ) -> Result<VerificationInfo, PrivilegeError> {
        Ok(VerificationInfo::default())
    }

    fn AuthSuccess(&self, _auth_user: &str, _auth_host: &str) {}

    fn GetAuthWithoutVerification(&self, _user: &str, _host: &str) -> bool {
        self.allow
    }

    fn MatchIdentity(
        &self,
        _ctx: &Context,
        user: &str,
        host: &str,
        _skip_name_resolve: bool,
    ) -> (String, String, bool) {
        (user.to_owned(), host.to_owned(), self.allow)
    }

    fn MatchUserResourceGroupName(
        &self,
        _exec: &mut dyn RestrictedSqlExecutor,
        resource_group_name: &str,
    ) -> (String, bool) {
        (resource_group_name.to_owned(), self.allow)
    }

    fn DBIsVisible(&self, _active_role: &[RoleIdentity], _db: &str) -> bool {
        self.allow
    }

    fn UserPrivilegesTable(
        &self,
        _active_roles: &[RoleIdentity],
        _user: &str,
        _host: &str,
    ) -> Vec<Vec<Datum>> {
        Vec::new()
    }

    fn ActiveRoles(
        &self,
        _ctx: &Context,
        _sctx: &dyn SessionContext,
        _role_list: &[RoleIdentity],
    ) -> (bool, String) {
        (self.allow, String::new())
    }

    fn FindEdge(&self, _ctx: &Context, _role: &RoleIdentity, _user: &UserIdentity) -> bool {
        self.allow
    }

    fn GetDefaultRoles(&self, _ctx: &Context, _user: &str, _host: &str) -> Vec<RoleIdentity> {
        Vec::new()
    }

    fn GetAllRoles(&self, _user: &str, _host: &str) -> Vec<RoleIdentity> {
        Vec::new()
    }

    fn IsDynamicPrivilege(&self, _priv_name: &str) -> bool {
        self.allow
    }

    fn GetAuthPluginForConnection(
        &self,
        _ctx: &Context,
        _user: &str,
        _host: &str,
    ) -> Result<String, PrivilegeError> {
        Ok(String::new())
    }

    fn GetUserResources(&self, _user: &str, _host: &str) -> Result<i64, PrivilegeError> {
        Ok(0)
    }
}

/// 以 HashMap 存储 privilege-key → Manager 的会话上下文桩。
#[derive(Default)]
struct TestSessionContext {
    values: HashMap<KeyType, Arc<dyn Manager>>,
}

impl PrivilegeManagerKeyProvider for TestSessionContext {
    fn value(&self, key: KeyType) -> Option<Arc<dyn Manager>> {
        self.values.get(&key).cloned()
    }
}

impl SessionContext for TestSessionContext {
    fn set_value(&mut self, key: KeyType, value: Option<Arc<dyn Manager>>) {
        // None 表示解绑：从 map 中移除对应 key。
        match value {
            Some(value) => {
                self.values.insert(key, value);
            }
            None => {
                self.values.remove(&key);
            }
        }
    }
}

/// 确认 KEY 的 Stringer / Display 与 Go `"privilege-key"` 一致。
#[test]
fn privilege_key_matches_go_stringer() {
    assert_eq!(KEY.String(), "privilege-key");
    assert_eq!(KEY.to_string(), "privilege-key");
}

/// 确认 VerificationInfo 默认值对齐 Go 零值（沙箱/错密标志为 false，资源组名为空）。
#[test]
fn verification_info_default_matches_go_zero_value() {
    let info = VerificationInfo::default();
    assert!(!info.InSandBoxMode);
    assert!(!info.FailedDueToWrongPassword);
    assert!(info.ResourceGroupName.is_empty());
}

/// 校验 Bind → Get → 替换 → 解绑的上下文往返与权限校验结果切换。
#[test]
fn manager_binding_matches_go_context_round_trip_and_replacement() {
    let mut ctx = TestSessionContext::default();
    assert!(GetPrivilegeManager(&ctx).is_none());

    let first: Arc<dyn Manager> = Arc::new(TestManager { allow: true });
    BindPrivilegeManager(&mut ctx, Some(Arc::clone(&first)));
    let fetched = GetPrivilegeManager(&ctx).expect("manager must be bound");
    assert!(Arc::ptr_eq(&fetched, &first));
    assert!(fetched.RequestVerification(&[], "db", "table", "column", PrivilegeType(1)));

    let replacement: Arc<dyn Manager> = Arc::new(TestManager { allow: false });
    BindPrivilegeManager(&mut ctx, Some(Arc::clone(&replacement)));
    let fetched = GetPrivilegeManager(&ctx).expect("replacement manager must be bound");
    assert!(Arc::ptr_eq(&fetched, &replacement));
    assert!(!fetched.RequestVerification(&[], "db", "table", "column", PrivilegeType(1)));

    BindPrivilegeManager(&mut ctx, None);
    assert!(GetPrivilegeManager(&ctx).is_none());
}
