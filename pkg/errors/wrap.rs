// Copyright 2026 AsterSQL.

// 错误包装与单因链工具（对应 Go `pkg/errors` 的 Wrap/WithStack/WithMessage 等）。
//
// 提供在已有 `SharedError` 上附加上下文消息与调用栈的构造函数，以及
// `Unwrap`/`Cause`/`Find`/`GetErrStackMsg`/`HasStack` 等沿单因错误链（single-cause
// chain，每层至多一个 cause）导航与消息拼接的辅助 API。

use std::error::Error as StdError;
use std::fmt;

use super::core::{
    format_message, fundamental_message, fundamental_stack_tracer, without_fundamental_stack,
};
use super::{ErrorArg, NewStack, SharedError, Stack, StackTrace, StackTraceCarrier, StackTracer};

/// 仅附加调用栈的包装错误：Display 透传 cause，扩展 Debug 追加栈跟踪。
struct WithStackError {
    cause: SharedError,
    stack: Stack,
}

impl fmt::Display for WithStackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.cause, formatter)
    }
}

impl fmt::Debug for WithStackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if formatter.alternate() {
            // `%+v` / `{:#?}`：先递归打印内层，再输出本层栈。
            fmt_extended(&self.cause, formatter)?;
            write!(formatter, "{:#?}", self.stack.stack_trace())
        } else {
            fmt::Display::fmt(self, formatter)
        }
    }
}

impl StdError for WithStackError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(&self.cause)
    }
}

impl StackTracer for WithStackError {
    fn stack_trace(&self) -> StackTrace {
        self.stack.stack_trace()
    }

    fn empty(&self) -> bool {
        self.stack.empty()
    }
}

/// 附加上下文消息的包装错误；`cause_has_stack` 缓存内层是否已有栈，供 `HasStack` 快查。
struct WithMessageError {
    cause: SharedError,
    message: String,
    cause_has_stack: bool,
}

impl fmt::Display for WithMessageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.message, self.cause)
    }
}

impl fmt::Debug for WithMessageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if formatter.alternate() {
            fmt_extended(&self.cause, formatter)?;
            write!(formatter, "\n{}", self.message)
        } else {
            fmt::Display::fmt(self, formatter)
        }
    }
}

impl StdError for WithMessageError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(&self.cause)
    }
}

/// 取本库单因链的下一层 cause（WithStack / WithMessage / Normalize）。
fn immediate_cause(error: &SharedError) -> Option<&SharedError> {
    if let Some(stacked) = error.downcast_ref::<WithStackError>() {
        return Some(&stacked.cause);
    }
    if let Some(cause) = error
        .downcast_ref::<WithMessageError>()
        .map(|messaged| &messaged.cause)
    {
        return Some(cause);
    }
    super::normalize::error_cause(error)
}

/// 递归输出扩展 Debug：先内层 cause，再本层栈或消息行。
fn fmt_extended(error: &SharedError, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    if let Some(stacked) = error.downcast_ref::<WithStackError>() {
        fmt_extended(&stacked.cause, formatter)?;
        return write!(formatter, "{:#?}", stacked.stack.stack_trace());
    }
    if let Some(messaged) = error.downcast_ref::<WithMessageError>() {
        fmt_extended(&messaged.cause, formatter)?;
        return write!(formatter, "\n{}", messaged.message);
    }
    if fundamental_stack_tracer(error).is_some() || super::normalize::error_message(error).is_some()
    {
        return fmt::Debug::fmt(error, formatter);
    }
    fmt::Display::fmt(error, formatter)
}

impl StackTraceCarrier for SharedError {
    fn stack_tracer(&self) -> Option<&dyn StackTracer> {
        self.downcast_ref::<WithStackError>()
            .map(|stacked| stacked as &dyn StackTracer)
            .or_else(|| fundamental_stack_tracer(self))
    }

    fn cause(&self) -> Option<&dyn StackTraceCarrier> {
        immediate_cause(self).map(|cause| cause as &dyn StackTraceCarrier)
    }
}

/// Reports whether a non-empty stack exists in the single-cause chain.
/// 报告单因错误链中是否已有非空调用栈；`WithMessage` 走缓存字段以避免重复遍历。
pub fn HasStack(error: &SharedError) -> bool {
    if let Some(messaged) = error.downcast_ref::<WithMessageError>() {
        return messaged.cause_has_stack;
    }
    super::GetStackTracer(error).is_some_and(|tracer| !tracer.empty())
}

/// Always adds a stack to a present error.
/// 总是为存在的错误再包一层调用栈（`NewStack(1)` 跳过本函数帧）。
#[inline(never)]
pub fn WithStack(error: Option<SharedError>) -> Option<SharedError> {
    let cause = error?;
    Some(SharedError::new(WithStackError {
        cause,
        stack: NewStack(1),
    }))
}

/// Adds a stack only when the error chain does not already contain one.
/// 仅当错误链尚无栈时附加；已有栈则原样返回，实现去重。
#[inline(never)]
pub fn AddStack(error: Option<SharedError>) -> Option<SharedError> {
    let cause = error?;
    if HasStack(&cause) {
        return Some(cause);
    }
    Some(SharedError::new(WithStackError {
        cause,
        stack: NewStack(1),
    }))
}

