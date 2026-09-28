// Copyright 2026 AsterSQL.
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

// exeerrors 迁移回归测试：锁定 Go 错误码、错误类与自定义消息模板。
//
// 验证标准错误的 Code/RFCCode、NewStdErr 自定义模板，以及其余实例
// 的标准消息与 errno 表一致。

use dbterror_exeerrors::{errno as mysql, exeerrors::*};

/// 断言错误实例的数值码与 RFC 形式（`class:code`）与期望一致。
macro_rules! assert_error {
    ($error:ident, $code:expr, $class:literal) => {{
        assert_eq!($error.Code(), $code as i32, stringify!($error));
        assert_eq!(
            $error.RFCCode(),
            format!("{}:{}", $class, $code),
            stringify!($error)
        );
    }};
}

/// 断言 `NewStd` 错误除码值与错误类外，还继承对应 errno 的标准消息。
macro_rules! assert_standard_error {
    ($error:ident, $code:ident, $class:literal) => {{
        assert_error!($error, mysql::$code, $class);
        assert_eq!(
            $error.GetMsg(),
            mysql::MySQLErrName[&mysql::$code].Raw,
            stringify!($error)
        );
    }};
}

#[test]
/// 抽检若干标准错误的码值与错误类（executor/privilege/ddl）。
fn standard_errors_preserve_go_codes_and_classes() {
    assert_standard_error!(ErrGetStartTS, ErrGetStartTS, "executor");
    assert_standard_error!(ErrUnknownPlan, ErrUnknownPlan, "executor");
    assert_standard_error!(ErrSubqueryMoreThan1Row, ErrSubqueryNo1Row, "executor");
    assert_standard_error!(ErrColumnsNotMatched, ErrColumnNotMatched, "executor");
    assert_standard_error!(ErrRoleNotGranted, ErrRoleNotGranted, "privilege");
    assert_standard_error!(ErrDeadlock, ErrLockDeadlock, "executor");
    assert_standard_error!(ErrSavepointNotExists, ErrSpDoesNotExist, "executor");
    assert_standard_error!(ErrWrongStringLength, ErrWrongStringLength, "ddl");
    assert_standard_error!(
        ErrLoadDataPreCheckFailed,
        ErrLoadDataPreCheckFailed,
        "executor"
    );
    assert_standard_error!(ErrMaxKeysReadExceeded, ErrMaxKeysReadExceeded, "executor");
}

#[test]
/// 校验 NewStdErr 自定义消息模板及所属错误类。
fn custom_errors_preserve_go_templates_and_classes() {
    assert_error!(ErrFuncNotEnabled, mysql::ErrNotSupportedYet, "executor");
    assert_eq!(
        ErrFuncNotEnabled.GetMsg(),
        "%-.32s is not supported. To enable this experimental feature, set '%-.32s' in the configuration file."
    );

    assert_error!(
        ErrUnsupportedFlashbackTmpTable,
        mysql::ErrUnsupportedDDLOperation,
        "ddl"
    );
    assert_eq!(
        ErrUnsupportedFlashbackTmpTable.GetMsg(),
        "Recover/flashback table is not supported on temporary tables"
    );

    assert_error!(
        ErrTruncateWrongInsertValue,
        mysql::ErrTruncatedWrongValue,
        "table"
    );
    assert_eq!(
        ErrTruncateWrongInsertValue.GetMsg(),
        "Incorrect %-.32s value: '%-.128s' for column '%.192s' at row %d"
    );

    assert_error!(ErrUserNameNeedPrefix, mysql::ErrUsername, "ddl");
    assert_eq!(
        ErrUserNameNeedPrefix.GetMsg(),
        "User name must start with `%s.` (use `%s.%s` instead)"
    );
}

