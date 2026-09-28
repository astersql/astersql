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

// 规划器与执行器相关的 `dbterror` 静态错误表。
//
// 由 Go `planner_terror.go` 迁移：用 `LazyLock` 惰性构造 `terror::Error`，
// 按错误类挂到 ClassOptimizer / ClassExpression / ClassExecutor，并保留 MySQL 错误码映射。
// 部分符号名与 errno 常量不完全同名（如 UnknownColumn→ErrBadField），属 Go 既有语义。

// 本文件由 pkg/util/dbterror/plannererrors/planner_terror.go 迁移而来，保留 Go 的错误码与错误类映射。
//

use std::sync::LazyLock;

use crate::{dbterror, errno as mysql, terror};

// error definitions.
// 下面保持 Go var 块的声明顺序和错误类选择；NewStdErr 分支保留 Go 中自定义消息的特殊语义。
/// 不支持的类型：优化器遇到无法处理的类型时返回。
pub static ErrUnsupportedType: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrUnsupportedType));
pub static ErrAnalyzeMissIndex: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrAnalyzeMissIndex));
pub static ErrAnalyzeMissColumn: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrAnalyzeMissColumn));
pub static ErrWrongParamCount: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWrongParamCount));
pub static ErrSchemaChanged: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrSchemaChanged));
pub static ErrTablenameNotAllowedHere: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrTablenameNotAllowedHere));
pub static ErrNotSupportedYet: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrNotSupportedYet));
pub static ErrWrongUsage: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWrongUsage));
pub static ErrUnknown: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrUnknown));
pub static ErrUnknownTable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrUnknownTable));
pub static ErrNoSuchTable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrNoSuchTable));
pub static ErrViewRecursive: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrViewRecursive));
pub static ErrWrongArguments: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWrongArguments));
pub static ErrWrongNumberOfColumnsInSelect: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWrongNumberOfColumnsInSelect));
pub static ErrBadGeneratedColumn: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrBadGeneratedColumn));
pub static ErrFieldNotInGroupBy: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrFieldNotInGroupBy));
pub static ErrAggregateOrderNonAggQuery: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrAggregateOrderNonAggQuery));
pub static ErrFieldInOrderNotSelect: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrFieldInOrderNotSelect));
pub static ErrAggregateInOrderNotSelect: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrAggregateInOrderNotSelect));
pub static ErrBadTable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrBadTable));
pub static ErrKeyDoesNotExist: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrKeyDoesNotExist));
pub static ErrOperandColumns: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrOperandColumns));
pub static ErrInvalidGroupFuncUse: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrInvalidGroupFuncUse));
pub static ErrIllegalReference: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrIllegalReference));
pub static ErrNoDB: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrNoDB));
pub static ErrUnknownExplainFormat: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrUnknownExplainFormat));
pub static ErrWrongGroupField: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWrongGroupField));
pub static ErrDupFieldName: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrDupFieldName));
pub static ErrNonUpdatableTable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrNonUpdatableTable));
pub static ErrMultiUpdateKeyConflict: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrMultiUpdateKeyConflict));
pub static ErrInternal: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrInternal));
pub static ErrNonUniqTable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrNonuniqTable));
pub static ErrWindowInvalidWindowFuncUse: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWindowInvalidWindowFuncUse));
pub static ErrWindowInvalidWindowFuncAliasUse: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWindowInvalidWindowFuncAliasUse));
pub static ErrWindowNoSuchWindow: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWindowNoSuchWindow));
pub static ErrWindowCircularityInWindowGraph: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWindowCircularityInWindowGraph));
pub static ErrWindowNoChildPartitioning: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWindowNoChildPartitioning));
pub static ErrWindowNoInherentFrame: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWindowNoInherentFrame));
pub static ErrWindowNoRedefineOrderBy: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWindowNoRedefineOrderBy));
pub static ErrWindowDuplicateName: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWindowDuplicateName));
pub static ErrPartitionClauseOnNonpartitioned: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrPartitionClauseOnNonpartitioned));
pub static ErrWindowFrameStartIllegal: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWindowFrameStartIllegal));
pub static ErrWindowFrameEndIllegal: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWindowFrameEndIllegal));
pub static ErrWindowFrameIllegal: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWindowFrameIllegal));
pub static ErrWindowRangeFrameOrderType: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWindowRangeFrameOrderType));
pub static ErrWindowRangeFrameTemporalType: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWindowRangeFrameTemporalType));
pub static ErrWindowRangeFrameNumericType: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWindowRangeFrameNumericType));
pub static ErrWindowRangeBoundNotConstant: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWindowRangeBoundNotConstant));
pub static ErrWindowRowsIntervalUse: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWindowRowsIntervalUse));
pub static ErrWindowFunctionIgnoresFrame: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWindowFunctionIgnoresFrame));
pub static ErrInvalidNumberOfArgs: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrInvalidNumberOfArgs));
pub static ErrFieldInGroupingNotGroupBy: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrFieldInGroupingNotGroupBy));
pub static ErrUnsupportedOnGeneratedColumn: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrUnsupportedOnGeneratedColumn));
pub static ErrPrivilegeCheckFail: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrPrivilegeCheckFail));
pub static ErrInvalidWildCard: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrInvalidWildCard));
pub static ErrMixOfGroupFuncAndFields: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    dbterror::ClassOptimizer.NewStd(mysql::ErrMixOfGroupFuncAndFieldsIncompatible)
});
/// 精度过大：挂在 ClassExpression，与多数优化器错误不同类。
pub static ErrTooBigPrecision: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassExpression.NewStd(mysql::ErrTooBigPrecision));
pub static ErrDBaccessDenied: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrDBaccessDenied));
pub static ErrTableaccessDenied: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrTableaccessDenied));
pub static ErrSpecificAccessDenied: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrSpecificAccessDenied));
pub static ErrViewNoExplain: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrViewNoExplain));
pub static ErrWrongValueCountOnRow: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWrongValueCountOnRow));
pub static ErrViewInvalid: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrViewInvalid));
pub static ErrNoSuchThread: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrNoSuchThread));
pub static ErrUnknownColumn: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrBadField));
pub static ErrCartesianProductUnsupported: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrCartesianProductUnsupported));
pub static ErrStmtNotFound: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrPreparedStmtNotFound));
pub static ErrAmbiguous: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrNonUniq));
pub static ErrUnresolvedHintName: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrUnresolvedHintName));
pub static ErrNotHintUpdatable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrNotHintUpdatable));
pub static ErrWarnConflictingHint: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrWarnConflictingHint));
pub static ErrCTERecursiveRequiresUnion: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrCTERecursiveRequiresUnion));
pub static ErrCTERecursiveRequiresNonRecursiveFirst: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| {
        dbterror::ClassOptimizer.NewStd(mysql::ErrCTERecursiveRequiresNonRecursiveFirst)
    });
