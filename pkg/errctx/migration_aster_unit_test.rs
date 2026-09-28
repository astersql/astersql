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

// `errctx` 迁移单元测试。
//
// 验证从 Go(TiDB) 迁移到 Rust 的错误上下文（Error Context）在以下行为上与 Go 一致：
// - `HandleError` / `HandleErrorWithAlias`：按 ErrGroup 级别返回错误、降级为 warning 或忽略；
// - 错误组（`errors::Join`）按顺序处理，遇到首个需返回的错误即停止；
// - `LevelMap` 拷贝语义与 `ErrGroupForCode` 对全部已知 MySQL 错误码的分组；
// - `AppendNote` 与 `ResolveErrLevel`（由 sql_mode 推导处置级别）与 Go 对齐。

use std::sync::Arc;

use astersql_errctx::contextutil::{
    NewStaticWarnHandler, WarnAppender, WarnHandler, WarnLevelNote, WarnLevelWarning,
};
use astersql_errctx::errctx::{
    Context, ErrGroup, Level, LevelMap, NewContext, NewContextWithLevels, ResolveErrLevel,
};
use astersql_errctx::errno;
use astersql_errctx::errors::{self, MySQLErrorCode, Normalize, SharedError};

/// 构造带 MySQL 错误码的共享错误，便于走 ErrGroup 分类路径。
fn sql_error(code: u16, message: &str) -> SharedError {
    SharedError::new(Normalize(message, &[MySQLErrorCode(i32::from(code))]))
}

/// 断言 Option 中的错误与期望值是同一共享实例（指针相等）。
fn assert_same(actual: Option<SharedError>, expected: &SharedError) {
    assert!(actual.is_some_and(|actual| actual.ptr_eq(expected)));
}

/// 覆盖默认 Error / Warn / Ignore 三级处置，以及 WithAlias 的对外消息替换。
#[test]
fn context_matches_error_warn_ignore_and_alias_behavior() {
    let warnings = Arc::new(NewStaticWarnHandler(0));
    let handler: Arc<dyn WarnAppender + Send + Sync> = warnings.clone();
    let ctx = NewContext(handler);
    let internal = sql_error(errno::ErrDataOutOfRange, "overflow");
    let returned = errors::New("public error");
    let warning = errors::New("public warning");

    // 默认 LevelError：对外返回 alias 的 returned 错误。
    assert_same(
        ctx.HandleErrorWithAlias(Some(&internal), returned.clone(), warning.clone()),
        &returned,
    );

    // 将截断组降为 Warn：不返回错误，而是追加 warning 别名。
    let warn_ctx = ctx.WithErrGroupLevel(ErrGroup::ErrGroupTruncate, Level::LevelWarn);
    assert_eq!(
        ctx.LevelForGroup(ErrGroup::ErrGroupTruncate),
        Level::LevelError
    );
    assert!(
        warn_ctx
            .HandleErrorWithAlias(Some(&internal), returned.clone(), warning.clone())
            .is_none()
    );
    let got = warnings.CopyWarnings(Vec::new());
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].Level, WarnLevelWarning);
    assert!(got[0].Err.as_ref().is_some_and(|err| err.ptr_eq(&warning)));

    // Ignore：既不返回也不再追加 warning。
    let ignore_ctx = warn_ctx.WithErrGroupLevel(ErrGroup::ErrGroupTruncate, Level::LevelIgnore);
    assert!(ignore_ctx.HandleError(Some(internal.clone())).is_none());
    assert_eq!(warnings.WarningCount(), 1);

    // Strict：所有组强制为 Error，返回内部原始错误。
    let strict = warn_ctx.WithStrictErrGroupLevel();
    assert_eq!(strict.LevelMap(), [Level::LevelError; 7]);
    assert_same(strict.HandleError(Some(internal.clone())), &internal);

    // 无错误码的普通错误与 None：分别原样返回 / 无操作。
    let plain = errors::New("not a coded error");
    assert_same(
        warn_ctx.HandleErrorWithAlias(Some(&plain), returned.clone(), warning),
        &returned,
    );
    assert!(
        warn_ctx
            .HandleErrorWithAlias(None, returned.clone(), returned)
            .is_none()
    );
}

