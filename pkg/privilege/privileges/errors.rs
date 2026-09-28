// Copyright 2018 PingCAP, Inc.
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

// 权限子系统错误类型：覆盖鉴权失败、账户锁定、授权缺失与加载失败等。

use thiserror::Error;

/// 权限相关错误枚举（对应 Go 侧 privilege 错误码语义）。
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum PrivilegeError {
    /// 无效的权限类型名。
    #[error("invalid privilege type: {0}")]
    InvalidPrivilegeType(String),
    /// 用户不存在对应 GRANT。
    #[error("there is no such grant defined for user {0}")]
    NonexistingGrant(String),
    /// 从系统表加载权限失败。
    #[error("failed to load privilege: {0}")]
    LoadPrivilege(String),
    /// 访问被拒绝（用户/主机不匹配或无权限）。
    #[error("access denied for user '{user}'@'{host}'")]
    AccessDenied { user: String, host: String },
    /// 账户已被锁定。
    #[error("account '{user}'@'{host}' is locked")]
    AccountLocked { user: String, host: String },
    /// 因连续登录失败触发密码锁定（FAILED_LOGIN_ATTEMPTS）。
    #[error(
        "account '{user}'@'{host}' blocked after {attempts} failed logins ({remaining} remaining)"
    )]
    PasswordLock {
        user: String,
        host: String,
        attempts: i64,
        remaining: String,
    },
    /// 必须先修改密码才能登录。
    #[error("password must be changed before login")]
    MustChangePassword,
    /// 权限系统表不存在。
    #[error("table does not exist: {0}")]
    NoSuchTable(String),
    /// 认证过程失败。
    #[error("authentication failed: {0}")]
    Authentication(String),
    /// 权限相关 JSON（如 user_attributes）解析失败。
    #[error("invalid JSON: {0}")]
    InvalidJson(String),
    /// I/O 错误。
    #[error("I/O error: {0}")]
    Io(String),
}

/// 与 Go `dbterror.ClassPrivilege.NewStd` 对应的标准错误定义。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrivilegeErrorKind {
    name: &'static str,
    code: u16,
    message_template: &'static str,
}

impl PrivilegeErrorKind {
    const fn new(name: &'static str, code: u16, message_template: &'static str) -> Self {
        Self {
            name,
            code,
            message_template,
        }
    }

    /// Go 变量/错误常量名称。
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// MySQL/TiDB 标准错误码。
    pub const fn code(self) -> u16 {
        self.code
    }

    /// `pkg/errno/errname.go` 中的格式化消息模板。
    pub const fn message_template(self) -> &'static str {
        self.message_template
    }
}

/// 无效权限类型。
pub const ErrInvalidPrivilegeType: PrivilegeErrorKind =
    PrivilegeErrorKind::new("ErrInvalidPrivilegeType", 8050, "unknown privilege type %s");
/// 不存在的 GRANT。
pub const ErrNonexistingGrant: PrivilegeErrorKind = PrivilegeErrorKind::new(
    "ErrNonexistingGrant",
    1141,
    "There is no such grant defined for user '%-.48s' on host '%-.255s'",
);
/// 加载权限失败。
pub const ErrLoadPrivilege: PrivilegeErrorKind =
    PrivilegeErrorKind::new("ErrLoadPrivilege", 8049, "Load privilege table fail: %s");
/// 访问拒绝。
pub const ErrAccessDenied: PrivilegeErrorKind = PrivilegeErrorKind::new(
    "ErrAccessDenied",
    1045,
    "Access denied for user '%-.48s'@'%-.255s' (using password: %s)",
);
/// 账户已锁定。
pub const ErrAccountHasBeenLocked: PrivilegeErrorKind = PrivilegeErrorKind::new(
    "ErrAccountHasBeenLocked",
    3118,
    "Access denied for user '%s'@'%s'. Account is locked.",
);
/// 因密码锁定策略拒绝访问。
pub const ErUserAccessDeniedForUserAccountBlockedByPasswordLock: PrivilegeErrorKind =
    PrivilegeErrorKind::new(
        "ErUserAccessDeniedForUserAccountBlockedByPasswordLock",
        3955,
        "Access denied for user '%s'@'%s'. Account is blocked for %s day(s) (%s day(s) remaining) due to %d consecutive failed logins.",
    );
/// 登录前必须修改密码。
pub const ErrMustChangePasswordLogin: PrivilegeErrorKind = PrivilegeErrorKind::new(
    "ErrMustChangePasswordLogin",
    1862,
    "Your password has expired. To log in you must change it using a client that supports expired passwords.",
);