pub static ErrCTERecursiveForbidsAggregation: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrCTERecursiveForbidsAggregation));
pub static ErrCTERecursiveForbiddenJoinOrder: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrCTERecursiveForbiddenJoinOrder));
pub static ErrInvalidLateralJoin: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrInvalidLateralJoin));
pub static ErrInvalidRequiresSingleReference: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrInvalidRequiresSingleReference));
pub static ErrSQLInReadOnlyMode: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrReadOnlyMode));
pub static ErrDeleteNotFoundColumn: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrDeleteNotFoundColumn));
// Since we cannot know if user logged in with a password, use message of ErrAccessDeniedNoPassword instead
// Go 这里不是使用 ErrAccessDenied 的默认消息，而是显式取无密码登录场景的消息模板。
/// 访问拒绝：故意使用无密码场景消息模板（见上方英文/中文说明）。
pub static ErrAccessDenied: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    dbterror::ClassOptimizer.NewStdErr(
        mysql::ErrAccessDenied,
        &mysql::MySQLErrName[&mysql::ErrAccessDeniedNoPassword],
    )
});
pub static ErrBadNull: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrBadNull));
pub static ErrNotSupportedWithSem: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrNotSupportedWithSem));
pub static ErrAsOf: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrAsOf));
pub static ErrOptOnTemporaryTable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrOptOnTemporaryTable));
pub static ErrOptOnCacheTable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrOptOnCacheTable));
pub static ErrDropTableOnTemporaryTable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrDropTableOnTemporaryTable));
// ErrPartitionNoTemporary returns when partition at temporary mode
// ErrPartitionNoTemporary 保留 Go 注释中的临时表分区错误含义。
pub static ErrPartitionNoTemporary: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrPartitionNoTemporary));
pub static ErrViewSelectTemporaryTable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrViewSelectTmptable));
pub static ErrSubqueryMoreThan1Row: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrSubqueryNo1Row));
pub static ErrKeyPart0: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrKeyPart0));
pub static ErrGettingNoopVariable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrGettingNoopVariable));

/// 以下四项为执行器类错误，覆盖预处理语句的多语句/参数/DDL 限制。
pub static ErrPrepareMulti: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrPrepareMulti));
pub static ErrUnsupportedPs: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrUnsupportedPs));
pub static ErrPsManyParam: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrPsManyParam));
pub static ErrPrepareDDL: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassExecutor.NewStd(mysql::ErrPrepareDDL));
pub static ErrRowIsReferenced2: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrRowIsReferenced2));
pub static ErrNoReferencedRow2: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrNoReferencedRow2));
pub static ErrSpDoesNotExist: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassOptimizer.NewStd(mysql::ErrSpDoesNotExist));

