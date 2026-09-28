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

// Executor 预构造错误实例表。
//
// 对应 Go `pkg/util/dbterror/exeerrors/errors.go`：将 MySQL 错误码映射到
// `ClassExecutor` / `ClassPrivilege` / `ClassDDL` / `ClassTable` 等错误类，
// 以 `LazyLock` 在首次访问时构造（对应 Go 包级 var 初始化）。

// 本文件由 pkg/util/dbterror/exeerrors/errors.go 迁移而来，保留 executor 错误码到 dbterror 错误类的包级映射。

use std::sync::LazyLock;

use crate::errno as mysql;
use crate::parser::mysql::errname as parser_mysql;

// 保持 Go 源码中的 dbterror 限定名，同时让静态项显式持有构造器返回的 boxed terror Error。
/// 局部别名：复用上层错误类，并将构造结果统一为 `Box<terror::Error>`。
mod dbterror {
    pub use crate::dbterror::{ClassDDL, ClassExecutor, ClassPrivilege, ClassTable};

    /// 与 Go `*terror.Error` 指针语义对齐的 boxed 错误类型别名。
    pub type Error = Box<crate::terror::Error>;
}

// Error instances.
// 下列变量保持 Go var 块顺序；LazyLock 对应 Go 的包级初始化，并复用 dbterror 的真实构造器。
/// 获取事务 StartTS（事务开始时间戳）失败。
pub static ErrGetStartTS: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrGetStartTS));
pub static ErrUnknownPlan: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrUnknownPlan));
pub static ErrPrepareMulti: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrPrepareMulti));
pub static ErrPrepareDDL: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrPrepareDDL));
pub static ErrResultIsEmpty: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrResultIsEmpty));
pub static ErrBuildExecutor: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrBuildExecutor));
pub static ErrBatchInsertFail: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrBatchInsertFail));
pub static ErrUnsupportedPs: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrUnsupportedPs));
pub static ErrSubqueryMoreThan1Row: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrSubqueryNo1Row));
pub static ErrIllegalGrantForTable: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrIllegalGrantForTable));
pub static ErrColumnsNotMatched: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrColumnNotMatched));

pub static ErrCantCreateUserWithGrant: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrCantCreateUserWithGrant));
pub static ErrPasswordNoMatch: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrPasswordNoMatch));
pub static ErrCannotUser: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrCannotUser));
pub static ErrGrantRole: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrGrantRole));
pub static ErrPasswordFormat: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrPasswordFormat));
pub static ErrCantChangeTxCharacteristics: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrCantChangeTxCharacteristics));
pub static ErrPsManyParam: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrPsManyParam));
pub static ErrAdminCheckTable: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrAdminCheckTable));
pub static ErrDBaccessDenied: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrDBaccessDenied));
pub static ErrTableaccessDenied: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrTableaccessDenied));
pub static ErrBadDB: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrBadDB));
pub static ErrWrongObject: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrWrongObject));
pub static ErrWrongUsage: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrWrongUsage));
pub static ErrRoleNotGranted: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassPrivilege.NewStd(mysql::ErrRoleNotGranted));
/// 死锁：事务因锁冲突被中止（映射 `ErrLockDeadlock`）。
pub static ErrDeadlock: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrLockDeadlock));
pub static ErrQueryInterrupted: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrQueryInterrupted));
pub static ErrMaxExecTimeExceeded: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrMaxExecTimeExceeded));
pub static ErrResourceGroupQueryRunawayInterrupted: LazyLock<dbterror::Error> =
    LazyLock::new(|| {
        dbterror::ClassExecutor.NewStd(mysql::ErrResourceGroupQueryRunawayInterrupted)
    });
pub static ErrQueryExecStopped: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrQueryExecStopped));
pub static ErrResourceGroupQueryRunawayQuarantine: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrResourceGroupQueryRunawayQuarantine));
pub static ErrDynamicPrivilegeNotRegistered: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrDynamicPrivilegeNotRegistered));
pub static ErrIllegalPrivilegeLevel: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrIllegalPrivilegeLevel));
/// 非法的 Region（数据分片）切分范围。
pub static ErrInvalidSplitRegionRanges: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrInvalidSplitRegionRanges));
pub static ErrViewInvalid: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrViewInvalid));
pub static ErrInstanceScope: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrInstanceScope));
pub static ErrSettingNoopVariable: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrSettingNoopVariable));
pub static ErrLazyUniquenessCheckFailure: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrLazyUniquenessCheckFailure));
pub static ErrMemoryExceedForQuery: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrMemoryExceedForQuery));
pub static ErrMemoryExceedForInstance: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrMemoryExceedForInstance));
pub static ErrDeleteNotFoundColumn: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrDeleteNotFoundColumn));

pub static ErrBRIEBackupFailed: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrBRIEBackupFailed));
pub static ErrBRIERestoreFailed: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrBRIERestoreFailed));
pub static ErrBRIEImportFailed: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrBRIEImportFailed));
pub static ErrBRIEExportFailed: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrBRIEExportFailed));
pub static ErrBRJobNotFound: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrBRJobNotFound));
/// CTE（公共表表达式）递归深度超过上限。
pub static ErrCTEMaxRecursionDepth: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrCTEMaxRecursionDepth));
pub static ErrPluginIsNotLoaded: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrPluginIsNotLoaded));
pub static ErrSetPasswordAuthPlugin: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrSetPasswordAuthPlugin));
/// 实验特性未启用：使用自定义消息模板而非标准 errno 文案。
pub static ErrFuncNotEnabled: LazyLock<dbterror::Error> = LazyLock::new(|| {
    // Go 这里用 parser_mysql.Message 保存格式化模板和 nil 参数；保留模板文本。
    dbterror::ClassExecutor.NewStdErr(
        mysql::ErrNotSupportedYet,
        &parser_mysql::Message(
            "%-.32s is not supported. To enable this experimental feature, set '%-.32s' in the configuration file.",
            &[],
        ),
    )
});
pub static ErrSavepointNotExists: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrSpDoesNotExist));
pub static ErrForeignKeyCascadeDepthExceeded: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrForeignKeyCascadeDepthExceeded));
pub static ErrPasswordExpireAnonymousUser: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrPasswordExpireAnonymousUser));
pub static ErrMustChangePassword: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrMustChangePassword));

// Dual-password (MySQL 8.0 RETAIN CURRENT PASSWORD / DISCARD OLD PASSWORD) errors.
// 双密码特性错误保持 Go 中的三项连续声明，均归到 executor 错误类。
/// 双密码特性：第二个密码不能为空。
pub static ErrSecondPasswordCannotBeEmpty: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrSecondPasswordCannotBeEmpty));
pub static ErrPasswordCannotBeRetainedOnPluginChange: LazyLock<dbterror::Error> =
    LazyLock::new(|| {
        dbterror::ClassExecutor.NewStd(mysql::ErrPasswordCannotBeRetainedOnPluginChange)
    });
pub static ErrCurrentPasswordCannotBeRetained: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrCurrentPasswordCannotBeRetained));

pub static ErrWrongStringLength: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassDDL.NewStd(mysql::ErrWrongStringLength));
pub static ErrUnsupportedFlashbackTmpTable: LazyLock<dbterror::Error> = LazyLock::new(|| {
    dbterror::ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::Message(
            "Recover/flashback table is not supported on temporary tables",
            &[],
        ),
    )
});
pub static ErrTruncateWrongInsertValue: LazyLock<dbterror::Error> = LazyLock::new(|| {
    dbterror::ClassTable.NewStdErr(
        mysql::ErrTruncatedWrongValue,
        &parser_mysql::Message(
            "Incorrect %-.32s value: '%-.128s' for column '%.192s' at row %d",
            &[],
        ),
    )
});
pub static ErrExistsInHistoryPassword: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrExistsInHistoryPassword));
pub static ErrUserNameNeedPrefix: LazyLock<dbterror::Error> = LazyLock::new(|| {
    dbterror::ClassDDL.NewStdErr(
        mysql::ErrUsername,
        &parser_mysql::Message("User name must start with `%s.` (use `%s.%s` instead)", &[]),
    )
});

pub static ErrWarnTooFewRecords: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrWarnTooFewRecords));
pub static ErrWarnTooManyRecords: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrWarnTooManyRecords));
/// LOAD DATA 从服务器本地盘读取相关错误组起始项。
pub static ErrLoadDataFromServerDisk: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrLoadDataFromServerDisk));
pub static ErrLoadParquetFromLocal: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrLoadParquetFromLocal));
pub static ErrLoadDataEmptyPath: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrLoadDataEmptyPath));
pub static ErrLoadDataUnsupportedFormat: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrLoadDataUnsupportedFormat));
pub static ErrLoadDataInvalidURI: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrLoadDataInvalidURI));
pub static ErrLoadDataCantAccess: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrLoadDataCantAccess));
pub static ErrLoadDataCantRead: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrLoadDataCantRead));
pub static ErrLoadDataWrongFormatConfig: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrLoadDataWrongFormatConfig));
pub static ErrUnknownOption: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrUnknownOption));
pub static ErrInvalidOptionVal: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrInvalidOptionVal));
pub static ErrDuplicateOption: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrDuplicateOption));
pub static ErrLoadDataUnsupportedOption: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrLoadDataUnsupportedOption));
pub static ErrLoadDataDuplicateKeyConflict: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrLoadDataDuplicateKeyConflict));
pub static ErrLoadDataJobNotFound: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrLoadDataJobNotFound));
pub static ErrLoadDataInvalidOperation: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrLoadDataInvalidOperation));
pub static ErrLoadDataLocalUnsupportedOption: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrLoadDataLocalUnsupportedOption));
pub static ErrLoadDataPreCheckFailed: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrLoadDataPreCheckFailed));

/// 单次读取的 key 数量超过上限。
pub static ErrMaxKeysReadExceeded: LazyLock<dbterror::Error> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrMaxKeysReadExceeded));
