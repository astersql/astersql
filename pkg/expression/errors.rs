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

// 表达式求值错误定义与处理入口。
//
// 本模块集中声明 Expression 类错误码（如除零、参数个数不匹配、JSON/函数索引相关错误），
// 并提供按求值上下文（EvalContext）策略上报日期时间错误、除零错误以及
// `max_allowed_packet` 溢出警告的辅助函数。错误上下文（errCtx）决定告警或硬失败。

use std::sync::LazyLock;

use dbterror_dependency as dbterror;
use errno_dependency::errcode as errno;
use parser_mysql_dependency as pmysql;

use crate::{EvalContext, contextutil, errCtx, typeCtx};

/// 惰性初始化的 dbterror 标准错误句柄。
type DbError = LazyLock<Box<dbterror::terror::Error>>;
/// 可在上下文间共享的表达式错误（SharedError）。
type ExpressionError = contextutil::errors::SharedError;

/// 用 errno 错误码构造 Expression 类标准错误的便捷宏。
macro_rules! standard_error {
    ($visibility:vis $name:ident, $code:ident) => {
        $visibility static $name: DbError =
            LazyLock::new(|| dbterror::ClassExpression.NewStd(errno::$code));
    };
}

// —— 对外公开的标准表达式错误 ——
standard_error!(pub ErrIncorrectParameterCount, ErrWrongParamcountToNativeFct);
standard_error!(pub ErrDivisionByZero, ErrDivisionByZero);
standard_error!(pub ErrRegexp, ErrRegexp);
standard_error!(pub ErrOperandColumns, ErrOperandColumns);
standard_error!(pub ErrCutValueGroupConcat, ErrCutValueGroupConcat);
pub static ErrFunctionsNoopImpl: DbError = LazyLock::new(|| {
    dbterror::ClassExpression.NewStdErr(
        errno::ErrNotSupportedYet,
        &pmysql::errname::Message(
            "function %s has only noop implementation in tidb now, use tidb_enable_noop_functions to enable these functions",
            &[],
        ),
    )
});
standard_error!(pub ErrInvalidArgumentForLogarithm, ErrInvalidArgumentForLogarithm);
standard_error!(pub ErrIncorrectType, ErrIncorrectType);
standard_error!(pub ErrInvalidTypeForJSON, ErrInvalidTypeForJSON);
standard_error!(pub ErrInvalidTableSample, ErrInvalidTableSample);
standard_error!(pub ErrNotSupportedYet, ErrNotSupportedYet);
standard_error!(pub ErrInvalidJSONForFuncIndex, ErrInvalidJSONValueForFuncIndex);
standard_error!(pub ErrDataOutOfRangeFuncIndex, ErrDataOutOfRangeFunctionalIndex);
standard_error!(pub ErrFuncIndexDataIsTooLong, ErrFunctionalIndexDataIsTooLong);
standard_error!(pub ErrFunctionNotExists, ErrSpDoesNotExist);

// —— 包内使用的标准错误（含 zlib、参数、字符集、弃用语法等） ——
standard_error!(errZlibZData, ErrZlibZData);
standard_error!(errZlibZBuf, ErrZlibZBuf);
standard_error!(errIncorrectArgs, ErrWrongArguments);
standard_error!(errUnknownCharacterSet, ErrUnknownCharacterSet);
static errDefaultValue: DbError = LazyLock::new(|| {
    dbterror::ClassExpression.NewStdErr(
        errno::ErrInvalidDefault,
        &pmysql::errname::Message("invalid default value", &[]),
    )
});
standard_error!(
    errDeprecatedSyntaxNoReplacement,
    ErrWarnDeprecatedSyntaxNoReplacement
);
standard_error!(
    errWarnAllowedPacketOverflowed,
    ErrWarnAllowedPacketOverflowed
);
standard_error!(errWarnOptionIgnored, WarnOptionIgnored);
standard_error!(errTruncatedWrongValue, ErrTruncatedWrongValue);
standard_error!(errUnknownLocale, ErrUnknownLocale);
standard_error!(errNonUniq, ErrNonUniq);
standard_error!(errWrongValueForType, ErrWrongValueForType);
standard_error!(errUnknown, ErrUnknown);
standard_error!(pub(crate) errSpecificAccessDenied, ErrSpecificAccessDenied);
standard_error!(errUserLockDeadlock, ErrUserLockDeadlock);
standard_error!(errUserLockWrongName, ErrUserLockWrongName);
standard_error!(errJSONInBooleanContext, ErrJSONInBooleanContext);
standard_error!(errBadNull, ErrBadNull);
standard_error!(errSequenceAccessDenied, ErrTableaccessDenied);
static errUnsupportedJSONComparison: DbError = LazyLock::new(|| {
    dbterror::ClassExpression.NewStdErr(
        errno::ErrNotSupportedYet,
        &pmysql::errname::Message(
            "comparison of JSON in the LEAST and GREATEST operators",
            &[],
        ),
    )
});

/// 将 terror::Error 包装为可共享的 ExpressionError。
fn shared(error: &dbterror::terror::Error) -> ExpressionError {
    ExpressionError::new(error.clone())
}

/// Reports a datetime error or warning according to the evaluation context.
/// 按求值上下文的错误策略上报日期时间类错误或警告；非时间错误原样返回。
pub fn handleInvalidTimeError(
    ctx: &dyn EvalContext,
    err: Option<ExpressionError>,
) -> Option<ExpressionError> {
    let err = err?;
    let is_time_error = crate::errors::ErrWrongValue.Equal(Some(&err))
        || crate::errors::ErrWrongValueForType.Equal(Some(&err))
        || crate::errors::ErrTruncatedWrongVal.Equal(Some(&err))
        || crate::errors::ErrInvalidWeekModeFormat.Equal(Some(&err))
        || crate::errors::ErrDatetimeFunctionOverflow.Equal(Some(&err))
        || crate::errors::ErrIncorrectDatetimeValue.Equal(Some(&err));
    if !is_time_error {
        return Some(err);
    }
    // 时间类错误交由 errCtx 按 SQL mode / Level 转为警告或继续抛出。
    errCtx(ctx).HandleError(Some(err))
}

/// Reports division by zero according to the evaluation context's SQL policy.
/// 按求值上下文策略处理除零错误（可降为警告或返回错误）。
pub fn handleDivisionByZeroError(ctx: &dyn EvalContext) -> Option<ExpressionError> {
    errCtx(ctx).HandleError(Some(shared(&ErrDivisionByZero)))
}

/// Reports max_allowed_packet overflow as a warning when truncation is tolerated.
/// 当允许截断为警告时写入告警；否则返回错误。`max_allowed_packet` 限制单次包最大字节数。
pub fn handleAllowedPacketOverflowed(
    ctx: &dyn EvalContext,
    expression_name: &str,
    max_allowed_packet_size: u64,
) -> Result<(), ExpressionError> {
    let err = errWarnAllowedPacketOverflowed
        .FastGenByArgs(&[expression_name.into(), max_allowed_packet_size.into()]);
    let type_context = typeCtx(ctx);
    let flags = type_context.Flags();
    // TruncateAsWarning / IgnoreTruncateErr 时降级为警告，否则硬失败。
    if flags.TruncateAsWarning() || flags.IgnoreTruncateErr() {
        type_context.AppendWarning(err);
        return Ok(());
    }
    Err(contextutil::errors::Trace(Some(err)).expect("present error remains present"))
}