static PLANNER_ERRORS: [&LazyLock<Box<terror::Error>>; 98] = [
    &ErrUnsupportedType,
    &ErrAnalyzeMissIndex,
    &ErrAnalyzeMissColumn,
    &ErrWrongParamCount,
    &ErrSchemaChanged,
    &ErrTablenameNotAllowedHere,
    &ErrNotSupportedYet,
    &ErrWrongUsage,
    &ErrUnknown,
    &ErrUnknownTable,
    &ErrNoSuchTable,
    &ErrViewRecursive,
    &ErrWrongArguments,
    &ErrWrongNumberOfColumnsInSelect,
    &ErrBadGeneratedColumn,
    &ErrFieldNotInGroupBy,
    &ErrAggregateOrderNonAggQuery,
    &ErrFieldInOrderNotSelect,
    &ErrAggregateInOrderNotSelect,
    &ErrBadTable,
    &ErrKeyDoesNotExist,
    &ErrOperandColumns,
    &ErrInvalidGroupFuncUse,
    &ErrIllegalReference,
    &ErrNoDB,
    &ErrUnknownExplainFormat,
    &ErrWrongGroupField,
    &ErrDupFieldName,
    &ErrNonUpdatableTable,
    &ErrMultiUpdateKeyConflict,
    &ErrInternal,
    &ErrNonUniqTable,
    &ErrWindowInvalidWindowFuncUse,
    &ErrWindowInvalidWindowFuncAliasUse,
    &ErrWindowNoSuchWindow,
    &ErrWindowCircularityInWindowGraph,
    &ErrWindowNoChildPartitioning,
    &ErrWindowNoInherentFrame,
    &ErrWindowNoRedefineOrderBy,
    &ErrWindowDuplicateName,
    &ErrPartitionClauseOnNonpartitioned,
    &ErrWindowFrameStartIllegal,
    &ErrWindowFrameEndIllegal,
    &ErrWindowFrameIllegal,
    &ErrWindowRangeFrameOrderType,
    &ErrWindowRangeFrameTemporalType,
    &ErrWindowRangeFrameNumericType,
    &ErrWindowRangeBoundNotConstant,
    &ErrWindowRowsIntervalUse,
    &ErrWindowFunctionIgnoresFrame,
    &ErrInvalidNumberOfArgs,
    &ErrFieldInGroupingNotGroupBy,
    &ErrUnsupportedOnGeneratedColumn,
    &ErrPrivilegeCheckFail,
    &ErrInvalidWildCard,
    &ErrMixOfGroupFuncAndFields,
    &ErrTooBigPrecision,
    &ErrDBaccessDenied,
    &ErrTableaccessDenied,
    &ErrSpecificAccessDenied,
    &ErrViewNoExplain,
    &ErrWrongValueCountOnRow,
    &ErrViewInvalid,
    &ErrNoSuchThread,
    &ErrUnknownColumn,
    &ErrCartesianProductUnsupported,
    &ErrStmtNotFound,
    &ErrAmbiguous,
    &ErrUnresolvedHintName,
    &ErrNotHintUpdatable,
    &ErrWarnConflictingHint,
    &ErrCTERecursiveRequiresUnion,
    &ErrCTERecursiveRequiresNonRecursiveFirst,
    &ErrCTERecursiveForbidsAggregation,
    &ErrCTERecursiveForbiddenJoinOrder,
    &ErrInvalidLateralJoin,
    &ErrInvalidRequiresSingleReference,
    &ErrSQLInReadOnlyMode,
    &ErrDeleteNotFoundColumn,
    &ErrAccessDenied,
    &ErrBadNull,
    &ErrNotSupportedWithSem,
    &ErrAsOf,
    &ErrOptOnTemporaryTable,
    &ErrOptOnCacheTable,
    &ErrDropTableOnTemporaryTable,
    &ErrPartitionNoTemporary,
    &ErrViewSelectTemporaryTable,
    &ErrSubqueryMoreThan1Row,
    &ErrKeyPart0,
    &ErrGettingNoopVariable,
    &ErrPrepareMulti,
    &ErrUnsupportedPs,
    &ErrPsManyParam,
    &ErrPrepareDDL,
    &ErrRowIsReferenced2,
    &ErrNoReferencedRow2,
    &ErrSpDoesNotExist,
];

/// 对齐 Go 包级 `var` 初始化：在注册表冻结前按声明顺序构造全部错误。
pub(crate) fn initialize_planner_errors() {
    for error in PLANNER_ERRORS {
        LazyLock::force(error);
    }
}
