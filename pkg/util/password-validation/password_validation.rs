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

// 密码策略校验实现：字典、用户名、LOW/MEDIUM/STRONG 分层检查。
//
// 对应 Go `pkg/util/password-validation`。策略强度由全局系统变量
// `validate_password_policy` 决定；失败时使用 TiDB 稳定错误码（如 1819）。
// 字符类别按 Unicode General Category 统计，与 Go rune 语义对齐。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::fmt;
use std::num::{IntErrorKind, ParseIntError};

use unicode_general_category::{GeneralCategory, get_general_category};

use crate::parser::auth::UserIdentity;
use crate::sessionctx::vardef;
use crate::sessionctx::variable::{self, GlobalVarAccessor, TiDBOptOn, VariableError};

/// Dictionary entries outside this byte-length range do not participate in matching.
/// 字典词字节长度上限；超出则不参与子串匹配。
pub const maxPwdValidationLength: usize = 100;
/// 字典词字节长度下限。
pub const minPwdValidationLength: usize = 4;

/// Errors returned by password validation while preserving TiDB's stable password error code.
///
/// 密码校验错误：系统变量错误透传；非法数值变量带解析源；无效密码保留稳定错误码。
#[derive(Debug)]
pub enum PasswordValidationError {
    /// 读取/解析全局系统变量失败。
    SystemVariable(VariableError),
    /// 计数类变量无法解析为整数。
    InvalidSystemVariableValue {
        name: &'static str,
        value: String,
        source: ParseIntError,
    },
    /// 密码不满足策略（MySQL 错误码通常为 1819）。
    InvalidPassword { code: u16, message: String },
}

impl fmt::Display for PasswordValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SystemVariable(error) => error.fmt(formatter),
            Self::InvalidSystemVariableValue { value, source, .. } => {
                let reason = match source.kind() {
                    IntErrorKind::PosOverflow | IntErrorKind::NegOverflow => "value out of range",
                    _ => "invalid syntax",
                };
                write!(formatter, "strconv.ParseInt: parsing {value:?}: {reason}")
            }
            Self::InvalidPassword { message, .. } => formatter.write_str(message),
        }
    }
}

impl std::error::Error for PasswordValidationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::SystemVariable(error) => Some(error),
            Self::InvalidSystemVariableValue { source, .. } => Some(source),
            Self::InvalidPassword { .. } => None,
        }
    }
}

impl From<VariableError> for PasswordValidationError {
    fn from(error: VariableError) -> Self {
        Self::SystemVariable(error)
    }
}

/// 密码校验结果别名。
pub type PasswordValidationResult<T> = Result<T, PasswordValidationError>;

/// The two session fields used by Go's ValidatePassword.
///
/// Keeping this as a trait lets the package use the integrated variable accessor now and lets
/// the package-level module integration implement it for the final SessionVars type later.
///
/// Go `ValidatePassword` 所需的两个会话字段：全局变量访问器与当前用户。
pub trait PasswordValidationSession {
    fn global_vars_accessor(&self) -> &dyn GlobalVarAccessor;
    fn user(&self) -> Option<&UserIdentity>;
}

/// Borrowed context for callers that do not own a complete TiDB session.
///
/// 借用式校验上下文，供未持有完整 Session 的调用方使用。
#[derive(Clone, Copy)]
pub struct PasswordValidationContext<'a> {
    /// 全局系统变量访问器。
    pub global_vars_accessor: &'a dyn GlobalVarAccessor,
    /// 当前登录/认证用户；无用户时跳过用户名检查。
    pub user: Option<&'a UserIdentity>,
}

impl<'a> PasswordValidationContext<'a> {
    /// 由访问器与可选用户构造借用上下文。
    pub fn new(
        global_vars_accessor: &'a dyn GlobalVarAccessor,
        user: Option<&'a UserIdentity>,
    ) -> Self {
        Self {
            global_vars_accessor,
            user,
        }
    }
}

impl PasswordValidationSession for PasswordValidationContext<'_> {
    fn global_vars_accessor(&self) -> &dyn GlobalVarAccessor {
        self.global_vars_accessor
    }

    fn user(&self) -> Option<&UserIdentity> {
        self.user
    }
}

impl PasswordValidationSession for variable::SessionVars {
    fn global_vars_accessor(&self) -> &dyn GlobalVarAccessor {
        self.GlobalVarsAccessor.as_ref()
    }

    fn user(&self) -> Option<&UserIdentity> {
        self.User.as_ref()
    }
}

/// 读取并解析整数型全局系统变量；解析失败包装为 InvalidSystemVariableValue。
fn parse_global_count(
    global_vars: &dyn GlobalVarAccessor,
    name: &'static str,
) -> PasswordValidationResult<i64> {
    let value = global_vars.get_global_sys_var(name)?;
    value.parse::<i64>().map_err(
        |source| PasswordValidationError::InvalidSystemVariableValue {
            name,
            value,
            source,
        },
    )
}

/// 用 TiDB `ErrNotValidPassword` 描述符生成带稳定错误码的 InvalidPassword。
fn invalid_password(reason: &str) -> PasswordValidationError {
    let descriptor = &variable::error::ErrNotValidPassword;
    PasswordValidationError::InvalidPassword {
        code: descriptor.code,
        message: descriptor.format(&[reason]),
    }
}

