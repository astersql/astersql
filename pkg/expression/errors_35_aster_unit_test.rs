// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// `errors.rs` 错误路由行为的单元测试。
//
// 通过轻量 `TestEvalContext` 模拟类型标志与错误级别，验证除零、
// `max_allowed_packet` 溢出以及无效时间错误在警告/严格模式下的分流是否与 Go 一致。

use std::sync::Arc;

use crate::expression_errors::*;
use crate::{contextutil, errctx, exprctx, mysql, types};
use contextutil::{WarnAppender, WarnHandler};

/// 空用户变量读取器：所有查询均返回 None。
struct EmptyUserVars;

impl exprctx::UserVarsReader for EmptyUserVars {
    fn GetUserVarVal(&self, _name: &str) -> Option<types::Datum> {
        None
    }

    fn GetUserVarType(&self, _name: &str) -> Option<types::FieldType> {
        None
    }

    fn Clone(&self) -> Box<dyn exprctx::UserVarsReader> {
        Box::new(Self)
    }
}

/// 测试用求值上下文：可配置类型 Flags 与错误组级别，并收集告警。
struct TestEvalContext {
    type_ctx: types::Context,
    err_ctx: errctx::Context,
    warnings: Arc<contextutil::StaticWarnHandler>,
    user_vars: EmptyUserVars,
}

impl TestEvalContext {
    /// 以除零错误组级别构造上下文。
    fn new(flags: types::Flags, division_level: errctx::Level) -> Self {
        Self::with_error_group(
            flags,
            errctx::ErrGroup::ErrGroupDividedByZero,
            division_level,
        )
    }

    /// 按指定错误组与级别构造上下文。
    fn with_error_group(
        flags: types::Flags,
        group: errctx::ErrGroup,
        level: errctx::Level,
    ) -> Self {
        let warnings = Arc::new(contextutil::NewStaticWarnHandler(0));
        let appender: Arc<dyn contextutil::WarnAppender + Send + Sync> = warnings.clone();
        let type_ctx = types::NewContext(flags, chrono_tz::UTC, appender.clone());
        let err_ctx = errctx::NewContext(appender).WithErrGroupLevel(group, level);
        Self {
            type_ctx,
            err_ctx,
            warnings,
            user_vars: EmptyUserVars,
        }
    }
}

impl WarnAppender for TestEvalContext {
    fn AppendWarning(&self, error: contextutil::errors::SharedError) {
        self.warnings.AppendWarning(error);
    }

    fn AppendNote(&self, error: contextutil::errors::SharedError) {
        self.warnings.AppendNote(error);
    }
}

impl WarnHandler for TestEvalContext {
    fn WarningCount(&self) -> usize {
        self.warnings.WarningCount()
    }

    fn TruncateWarnings(&self, start: isize) -> Vec<contextutil::SQLWarn> {
        self.warnings.TruncateWarnings(start)
    }

    fn CopyWarnings(&self, destination: Vec<contextutil::SQLWarn>) -> Vec<contextutil::SQLWarn> {
        self.warnings.CopyWarnings(destination)
    }
}

impl exprctx::ParamValues for TestEvalContext {
    fn GetParamValue(&self, _index: usize) -> Result<types::Datum, exprctx::ParamError> {
        Err(exprctx::ParamError::IndexExceedsParamCount)
    }
}

impl exprctx::EvalContext for TestEvalContext {
    fn CtxID(&self) -> u64 {
        35
    }

    fn SQLMode(&self) -> mysql::SQLMode {
        mysql::SQLMode::default()
    }

    fn TypeCtx(&self) -> types::Context {
        self.type_ctx.clone()
    }

    fn ErrCtx(&self) -> errctx::Context {
        self.err_ctx.clone()
    }

    fn Location(&self) -> chrono_tz::Tz {
        chrono_tz::UTC
    }

    fn CurrentTime(
        &self,
    ) -> Result<chrono::DateTime<chrono_tz::Tz>, contextutil::errors::SharedError> {
        panic!("not used by error-routing tests")
    }

    fn CurrentDB(&self) -> String {
        String::new()
    }

    fn GetMaxAllowedPacket(&self) -> u64 {
        64 << 20
    }

    fn GetTiDBRedactLog(&self) -> String {
        "OFF".to_owned()
    }

    fn GetDefaultWeekFormatMode(&self) -> String {
        "0".to_owned()
    }

    fn GetDivPrecisionIncrement(&self) -> i32 {
        4
    }

    fn GetUserVarsReader(&self) -> &dyn exprctx::UserVarsReader {
        &self.user_vars
    }

    fn GetOptionalPropSet(&self) -> exprctx::OptionalEvalPropKeySet {
        exprctx::OptionalEvalPropKeySet::default()
    }

    fn GetOptionalPropProvider(
        &self,
        _key: exprctx::OptionalEvalPropKey,
    ) -> Option<&dyn exprctx::OptionalEvalPropProvider> {
        None
    }
}

