// Copyright 2026 AsterSQL.

// Juju 风格错误适配层。
//
// 将 TiDB/Go 侧常用的 `Trace`/`Annotate`/`NotFoundf` 等 API 映射到本包的
// `SharedError` 链：在包装上下文消息的同时，保证错误链上至多保留一份有效堆栈，
// 并以固定后缀模拟 Juju 的分类错误（not found、already exists 等）。

use super::core::{format_message, new_no_stack};
use super::wrap::{suspend_stack, with_stack};
use super::{AddStack, ErrorArg, Errorf, HasStack, SharedError, WithMessage};

/// Juju-compatible alias for [`AddStack`].
/// 等价于 [`AddStack`]：为尚无堆栈的错误补上一帧调用栈。
pub fn Trace(error: Option<SharedError>) -> Option<SharedError> {
    AddStack(error)
}

/// Adds a context message and ensures the chain has one non-empty stack.
/// 附加上下文消息；若原因错误尚无堆栈，则在此补捕获一份。
pub fn Annotate(error: Option<SharedError>, message: impl Into<String>) -> Option<SharedError> {
    let cause = error?;
    // 先记录原因是否已有堆栈，避免 WithMessage 后再误判
    let has_stack = HasStack(&cause);
    let annotated = WithMessage(Some(cause), message).expect("present cause remains present");
    if has_stack {
        Some(annotated)
    } else {
        // skip=2：跳过 with_stack 自身与本 Annotate 帧
        Some(with_stack(annotated, 2))
    }
}

/// Formats a context message and ensures the chain has one non-empty stack.
/// 格式化上下文消息后附加；堆栈策略与 [`Annotate`] 相同。
pub fn Annotatef(
    error: Option<SharedError>,
    format: &str,
    args: &[ErrorArg],
) -> Option<SharedError> {
    let cause = error?;
    let has_stack = HasStack(&cause);
    let annotated = WithMessage(Some(cause), format_message(format, args))
        .expect("present cause remains present");
    if has_stack {
        Some(annotated)
    } else {
        Some(with_stack(annotated, 2))
    }
}

/// Creates an error carrying an explicitly empty stack.
/// 构造显式无堆栈的基础错误，供上层稍后通过 [`Trace`] 补栈。
pub fn NewNoStackError(message: impl Into<String>) -> SharedError {
    new_no_stack(message)
}

/// Formats an error carrying an explicitly empty stack.
/// 格式化后构造显式无堆栈错误。
pub fn NewNoStackErrorf(format: &str, args: &[ErrorArg]) -> SharedError {
    NewNoStackError(format_message(format, args))
}

/// Clears library-owned stacks while allowing a later [`Trace`] to add one.
/// 挂起（清除）库持有的堆栈标记，允许后续再次 [`Trace`] 捕获新栈。
pub fn SuspendStack(error: Option<SharedError>) -> Option<SharedError> {
    error.map(suspend_stack)
}

/// Returns alternate debug formatting, including a stack when available.
/// 返回 `{:#?}` 调试串；有堆栈时会一并展开，便于日志排查。
pub fn ErrorStack(error: Option<&SharedError>) -> String {
    error.map_or_else(String::new, |error| format!("{error:#?}"))
}

/// Reports whether the rendered error contains Juju's not-found marker.
/// 通过渲染文本是否含 `"not found"` 判断是否为“未找到”类错误。
pub fn IsNotFound(error: &SharedError) -> bool {
    error.to_string().contains("not found")
}

/// Creates an error with Juju's not-found suffix.
/// 构造带 `" not found"` 后缀的格式化错误。
pub fn NotFoundf(format: &str, args: &[ErrorArg]) -> SharedError {
    suffixed_error(format, args, " not found")
}

/// Creates an error with Juju's bad-request suffix.
/// 构造带 `" bad request"` 后缀的格式化错误。
pub fn BadRequestf(format: &str, args: &[ErrorArg]) -> SharedError {
    suffixed_error(format, args, " bad request")
}

/// Creates an error with Juju's not-supported suffix.
/// 构造带 `" not supported"` 后缀的格式化错误。
pub fn NotSupportedf(format: &str, args: &[ErrorArg]) -> SharedError {
    suffixed_error(format, args, " not supported")
}

/// Creates an error with Juju's not-valid suffix.
/// 构造带 `" not valid"` 后缀的格式化错误。
pub fn NotValidf(format: &str, args: &[ErrorArg]) -> SharedError {
    suffixed_error(format, args, " not valid")
}

/// Reports whether the rendered error contains Juju's already-exists marker.
/// 通过渲染文本是否含 `"already exists"` 判断是否为“已存在”类错误。
pub fn IsAlreadyExists(error: &SharedError) -> bool {
    error.to_string().contains("already exists")
}

/// Creates an error with Juju's already-exists suffix.
/// 构造带 `" already exists"` 后缀的格式化错误。
pub fn AlreadyExistsf(format: &str, args: &[ErrorArg]) -> SharedError {
    suffixed_error(format, args, " already exists")
}

/// 将格式串与分类后缀拼接后交给 [`Errorf`]，生成带堆栈的基础错误。
fn suffixed_error(format: &str, args: &[ErrorArg], suffix: &str) -> SharedError {
    let mut suffixed = String::with_capacity(format.len() + suffix.len());
    suffixed.push_str(format);
    suffixed.push_str(suffix);
    Errorf(&suffixed, args)
}
