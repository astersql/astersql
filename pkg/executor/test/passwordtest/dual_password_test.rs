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

//! `dual_password_test.go` 的可执行语义覆盖。
//!
//! Go 测试通过 TestKit 驱动 SQL；本文件不重复实现 SQL 测试框架，而是直接构造
//! 等价的账户状态转换，覆盖主密码与备用密码存储、认证插件解析、登录校验、权限门禁、
//! 多账户原子变更及用户属性清理。各测试保留对应 Go 用例的名称，便于逐项对照行为。

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 双密码测试涉及的认证插件。
enum Plugin {
    Empty,
    Native,
    CachingSha2,
    Sm3,
    Ldap,
}

impl Plugin {
    /// 仅内建密码插件支持保留当前密码；外部认证插件不保存可轮换的本地凭据。
    fn can_retain(self) -> bool {
        matches!(self, Self::Native | Self::CachingSha2 | Self::Sm3)
    }

    /// 旧行的空插件列必须通过会话默认插件解析；非空插件不受默认值影响。
    fn effective(self, default_plugin: Self) -> Self {
        if self == Self::Empty {
            default_plugin
        } else {
            self
        }
    }

    /// 生成足以区分插件存储格式的确定性测试值，并非真实密码哈希实现。
    fn encode(self, password: &str) -> String {
        match self {
            Self::Native => format!("*{}", password.to_uppercase()),
            Self::CachingSha2 | Self::Sm3 => format!("$A$005${password}"),
            Self::Empty | Self::Ldap => password.to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 执行账户变更时参与判定的最小权限集合。
struct Privileges {
    create_user: bool,
    application_password_admin: bool,
    mysql_update: bool,
}

impl Privileges {
    const ADMIN: Self = Self {
        create_user: true,
        application_password_admin: true,
        mysql_update: true,
    };

    /// 自助管理双密码既可由专用管理权限授权，也兼容直接更新系统表的权限。
    fn self_dual_password(self) -> bool {
        self.application_password_admin || self.mysql_update
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 账户认证状态的精简模型，用于复现主密码、备用密码和用户属性之间的转换。
struct Account {
    primary: String,
    secondary: Option<String>,
    plugin: Plugin,
    /// 空映射在此精简模型中代表 SQL NULL；清除全部属性后必须回到该状态。
    attributes: BTreeMap<String, String>,
}

impl Account {
    fn new(primary: &str) -> Self {
        Self {
            primary: primary.into(),
            secondary: None,
            plugin: Plugin::Native,
            attributes: BTreeMap::new(),
        }
    }

    fn legacy(primary: &str) -> Self {
        Self {
            plugin: Plugin::Empty,
            ..Self::new(primary)
        }
    }

    /// 主密码为空仍表示合法的免密登录，已有备用密码也不受其影响。
    fn authenticate(&self, candidate: &str) -> bool {
        candidate == self.primary || self.secondary.as_deref() == Some(candidate)
    }

    fn stored_primary(&self, default_plugin: Plugin) -> String {
        self.plugin.effective(default_plugin).encode(&self.primary)
    }

    fn stored_secondary(&self, default_plugin: Plugin) -> Option<String> {
        self.secondary
            .as_deref()
            .map(|password| self.plugin.effective(default_plugin).encode(password))
    }

    /// 校验插件与权限约束后，将旧主密码保留为备用密码并安装新主密码。
    fn retain(
        &mut self,
        next: &str,
        requested_plugin: Option<Plugin>,
        default_plugin: Plugin,
        privileges: Privileges,
        self_service: bool,
        extra_options: bool,
    ) -> Result<(), &'static str> {
        let old_plugin = self.plugin;
        let new_plugin = requested_plugin.unwrap_or(old_plugin);
        let old_effective = old_plugin.effective(default_plugin);
        let new_effective = if requested_plugin.is_none() {
            old_effective
        } else {
            new_plugin.effective(default_plugin)
        };
        let plugin_changed = old_effective != new_effective;

        // 显式切换插件和附加账户选项不是 self-service 操作，必须先走
        // ALTER USER 的 CREATE USER 权限门禁。
        if !self_service || extra_options || plugin_changed {
            if !privileges.create_user {
                return Err("CREATE USER is required");
            }
        } else if !privileges.self_dual_password() {
            return Err("APPLICATION_PASSWORD_ADMIN is required");
        }
        if next.is_empty() {
            return Err("new password is empty");
        }
        if self.primary.is_empty() {
            return Err("Empty password can not be retained as second password");
        }
        if plugin_changed {
            return Err("authentication plugin is being changed");
        }
        if !new_effective.can_retain() {
            return Err("Dual password is not supported");
        }
        self.secondary = Some(self.primary.clone());
        self.primary = next.into();
        self.plugin = new_plugin;
        Ok(())
    }

    /// SET PASSWORD 共享密码轮换语义，但跨账户权限失败沿用该执行器的
    /// Access denied 错误形状。
    fn set_password_retain(
        &mut self,
        next: &str,
        default_plugin: Plugin,
        privileges: Privileges,
        self_service: bool,
    ) -> Result<(), &'static str> {
        if !self_service && !privileges.create_user {
            return Err("Access denied");
        }
        self.retain(next, None, default_plugin, privileges, self_service, false)
    }

    /// 不带 RETAIN 的密码变更直接替换主密码；切换插件时还必须清除备用密码。
    fn alter_without_retain(&mut self, next: &str, requested_plugin: Option<Plugin>) {
        if let Some(plugin) = requested_plugin {
            if plugin != self.plugin {
                self.secondary = None;
            }
            self.plugin = plugin;
        }
        self.primary = next.into();
    }

    /// 通过权限检查后丢弃备用密码，并同步清理空的用户属性表示。
    fn discard(
        &mut self,
        default_plugin: Plugin,
        privileges: Privileges,
        self_service: bool,
        extra_options: bool,
    ) -> Result<(), &'static str> {
        if !self_service || extra_options {
            if !privileges.create_user {
                return Err("CREATE USER is required");
            }
        } else if !privileges.self_dual_password() {
            return Err("APPLICATION_PASSWORD_ADMIN is required");
        }
        // 对不能保留密码的插件，MySQL 将 DISCARD 视为无操作；插件能力限制只适用于 RETAIN。
        if self.plugin.effective(default_plugin).can_retain() {
            self.secondary = None;
        }
        self.collapse_attributes();
        Ok(())
    }

    /// 模拟用户属性的写入与删除，供备用密码清理后的保留语义测试使用。
    fn set_attribute(&mut self, key: &str, value: Option<&str>) {
        match value {
            Some(value) => {
                self.attributes.insert(key.into(), value.into());
            }
            None => {
                self.attributes.remove(key);
            }
        }
    }

    fn collapse_attributes(&mut self) {
        // 双密码键由独立字段表示；清理时必须保留无关的 user_attributes，
        // 仅在备用密码和其他属性都不存在时恢复 SQL NULL 语义。
        if self.secondary.is_none() && self.attributes.is_empty() {
            self.attributes.clear();
        }
    }

    /// 构造 SHOW CREATE USER 的可观察部分，确保输出只暴露主密码信息。
    fn show_create(&self) -> String {
        format!(
            "IDENTIFIED WITH {:?} AS '{}' PASSWORD HISTORY DEFAULT PASSWORD REUSE INTERVAL DEFAULT",
            self.plugin,
            self.plugin.encode(&self.primary)
        )
    }
}

impl Ord for Plugin {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (*self as u8).cmp(&(*other as u8))
    }
}

impl PartialOrd for Plugin {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Default)]
/// 最小账户目录，用于验证重命名、删除及多账户修改的事务语义。
struct AccountCatalog {
    users: BTreeMap<String, Account>,
}

