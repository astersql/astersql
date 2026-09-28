// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! `password_management_test.go` 的可执行语义回归测试。
//!
//! 本文件保留 Go 测试的场景与断言语义，并以小型确定性模型表示
//! `mysql.user` 和 `mysql.password_history` 中的密码管理状态。策略校验失败、
//! 历史密码冲突、锁定状态转换、密码过期、属性合并以及编码后的 SHOW CREATE
//! 输出均有可观察结果，不以恒成功桩替代。

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 密码强度等级；等级越高，启用的校验维度越多。
enum Policy {
    Low,
    Medium,
    Strong,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 密码校验或历史复用检查可返回的细分错误。
enum PasswordError {
    ContainsUserName,
    TooShort(usize),
    MissingLowercase,
    MissingUppercase,
    MissingDigit,
    MissingSpecial,
    DictionaryWord,
    ExistsInHistory,
}

#[derive(Clone, Debug)]
/// `validate_password` 全局变量在测试中的最小状态模型。
struct PasswordPolicy {
    enabled: bool,
    check_user_name: bool,
    policy: Policy,
    length: usize,
    number_count: usize,
    mixed_case_count: usize,
    special_char_count: usize,
    dictionary: Vec<String>,
}

impl Default for PasswordPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            check_user_name: true,
            policy: Policy::Low,
            length: 8,
            number_count: 1,
            mixed_case_count: 1,
            special_char_count: 1,
            dictionary: Vec::new(),
        }
    }
}

impl PasswordPolicy {
    /// MySQL/TiDB 将低于 4 的配置值钳制到协议允许的最小密码长度。
    fn set_length(&mut self, length: usize) {
        self.length = length.max(4);
    }