/// 嵌套 Join 错误组：先处理可降级项，遇到首个需返回错误后停止后续项。
#[test]
fn context_handles_error_groups_in_order_and_stops_at_first_returned_error() {
    let warnings = Arc::new(NewStaticWarnHandler(0));
    let handler: Arc<dyn WarnAppender + Send + Sync> = warnings.clone();
    let warn_ctx =
        NewContext(handler).WithErrGroupLevel(ErrGroup::ErrGroupTruncate, Level::LevelWarn);
    let overflow = sql_error(errno::ErrDataOutOfRange, "overflow");
    let fatal = errors::New("fatal");
    let after_fatal = sql_error(errno::ErrDataTooLong, "late warning");
    let nested = errors::Join(&[Some(overflow.clone())]).expect("non-empty nested group");
    let group = errors::Join(&[Some(nested), Some(fatal.clone()), Some(after_fatal)])
        .expect("non-empty group");

    // overflow 降为 warning，fatal 返回；after_fatal 不应再被处理。
    assert_same(warn_ctx.HandleError(Some(group)), &fatal);
    let got = warnings.CopyWarnings(Vec::new());
    assert_eq!(got.len(), 1);
    assert!(got[0].Err.as_ref().is_some_and(|err| err.ptr_eq(&overflow)));
}

/// LevelMap 派生拷贝独立；ErrGroupForCode 覆盖 Go 侧全部已知分类码。
#[test]
fn level_maps_are_copied_and_all_go_error_codes_are_classified() {
    let handler: Arc<dyn WarnAppender + Send + Sync> = Arc::new(NewStaticWarnHandler(0));
    let mut levels: LevelMap = [Level::LevelError; 7];
    levels[ErrGroup::ErrGroupAutoIncReadFailed as usize] = Level::LevelWarn;
    let ctx = NewContextWithLevels(levels, handler);
    let mut replacement = [Level::LevelError; 7];
    replacement[ErrGroup::ErrGroupAutoIncReadFailed as usize] = Level::LevelIgnore;
    let replaced = ctx.WithErrGroupLevels(replacement);

    assert_eq!(
        ctx.LevelForGroup(ErrGroup::ErrGroupAutoIncReadFailed),
        Level::LevelWarn
    );
    assert_eq!(replaced.LevelMap(), replacement);
    // 与 Go errctx 中错误码→分组映射表一一对应。
    let classifications = [
        (
            ErrGroup::ErrGroupTruncate,
            &[
                errno::ErrTruncatedWrongValue,
                errno::ErrDataTooLong,
                errno::ErrTruncatedWrongValueForField,
                errno::ErrWarnDataOutOfRange,
                errno::ErrDataOutOfRange,
                errno::ErrBadNumber,
                errno::ErrWrongValueForType,
                errno::ErrDatetimeFunctionOverflow,
                errno::WarnDataTruncated,
                errno::ErrIncorrectDatetimeValue,
            ][..],
        ),
        (
            ErrGroup::ErrGroupBadNull,
            &[errno::ErrBadNull, errno::ErrWarnNullToNotnull][..],
        ),
        (
            ErrGroup::ErrGroupNoDefault,
            &[errno::ErrNoDefaultForField][..],
        ),
        (
            ErrGroup::ErrGroupDividedByZero,
            &[errno::ErrDivisionByZero][..],
        ),
        (
            ErrGroup::ErrGroupAutoIncReadFailed,
            &[errno::ErrAutoincReadFailed][..],
        ),
        (
            ErrGroup::ErrGroupNoMatchedPartition,
            &[
                errno::ErrNoPartitionForGivenValue,
                errno::ErrRowDoesNotMatchGivenPartitionSet,
            ][..],
        ),
        (ErrGroup::ErrGroupDupKey, &[errno::ErrDupEntry][..]),
    ];
    for (group, codes) in classifications {
        for &code in codes {
            assert_eq!(Context::ErrGroupForCode(code), Some(group), "code {code}");
        }
    }
    assert_eq!(Context::ErrGroupForCode(u16::MAX), None);
}

/// AppendNote 写入 Note 级警告；ResolveErrLevel 按 ignore / warn 标志推导级别。
#[test]
fn append_note_and_resolve_level_match_go() {
    let warnings = Arc::new(NewStaticWarnHandler(0));
    let handler: Arc<dyn WarnAppender + Send + Sync> = warnings.clone();
    let ctx = NewContext(handler);
    ctx.AppendNote(errors::New("note"));

    let got = warnings.CopyWarnings(Vec::new());
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].Level, WarnLevelNote);
    // (ignore, warn) → Level：ignore 优先，其次 warn，否则 error。
    assert_eq!(ResolveErrLevel(true, false), Level::LevelIgnore);
    assert_eq!(ResolveErrLevel(true, true), Level::LevelIgnore);
    assert_eq!(ResolveErrLevel(false, true), Level::LevelWarn);
    assert_eq!(ResolveErrLevel(false, false), Level::LevelError);
}