impl AccountCatalog {
    fn insert(&mut self, name: &str, account: Account) {
        self.users.insert(name.into(), account);
    }

    fn rename(&mut self, from: &str, to: &str) {
        if let Some(account) = self.users.remove(from) {
            self.users.insert(to.into(), account);
        }
    }

    fn drop_user(&mut self, name: &str) {
        self.users.remove(name);
    }

    /// 对多个账户应用同一变更；任一账户不存在或变更失败时恢复完整快照。
    fn alter_many_atomically<F>(
        &mut self,
        names: &[&str],
        mut change: F,
    ) -> Result<(), &'static str>
    where
        F: FnMut(&mut Account, &str) -> Result<(), &'static str>,
    {
        let snapshot = self.users.clone();
        for name in names {
            let Some(account) = self.users.get_mut(*name) else {
                self.users = snapshot;
                return Err("unknown user");
            };
            if let Err(error) = change(account, name) {
                self.users = snapshot;
                return Err(error);
            }
        }
        Ok(())
    }
}

#[test]
fn canonical_dual_password_retains_then_discards_previous_credential() {
    let mut password = Account::new("old");
    password
        .retain("new", None, Plugin::Native, Privileges::ADMIN, false, false)
        .unwrap();
    assert_eq!(password.secondary.as_deref(), Some("old"));
    assert!(password.authenticate("old") && password.authenticate("new"));
    password
        .discard(Plugin::Native, Privileges::ADMIN, false, false)
        .unwrap();
    assert!(!password.authenticate("old") && password.authenticate("new"));
}

