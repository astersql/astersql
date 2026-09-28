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

// 密码策略校验 Go 对照单元测试。
//
// 对应 Go `password_validation_test.go`：字典、用户名、LOW/MEDIUM 与总入口 `ValidatePassword`
// 在 LOW→MEDIUM→STRONG 下的通过/失败样例。

#![allow(non_snake_case)]

use std::collections::HashMap;

use crate::{
    PasswordValidationContext, ValidateDictionaryPassword, ValidatePassword,
    ValidatePasswordLowPolicy, ValidatePasswordMediumPolicy, ValidateUserNameInPassword,
};
use astersql_sessionctx_vardef as vardef;
use astersql_sessionctx_variable::{Context, GlobalVarAccessor, SessionVars, VariableError};
use parser_auth::parser::auth::auth::UserIdentity;

/// Rust counterpart of Go's MockGlobalAccessor used by this test group.
///
/// Go `MockGlobalAccessor` 的 Rust 对照：内存 map 存 `validate_password_*`。
#[derive(Default)]
struct TestGlobalAccessor {
    values: HashMap<String, String>,
}

impl TestGlobalAccessor {
    /// 填入与 Go 测试一致的默认策略变量。
    fn go_defaults() -> Self {
        let mut accessor = Self::default();
        for (name, value) in [
            (vardef::ValidatePasswordPolicy, "LOW"),
            (vardef::ValidatePasswordCheckUserName, "ON"),
            (vardef::ValidatePasswordLength, "8"),
            (vardef::ValidatePasswordMixedCaseCount, "1"),
            (vardef::ValidatePasswordNumberCount, "1"),
            (vardef::ValidatePasswordSpecialCharCount, "1"),
            (vardef::ValidatePasswordDictionary, ""),
        ] {
            accessor.set(name, value);
        }
        accessor
    }

    fn set(&mut self, name: &str, value: &str) {
        self.values.insert(name.to_owned(), value.to_owned());
    }
}

impl GlobalVarAccessor for TestGlobalAccessor {
    fn get_global_sys_var(&self, name: &str) -> Result<String, VariableError> {
        self.values
            .get(name)
            .cloned()
            .ok_or_else(|| VariableError::unknown(name))
    }

    fn set_global_sys_var_only(
        &mut self,
        _ctx: &Context,
        name: &str,
        value: &str,
        _update_local: bool,
    ) -> Result<(), VariableError> {
        self.set(name, value);
        Ok(())
    }

    fn get_tidb_table_value(&self, name: &str) -> Result<String, VariableError> {
        Err(VariableError::unknown(name))
    }

    fn set_tidb_table_value(
        &mut self,
        _name: &str,
        _value: &str,
        _comment: &str,
    ) -> Result<(), VariableError> {
        Ok(())
    }
}

/// 固定 username=`user`、auth_username=`authuser`。
fn test_user() -> UserIdentity {
    UserIdentity {
        username: "user".to_owned(),
        auth_username: "authuser".to_owned(),
        ..Default::default()
    }
}

/// Go: TestValidateDictionaryPassword.
/// 字典子串命中与中文/标点词；`true` 表示通过（未命中字典）。
#[test]
fn TestValidateDictionaryPassword() {
    let mut accessor = TestGlobalAccessor::go_defaults();
    accessor.set(
        vardef::ValidatePasswordDictionary,
        "abc;123;1234;5678;HIJK;中文测试;。，；！",
    );

    for (password, expected) in [
        ("abcdefg", true),
        ("abcd123efg", true),
        ("abcd1234efg", false),
        ("abcd12345efg", false),
        ("abcd123efghij", true),
        ("abcd123efghijk", false),
        ("abcd123efghij中文测试", false),
        ("abcd123。，；！", false),
    ] {
        assert_eq!(
            ValidateDictionaryPassword(password, &accessor).unwrap(),
            expected,
            "{password}"
        );
    }
}

/// Go: TestValidateUserNameInPassword.
/// ON 时检查正/反序用户名；OFF 时全部放行。
#[test]
fn TestValidateUserNameInPassword() {
    let mut accessor = TestGlobalAccessor::go_defaults();
    let user = test_user();
    let testcases = [
        ("", ""),
        ("user", "Password Contains User Name"),
        ("authuser", "Password Contains User Name"),
        ("resu000", "Password Contains Reversed User Name"),
        ("resuhtua", "Password Contains Reversed User Name"),
        ("User", ""),
        ("authUser", ""),
        ("Resu", ""),
        ("Resuhtua", ""),
    ];

    accessor.set(vardef::ValidatePasswordCheckUserName, "ON");
    for (password, expected) in testcases {
        assert_eq!(
            ValidateUserNameInPassword(password, Some(&user), &accessor).unwrap(),
            expected,
            "{password}"
        );
    }

    accessor.set(vardef::ValidatePasswordCheckUserName, "OFF");
    for (password, _) in testcases {
        assert_eq!(
            ValidateUserNameInPassword(password, Some(&user), &accessor).unwrap(),
            "",
            "{password}"
        );
    }
}

