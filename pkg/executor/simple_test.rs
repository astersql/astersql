// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Simple 执行器用户选项相关单元测试。
//
// 覆盖用户身份格式化、双密码（retain/discard）意图解析，以及认证插件（auth plugin）默认值与显式指定。

use crate::simple::{
    Operation, SimpleBackend, SimpleError, UserIdentity, UserRecord, UserSpec,
    appendStatsDeltaTargetTableIDs, dualPasswordOption, dualPasswordRequested, effectiveAuthPlugin,
    passwordReuseInfo, passwordVerification, userIdentityToUserList, userInfo,
};
use std::sync::Mutex;

/// 构造测试用用户身份（UserIdentity）：用户名 + 主机名。
fn user(name: &str, host: &str) -> UserIdentity {
    UserIdentity {
        username: name.into(),
        hostname: host.into(),
        current_user: false,
    }
}

/// 验证身份列表格式、双密码标志与 effectiveAuthPlugin 的默认/覆盖行为。
#[test]
fn simple_user_options_preserve_identity_and_dual_password_intent() {
    // 用户身份应格式化为 MySQL 风格的 'user'@'host' 列表。
    assert_eq!(
        userIdentityToUserList(&[user("alice", "%"), user("bob", "localhost")]),
        vec!["'alice'@'%'", "'bob'@'localhost'"]
    );
    // retain_current_password=true 表示保留旧密码（双密码语义）。
    let spec = UserSpec {
        user: user("alice", "%"),
        password: String::new(),
        auth_plugin: String::new(),
        auth_string: String::new(),
        retain_current_password: true,
        discard_old_password: false,
    };
    assert_eq!(dualPasswordOption(&spec), (true, false));
    assert!(dualPasswordRequested(&[spec]));
    // 未指定插件时回退到 mysql_native_password；显式插件优先于 fallback。
    assert_eq!(effectiveAuthPlugin("", ""), "mysql_native_password");
    assert_eq!(
        effectiveAuthPlugin("auth_socket", "fallback"),
        "auth_socket"
    );
}

#[test]
fn stats_delta_target_ids_preserve_go_table_then_partition_order() {
    let mut target_ids = vec![7];
    assert_eq!(
        appendStatsDeltaTargetTableIDs(&mut target_ids, 20, &[22, 21, 22]),
        vec![7, 20, 22, 21, 22]
    );
}

#[derive(Default)]
struct PasswordHistoryBackend {
    history: Vec<(String, i64)>,
    operations: Mutex<Vec<Operation>>,
}

impl SimpleBackend for PasswordHistoryBackend {
    fn execute(&self, operation: Operation) -> Result<(), SimpleError> {
        self.operations.lock().unwrap().push(operation);
        Ok(())
    }

    fn query_user(
        &self,
        user: &UserIdentity,
        _for_update: bool,
    ) -> Result<Option<UserRecord>, SimpleError> {
        Ok(Some(UserRecord {
            identity: user.clone(),
            password_history: self.history.clone(),
            ..UserRecord::default()
        }))
    }

    fn username_variants(&self, _name: &str) -> Vec<String> {
        Vec::new()
    }
    fn validate_username(&self, _name: &str) -> Result<(), SimpleError> {
        Ok(())
    }
    fn validate_username_format(&self, _name: &str) -> bool {
        true
    }
    fn verify_privilege(&self, _privilege: &str) -> bool {
        true
    }
    fn verify_role_edge(
        &self,
        _role: &UserIdentity,
        _user: &UserIdentity,
    ) -> Result<bool, SimpleError> {
        Ok(true)
    }
    fn validate_password(&self, _user: &UserIdentity, _password: &str) -> Result<(), SimpleError> {
        Ok(())
    }
    fn hash_password(&self, _plugin: &str, password: &str) -> Result<String, SimpleError> {
        Ok(password.into())
    }
    fn check_hashing_password(
        &self,
        hash: &str,
        password: &str,
        _plugin: &str,
    ) -> Result<bool, SimpleError> {
        Ok(hash == password)
    }
    fn auth_plugin_clear_text(&self, _plugin: &str) -> bool {
        false
    }
    fn default_auth_plugin(&self) -> Result<String, SimpleError> {
        Ok("mysql_native_password".into())
    }
    fn global_password_history(&self) -> i64 {
        0
    }
    fn global_password_reuse_interval(&self) -> i64 {
        0
    }
    fn now_unix(&self) -> i64 {
        1_000_000
    }
    fn current_user(&self) -> Option<UserIdentity> {
        None
    }
    fn active_roles(&self) -> Vec<UserIdentity> {
        Vec::new()
    }
    fn set_current_user(&self, _users: &[UserIdentity]) {}
    fn in_transaction(&self) -> bool {
        false
    }
    fn pessimistic_transaction(&self) -> bool {
        false
    }
    fn restricted_read_only(&self) -> bool {
        false
    }
    fn is_starter_deployment(&self) -> bool {
        false
    }
    fn skip_grant_table(&self) -> bool {
        false
    }
    fn validate_password_enabled(&self) -> bool {
        false
    }
    fn resource_group_exists(&self, _name: &str) -> bool {
        true
    }
    fn placement_policy_exists(&self, _name: &str) -> bool {
        true
    }
    fn broadcast(&self, _sql: &str) -> Result<(), SimpleError> {
        Ok(())
    }
    fn warn(&self, _warning: SimpleError) {}
}

#[test]
fn password_verification_keeps_space_for_the_new_history_row() {
    let backend = PasswordHistoryBackend {
        history: vec![
            ("old-1".into(), 3),
            ("old-2".into(), 2),
            ("old-3".into(), 1),
        ],
        ..PasswordHistoryBackend::default()
    };
    let candidate = userInfo {
        host: "%".into(),
        user: "alice".into(),
        pwd: "new".into(),
        ..userInfo::default()
    };

    assert_eq!(
        passwordVerification(
            &backend,
            &candidate,
            &passwordReuseInfo {
                passwordHistory: 3,
                passwordReuseInterval: 0
            },
            "mysql_native_password",
        ),
        Ok((true, 1))
    );
}

#[test]
fn password_verification_skips_reuse_checks_when_both_policies_are_disabled() {
    let backend = PasswordHistoryBackend {
        history: vec![("same".into(), 1)],
        ..PasswordHistoryBackend::default()
    };
    let candidate = userInfo {
        host: "%".into(),
        user: "alice".into(),
        pwd: "same".into(),
        ..userInfo::default()
    };

    assert_eq!(
        passwordVerification(
            &backend,
            &candidate,
            &passwordReuseInfo {
                passwordHistory: 0,
                passwordReuseInterval: 0
            },
            "mysql_native_password",
        ),
        Ok((true, 2))
    );
}