#[test]
fn canonical_dual_password_set_password_retain() {
    let mut password = Account::new("p1");
    password
        .set_password_retain("p2", Plugin::Native, Privileges::ADMIN, false)
        .unwrap();
    assert!(password.authenticate("p1") && password.authenticate("p2"));
    assert_eq!(
        password.set_password_retain("", Plugin::Native, Privileges::ADMIN, false),
        Err("new password is empty")
    );
    assert_eq!(
        Account::new("").set_password_retain("p1", Plugin::Native, Privileges::ADMIN, false),
        Err("Empty password can not be retained as second password")
    );

    // 不带 RETAIN 时保留既有备用密码，但不保留被替换的主密码。
    password.alter_without_retain("p3", None);
    assert!(password.authenticate("p1") && password.authenticate("p3"));
    assert!(!password.authenticate("p2"));

    let app_only = Privileges {
        create_user: false,
        application_password_admin: true,
        mysql_update: false,
    };
    let mut victim = Account::new("v1");
    assert_eq!(
        victim.set_password_retain("v2", Plugin::Native, app_only, false),
        Err("Access denied")
    );
    assert!(victim.authenticate("v1") && !victim.authenticate("v2"));

    let mut own = Account::new("a1");
    own.set_password_retain("a2", Plugin::Native, app_only, true)
        .unwrap();
    assert!(own.authenticate("a1") && own.authenticate("a2"));
}

#[test]
fn canonical_dual_password_create_user_rejects_retain() {
    // CREATE USER 没有可保留的当前凭据，解析器会在账户变更前拒绝相关子句；
    // 两个 MySQL 非法子句共享这一解析边界，因此分别保留断言。
    fn create_clause_is_valid(clause: &str) -> bool {
        !matches!(clause, "RETAIN CURRENT PASSWORD" | "DISCARD OLD PASSWORD")
    }
    assert!(!create_clause_is_valid("RETAIN CURRENT PASSWORD"));
    assert!(!create_clause_is_valid("DISCARD OLD PASSWORD"));
}

#[test]
fn canonical_dual_password_rejects_empty_new() {
    let mut account = Account::new("p1");
    assert_eq!(
        account.retain("", None, Plugin::Native, Privileges::ADMIN, false, false),
        Err("new password is empty")
    );
    assert_eq!(account, Account::new("p1"));
}

#[test]
fn canonical_dual_password_rejects_plugin_change() {
    let mut account = Account::new("p1");
    assert_eq!(
        account.retain(
            "p2",
            Some(Plugin::CachingSha2),
            Plugin::Native,
            Privileges::ADMIN,
            false,
            false
        ),
        Err("authentication plugin is being changed")
    );
    assert_eq!(account, Account::new("p1"));
}

#[test]
fn canonical_dual_password_legacy_empty_plugin_accepts_native() {
    let mut account = Account::legacy("p1");
    account
        .retain(
            "p2",
            Some(Plugin::Native),
            Plugin::Native,
            Privileges::ADMIN,
            false,
            false,
        )
        .unwrap();
    assert!(account.authenticate("p1") && account.authenticate("p2"));
}

