// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Planner 错误转 SQLError 的一致性测试。
//
// 对应 Go 同名测试：遍历优化器错误列表，确认 `ToSQLError` 得到的
// Code 与原 terror.Code 相同且不为 unknown。

#![allow(dead_code)]
#![allow(non_snake_case)]

use dbterror_plannererrors::{errno as mysql, planner_terror::*, terror};

// TestError 对应 Go 的同名测试：遍历 planner 错误列表，确认 SQLError.Code 与原 terror.Code 一致且不为 unknown。
#[test]
/// 遍历 planner 错误，断言 SQLError.Code 与 terror.Code 一致。
fn TestError() {
    let kvErrs = [
        &*ErrUnsupportedType,
        &*ErrAnalyzeMissIndex,
        &*ErrAnalyzeMissColumn,
        &*ErrWrongParamCount,
        &*ErrSchemaChanged,
        &*ErrTablenameNotAllowedHere,
        &*ErrNotSupportedYet,
        &*ErrWrongUsage,
        &*ErrUnknownTable,
        &*ErrWrongArguments,
        &*ErrWrongNumberOfColumnsInSelect,
        &*ErrBadGeneratedColumn,
        &*ErrFieldNotInGroupBy,
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
        &*ErrKeyPart0,
    ];

    // 逐个转为 SQLError，要求码值有效且与原错误 Code 相同。
    for err in kvErrs {
        let code = terror::ToSQLError(err).Code;
        assert!(
            code != mysql::ErrUnknown && code == err.Code() as u16,
            "err: {err:?}",
        );
    }
}
