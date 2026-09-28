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

// errctx::Context 行为测试：默认返回、级别派生、严格模式、错误组与 LevelMap 独立性。

use std::sync::{Arc, Mutex};

use super::{contextutil, errctx, errors, types};

/// 断言 Option 中的 SharedError 与期望是同一底层指针。
fn assert_same(actual: Option<errors::SharedError>, expected: &errors::SharedError) {
    assert!(actual.is_some_and(|actual| actual.ptr_eq(expected)));
}

/// 构造把警告写入共享 Mutex 的 WarnAppender，便于断言。
fn new_func_warn_appender(
    warn: &Arc<Mutex<Option<errors::SharedError>>>,
) -> errctx::WarnAppenderRef {
    let warn = Arc::clone(warn);
    Arc::new(contextutil::funcWarnAppender {
        fn_: Box::new(move |level, err| {
            assert_eq!(contextutil::WarnLevelWarning, level);
            *warn.lock().expect("warning recorder poisoned") = Some(err);
        }),
    })
}

/// 读取已记录的警告（若有）。
fn recorded_warning(warn: &Arc<Mutex<Option<errors::SharedError>>>) -> Option<errors::SharedError> {
    warn.lock().expect("warning recorder poisoned").clone()
}

/// 覆盖 Context 的核心语义：HandleErrorWithAlias、With* 派生、Join 组错误与 LevelMap。
#[test]
fn test_context() {
    let warn = Arc::new(Mutex::new(None));
    let mut ctx = errctx::NewContext(new_func_warn_appender(&warn));

    let test_internal_err = errors::SharedError::new((**types::ErrOverflow).clone());
    let test_err = errors::New("error");
    let test_warn = errors::New("warn");

    // By default, all errors are returned directly.
    // 默认：全部错误直接返回给调用方。
    assert_same(
        ctx.HandleErrorWithAlias(
            Some(&test_internal_err),
            test_err.clone(),
            test_warn.clone(),
        ),
        &test_err,
    );

    // WithErrGroupLevel returns a new context and does not mutate the original.
    // WithErrGroupLevel 返回新上下文，不修改原 ctx。
    let new_ctx =
        ctx.WithErrGroupLevel(errctx::ErrGroup::ErrGroupTruncate, errctx::Level::LevelWarn);
    assert_same(
        ctx.HandleErrorWithAlias(
            Some(&test_internal_err),
            test_err.clone(),
            test_warn.clone(),
        ),
        &test_err,
    );
    // Truncate 组降为 Warn：不返回错误，而是追加 warn 别名。
    assert!(
        new_ctx
            .HandleErrorWithAlias(
                Some(&test_internal_err),
                test_err.clone(),
                test_warn.clone(),
            )
            .is_none()
    );
    assert!(recorded_warning(&warn).is_some_and(|warn| warn.ptr_eq(&test_warn)));

    let levels = new_ctx.LevelMap();
    let groups = [
        errctx::ErrGroup::ErrGroupTruncate,
        errctx::ErrGroup::ErrGroupDupKey,
        errctx::ErrGroup::ErrGroupBadNull,
        errctx::ErrGroup::ErrGroupNoDefault,
        errctx::ErrGroup::ErrGroupDividedByZero,
        errctx::ErrGroup::ErrGroupAutoIncReadFailed,
        errctx::ErrGroup::ErrGroupNoMatchedPartition,
    ];
    assert_eq!(groups.len(), errctx::errGroupCount);
    for group in groups {
        if group == errctx::ErrGroup::ErrGroupTruncate {
            assert_eq!(levels[group as usize], errctx::Level::LevelWarn);
        } else {
            assert_eq!(levels[group as usize], errctx::Level::LevelError);
            assert_eq!(levels[group as usize], new_ctx.LevelForGroup(group));
        }
    }

    *warn.lock().expect("warning recorder poisoned") = None;
    let new_ctx2 = new_ctx.WithStrictErrGroupLevel();
    // new_ctx is unchanged; new_ctx2 returns all errors in strict mode.
    // new_ctx 仍为 Warn；new_ctx2 严格模式全部返回错误。
    assert!(
        new_ctx
            .HandleErrorWithAlias(
                Some(&test_internal_err),
                test_err.clone(),
                test_warn.clone(),
            )
            .is_none()
    );
    assert!(recorded_warning(&warn).is_some_and(|warn| warn.ptr_eq(&test_warn)));
    assert_same(
        new_ctx2.HandleErrorWithAlias(
            Some(&test_internal_err),
            test_err.clone(),
            test_warn.clone(),
        ),
        &test_err,
    );
    assert_eq!(
        [errctx::Level::LevelError; errctx::errGroupCount],
        new_ctx2.LevelMap()
    );

    // errors::Join provides the same ordered multi-error boundary as multierr.Append.
    // Join 组成错误组后按序处理：ctx 返回首个需返回的子错误。
    let test_errs = errors::Join(&[Some(test_internal_err.clone()), Some(test_err.clone())])
        .expect("two errors form a non-empty error group");
    assert_same(ctx.HandleError(Some(test_errs.clone())), &test_internal_err);
    // new_ctx 对 Truncate 降级，故跳过 overflow，返回第二个 test_err。
    assert_same(new_ctx.HandleError(Some(test_errs)), &test_err);
    assert!(recorded_warning(&warn).is_some_and(|warn| warn.ptr_eq(&test_internal_err)));

    // A nil Go error maps to None and is returned unchanged.
    // Go 的 nil 错误对应 None，原样返回。
    assert!(ctx.HandleError(None).is_none());

    // Explicit level maps are copied by value and remain independent.
    // 显式 LevelMap 按值拷贝，彼此独立。
    let mut levels = [errctx::Level::LevelError; errctx::errGroupCount];
    levels[errctx::ErrGroup::ErrGroupAutoIncReadFailed as usize] = errctx::Level::LevelWarn;
    ctx = errctx::NewContextWithLevels(levels, new_func_warn_appender(&warn));
    assert_eq!(levels, ctx.LevelMap());

    let mut levels2 = [errctx::Level::LevelError; errctx::errGroupCount];
    levels2[errctx::ErrGroup::ErrGroupAutoIncReadFailed as usize] = errctx::Level::LevelIgnore;
    ctx = ctx.WithErrGroupLevels(levels2);
    assert_eq!(levels2, ctx.LevelMap());

    // Replacing the context map does not mutate either original map.
    // 替换上下文映射不会改动原先的 levels / levels2 数组。
    ctx = ctx.WithErrGroupLevels([errctx::Level::LevelError; errctx::errGroupCount]);
    assert_eq!(
        [errctx::Level::LevelError; errctx::errGroupCount],
        ctx.LevelMap()
    );
    assert_eq!(
        errctx::Level::LevelWarn,
        levels[errctx::ErrGroup::ErrGroupAutoIncReadFailed as usize]
    );
    assert_eq!(
        errctx::Level::LevelIgnore,
        levels2[errctx::ErrGroup::ErrGroupAutoIncReadFailed as usize]
    );
}