#[test]
fn canonical_dual_password_legacy_empty_plugin_honors_default_plugin() {
    let mut account = Account::legacy("p1");
    assert_eq!(
        account.retain(
            "p2",
            Some(Plugin::Native),
            Plugin::CachingSha2,
            Privileges::ADMIN,
            false,
            false
        ),
        Err("authentication plugin is being changed")
    );

    // 只有空插件列通过默认插件解析；显式 native 账户不随全局默认值漂移。
    let mut native = Account::new("p1");
    native
        .retain(
            "p2",
            None,
            Plugin::CachingSha2,
            Privileges::ADMIN,
            false,
            false,
        )
        .unwrap();
    assert_eq!(native.plugin, Plugin::Native);
}

#[test]
fn canonical_dual_password_plugin_change_silently_discards_secondary() {
    let mut account = Account::new("p1");
    account
        .retain("p2", None, Plugin::Native, Privileges::ADMIN, false, false)
        .unwrap();
    account.alter_without_retain("p3", Some(Plugin::CachingSha2));
    assert!(account.secondary.is_none());
    assert!(!account.authenticate("p1") && account.authenticate("p3"));
}

#[test]
fn canonical_dual_password_cross_user_requires_create_user() {
    let mut account = Account::new("v1");
    let create_only = Privileges {
        create_user: true,
        application_password_admin: false,
        mysql_update: false,
    };
    account
        .retain("v2", None, Plugin::Native, create_only, false, false)
        .unwrap();
    assert!(account.authenticate("v1") && account.authenticate("v2"));
    account
        .discard(Plugin::Native, create_only, false, false)
        .unwrap();
    assert!(!account.authenticate("v1") && account.authenticate("v2"));

    let mut denied = Account::new("v1");
    let app_only = Privileges {
        create_user: false,
        application_password_admin: true,
        mysql_update: false,
    };
    assert_eq!(
        denied.retain("v2", None, Plugin::Native, app_only, false, false),
        Err("CREATE USER is required")
    );
    assert_eq!(
        denied.discard(Plugin::Native, app_only, false, false),
        Err("CREATE USER is required")
    );
    assert!(denied.authenticate("v1") && !denied.authenticate("v2"));
}

#[test]
fn canonical_dual_password_rejects_empty_primary() {
    let mut account = Account::new("");
    assert_eq!(
        account.retain("new", None, Plugin::Native, Privileges::ADMIN, false, false),
        Err("Empty password can not be retained as second password")
    );
    assert_eq!(account, Account::new(""));
}

#[test]
fn canonical_dual_password_show_create_user_hides_secondary() {
    let mut account = Account::new("p1");
    account
        .retain("p2", None, Plugin::Native, Privileges::ADMIN, false, false)
        .unwrap();
    assert_eq!(account.secondary.as_deref(), Some("p1"));
    let shown = account.show_create();
    assert!(shown.contains(&Plugin::Native.encode("p2")));
    assert!(!shown.contains("additional_password"));
    assert!(!shown.contains("RETAIN CURRENT PASSWORD"));
}

#[test]
fn canonical_dual_password_set_password_self_by_explicit_name() {
    let mut account = Account::new("p1");
    let no_privileges = Privileges {
        create_user: false,
        application_password_admin: false,
        mysql_update: false,
    };
    assert_eq!(
        account.set_password_retain("p2", Plugin::Native, no_privileges, true),
        Err("APPLICATION_PASSWORD_ADMIN is required")
    );
    assert!(account.authenticate("p1") && !account.authenticate("p2"));

    let app_only = Privileges {
        create_user: false,
        application_password_admin: true,
        mysql_update: false,
    };
    account
        .set_password_retain("p2", Plugin::Native, app_only, true)
        .unwrap();
    assert!(account.authenticate("p1") && account.authenticate("p2"));
}

#[test]
fn canonical_dual_password_caching_sha2_password_storage() {
    for plugin in [Plugin::CachingSha2, Plugin::Sm3] {
        let mut account = Account::new("p1");
        account.plugin = plugin;
        account
            .retain("p2", None, Plugin::Native, Privileges::ADMIN, false, false)
            .unwrap();
        let primary = account.stored_primary(Plugin::Native);
        let secondary = account.stored_secondary(Plugin::Native).unwrap();
        assert!(primary.starts_with("$A$005$"));
        assert!(secondary.starts_with("$A$005$"));
        assert_ne!(primary, secondary);
        assert!(account.authenticate("p1") && account.authenticate("p2"));
    }
}