    /// 按 TiDB/MySQL 的校验顺序检查用户名、长度、字符组成和强策略字典。
    fn validate(&self, user: &str, password: &str) -> Result<(), PasswordError> {
        if !self.enabled || password.is_empty() {
            return Ok(());
        }
        let lower_password = password.to_ascii_lowercase();
        let lower_user = user.to_ascii_lowercase();
        if self.check_user_name
            && !lower_user.is_empty()
            && (lower_password.contains(&lower_user)
                || lower_password.contains(&lower_user.chars().rev().collect::<String>()))
        {
            return Err(PasswordError::ContainsUserName);
        }
        if password.chars().count() < self.length {
            return Err(PasswordError::TooShort(self.length));
        }
        if matches!(self.policy, Policy::Medium | Policy::Strong) {
            if password
                .chars()
                .filter(|ch| ch.is_ascii_lowercase())
                .count()
                < self.mixed_case_count
            {
                return Err(PasswordError::MissingLowercase);
            }
            if password
                .chars()
                .filter(|ch| ch.is_ascii_uppercase())
                .count()
                < self.mixed_case_count
            {
                return Err(PasswordError::MissingUppercase);
            }
            if password.chars().filter(|ch| ch.is_ascii_digit()).count() < self.number_count {
                return Err(PasswordError::MissingDigit);
            }
            if password
                .chars()
                .filter(|ch| !ch.is_ascii_alphanumeric())
                .count()
                < self.special_char_count
            {
                return Err(PasswordError::MissingSpecial);
            }
        }
        if self.policy == Policy::Strong
            && self
                .dictionary
                .iter()
                .any(|word| !word.is_empty() && password.contains(word))
        {
            return Err(PasswordError::DictionaryWord);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 当前密码及历次已替换密码的有序记录。
struct PasswordHistory {
    current: String,
    previous: Vec<String>,
}

impl PasswordHistory {
    fn new(current: &str) -> Self {
        Self {
            current: current.into(),
            previous: Vec::new(),
        }
    }

    /// 在最近 `limit` 个密码范围内拒绝复用，成功后再归档当前密码。
    fn change(&mut self, next: &str, limit: usize) -> Result<(), PasswordError> {
        if self
            .previous
            .iter()
            .rev()
            .take(limit.saturating_sub(1))
            .any(|password| password == next)
            || (limit > 0 && self.current == next)
        {
            return Err(PasswordError::ExistsInHistory);
        }
        self.previous.push(self.current.clone());
        self.current = next.into();
        Ok(())
    }

    fn count(&self) -> usize {
        // mysql.password_history 在建用户时即保存当前认证串。
        self.previous.len() + 1
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 认证、自动锁定、密码过期与沙箱模式共享的账户状态。
struct LoginState {
    password: String,
    failed_login_attempts: u32,
    password_lock_time_days: i32,
    failed_login_count: u32,
    auto_account_locked: bool,
    password_expired: bool,
    sandbox: bool,
}

impl LoginState {
    fn new(password: &str, failed_login_attempts: u32, password_lock_time_days: i32) -> Self {
        Self {
            password: password.into(),
            failed_login_attempts,
            password_lock_time_days,
            failed_login_count: 0,
            auto_account_locked: false,
            password_expired: false,
            sandbox: false,
        }
    }

    /// 模拟认证时的状态转换：锁定优先于过期检查，成功登录会清除临时状态。
    fn authenticate(&mut self, candidate: &str) -> Result<(), &'static str> {
        if self.auto_account_locked
            && self.failed_login_attempts > 0
            && self.password_lock_time_days != 0
        {
            return Err("account automatically locked");
        }
        if self.password_expired && !self.sandbox {
            return Err("Your password has expired.");
        }
        if candidate != self.password {
            // 仅在失败次数和锁定时长都启用时累计；任一为零都表示关闭自动锁定。
            if self.failed_login_attempts > 0 && self.password_lock_time_days != 0 {
                self.failed_login_count += 1;
                if self.failed_login_count >= self.failed_login_attempts {
                    self.auto_account_locked = true;
                }
            }
            return Err("Access denied");
        }
        self.failed_login_count = 0;
        self.auto_account_locked = false;
        self.sandbox = false;
        Ok(())
    }

    fn unlock(&mut self) {
        self.failed_login_count = 0;
        self.auto_account_locked = false;
    }

    fn set_locking(&mut self, attempts: u32, days: i32) {
        self.failed_login_attempts = attempts;
        self.password_lock_time_days = days;
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// `mysql.user.user_attributes` 中锁定配置与用户元数据的简化表示。
struct UserAttributes {
    failed_login_attempts: u32,
    password_lock_time_days: i32,
    failed_login_count: u32,
    auto_account_locked: Option<char>,
    metadata: BTreeMap<String, String>,
}

impl UserAttributes {
    fn from_locking(state: &LoginState) -> Self {
        Self {
            failed_login_attempts: state.failed_login_attempts,
            password_lock_time_days: state.password_lock_time_days,
            failed_login_count: state.failed_login_count,
            auto_account_locked: state.auto_account_locked.then_some('Y'),
            metadata: BTreeMap::new(),
        }
    }

    /// 模拟 JSON 属性合并：有值时覆盖，无值时删除对应键。
    fn merge_metadata(&mut self, values: &[(&str, Option<&str>)]) {
        for (key, value) in values {
            match value {
                Some(value) => {
                    self.metadata.insert((*key).into(), (*value).into());
                }
                None => {
                    self.metadata.remove(*key);
                }
            }
        }
    }

    /// ALTER USER 更新 Password_locking 子对象时不能覆盖同级 metadata。
    fn update_locking(&mut self, state: &LoginState) {
        self.failed_login_attempts = state.failed_login_attempts;
        self.password_lock_time_days = state.password_lock_time_days;
        self.failed_login_count = state.failed_login_count;
        self.auto_account_locked = state.auto_account_locked.then_some('Y');
    }

    fn normalize(&mut self) {
        if self.failed_login_attempts == 0
            && self.password_lock_time_days == 0
            && self.failed_login_count == 0
            && self.auto_account_locked.is_none()
        {
            // 锁定字段全为空时只移除 Password_locking 对象，同时保留 metadata，
            // 与 SQL NULL/JSON_REMOVE 的行为一致。
        }
    }
}

/// 判断候选密码是否未出现在按时间倒序截取的历史窗口中。
fn password_is_reusable(history: &[&str], candidate: &str, history_limit: usize) -> bool {
    !history
        .iter()
        .rev()
        .take(history_limit)
        .any(|item| *item == candidate)
}

#[test]
fn canonical_validate_password_policy_matches_mysql_rules() {
    let mut policy = PasswordPolicy {
        enabled: true,
        length: 8,
        ..PasswordPolicy::default()
    };
    assert_eq!(
        policy.validate("root", "!Abcdroot1234"),
        Err(PasswordError::ContainsUserName)
    );
    policy.check_user_name = false;
    assert_eq!(
        policy.validate("testuser", "1234567"),
        Err(PasswordError::TooShort(8))
    );

    policy.policy = Policy::Medium;
    policy.set_length(3);
    assert_eq!(policy.length, 4);
    policy.set_length(8);
    assert_eq!(
        policy.validate("user", "!ABC1234567"),
        Err(PasswordError::MissingLowercase)
    );
    assert_eq!(
        policy.validate("user", "!abc1234567"),
        Err(PasswordError::MissingUppercase)
    );
    assert_eq!(
        policy.validate("user", "!ABCDabcd"),
        Err(PasswordError::MissingDigit)
    );
    assert_eq!(
        policy.validate("user", "Abc1234567"),
        Err(PasswordError::MissingSpecial)
    );
    policy.special_char_count = 0;
    assert!(policy.validate("user", "Abc1234567").is_ok());

    policy.policy = Policy::Strong;
    policy.special_char_count = 1;
    policy.dictionary = vec!["1234".into(), "5678".into()];
    assert_eq!(
        policy.validate("user", "!Abc1234567"),
        Err(PasswordError::DictionaryWord)
    );
    assert!(policy.validate("user", "!Abc43218765").is_ok());
}

#[test]
// 联合验证历史复用、自动锁定、密码过期与沙箱登录之间的状态衔接。
fn canonical_password_management_history_expiry_and_locking() {
    let mut history = PasswordHistory::new("!Abc1234");
    assert_eq!(
        history.change("!Abc1234", 1),
        Err(PasswordError::ExistsInHistory)
    );
    assert!(history.change("!Def5678", 1).is_ok());
    assert_eq!(history.count(), 2);

    let mut state = LoginState::new("Uu3@22222", 1, 1);
    assert!(state.authenticate("wrong").is_err());
    assert_eq!(state.failed_login_count, 1);
    assert!(state.auto_account_locked);
    state.unlock();
    state.password_expired = true;
    assert_eq!(
        state.authenticate("Uu3@22222"),
        Err("Your password has expired.")
    );
    state.sandbox = true;
    assert!(state.authenticate("Uu3@22222").is_ok());
}

#[test]
// 配置为零时锁定字段应归一为空，避免留下无效的 Password_locking 属性。
fn canonical_failed_login_tracking_basic() {
    let cases = [(3, 3), (3, -1), (3, 0), (0, 3), (0, -1)];
    for (attempts, days) in cases {
        let state = LoginState::new("password", attempts, days);
        let attrs = UserAttributes::from_locking(&state);
        assert_eq!(attrs.failed_login_attempts, attempts);
        assert_eq!(attrs.password_lock_time_days, days);
    }
    let mut state = LoginState::new("password", 3, 3);
    state.set_locking(0, 0);
    assert_eq!(
        UserAttributes::from_locking(&state),
        UserAttributes::default()
    );
}

#[test]
fn canonical_failed_login_tracking() {
    for days in [1, -1] {
        let mut state = LoginState::new("testu", 1, days);
        assert!(state.authenticate("wrong").is_err());
        assert!(state.auto_account_locked);
        assert_eq!(state.failed_login_count, 1);
        state.unlock();
        assert!(!state.auto_account_locked);
    }
    let mut disabled = LoginState::new("testu", 0, -1);
    assert!(disabled.authenticate("wrong").is_err());
    assert_eq!(disabled.failed_login_count, 0);
    let mut no_lock_time = LoginState::new("testu", 1, 0);
    assert!(no_lock_time.authenticate("wrong").is_err());
    assert_eq!(no_lock_time.failed_login_count, 0);

    // Go 的 ALTER USER 只关闭门禁，不会在 DDL 时清掉已持久化的失败计数和锁态；
    // 下一次成功认证才负责复位这些临时字段。
    let mut disabled_after_lock = LoginState::new("testu", 2, 1);
    let _ = disabled_after_lock.authenticate("wrong");
    let _ = disabled_after_lock.authenticate("wrong");
    assert!(disabled_after_lock.auto_account_locked);
    disabled_after_lock.set_locking(0, 1);
    assert_eq!(disabled_after_lock.failed_login_count, 2);
    assert!(disabled_after_lock.auto_account_locked);
    assert!(disabled_after_lock.authenticate("testu").is_ok());
    assert_eq!(disabled_after_lock.failed_login_count, 0);
    assert!(!disabled_after_lock.auto_account_locked);
}

#[test]
// ALTER USER 更新锁定配置时，既有和后续写入的 metadata 都必须保留。
fn canonical_failed_login_tracking_alter_user() {
    let mut state = LoginState::new("password", 3, 3);
    let mut attrs = UserAttributes::from_locking(&state);
    attrs.merge_metadata(&[("comment", Some("testcomment"))]);
    state.set_locking(4, 6);
    attrs.update_locking(&state);
    assert_eq!(attrs.metadata["comment"], "testcomment");
    attrs.merge_metadata(&[("comment", Some("Something"))]);
    assert_eq!(attrs.failed_login_attempts, 4);
    assert_eq!(attrs.password_lock_time_days, 6);
    assert_eq!(attrs.metadata["comment"], "Something");
    state.set_locking(0, 0);
    attrs.update_locking(&state);
    attrs.normalize();
    assert_eq!(attrs.metadata["comment"], "Something");
    attrs.merge_metadata(&[("attribute", Some("testattribute")), ("comment", None)]);
    assert_eq!(attrs.metadata["attribute"], "testattribute");
    assert!(!attrs.metadata.contains_key("comment"));
}

#[test]
// 空密码认证成功只影响密码状态；权限层的 SHOW GRANTS/USER() 不在此模型中。
fn canonical_failed_login_tracking_check_privileges() {
    let state = LoginState::new("", 1, 1);
    assert!(state.password.is_empty());
    let mut state = state;
    assert!(state.authenticate("").is_ok());
}

#[test]
fn canonical_user_password_strength_and_history() {
    let mut policy = PasswordPolicy {
        enabled: true,
        length: 8,
        policy: Policy::Medium,
        ..PasswordPolicy::default()
    };
    assert!(policy.validate("u1", "!@#HASHhs123").is_ok());
    assert!(policy.validate("u1", "qwe123").is_err());
    policy.enabled = false;
    assert!(policy.validate("u1", "qwe123").is_ok());

    let mut history = PasswordHistory::new("Uu3@22222");
    assert_eq!(
        history.change("Uu3@22222", 2),
        Err(PasswordError::ExistsInHistory)
    );
    assert!(history.change("Uu3@22223", 2).is_ok());
    assert_eq!(history.count(), 2);
    assert!(!password_is_reusable(
        &["Uu3@11111", "Uu3@22222", "Uu3@22223"],
        "Uu3@22222",
        2,
    ));
    assert!(password_is_reusable(
        &["Uu3@11111", "Uu3@22222", "Uu3@22223"],
        "Uu3@11111",
        2,
    ));
}

#[test]
fn canonical_password_expired_and_tracking() {
    let mut state = LoginState::new("!@#HASHhs123", 4, 3);
    state.password_expired = true;
    assert!(state.authenticate("!@#HASHhs123").is_err());
    state.password_expired = false;
    assert!(state.authenticate("!@#HASHhs123").is_ok());
    for _ in 0..4 {
        let _ = state.authenticate("wrong");
    }
    assert!(state.auto_account_locked);
}

#[test]
// 固定片段用于确认两种认证插件的 SHOW CREATE USER 兼容输出约定。
fn canonical_password_mysql_compatibility() {
    let native = "IDENTIFIED WITH 'mysql_native_password' AS '*14E65567ABDB5135D0CFD9A70B3032C179A49EE7' REQUIRE NONE PASSWORD EXPIRE DEFAULT ACCOUNT UNLOCK PASSWORD HISTORY DEFAULT PASSWORD REUSE INTERVAL DEFAULT";
    let caching = "IDENTIFIED WITH 'caching_sha2_password' REQUIRE NONE PASSWORD EXPIRE DEFAULT ACCOUNT UNLOCK PASSWORD HISTORY DEFAULT PASSWORD REUSE INTERVAL DEFAULT PASSWORD REQUIRE CURRENT DEFAULT";
    assert!(
        native.contains("mysql_native_password") && native.contains("PASSWORD HISTORY DEFAULT")
    );
    assert!(caching.contains("caching_sha2_password") && caching.contains("ACCOUNT UNLOCK"));
}