/// 除零：Warn 级别记告警并返回 None；Error 级别返回 ErrDivisionByZero。
#[test]
fn division_by_zero_obeys_error_context() {
    let warning = TestEvalContext::new(types::Flags(0), errctx::Level::LevelWarn);
    assert!(handleDivisionByZeroError(&warning).is_none());
    assert_eq!(warning.WarningCount(), 1);

    let strict = TestEvalContext::new(types::Flags(0), errctx::Level::LevelError);
    let error = handleDivisionByZeroError(&strict).expect("strict mode returns the error");
    assert!(ErrDivisionByZero.Equal(Some(&error)));
    assert_eq!(strict.WarningCount(), 0);

    let ignored = TestEvalContext::new(types::Flags(0), errctx::Level::LevelIgnore);
    assert!(handleDivisionByZeroError(&ignored).is_none());
    assert_eq!(ignored.WarningCount(), 0);
}

/// `max_allowed_packet` 溢出：TruncateAsWarning 时记告警，否则返回错误且消息含函数名与大小。
#[test]
fn allowed_packet_overflow_obeys_type_flags_and_formats_arguments() {
    let warning = TestEvalContext::new(
        types::Flags(0).WithTruncateAsWarning(true),
        errctx::Level::LevelError,
    );
    handleAllowedPacketOverflowed(&warning, "repeat", 1024).unwrap();
    let warnings = warning.CopyWarnings(Vec::new());
    assert_eq!(warnings.len(), 1);
    let warning_error = warnings[0].Err.as_ref().expect("packet warning error");
    assert!(warning_error.to_string().contains("repeat()"));
    assert!(warning_error.to_string().contains("1024"));

    let ignored = TestEvalContext::new(
        types::Flags(0).WithIgnoreTruncateErr(true),
        errctx::Level::LevelError,
    );
    handleAllowedPacketOverflowed(&ignored, "space", 2048).unwrap();
    let warnings = ignored.CopyWarnings(Vec::new());
    assert_eq!(warnings.len(), 1);
    let warning_error = warnings[0].Err.as_ref().expect("packet warning error");
    assert!(warning_error.to_string().contains("space()"));
    assert!(warning_error.to_string().contains("2048"));

    let strict = TestEvalContext::new(types::Flags(0), errctx::Level::LevelError);
    let error = handleAllowedPacketOverflowed(&strict, "repeat", 1024).unwrap_err();
    assert!(error.to_string().contains("repeat()"));
    assert!(error.to_string().contains("1024"));
    assert_eq!(strict.WarningCount(), 0);
}

/// 六类时间错误走 errCtx 路由；其中 Go 未将 week-mode 错误登记进 Truncate 组。
#[test]
fn invalid_time_only_routes_the_six_go_error_classes() {
    let warning = TestEvalContext::with_error_group(
        types::Flags(0),
        errctx::ErrGroup::ErrGroupTruncate,
        errctx::Level::LevelWarn,
    );
    let grouped_time_errors = [
        crate::errors::ErrWrongValue.GenWithStackByArgs(&["datetime".into(), "bad".into()]),
        crate::errors::ErrWrongValueForType.GenWithStackByArgs(&[
            "datetime".into(),
            "bad".into(),
            "date".into(),
        ]),
        crate::errors::ErrTruncatedWrongVal.GenWithStackByArgs(&["datetime".into(), "bad".into()]),
        crate::errors::ErrDatetimeFunctionOverflow.GenWithStackByArgs(&["datetime".into()]),
        crate::errors::ErrIncorrectDatetimeValue.GenWithStackByArgs(&["bad datetime".into()]),
    ];
    for time_error in grouped_time_errors {
        assert!(
            handleInvalidTimeError(&warning, Some(time_error)).is_none(),
            "Go time error registered in ErrGroupTruncate must become a warning"
        );
    }
    assert_eq!(warning.WarningCount(), 5);

    let week_mode = crate::errors::ErrInvalidWeekModeFormat
        .GenWithStackByArgs(&[contextutil::errors::ErrorArg::from("week")]);
    let returned = handleInvalidTimeError(&warning, Some(week_mode))
        .expect("Go errctx leaves the ungrouped week-mode error unchanged");
    assert!(crate::errors::ErrInvalidWeekModeFormat.Equal(Some(&returned)));
    assert_eq!(warning.WarningCount(), 5);

    let strict = TestEvalContext::with_error_group(
        types::Flags(0),
        errctx::ErrGroup::ErrGroupTruncate,
        errctx::Level::LevelError,
    );
    let time_error = crate::errors::ErrIncorrectDatetimeValue
        .GenWithStackByArgs(&[contextutil::errors::ErrorArg::from("bad datetime")]);
    let returned = handleInvalidTimeError(&strict, Some(time_error))
        .expect("strict mode returns the datetime error");
    assert!(crate::errors::ErrIncorrectDatetimeValue.Equal(Some(&returned)));
    assert_eq!(strict.WarningCount(), 0);

    let unrelated = contextutil::errors::NewNoStackError("unrelated");
    let returned = handleInvalidTimeError(&warning, Some(unrelated.clone()))
        .expect("unrelated errors pass through");
    assert!(returned.ptr_eq(&unrelated));
    assert!(handleInvalidTimeError(&warning, None).is_none());
}