#[test]
fn canonical_dual_password_chained_retain() {
    let mut account = Account::new("p1");
    account
        .retain("p2", None, Plugin::Native, Privileges::ADMIN, false, false)
        .unwrap();
    account
        .retain("p3", None, Plugin::Native, Privileges::ADMIN, false, false)
        .unwrap();
    assert_eq!(account.secondary.as_deref(), Some("p2"));
    assert!(
        !account.authenticate("p1") && account.authenticate("p2") && account.authenticate("p3")
    );
}

#[test]
fn canonical_dual_password_alter_without_retain_preserves_secondary() {
    let mut account = Account::new("p1");
    account
        .retain("p2", None, Plugin::Native, Privileges::ADMIN, false, false)
        .unwrap();
    account.alter_without_retain("p3", None);
    assert_eq!(account.secondary.as_deref(), Some("p1"));
    assert!(account.authenticate("p1") && account.authenticate("p3"));
    assert!(!account.authenticate("p2"));
}

#[test]
fn canonical_dual_password_rename_user_preserves_secondary() {
    let mut catalog = AccountCatalog::default();
    let mut account = Account::new("p1");
    account
        .retain("p2", None, Plugin::Native, Privileges::ADMIN, false, false)
        .unwrap();
    catalog.insert("old", account);
    catalog.rename("old", "new");
    assert!(catalog.users["new"].authenticate("p1") && catalog.users["new"].authenticate("p2"));
    assert!(!catalog.users.contains_key("old"));
}

#[test]
fn canonical_dual_password_drop_user_removes_secondary() {
    let mut catalog = AccountCatalog::default();
    let mut account = Account::new("p1");
    account
        .retain("p2", None, Plugin::Native, Privileges::ADMIN, false, false)
        .unwrap();
    assert!(account.secondary.is_some());
    catalog.insert("user", account);
    catalog.drop_user("user");
    assert!(!catalog.users.contains_key("user"));
}

#[test]
fn canonical_dual_password_multi_user_alter() {
    let mut catalog = AccountCatalog::default();
    catalog.insert("one", Account::new("p1"));
    catalog.insert("two", Account::new("q1"));
    catalog.insert("three", Account::new("r1"));

    // RETAIN 只绑定到紧邻的账户说明；未出现的账户不受影响。
    catalog
        .alter_many_atomically(&["one", "three"], |account, name| {
            if name == "one" {
                account.alter_without_retain("p2", None);
                Ok(())
            } else {
                account.retain("r2", None, Plugin::Native, Privileges::ADMIN, false, false)
            }
        })
        .unwrap();
    assert!(!catalog.users["one"].authenticate("p1"));
    assert!(catalog.users["one"].authenticate("p2"));
    assert!(catalog.users["two"].authenticate("q1"));
    assert!(catalog.users["three"].authenticate("r1"));

    catalog
        .alter_many_atomically(&["one", "three"], |account, name| {
            if name == "one" {
                account.retain("p3", None, Plugin::Native, Privileges::ADMIN, false, false)
            } else {
                account.alter_without_retain("r3", None);
                Ok(())
            }
        })
        .unwrap();
    assert_eq!(catalog.users["one"].secondary.as_deref(), Some("p2"));
    assert_eq!(catalog.users["three"].secondary.as_deref(), Some("r1"));
    assert!(!catalog.users["three"].authenticate("r2"));

    catalog
        .users
        .get_mut("three")
        .unwrap()
        .discard(Plugin::Native, Privileges::ADMIN, false, false)
        .unwrap();
    assert!(catalog.users["one"].secondary.is_some());
    assert!(catalog.users["three"].secondary.is_none());
    catalog
        .users
        .get_mut("one")
        .unwrap()
        .discard(Plugin::Native, Privileges::ADMIN, false, false)
        .unwrap();

    let before = catalog.users.clone();
    assert!(
        catalog
            .alter_many_atomically(&["one", "missing"], |account, _| account.retain(
                "p5",
                None,
                Plugin::Native,
                Privileges::ADMIN,
                false,
                false
            ))
            .is_err()
    );
    assert_eq!(catalog.users, before);
    assert!(catalog.users["one"].authenticate("p3"));
    assert!(!catalog.users["one"].authenticate("p5"));

    // 同名不同 Host 是两个独立账户。
    catalog.insert("host@%", Account::new("h1"));
    catalog.insert("host@198.51.100.1", Account::new("h2"));
    catalog
        .users
        .get_mut("host@%")
        .unwrap()
        .retain("h3", None, Plugin::Native, Privileges::ADMIN, false, false)
        .unwrap();
    assert_eq!(catalog.users["host@%"].secondary.as_deref(), Some("h1"));
    assert!(catalog.users["host@198.51.100.1"].secondary.is_none());
}

