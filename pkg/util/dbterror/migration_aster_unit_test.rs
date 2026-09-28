// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// dbterror 迁移回归测试：错误类包装、NewStd 与 DDL/reorg 边界。
//
// 确认 ErrClass 与 parser terror 类一致，DDL NewStd 生成正确 RFC，
// 以及 reorganization（表结构重组）可重试错误码/消息集合与 Go 对齐。

use astersql_errno::{errcode as errno, errname};
use astersql_parser_terror as terror;
use std::collections::HashSet;
use std::process::Command;

use crate::*;

#[test]
/// 包装后的 Class* 与底层 terror.Class* 为同一实例。
fn error_classes_match_parser_terror_classes() {
    assert_eq!(ClassAutoid.inner, terror::ClassAutoid);
    assert_eq!(ClassDDL.inner, terror::ClassDDL);
    assert_eq!(ClassDomain.inner, terror::ClassDomain);
    assert_eq!(ClassExecutor.inner, terror::ClassExecutor);
    assert_eq!(ClassExpression.inner, terror::ClassExpression);
    assert_eq!(ClassAdmin.inner, terror::ClassAdmin);
    assert_eq!(ClassKV.inner, terror::ClassKV);
    assert_eq!(ClassMeta.inner, terror::ClassMeta);
    assert_eq!(ClassOptimizer.inner, terror::ClassOptimizer);
    assert_eq!(ClassPrivilege.inner, terror::ClassPrivilege);
    assert_eq!(ClassSchema.inner, terror::ClassSchema);
    assert_eq!(ClassServer.inner, terror::ClassServer);
    assert_eq!(ClassStructure.inner, terror::ClassStructure);
    assert_eq!(ClassVariable.inner, terror::ClassVariable);
    assert_eq!(ClassXEval.inner, terror::ClassXEval);
    assert_eq!(ClassTable.inner, terror::ClassTable);
    assert_eq!(ClassTypes.inner, terror::ClassTypes);
    assert_eq!(ClassJSON.inner, terror::ClassJSON);
    assert_eq!(ClassTiKV.inner, terror::ClassTiKV);
    assert_eq!(ClassSession.inner, terror::ClassSession);
    assert_eq!(ClassPlugin.inner, terror::ClassPlugin);
    assert_eq!(ClassUtil.inner, terror::ClassUtil);
}

#[test]
/// `ClassDDL.NewStd` 使用 errno 消息，RFCCode 形如 `ddl:<code>`。
fn new_std_uses_errno_message_code_and_ddl_rfc_class() {
    let err = ClassDDL.NewStd(errno::ErrInvalidDDLWorker);
    assert_eq!(err.Code(), errno::ErrInvalidDDLWorker as i32);
    assert_eq!(
        err.GetMsg(),
        errname::MySQLErrName[&errno::ErrInvalidDDLWorker].Raw
    );
    assert_eq!(err.RFCCode(), format!("ddl:{}", errno::ErrInvalidDDLWorker));
}

#[test]
/// `NewStd` 保留 Go `terror.ErrCode` 的完整宽度，仅标准消息查找按 uint16 回绕。
fn new_std_preserves_wide_error_code() {
    let code = terror::ErrCode(errno::ErrDupEntry as isize + 65536);
    let err = ClassDDL.NewStd(code);

    assert_eq!(err.Code(), code.0 as i32);
    assert_eq!(err.RFCCode(), format!("ddl:{}", code.0));
    assert_eq!(err.GetMsg(), errname::MySQLErrName[&errno::ErrDupEntry].Raw);
}

#[test]
/// 抽检 DDL 自定义模板错误的 Code 与消息文案。
fn ddl_errors_preserve_go_custom_templates_and_codes() {
    assert_eq!(ErrWaitReorgTimeout.Code(), errno::ErrLockWaitTimeout as i32);
    assert_eq!(
        ErrWaitReorgTimeout.GetMsg(),
        errname::MySQLErrName[&errno::ErrWaitReorgTimeout].Raw
    );
    assert_eq!(ErrAlterTiFlashModeForTableWithoutTiFlashReplica.Code(), 0);
    assert_eq!(
        ErrUnsupportedModifyCollation.GetMsg(),
        "Unsupported modifying collation from %s to %s"
    );
}

#[test]
/// reorg 可重试错误码集合与消息切片完整内容与 Go 一致。
fn reorg_retryable_lists_match_go_exactly() {
    let expected_codes = HashSet::from([
        errno::ErrPDServerTimeout,
        errno::ErrTiKVServerTimeout,
        errno::ErrTiKVServerBusy,
        errno::ErrResolveLockTimeout,
        errno::ErrRegionUnavailable,
        errno::ErrTxnAbortedByGC,
        errno::ErrWriteConflict,
        errno::ErrTiKVStoreLimit,
        errno::ErrTiKVStaleCommand,
        errno::ErrTiKVMaxTimestampNotSynced,
        errno::ErrTiFlashServerTimeout,
        errno::ErrTiFlashServerBusy,
        errno::ErrInfoSchemaExpired,
        errno::ErrInfoSchemaChanged,
        errno::ErrWriteConflictInTiDB,
        errno::ErrTxnRetryable,
        errno::ErrNotOwner,
        errno::ErrInvalidSplitRegionRanges,
        terror::CodeResultUndetermined.0 as u16,
    ]);

    assert_eq!(&*ReorgRetryableErrCodes, &expected_codes);
    assert!(!ReorgRetryableErrCodes.contains(&errno::ErrDupEntry));
    assert_eq!(
        ReorgRetryableErrMsgs,
        [
            "context deadline exceeded",
            "requested lease not found",
            "mvcc: required revision has been compacted",
            "All returned regions have no leaders",
        ]
    );
}

#[test]
/// Go 包变量会在 `RegisterFinish` 前完成注册；冻结后首次读取任一 DDL 错误不得再注册。
fn ddl_errors_are_registered_before_registry_freeze() {
    const FREEZE_HELPER_ENV: &str = "DBTERROR_DDL_REGISTER_FINISH_HELPER";

    if std::env::var_os(FREEZE_HELPER_ENV).is_some() {
        terror::RegisterFinish();
        let access_after_freeze = std::panic::catch_unwind(|| {
            assert_eq!(ErrForbiddenDDL.Code(), errno::ErrForbiddenDDL as i32);
        });
        assert!(
            access_after_freeze.is_ok(),
            "DDL errors must be eagerly registered before RegisterFinish"
        );
        return;
    }

    let status = Command::new(std::env::current_exe().expect("current test executable"))
        .arg("--exact")
        .arg("migration_aster_unit_test::ddl_errors_are_registered_before_registry_freeze")
        .env(FREEZE_HELPER_ENV, "1")
        .status()
        .expect("run DDL registration subprocess helper");
    assert!(status.success(), "DDL registration helper failed: {status}");
}
