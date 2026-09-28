// Copyright 2026 AsterSQL.

// 密码策略校验迁移补充单元测试。
//
// 用 mock 全局变量访问器对照 Go：字典词匹配与字节长度窗口、用户名正/反序、
// Unicode 标量长度（LOW）、字符类别优先级（MEDIUM）、以及 LOW/MEDIUM/STRONG
// 总流程与系统变量查找顺序/错误形态。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::{
    PasswordValidationContext, PasswordValidationError, ValidateDictionaryPassword,
    ValidatePassword, ValidatePasswordLowPolicy, ValidatePasswordMediumPolicy,
    ValidateUserNameInPassword,
};
use astersql_sessionctx_vardef as vardef;
use astersql_sessionctx_variable::{Context, GlobalVarAccessor, VariableError, VariableErrorKind};
use parser_auth::parser::auth::auth::UserIdentity;

/// 可记录读变量调用顺序的 mock 全局变量访问器。
#[derive(Default)]
struct TestGlobalAccessor {
    values: HashMap<String, String>,
    calls: Arc<Mutex<Vec<String>>>,
}

impl TestGlobalAccessor {
    /// 填入与 Go 测试一致的 `validate_password_*` 默认值。
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

    /// 返回已发生的 `get_global_sys_var` 名称序列。
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl GlobalVarAccessor for TestGlobalAccessor {
    fn get_global_sys_var(&self, name: &str) -> Result<String, VariableError> {
        self.calls.lock().unwrap().push(name.to_owned());
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

/// 固定 username=`user`、auth_username=`authuser` 的测试身份。
fn user() -> UserIdentity {
    UserIdentity {
        username: "user".to_owned(),
        auth_username: "authuser".to_owned(),
        ..Default::default()
    }
}

/// 字典匹配：子串命中、中文/标点词、以及超长字典项不参与匹配。
#[test]
fn dictionary_validation_matches_go_cases_and_byte_length_limits() {
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

    // 超过 maxPwdValidationLength 的词忽略；合法长度词仍可命中。
    accessor.set(
        vardef::ValidatePasswordDictionary,
        &format!("{};CaSe", "x".repeat(101)),
    );
    assert!(!ValidateDictionaryPassword("prefix-case-suffix", &accessor).unwrap());
    assert!(ValidateDictionaryPassword(&format!("a{}z", "x".repeat(101)), &accessor).unwrap());
}

/// 用户名正序/反序匹配大小写敏感；开关 OFF 或无用户时跳过。
#[test]
fn username_validation_matches_go_order_case_and_switch() {
    let mut accessor = TestGlobalAccessor::go_defaults();
    let user = user();
    for (password, expected) in [
        ("", ""),
        ("user", "Password Contains User Name"),
        ("authuser", "Password Contains User Name"),
        ("resu000", "Password Contains Reversed User Name"),
        ("resuhtua", "Password Contains Reversed User Name"),
        ("User", ""),
        ("authUser", ""),
        ("Resu", ""),
        ("Resuhtua", ""),
    ] {
        assert_eq!(
            ValidateUserNameInPassword(password, Some(&user), &accessor).unwrap(),
            expected,
            "{password}"
        );
    }

    accessor.set(vardef::ValidatePasswordCheckUserName, "OFF");
    assert_eq!(
        ValidateUserNameInPassword("authuser", Some(&user), &accessor).unwrap(),
        ""
    );
    assert_eq!(
        ValidateUserNameInPassword("anything", None, &accessor).unwrap(),
        ""
    );
}

/// LOW 策略按 Unicode 标量计长；非法长度变量值返回 InvalidSystemVariableValue。
#[test]
fn low_policy_counts_unicode_scalars_like_go_runes() {
    let mut accessor = TestGlobalAccessor::go_defaults();
    accessor.set(vardef::ValidatePasswordLength, "4");
    assert_eq!(
        ValidatePasswordLowPolicy("密碼a", &accessor).unwrap(),
        "Require Password Length: 4"
    );
    assert_eq!(ValidatePasswordLowPolicy("密碼ab", &accessor).unwrap(), "");

    accessor.set(vardef::ValidatePasswordLength, "invalid");
    assert!(matches!(
        ValidatePasswordLowPolicy("password", &accessor),
        Err(PasswordValidationError::InvalidSystemVariableValue { name, .. })
            if name == vardef::ValidatePasswordLength
    ));
}

/// MEDIUM：小写→大写→数字→特殊字符的报错优先级；Unicode 数字/字母类别与 Go 一致。
#[test]
fn medium_policy_matches_go_priority_and_unicode_categories() {
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
        ("!@Aa١٢٣", ""),
    ] {
        assert_eq!(
            ValidatePasswordMediumPolicy(password, &accessor).unwrap(),
            expected,
            "{password}"
        );
    }

    accessor.set(vardef::ValidatePasswordNumberCount, "0");
    accessor.set(vardef::ValidatePasswordSpecialCharCount, "3");
    assert_eq!(
        ValidatePasswordMediumPolicy("AaⅧ!?", &accessor).unwrap(),
        "Require Password Non-alphanumeric Count: 3"
    );
}

/// ValidatePassword：LOW→MEDIUM→STRONG 分层，STRONG 额外走字典。
#[test]
fn validate_password_matches_go_low_medium_and_strong_flow() {
    let mut accessor = TestGlobalAccessor::go_defaults();
    let user = user();

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

/// 保留 Go 的变量查找顺序与错误码 1819 / UnknownSystemVariable 形态。
#[test]
fn validate_password_preserves_go_lookup_order_and_error_shape() {
    let mut accessor = TestGlobalAccessor::go_defaults();
    accessor.set(vardef::ValidatePasswordPolicy, "LOW");
    let user = user();
    let session = PasswordValidationContext::new(&accessor, Some(&user));

    let error = ValidatePassword(&session, "user1234").unwrap_err();
    assert!(matches!(
        error,
        PasswordValidationError::InvalidPassword { code: 1819, ref message }
            if message == "Your password does not satisfy the current policy requirements (Password Contains User Name)"
    ));
    assert_eq!(
        &accessor.calls()[..2],
        [
            vardef::ValidatePasswordPolicy.to_owned(),
            vardef::ValidatePasswordCheckUserName.to_owned(),
        ]
    );

    let missing = TestGlobalAccessor::default();
    let session = PasswordValidationContext::new(&missing, None);
    assert!(matches!(
        ValidatePassword(&session, "anything"),
        Err(PasswordValidationError::SystemVariable(ref error))
            if error.kind() == VariableErrorKind::UnknownSystemVariable
    ));
}