#[test]
/// 批量确认其余 executor 错误实例消息等于 MySQLErrName 原文。
fn all_go_error_instances_initialize_with_their_standard_message() {
    macro_rules! check {
        ($error:ident, $code:ident) => {{
            assert_error!($error, mysql::$code, "executor");
            assert_eq!($error.GetMsg(), mysql::MySQLErrName[&mysql::$code].Raw);
        }};
    }

    check!(ErrPrepareMulti, ErrPrepareMulti);
    check!(ErrPrepareDDL, ErrPrepareDDL);
    check!(ErrResultIsEmpty, ErrResultIsEmpty);
    check!(ErrBuildExecutor, ErrBuildExecutor);
    check!(ErrBatchInsertFail, ErrBatchInsertFail);
    check!(ErrUnsupportedPs, ErrUnsupportedPs);
    check!(ErrIllegalGrantForTable, ErrIllegalGrantForTable);
    check!(ErrCantCreateUserWithGrant, ErrCantCreateUserWithGrant);
    check!(ErrPasswordNoMatch, ErrPasswordNoMatch);
    check!(ErrCannotUser, ErrCannotUser);
    check!(ErrGrantRole, ErrGrantRole);
    check!(ErrPasswordFormat, ErrPasswordFormat);
    check!(
        ErrCantChangeTxCharacteristics,
        ErrCantChangeTxCharacteristics
    );
    check!(ErrPsManyParam, ErrPsManyParam);
    check!(ErrAdminCheckTable, ErrAdminCheckTable);
    check!(ErrDBaccessDenied, ErrDBaccessDenied);
    check!(ErrTableaccessDenied, ErrTableaccessDenied);
    check!(ErrBadDB, ErrBadDB);
    check!(ErrWrongObject, ErrWrongObject);
    check!(ErrWrongUsage, ErrWrongUsage);
    check!(ErrQueryInterrupted, ErrQueryInterrupted);
    check!(ErrMaxExecTimeExceeded, ErrMaxExecTimeExceeded);
    check!(
        ErrResourceGroupQueryRunawayInterrupted,
        ErrResourceGroupQueryRunawayInterrupted
    );
    check!(ErrQueryExecStopped, ErrQueryExecStopped);
    check!(
        ErrResourceGroupQueryRunawayQuarantine,
        ErrResourceGroupQueryRunawayQuarantine
    );
    check!(
        ErrDynamicPrivilegeNotRegistered,
        ErrDynamicPrivilegeNotRegistered
    );
    check!(ErrIllegalPrivilegeLevel, ErrIllegalPrivilegeLevel);
    check!(ErrInvalidSplitRegionRanges, ErrInvalidSplitRegionRanges);
    check!(ErrViewInvalid, ErrViewInvalid);
    check!(ErrInstanceScope, ErrInstanceScope);
    check!(ErrSettingNoopVariable, ErrSettingNoopVariable);
    check!(ErrLazyUniquenessCheckFailure, ErrLazyUniquenessCheckFailure);
    check!(ErrMemoryExceedForQuery, ErrMemoryExceedForQuery);
    check!(ErrMemoryExceedForInstance, ErrMemoryExceedForInstance);
    check!(ErrDeleteNotFoundColumn, ErrDeleteNotFoundColumn);
    check!(ErrBRIEBackupFailed, ErrBRIEBackupFailed);
    check!(ErrBRIERestoreFailed, ErrBRIERestoreFailed);
    check!(ErrBRIEImportFailed, ErrBRIEImportFailed);
    check!(ErrBRIEExportFailed, ErrBRIEExportFailed);
    check!(ErrBRJobNotFound, ErrBRJobNotFound);
    check!(ErrCTEMaxRecursionDepth, ErrCTEMaxRecursionDepth);
    check!(ErrPluginIsNotLoaded, ErrPluginIsNotLoaded);
    check!(ErrSetPasswordAuthPlugin, ErrSetPasswordAuthPlugin);
    check!(
        ErrForeignKeyCascadeDepthExceeded,
        ErrForeignKeyCascadeDepthExceeded
    );
    check!(
        ErrPasswordExpireAnonymousUser,
        ErrPasswordExpireAnonymousUser
    );
    check!(ErrMustChangePassword, ErrMustChangePassword);
    check!(
        ErrSecondPasswordCannotBeEmpty,
        ErrSecondPasswordCannotBeEmpty
    );
    check!(
        ErrPasswordCannotBeRetainedOnPluginChange,
        ErrPasswordCannotBeRetainedOnPluginChange
    );
    check!(
        ErrCurrentPasswordCannotBeRetained,
        ErrCurrentPasswordCannotBeRetained
    );
    check!(ErrExistsInHistoryPassword, ErrExistsInHistoryPassword);
    check!(ErrWarnTooFewRecords, ErrWarnTooFewRecords);
    check!(ErrWarnTooManyRecords, ErrWarnTooManyRecords);
    check!(ErrLoadDataFromServerDisk, ErrLoadDataFromServerDisk);
    check!(ErrLoadParquetFromLocal, ErrLoadParquetFromLocal);
    check!(ErrLoadDataEmptyPath, ErrLoadDataEmptyPath);
    check!(ErrLoadDataUnsupportedFormat, ErrLoadDataUnsupportedFormat);
    check!(ErrLoadDataInvalidURI, ErrLoadDataInvalidURI);
    check!(ErrLoadDataCantAccess, ErrLoadDataCantAccess);
    check!(ErrLoadDataCantRead, ErrLoadDataCantRead);
    check!(ErrLoadDataWrongFormatConfig, ErrLoadDataWrongFormatConfig);
    check!(ErrUnknownOption, ErrUnknownOption);
    check!(ErrInvalidOptionVal, ErrInvalidOptionVal);
    check!(ErrDuplicateOption, ErrDuplicateOption);
    check!(ErrLoadDataUnsupportedOption, ErrLoadDataUnsupportedOption);
    check!(
        ErrLoadDataDuplicateKeyConflict,
        ErrLoadDataDuplicateKeyConflict
    );
    check!(ErrLoadDataJobNotFound, ErrLoadDataJobNotFound);
    check!(ErrLoadDataInvalidOperation, ErrLoadDataInvalidOperation);
    check!(
        ErrLoadDataLocalUnsupportedOption,
        ErrLoadDataLocalUnsupportedOption
    );
}
