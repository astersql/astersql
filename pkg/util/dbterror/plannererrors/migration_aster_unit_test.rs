// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 规划器（planner）错误码迁移回归测试：校验错误类、MySQL 错误码与消息模板与 Go 一致。
//
// 规划器负责生成执行计划；本测试在 `ToSQLError` 冻结注册表前强制初始化全部 LazyLock，
// 确保 ClassOptimizer / ClassExpression / ClassExecutor 下错误的 RFC 前缀与数值码不被漂移。

use dbterror_plannererrors::{dbterror, errno as mysql, planner_terror::*, terror};

/// Go 的包级错误变量会在 `RegisterFinish` 冻结注册表前全部完成注册。
#[test]
fn planner_errors_are_registered_before_registry_freeze() {
    terror::RegisterFinish();

    assert_eq!(ErrSpDoesNotExist.Code(), mysql::ErrSpDoesNotExist as i32);
}

/// 枚举优化器、表达式与执行器侧规划相关错误，核对错误类、SQL 码与 AccessDenied 消息模板。
#[test]
fn all_planner_errors_preserve_go_codes_and_classes() {
    // 优化器（Optimizer）类错误：解析/绑定/窗口/CTE/权限等计划阶段常见失败。
    let optimizer_errors = [
        &*ErrUnsupportedType,
        &*ErrAnalyzeMissIndex,
        &*ErrAnalyzeMissColumn,
        &*ErrWrongParamCount,
        &*ErrSchemaChanged,
        &*ErrTablenameNotAllowedHere,
        &*ErrNotSupportedYet,
        &*ErrWrongUsage,
        &*ErrUnknown,
        &*ErrUnknownTable,
        &*ErrNoSuchTable,
        &*ErrViewRecursive,
        &*ErrWrongArguments,
        &*ErrWrongNumberOfColumnsInSelect,
        &*ErrBadGeneratedColumn,
        &*ErrFieldNotInGroupBy,
        &*ErrAggregateOrderNonAggQuery,
        &*ErrFieldInOrderNotSelect,
        &*ErrAggregateInOrderNotSelect,
        &*ErrBadTable,
        &*ErrKeyDoesNotExist,
        &*ErrOperandColumns,
        &*ErrInvalidGroupFuncUse,
        &*ErrIllegalReference,
        &*ErrNoDB,
        &*ErrUnknownExplainFormat,
        &*ErrWrongGroupField,
        &*ErrDupFieldName,
        &*ErrNonUpdatableTable,
        &*ErrMultiUpdateKeyConflict,
        &*ErrInternal,
        &*ErrNonUniqTable,
        &*ErrWindowInvalidWindowFuncUse,
        &*ErrWindowInvalidWindowFuncAliasUse,
        &*ErrWindowNoSuchWindow,
        &*ErrWindowCircularityInWindowGraph,
        &*ErrWindowNoChildPartitioning,
        &*ErrWindowNoInherentFrame,
        &*ErrWindowNoRedefineOrderBy,
        &*ErrWindowDuplicateName,
        &*ErrPartitionClauseOnNonpartitioned,
        &*ErrWindowFrameStartIllegal,
        &*ErrWindowFrameEndIllegal,
        &*ErrWindowFrameIllegal,
        &*ErrWindowRangeFrameOrderType,
        &*ErrWindowRangeFrameTemporalType,
        &*ErrWindowRangeFrameNumericType,
        &*ErrWindowRangeBoundNotConstant,
        &*ErrWindowRowsIntervalUse,
        &*ErrWindowFunctionIgnoresFrame,
        &*ErrInvalidNumberOfArgs,
        &*ErrFieldInGroupingNotGroupBy,
        &*ErrUnsupportedOnGeneratedColumn,
        &*ErrPrivilegeCheckFail,
        &*ErrInvalidWildCard,
        &*ErrMixOfGroupFuncAndFields,
        &*ErrDBaccessDenied,
        &*ErrTableaccessDenied,
        &*ErrSpecificAccessDenied,
        &*ErrViewNoExplain,
        &*ErrWrongValueCountOnRow,
        &*ErrViewInvalid,
        &*ErrNoSuchThread,
        &*ErrUnknownColumn,
        &*ErrCartesianProductUnsupported,
        &*ErrStmtNotFound,
        &*ErrAmbiguous,
        &*ErrUnresolvedHintName,
        &*ErrNotHintUpdatable,
        &*ErrWarnConflictingHint,
        &*ErrCTERecursiveRequiresUnion,
        &*ErrCTERecursiveRequiresNonRecursiveFirst,
        &*ErrCTERecursiveForbidsAggregation,
        &*ErrCTERecursiveForbiddenJoinOrder,
        &*ErrInvalidLateralJoin,
        &*ErrInvalidRequiresSingleReference,
        &*ErrSQLInReadOnlyMode,
        &*ErrDeleteNotFoundColumn,
        &*ErrAccessDenied,
        &*ErrBadNull,
        &*ErrNotSupportedWithSem,
        &*ErrAsOf,
        &*ErrOptOnTemporaryTable,
        &*ErrOptOnCacheTable,
        &*ErrDropTableOnTemporaryTable,
        &*ErrPartitionNoTemporary,
        &*ErrViewSelectTemporaryTable,
        &*ErrSubqueryMoreThan1Row,
        &*ErrKeyPart0,
        &*ErrGettingNoopVariable,
        &*ErrRowIsReferenced2,
        &*ErrNoReferencedRow2,
        &*ErrSpDoesNotExist,
    ];
    // 表达式类：精度越界等与表达式求值相关的错误。
    let expression_errors = [&*ErrTooBigPrecision];
    // 执行器（Executor）类：预处理语句（Prepared Statement）相关限制。
    let executor_errors = [
        &*ErrPrepareMulti,
        &*ErrUnsupportedPs,
        &*ErrPsManyParam,
        &*ErrPrepareDDL,
    ];

    // Force every LazyLock before ToSQLError freezes the Go-compatible registry.
    for (errors, expected_class) in [
        (
            &optimizer_errors[..],
            dbterror::ClassOptimizer.inner.String(),
        ),
        (
            &expression_errors[..],
            dbterror::ClassExpression.inner.String(),
        ),
        (&executor_errors[..], dbterror::ClassExecutor.inner.String()),
    ] {
        for error in errors {
            assert!(
                error.RFCCode().starts_with(&format!("{expected_class}:")),
                "{}",
                error.RFCCode(),
            );
        }
    }

    // 转成 MySQL 协议 SQL 错误后，数值码须与 terror.Error::Code 一致。
    for error in optimizer_errors
        .iter()
        .chain(expression_errors.iter())
        .chain(executor_errors.iter())
    {
        let sql_error = terror::ToSQLError(error);
        assert_eq!(sql_error.Code, error.Code() as u16, "{error}");
    }

    assert_eq!(
        optimizer_errors.len() + expression_errors.len() + executor_errors.len(),
        98
    );
    assert_eq!(
        ErrAccessDenied.MessageTemplate(),
        mysql::MySQLErrName[&mysql::ErrAccessDeniedNoPassword].Raw,
    );
    assert_ne!(
        ErrAccessDenied.MessageTemplate(),
        mysql::MySQLErrName[&mysql::ErrAccessDenied].Raw,
    );
}