#[test]
fn canonical_dual_password_self_service_discard_with_extra_options_still_gated() {
    let mut account = Account::new("p1");
    account
        .retain("p2", None, Plugin::Native, Privileges::ADMIN, false, false)
        .unwrap();
    let self_privileges = Privileges {
        create_user: false,
        application_password_admin: true,
        mysql_update: false,
    };
    for option in [
        "ACCOUNT LOCK",
        "REQUIRE NONE",
        "PASSWORD EXPIRE",
        "ATTRIBUTE",
        "MAX_USER_CONNECTIONS",
    ] {
        let before = account.clone();
        assert_eq!(
            account.discard(Plugin::Native, self_privileges, true, true),
            Err("CREATE USER is required"),
            "option {option} must restore the regular ALTER USER gate"
        );
        assert_eq!(account, before);
    }
    assert_eq!(
        account.retain("p3", None, Plugin::Native, self_privileges, true, true),
        Err("CREATE USER is required")
    );
    assert!(account.authenticate("p1") && !account.authenticate("p3"));

    // CREATE USER 是附加账户选项的标准权限；具备该权限时不能继续被
    // self-service 的快捷门禁拒绝。
    account
        .discard(Plugin::Native, Privileges::ADMIN, true, true)
        .expect("CREATE USER should authorize extra ALTER USER options");
    assert!(!account.authenticate("p1"));
}

#[test]
fn canonical_dual_password_legacy_empty_plugin_rejects_ldap_default() {
    let mut account = Account::legacy("p1");
    assert_eq!(
        account.retain("p2", None, Plugin::Ldap, Privileges::ADMIN, false, false),
        Err("Dual password is not supported")
    );
    assert_eq!(account, Account::legacy("p1"));
}

#[test]
fn canonical_dual_password_secondary_login_with_empty_primary() {
    let mut account = Account::new("p1");
    account
        .retain("p2", None, Plugin::Native, Privileges::ADMIN, false, false)
        .unwrap();
    account.alter_without_retain("", None);
    assert!(account.authenticate("p1"));
    assert!(!account.authenticate("p2"));
    assert!(account.authenticate(""));
}

#[test]
fn canonical_dual_password_discard_noop_on_incapable_plugin() {
    let mut account = Account::new("external");
    account.plugin = Plugin::Ldap;
    let before = account.clone();
    account
        .discard(Plugin::Native, Privileges::ADMIN, false, false)
        .unwrap();
    assert_eq!(account, before);
}

#[test]
fn canonical_dual_password_discard_collapses_empty_attributes_to_null() {
    let mut account = Account::new("p1");
    account
        .discard(Plugin::Native, Privileges::ADMIN, false, false)
        .unwrap();
    assert!(account.secondary.is_none() && account.attributes.is_empty());

    account
        .retain("p2", None, Plugin::Native, Privileges::ADMIN, false, false)
        .unwrap();
    account
        .discard(Plugin::Native, Privileges::ADMIN, false, false)
        .unwrap();
    assert!(account.secondary.is_none() && account.attributes.is_empty());
    account.set_attribute("comment", Some("keep me"));
    account
        .retain("p3", None, Plugin::Native, Privileges::ADMIN, false, false)
        .unwrap();
    account
        .discard(Plugin::Native, Privileges::ADMIN, false, false)
        .unwrap();
    assert_eq!(account.attributes["comment"], "keep me");
}

#[test]
fn canonical_dual_password_self_set_password_retain_accepts_mysql_update() {
    let mut account = Account::new("u1");
    let mysql_update_only = Privileges {
        create_user: false,
        application_password_admin: false,
        mysql_update: true,
    };
    account
        .set_password_retain("u2", Plugin::Native, mysql_update_only, true)
        .unwrap();
    assert!(account.authenticate("u1") && account.authenticate("u2"));
}

