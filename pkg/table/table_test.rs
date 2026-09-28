// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// table 错误码与变更选项（Add/Update/CreateIdx）的单元测试。
//
// 对照 Go `table` 包测试：校验导出错误原型绑定的 MySQL 错误码，
// 以及 `NewAddRecordOpt` / `NewUpdateRecordOpt` / `NewCreateIdxOpt` 的选项累积语义。

use crate::*;

/// 抽查若干表相关错误原型的 MySQL 错误码映射。
#[test]
fn TestErrorCode() {
    assert_eq!(
        ErrColumnCantNull.Code(),
        i32::from(dbterror_dependency::errno::ErrBadNull)
    );
    assert_eq!(
        ErrUnknownColumn.Code(),
        i32::from(dbterror_dependency::errno::ErrBadField)
    );
    assert_eq!(
        errDuplicateColumn.Code(),
        i32::from(dbterror_dependency::errno::ErrFieldSpecifiedTwice)
    );
    assert_eq!(
        errGetDefaultFailed.Code(),
        i32::from(dbterror_dependency::errno::ErrFieldGetDefaultFailed)
    );
    assert_eq!(
        ErrNoDefaultValue.Code(),
        i32::from(dbterror_dependency::errno::ErrNoDefaultForField)
    );
    assert_eq!(
        ErrIndexOutBound.Code(),
        i32::from(dbterror_dependency::errno::ErrIndexOutBound)
    );
    assert_eq!(
        ErrUnsupportedOp.Code(),
        i32::from(dbterror_dependency::errno::ErrUnsupportedOp)
    );
    assert_eq!(
        ErrRowNotFound.Code(),
        i32::from(dbterror_dependency::errno::ErrRowNotFound)
    );
    assert_eq!(
        ErrTableStateCantNone.Code(),
        i32::from(dbterror_dependency::errno::ErrTableStateCantNone)
    );
    assert_eq!(
        ErrColumnStateCantNone.Code(),
        i32::from(dbterror_dependency::errno::ErrColumnStateCantNone)
    );
    assert_eq!(
        ErrColumnStateNonPublic.Code(),
        i32::from(dbterror_dependency::errno::ErrColumnStateNonPublic)
    );
    assert_eq!(
        ErrIndexStateCantNone.Code(),
        i32::from(dbterror_dependency::errno::ErrIndexStateCantNone)
    );
    assert_eq!(
        ErrInvalidRecordKey.Code(),
        i32::from(dbterror_dependency::errno::ErrInvalidRecordKey)
    );
    assert_eq!(
        ErrTruncatedWrongValueForField.Code(),
        i32::from(dbterror_dependency::errno::ErrTruncatedWrongValueForField)
    );
    assert_eq!(
        ErrUnknownPartition.Code(),
        i32::from(dbterror_dependency::errno::ErrUnknownPartition)
    );
    assert_eq!(
        ErrNoPartitionForGivenValue.Code(),
        i32::from(dbterror_dependency::errno::ErrNoPartitionForGivenValue)
    );
    assert_eq!(
        ErrLockOrActiveTransaction.Code(),
        i32::from(dbterror_dependency::errno::ErrLockOrActiveTransaction)
    );
}

/// 验证 Add/Update/CreateIdx 选项构建器的默认值与叠加语义。
#[test]
fn TestOptions() {
    // 默认 AddRecord 选项：无上下文、非更新、不预生成记录 ID。
    let default_add = NewAddRecordOpt(&[]);
    assert_eq!(default_add.Ctx(), None);
    assert!(!default_add.IsUpdate());
    assert!(!default_add.GenerateRecordID());
    assert_eq!(default_add.ReserveAutoID(), 0);
    assert_eq!(default_add.GetCreateIdxOpt(), CreateIdxOpt::default());

    // 叠加上下文、IsUpdate 与预留自增 ID 提示。
    let context = kv_dependency::Context::todo();
    let with_context = WithCtx(context.clone());
    let reserve = WithReserveAutoIDHint(12);
    let add = NewAddRecordOpt(&[&with_context, &IsUpdate, &reserve]);
    assert_eq!(add.Ctx(), Some(context.clone()));
    assert!(add.IsUpdate());
    assert!(add.GenerateRecordID());
    assert_eq!(add.ReserveAutoID(), 12);
    assert_eq!(add.GetCreateIdxOpt().Ctx(), Some(context.clone()));

    // UpdateRecord 可派生出「生成 / 保留记录 ID」两种 AddRecord 视图。
    let default_update = NewUpdateRecordOpt(&[]);
    assert_eq!(default_update.Ctx(), None);
    assert!(default_update.GetAddRecordOpt().IsUpdate());
    assert!(default_update.GetAddRecordOpt().GenerateRecordID());
    assert!(default_update.GetAddRecordOptKeepRecordID().IsUpdate());
    assert!(
        !default_update
            .GetAddRecordOptKeepRecordID()
            .GenerateRecordID()
    );
    assert_eq!(default_update.GetCreateIdxOpt(), CreateIdxOpt::default());

    let update = NewUpdateRecordOpt(&[&with_context]);
    assert_eq!(update.Ctx(), Some(context.clone()));
    assert_eq!(update.GetAddRecordOpt().Ctx(), Some(context.clone()));
    assert_eq!(
        update.GetAddRecordOptKeepRecordID().Ctx(),
        Some(context.clone())
    );
    assert_eq!(update.GetCreateIdxOpt().Ctx(), Some(context.clone()));

    // CreateIdx 选项：忽略断言与回填来源标志。
    let default_create = NewCreateIdxOpt(&[]);
    assert_eq!(default_create.Ctx(), None);
    assert!(!default_create.IgnoreAssertion());
    assert!(!default_create.FromBackFill());

    let create = NewCreateIdxOpt(&[&with_context, &WithIgnoreAssertion, &FromBackfill]);
    assert_eq!(create.Ctx(), Some(context));
    assert!(create.IgnoreAssertion());
    assert!(create.FromBackFill());
}