/// Go: TestValidatePasswordLowPolicy.
/// 最小长度不足时返回 Require Password Length 提示。
#[test]
fn TestValidatePasswordLowPolicy() {
    let mut accessor = TestGlobalAccessor::go_defaults();
    accessor.set(vardef::ValidatePasswordLength, "8");

    assert_eq!(
        ValidatePasswordLowPolicy("1234", &accessor).unwrap(),
        "Require Password Length: 8"
    );
    assert_eq!(
        ValidatePasswordLowPolicy("12345678", &accessor).unwrap(),
        ""
    );

    accessor.set(vardef::ValidatePasswordLength, "12");
    assert_eq!(
        ValidatePasswordLowPolicy("12345678", &accessor).unwrap(),
        "Require Password Length: 12"
    );

    accessor.set(vardef::ValidatePasswordLength, "invalid");
    assert_eq!(
        ValidatePasswordLowPolicy("12345678", &accessor)
            .unwrap_err()
            .to_string(),
        "strconv.ParseInt: parsing \"invalid\": invalid syntax"
    );
}

/// Go: TestValidatePasswordMediumPolicy.
/// 小写→大写→数字→特殊字符的报错优先级。
#[test]
fn TestValidatePasswordMediumPolicy() {
    let mut accessor = TestGlobalAccessor::go_defaults();
    accessor.set(vardef::ValidatePasswordMixedCaseCount, "1");
    accessor.set(vardef::ValidatePasswordSpecialCharCount, "2");
    accessor.set(vardef::ValidatePasswordNumberCount, "3");

    for (password, expected) in [
        ("!@A123", "Require Password Lowercase Count: 1"),
        ("!@a123", "Require Password Uppercase Count: 1"),
        ("!@Aa12", "Require Password Digit Count: 3"),
        ("!Aa123", "Require Password Non-alphanumeric Count: 2"),
        ("!@Aa123", ""),
    ] {
        assert_eq!(
            ValidatePasswordMediumPolicy(password, &accessor).unwrap(),
            expected,
            "{password}"
        );
    }
}

#[test]
/// Go `unicode.IsUpper/IsLower` also recognizes cased characters outside Lu/Ll.
fn medium_policy_uses_unicode_case_properties() {
    let mut accessor = TestGlobalAccessor::go_defaults();
    accessor.set(vardef::ValidatePasswordMixedCaseCount, "1");
    accessor.set(vardef::ValidatePasswordSpecialCharCount, "0");
    accessor.set(vardef::ValidatePasswordNumberCount, "0");

    // U+2160/U+2170 are LetterNumber characters with Unicode upper/lower properties.
    assert_eq!(ValidatePasswordMediumPolicy("Ⅰⅰ", &accessor).unwrap(), "");
}

/// Go: TestValidatePassword.
/// LOW/MEDIUM/STRONG 分层：用户名、字符类别与字典。
#[test]
fn TestValidatePassword() {
    let mut accessor = TestGlobalAccessor::go_defaults();
    let user = test_user();

    accessor.set(vardef::ValidatePasswordPolicy, "LOW");
    {
        let session = PasswordValidationContext::new(&accessor, Some(&user));
        assert!(ValidatePassword(&session, "1234").is_err());
        assert!(ValidatePassword(&session, "user1234").is_err());
        assert!(ValidatePassword(&session, "authuser1234").is_err());
        assert!(ValidatePassword(&session, "User1234").is_ok());
    }

    accessor.set(vardef::ValidatePasswordPolicy, "MEDIUM");
    {
        let session = PasswordValidationContext::new(&accessor, Some(&user));
        assert!(ValidatePassword(&session, "User1234").is_err());
        assert!(ValidatePassword(&session, "!User1234").is_ok());
        assert!(ValidatePassword(&session, "！User1234").is_ok());
    }

    accessor.set(vardef::ValidatePasswordPolicy, "STRONG");
    accessor.set(vardef::ValidatePasswordDictionary, "User");
    let session = PasswordValidationContext::new(&accessor, Some(&user));
    assert!(ValidatePassword(&session, "!User1234").is_err());
    assert!(ValidatePassword(&session, "!ABcd1234").is_ok());
}

#[test]
fn validate_password_accepts_the_real_session_vars_type() {
    let mut session = SessionVars::new(Box::new(TestGlobalAccessor::go_defaults()));
    session.User = Some(test_user());

    assert!(ValidatePassword(&session, "User1234").is_ok());
    assert!(ValidatePassword(&session, "user1234").is_err());
}