/// 以指定 skip 层数包装调用栈（供 crate 内部生成路径复用）。
pub(crate) fn with_stack(cause: SharedError, skip: usize) -> SharedError {
    SharedError::new(WithStackError {
        cause,
        stack: NewStack(skip),
    })
}

/// 包装空栈标记层，用于占位或配合 `suspend_stack` 清除可见栈。
pub(crate) fn with_empty_stack(cause: SharedError) -> SharedError {
    SharedError::new(WithStackError {
        cause,
        stack: Stack::default(),
    })
}

/// 尝试清除链上真实栈；若本就无栈可清则改为挂空栈包装。
pub(crate) fn suspend_stack(error: SharedError) -> SharedError {
    let (error, cleared) = clear_stack(error);
    if cleared {
        error
    } else {
        with_empty_stack(error)
    }
}

/// 递归清除 fundamental / WithStack 上的栈；WithMessage 透传并对 cause 重建包装。
fn clear_stack(error: SharedError) -> (SharedError, bool) {
    if let Some(fundamental) = without_fundamental_stack(&error) {
        return (fundamental, true);
    }
    if let Some(stacked) = error.downcast_ref::<WithStackError>() {
        let (cause, _) = clear_stack(stacked.cause.clone());
        return (with_empty_stack(cause), true);
    }
    if let Some(messaged) = error.downcast_ref::<WithMessageError>() {
        let (cause, cleared) = clear_stack(messaged.cause.clone());
        if !cleared {
            return (error, false);
        }
        return (
            SharedError::new(WithMessageError {
                cause,
                message: messaged.message.clone(),
                cause_has_stack: messaged.cause_has_stack,
            }),
            true,
        );
    }
    (error, false)
}

/// Adds a context message and a new stack to a present error.
/// 为存在的错误附加上下文消息，并再包一层新调用栈（等价于 WithMessage + WithStack）。
#[inline(never)]
pub fn Wrap(error: Option<SharedError>, message: impl Into<String>) -> Option<SharedError> {
    let cause = error?;
    let cause_has_stack = HasStack(&cause);
    let cause = SharedError::new(WithMessageError {
        cause,
        message: message.into(),
        cause_has_stack,
    });
    Some(SharedError::new(WithStackError {
        cause,
        stack: NewStack(1),
    }))
}

/// Formats a context message and adds it with a new stack.
/// 按 Go 风格格式化上下文消息后，再附加消息与新调用栈。
pub fn Wrapf(error: Option<SharedError>, format: &str, args: &[ErrorArg]) -> Option<SharedError> {
    let cause = error?;
    let cause_has_stack = HasStack(&cause);
    let cause = SharedError::new(WithMessageError {
        cause,
        message: format_message(format, args),
        cause_has_stack,
    });
    Some(SharedError::new(WithStackError {
        cause,
        stack: NewStack(1),
    }))
}

/// Adds a context message without adding a stack.
/// 仅附加上下文消息，不新增调用栈；缓存内层是否已有栈。
pub fn WithMessage(error: Option<SharedError>, message: impl Into<String>) -> Option<SharedError> {
    error.map(|cause| {
        let cause_has_stack = HasStack(&cause);
        SharedError::new(WithMessageError {
            cause,
            message: message.into(),
            cause_has_stack,
        })
    })
}

/// Returns the next node in this library's single-cause chain.
/// 返回本库单因链的下一节点（剥一层包装）。
pub fn Unwrap(error: Option<&SharedError>) -> Option<SharedError> {
    immediate_cause(error?).cloned()
}

/// Returns the deepest node in this library's single-cause chain.
/// 沿单因链走到最内层根错误并返回。
pub fn Cause(error: Option<&SharedError>) -> Option<SharedError> {
    let mut cause = error?.clone();
    while let Some(next) = Unwrap(Some(&cause)) {
        cause = next;
    }
    Some(cause)
}

/// Finds the first matching node from outermost to innermost.
/// 自外向内查找第一个满足谓词的节点；找不到则返回 None。
pub fn Find<F>(error: Option<&SharedError>, mut test: F) -> Option<SharedError>
where
    F: FnMut(&SharedError) -> bool,
{
    let mut found = None;
    super::WalkDeep(error, |current| {
        if test(current) {
            found = Some(current.clone());
            true
        } else {
            false
        }
    });
    found
}

/// Concatenates each wrapper's own message, excluding formatting decorations.
/// 拼接各层自身消息（跳过栈装饰），形成 `a: b: c` 风格的错误栈文本。
pub fn GetErrStackMsg(error: Option<&SharedError>) -> String {
    let Some(error) = error else {
        return String::new();
    };

    // WithStack 不贡献消息，直接下钻。
    if let Some(stacked) = error.downcast_ref::<WithStackError>() {
        return GetErrStackMsg(Some(&stacked.cause));
    }
    if let Some(messaged) = error.downcast_ref::<WithMessageError>() {
        let cause_message = GetErrStackMsg(Some(&messaged.cause));
        if cause_message.is_empty() {
            return messaged.message.clone();
        }
        return format!("{}: {cause_message}", messaged.message);
    }
    if let Some(message) = super::normalize::error_message(error) {
        if let Some(cause) = super::normalize::error_cause(error) {
            let cause_message = GetErrStackMsg(Some(cause));
            if !cause_message.is_empty() {
                return format!("{message}: {cause_message}");
            }
        }
        return message;
    }
    fundamental_message(error)
        .map(str::to_owned)
        .unwrap_or_else(|| error.to_string())
}