/// Checks whether the password contains a valid-length word from the configured dictionary.
///
/// 检查密码是否包含字典中合法长度（字节）的词；命中返回 `false`（不通过）。
pub fn ValidateDictionaryPassword(
    pwd: &str,
    global_vars: &dyn GlobalVarAccessor,
) -> PasswordValidationResult<bool> {
    let dictionary = global_vars.get_global_sys_var(vardef::ValidatePasswordDictionary)?;
    let words: Vec<&str> = dictionary.split(';').collect();
    if words.is_empty() {
        return Ok(true);
    }

    // 与 Go 一样先整体小写，再对每个词做长度窗口与子串匹配。
    let pwd = pwd.to_lowercase();
    for word in words {
        let word_len = word.len();
        if (minPwdValidationLength..=maxPwdValidationLength).contains(&word_len)
            && pwd.contains(&word.to_lowercase())
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Checks for the authenticated/login user name and its byte-reversed form.
///
/// 检查密码是否包含 auth/login 用户名或其字节反序；返回警告文案或空串。
pub fn ValidateUserNameInPassword(
    pwd: &str,
    current_user: Option<&UserIdentity>,
    global_vars: &dyn GlobalVarAccessor,
) -> PasswordValidationResult<String> {
    let check_user_name = global_vars.get_global_sys_var(vardef::ValidatePasswordCheckUserName)?;
    if !TiDBOptOn(&check_user_name) {
        return Ok(String::new());
    }

    let Some(current_user) = current_user else {
        return Ok(String::new());
    };
    // 按字节窗口匹配（大小写敏感），先 auth_username 再 username。
    let pwd_bytes = pwd.as_bytes();
    for username in [&current_user.auth_username, &current_user.username] {
        let username_bytes = username.as_bytes();
        if username_bytes.is_empty() {
            continue;
        }
        if pwd_bytes
            .windows(username_bytes.len())
            .any(|window| window == username_bytes)
        {
            return Ok("Password Contains User Name".to_owned());
        }

        let reversed: Vec<u8> = username_bytes.iter().rev().copied().collect();
        if pwd_bytes
            .windows(reversed.len())
            .any(|window| window == reversed.as_slice())
        {
            return Ok("Password Contains Reversed User Name".to_owned());
        }
    }
    Ok(String::new())
}

/// Applies the LOW policy's Unicode scalar length check.
///
/// LOW 策略：按 Unicode 标量个数（对应 Go rune）检查最小长度。
pub fn ValidatePasswordLowPolicy(
    pwd: &str,
    global_vars: &dyn GlobalVarAccessor,
) -> PasswordValidationResult<String> {
    let validate_length = parse_global_count(global_vars, vardef::ValidatePasswordLength)?;
    if (pwd.chars().count() as i64) < validate_length {
        return Ok(format!("Require Password Length: {validate_length}"));
    }
    Ok(String::new())
}

/// Applies the MEDIUM policy in the same lower/upper/digit/special error order as Go.
///
/// MEDIUM 策略：统计大小写字母、十进制数字与其余字符，报错顺序与 Go 相同。
pub fn ValidatePasswordMediumPolicy(
    pwd: &str,
    global_vars: &dyn GlobalVarAccessor,
) -> PasswordValidationResult<String> {
    let mut lower_case_count = 0_i64;
    let mut upper_case_count = 0_i64;
    let mut number_count = 0_i64;
    let mut special_char_count = 0_i64;

    // Go unicode.IsUpper/IsLower 使用 Unicode case property；数字使用 Nd。
    for rune in pwd.chars() {
        if rune.is_uppercase() {
            upper_case_count += 1;
        } else if rune.is_lowercase() {
            lower_case_count += 1;
        } else if get_general_category(rune) == GeneralCategory::DecimalNumber {
            number_count += 1;
        } else {
            special_char_count += 1;
        }
    }

    let mixed_case_count = parse_global_count(global_vars, vardef::ValidatePasswordMixedCaseCount)?;
    if lower_case_count < mixed_case_count {
        return Ok(format!(
            "Require Password Lowercase Count: {mixed_case_count}"
        ));
    }
    if upper_case_count < mixed_case_count {
        return Ok(format!(
            "Require Password Uppercase Count: {mixed_case_count}"
        ));
    }

    let require_number_count =
        parse_global_count(global_vars, vardef::ValidatePasswordNumberCount)?;
    if number_count < require_number_count {
        return Ok(format!(
            "Require Password Digit Count: {require_number_count}"
        ));
    }

    let require_special_char_count =
        parse_global_count(global_vars, vardef::ValidatePasswordSpecialCharCount)?;
    if special_char_count < require_special_char_count {
        return Ok(format!(
            "Require Password Non-alphanumeric Count: {require_special_char_count}"
        ));
    }
    Ok(String::new())
}

/// Validates a password with the configured LOW, MEDIUM, or STRONG policy.
///
/// 总入口：先用户名与 LOW；MEDIUM 再加字符类别；STRONG 再加字典检查。
pub fn ValidatePassword<S: PasswordValidationSession + ?Sized>(
    session_vars: &S,
    pwd: &str,
) -> PasswordValidationResult<()> {
    let global_vars = session_vars.global_vars_accessor();
    let validate_policy = global_vars.get_global_sys_var(vardef::ValidatePasswordPolicy)?;

    let warning = ValidateUserNameInPassword(pwd, session_vars.user(), global_vars)?;
    if !warning.is_empty() {
        return Err(invalid_password(&warning));
    }

    let warning = ValidatePasswordLowPolicy(pwd, global_vars)?;
    if !warning.is_empty() {
        return Err(invalid_password(&warning));
    }
    if validate_policy == "LOW" {
        return Ok(());
    }

    let warning = ValidatePasswordMediumPolicy(pwd, global_vars)?;
    if !warning.is_empty() {
        return Err(invalid_password(&warning));
    }
    if validate_policy == "MEDIUM" {
        return Ok(());
    }

    // STRONG：字典命中则失败（ValidateDictionaryPassword 返回 false）。
    if !ValidateDictionaryPassword(pwd, global_vars)? {
        return Err(invalid_password("Password contains word in the dictionary"));
    }
    Ok(())
}