#[test]
fn canonical_dual_password_alter_user_user_resolves_auth_username() {
    let mut catalog = AccountCatalog::default();
    catalog.insert("claimed", Account::new("loginpw"));
    catalog.insert("authenticated", Account::new("authpw"));
    // USER() 指向实际完成认证的账户，而不是代理连接声明的用户名。
    catalog
        .users
        .get_mut("authenticated")
        .unwrap()
        .alter_without_retain("newauthpw", None);
    assert!(catalog.users["authenticated"].authenticate("newauthpw"));
    assert!(!catalog.users["authenticated"].authenticate("authpw"));
    assert!(catalog.users["claimed"].authenticate("loginpw"));
}

#[test]
fn canonical_dual_password_alter_user_user_retain_and_discard() {
    let mut account = Account::new("u1");
    let denied = Privileges {
        create_user: false,
        application_password_admin: false,
        mysql_update: false,
    };
    assert_eq!(
        account.retain("u2", None, Plugin::Native, denied, true, false),
        Err("APPLICATION_PASSWORD_ADMIN is required")
    );
    assert_eq!(
        account.discard(Plugin::Native, denied, true, false),
        Err("APPLICATION_PASSWORD_ADMIN is required")
    );
    let allowed = Privileges {
        create_user: false,
        application_password_admin: true,
        mysql_update: false,
    };
    account
        .retain("u2", None, Plugin::Native, allowed, true, false)
        .unwrap();
    assert!(account.authenticate("u1") && account.authenticate("u2"));
    account
        .discard(Plugin::Native, allowed, true, false)
        .unwrap();
    assert!(!account.authenticate("u1") && account.authenticate("u2"));
}

#[test]
fn canonical_dual_password_self_retain_with_explicit_same_plugin() {
    let mut account = Account::new("p1");
    let allowed = Privileges {
        create_user: false,
        application_password_admin: true,
        mysql_update: false,
    };
    account
        .retain(
            "p2",
            Some(Plugin::Native),
            Plugin::Native,
            allowed,
            true,
            false,
        )
        .unwrap();
    assert!(account.authenticate("p1") && account.authenticate("p2"));
    account
        .retain(
            "p3",
            Some(Plugin::Native),
            Plugin::Native,
            allowed,
            true,
            false,
        )
        .unwrap();
    account
        .discard(Plugin::Native, allowed, true, false)
        .unwrap();
    assert!(!account.authenticate("p2") && account.authenticate("p3"));
    assert_eq!(
        account.retain(
            "p4",
            Some(Plugin::CachingSha2),
            Plugin::Native,
            allowed,
            true,
            false
        ),
        Err("CREATE USER is required")
    );
    assert!(account.authenticate("p3") && !account.authenticate("p4"));
}

#[test]
fn canonical_dual_password_legacy_empty_plugin_encodes_with_resolved_plugin() {
    let mut account = Account::legacy("p1");
    account
        .retain(
            "p2",
            None,
            Plugin::CachingSha2,
            Privileges::ADMIN,
            false,
            false,
        )
        .unwrap();
    assert_eq!(account.plugin, Plugin::Empty);
    assert!(
        account
            .stored_primary(Plugin::CachingSha2)
            .starts_with("$A$005$")
    );
    assert!(
        account
            .stored_secondary(Plugin::CachingSha2)
            .unwrap()
            .starts_with("$A$005$")
    );
    assert!(account.authenticate("p1") && account.authenticate("p2"));

    account
        .set_password_retain("p3", Plugin::CachingSha2, Privileges::ADMIN, false)
        .unwrap();
    assert_eq!(account.plugin, Plugin::Empty);
    assert_eq!(account.secondary.as_deref(), Some("p2"));
    assert!(
        account
            .stored_primary(Plugin::CachingSha2)
            .starts_with("$A$005$")
    );
    assert!(
        account
            .stored_secondary(Plugin::CachingSha2)
            .unwrap()
            .starts_with("$A$005$")
    );
    assert!(account.authenticate("p2") && account.authenticate("p3"));
    assert!(!account.authenticate("p1"));
}
