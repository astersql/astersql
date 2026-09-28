// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Server 错误绑定迁移对照测试。
//
// 校验 ClassServer 标准错误的 Code / RFCCode / MessageTemplate 与 errno 表一致，
// 并确认 AccessDenied 与 AccessDeniedNoPassword 变体彼此独立。

use astersql_server_err::{errno, errors::Error, server_err::*};
use std::process::Command;

/// 断言错误码、RFC 风格码与标准消息模板均绑定到给定 errno。
fn assert_standard(error: &Error, code: u16) {
    assert_eq!(error.Code(), i32::from(code));
    assert_eq!(error.RFCCode(), format!("server:{code}"));
    assert_eq!(
        error.MessageTemplate(),
        errno::MySQLErrName[&code].Raw,
        "server error must retain errno's standard message",
    );
}

/// 逐项对照 Go 声明顺序，确认全部 Server 标准错误绑定未漂移。
#[test]
fn server_errors_keep_go_class_code_and_standard_message_bindings() {
    assert_standard(ErrInvalidType.as_ref(), errno::ErrInvalidType);
    assert_standard(ErrInvalidSequence.as_ref(), errno::ErrInvalidSequence);
    assert_standard(ErrNotAllowedCommand.as_ref(), errno::ErrNotAllowedCommand);
    assert_standard(ErrAccessDenied.as_ref(), errno::ErrAccessDenied);
    assert_standard(
        ErrAccessDeniedNoPassword.as_ref(),
        errno::ErrAccessDeniedNoPassword,
    );
    assert_standard(ErrConCount.as_ref(), errno::ErrConCount);
    assert_standard(
        ErrTooManyUserConnections.as_ref(),
        errno::ErrTooManyUserConnections,
    );
    assert_standard(
        ErrSecureTransportRequired.as_ref(),
        errno::ErrSecureTransportRequired,
    );
    assert_standard(ErrUserPrefixMismatch.as_ref(), errno::ErrUserPrefixMismatch);
    assert_standard(
        ErrMultiStatementDisabled.as_ref(),
        errno::ErrMultiStatementDisabled,
    );
    assert_standard(
        ErrNewAbortingConnection.as_ref(),
        errno::ErrNewAbortingConnection,
    );
    assert_standard(
        ErrNotSupportedAuthMode.as_ref(),
        errno::ErrNotSupportedAuthMode,
    );
    assert_standard(ErrNetPacketTooLarge.as_ref(), errno::ErrNetPacketTooLarge);
    assert_standard(ErrMustChangePassword.as_ref(), errno::ErrMustChangePassword);
    assert_standard(ErrServerShutdown.as_ref(), errno::ErrServerShutdown);
}

/// AccessDenied 与 NoPassword 变体必须保持不同码与消息模板。
#[test]
fn access_denied_variants_remain_distinct_go_errors() {
    assert_ne!(ErrAccessDenied.Code(), ErrAccessDeniedNoPassword.Code());
    assert_ne!(
        ErrAccessDenied.RFCCode(),
        ErrAccessDeniedNoPassword.RFCCode()
    );
    assert_ne!(
        ErrAccessDenied.MessageTemplate(),
        ErrAccessDeniedNoPassword.MessageTemplate(),
    );
}

/// Go 包变量会在 `RegisterFinish` 前完成注册；冻结后首次读取 Server 错误不得再注册。
#[test]
fn server_errors_are_registered_before_registry_freeze() {
    const FREEZE_HELPER_ENV: &str = "SERVER_ERR_REGISTER_FINISH_HELPER";

    if std::env::var_os(FREEZE_HELPER_ENV).is_some() {
        astersql_server_err::terror::RegisterFinish();
        let access_after_freeze = std::panic::catch_unwind(|| {
            assert_eq!(ErrInvalidType.Code(), errno::ErrInvalidType as i32);
        });
        assert!(
            access_after_freeze.is_ok(),
            "Server errors must be eagerly registered before RegisterFinish"
        );
        return;
    }

    let status = Command::new(std::env::current_exe().expect("current test executable"))
        .arg("--exact")
        .arg("migration_aster_unit_test::server_errors_are_registered_before_registry_freeze")
        .env(FREEZE_HELPER_ENV, "1")
        .status()
        .expect("run Server registration subprocess helper");
    assert!(
        status.success(),
        "Server registration helper failed: {status}"
    );
}
