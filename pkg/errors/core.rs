// Copyright 2026 AsterSQL.

// 错误核心类型与 Go 风格格式化。
//
// 提供线程安全、可廉价克隆的 [`SharedError`]，以及映射 Go 可变参数的 [`ErrorArg`]；
// [`New`]/[`Errorf`] 构造带调用栈的基础错误（Fundamental）。

use std::error::Error as StdError;
use std::fmt;
use std::sync::Arc;

use super::stack::{NewStack, Stack, StackTrace, StackTracer};

/// The thread-safe dynamic error type shared by this module's public APIs.
/// 模块公开 API 共用的动态错误 trait 对象：`Send + Sync + 'static`。
pub type DynError = dyn StdError + Send + Sync + 'static;

/// An owned, cheaply cloneable error value.
/// 所有权错误值：内部用 `Arc` 共享，克隆廉价；可选挂载 [`ErrorGroup`]。
#[derive(Clone)]
pub struct SharedError {
    error: Arc<DynError>,
    /// 若该错误同时是多原因组，缓存其 [`ErrorGroup`] 视图。
    group: Option<Arc<dyn super::group::ErrorGroup>>,
}

impl SharedError {
    /// 将任意标准错误包装为共享错误（非组）。
    pub fn new<E>(error: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        Self {
            error: Arc::new(error),
            group: None,
        }
    }

    /// Owns an error that exposes multiple independent causes.
    /// 包装实现了 [`ErrorGroup`] 的错误，并同时缓存 group 视图。
    pub fn new_group<E>(error: E) -> Self
    where
        E: super::group::ErrorGroup,
    {
        let error = Arc::new(error);
        Self {
            error: error.clone(),
            group: Some(error),
        }
    }

    /// 尝试向下转型为具体错误类型 `E`。
    pub fn downcast_ref<E>(&self) -> Option<&E>
    where
        E: StdError + 'static,
    {
        self.error.downcast_ref::<E>()
    }

    /// 比较底层 `Arc` 指针是否指向同一错误对象。
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.error, &other.error)
    }

    /// 返回缓存的错误组视图（若有）。
    pub(crate) fn error_group(&self) -> Option<&dyn super::group::ErrorGroup> {
        self.group.as_deref()
    }
}

impl fmt::Display for SharedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.error, formatter)
    }
}

impl fmt::Debug for SharedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.error, formatter)
    }
}

impl StdError for SharedError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(self.error.as_ref())
    }
}

/// An owned argument accepted by the Rust mapping of Go's variadic format APIs.
/// 映射 Go 可变参数的已拥有参数枚举；`Redact` 变体用于日志脱敏包裹。
#[derive(Clone, Debug, PartialEq)]
pub enum ErrorArg {
    String(String),
    Bool(bool),
    Signed(i128),
    Unsigned(u128),
    Float(f64),
    Debug(String),
    #[doc(hidden)]
    Redacted(Box<ErrorArg>),
}

impl ErrorArg {
    /// 用 `Debug` 格式冻结任意值，避免延迟格式化时生命周期问题。
    pub fn debug<T>(value: T) -> Self
    where
        T: fmt::Debug,
    {
        Self::Debug(format!("{value:?}"))
    }

    /// Freezes an ephemeral string argument before deferred error formatting.
    /// 在延迟格式化前冻结可能别名可变存储的字符串（见 [`HackedStr`]）。
    pub fn from_hacked<T>(value: &T) -> Self
    where
        T: super::normalize::HackedStr + ?Sized,
    {
        Self::String(value.FreezeStr())
    }

    /// 生成用于 `%v` 等默认展示的字符串。
    fn display_value(&self) -> String {
        match self {
            Self::String(value) | Self::Debug(value) => value.clone(),
            Self::Bool(value) => value.to_string(),
            Self::Signed(value) => value.to_string(),
            Self::Unsigned(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::Redacted(value) => value.display_value(),
        }
    }

    /// 返回与 Go `fmt` 错误报告相近的类型名。
    fn go_type_name(&self) -> &'static str {
        match self {
            Self::String(_) | Self::Debug(_) => "string",
            Self::Bool(_) => "bool",
            Self::Signed(_) => "int",
            Self::Unsigned(_) => "uint",
            Self::Float(_) => "float64",
            Self::Redacted(value) => value.go_type_name(),
        }
    }

    fn format_verb(&self, verb: char) -> String {
        self.format_verb_with_precision(verb, None)
    }

    /// 按 Go 风格动词（`s`/`d`/`v`）格式化；脱敏参数用 `‹›` 包裹并转义。
    fn format_verb_with_precision(&self, verb: char, precision: Option<usize>) -> String {
        if let Self::Redacted(value) = self {
            let formatted = value.format_verb_with_precision(verb, precision);
            // 转义已有标记，避免嵌套脱敏边界混淆
            let escaped = formatted.replace('‹', "‹‹").replace('›', "››");
            return format!("‹{escaped}›");
        }
        match (verb, self) {
            ('s', Self::String(value)) => precision
                .map(|limit| value.chars().take(limit).collect())
                .unwrap_or_else(|| value.clone()),
            ('d', Self::Signed(value)) => value.to_string(),
            ('d', Self::Unsigned(value)) => value.to_string(),
            ('v', value) => value.display_value(),
            // 类型与动词不匹配时输出 Go 风格的 `%!v(type=value)` 诊断
            _ => format!("%!{verb}({}={})", self.go_type_name(), self.display_value()),
        }
    }
}

impl From<&str> for ErrorArg {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<String> for ErrorArg {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<bool> for ErrorArg {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

macro_rules! impl_signed_error_arg {
    ($($type:ty),+ $(,)?) => {
        $(
            impl From<$type> for ErrorArg {
                fn from(value: $type) -> Self {
                    Self::Signed(value as i128)
                }
            }
        )+
    };
}

macro_rules! impl_unsigned_error_arg {
    ($($type:ty),+ $(,)?) => {
        $(
            impl From<$type> for ErrorArg {
                fn from(value: $type) -> Self {
                    Self::Unsigned(value as u128)
                }
            }
        )+
    };
}

impl_signed_error_arg!(i8, i16, i32, i64, i128, isize);
impl_unsigned_error_arg!(u8, u16, u32, u64, u128, usize);

impl From<f32> for ErrorArg {
    fn from(value: f32) -> Self {
        Self::Float(value.into())
    }
}

impl From<f64> for ErrorArg {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

/// 带字面消息与可选调用栈的基础错误节点。
struct Fundamental {
    message: String,
    stack: Stack,
}

impl fmt::Display for Fundamental {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl StdError for Fundamental {}

impl fmt::Debug for Fundamental {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)?;
        // `#` 交替格式时追加堆栈展开
        if formatter.alternate() {
            write!(formatter, "{:#?}", self.stack.stack_trace())?;
        }
        Ok(())
    }
}

impl StackTracer for Fundamental {
    fn stack_trace(&self) -> StackTrace {
        self.stack.stack_trace()
    }

    fn empty(&self) -> bool {
        self.stack.empty()
    }
}

/// Creates an error with the supplied literal message.
/// 用字面消息创建带堆栈的错误；`inline(never)` 便于堆栈跳过本帧。
#[inline(never)]
pub fn New(message: impl Into<String>) -> SharedError {
    SharedError::new(Fundamental {
        message: message.into(),
        stack: NewStack(1),
    })
}

/// Formats the supported Go-style verbs and returns the resulting error.
/// 按支持的 Go 风格动词格式化消息并创建带堆栈的错误。
pub fn Errorf(format: &str, args: &[ErrorArg]) -> SharedError {
    SharedError::new(Fundamental {
        message: format_message(format, args),
        stack: NewStack(1),
    })
}

/// 解析 `%s`/`%d`/`%v`（可带精度）并消费 [`ErrorArg`]；`%%` 输出字面 `%`。
pub(crate) fn format_message(format: &str, args: &[ErrorArg]) -> String {
    let mut output = String::with_capacity(format.len());
    let mut chars = format.chars().peekable();
    let mut arguments = args.iter();

    while let Some(character) = chars.next() {
        if character != '%' {
            output.push(character);
            continue;
        }

        // `%%` → 字面百分号
        if chars.peek() == Some(&'%') {
            chars.next();
            output.push('%');
            continue;
        }

        // 收集直到字母动词的格式说明符
        let mut specification = String::new();
        let mut verb = None;
        while let Some(next) = chars.next() {
            specification.push(next);
            if next.is_ascii_alphabetic() {
                verb = Some(next);
                break;
            }
        }

        let Some(verb @ ('s' | 'd' | 'v')) = verb else {
            // 不支持的动词：原样回写 `%` + 说明符
            output.push('%');
            output.push_str(&specification);
            continue;
        };
        let precision = specification
            .strip_suffix(verb)
            .and_then(|prefix| prefix.rsplit_once('.'))
            .and_then(|(_, digits)| digits.parse::<usize>().ok());
        match arguments.next() {
            Some(argument) => {
                output.push_str(&argument.format_verb_with_precision(verb, precision))
            }
            None => output.push_str(&format!("%!{verb}(MISSING)")),
        }
    }

    output
}

/// 若底层是 Fundamental，返回其消息切片。
pub(crate) fn fundamental_message(error: &SharedError) -> Option<&str> {
    error
        .downcast_ref::<Fundamental>()
        .map(|fundamental| fundamental.message.as_str())
}

/// 若底层是 Fundamental，返回其 [`StackTracer`] 视图。
pub(crate) fn fundamental_stack_tracer(error: &SharedError) -> Option<&dyn StackTracer> {
    error
        .downcast_ref::<Fundamental>()
        .map(|fundamental| fundamental as &dyn StackTracer)
}

/// 去掉 Fundamental 上的堆栈，保留相同消息的无栈副本。
pub(crate) fn without_fundamental_stack(error: &SharedError) -> Option<SharedError> {
    let fundamental = error.downcast_ref::<Fundamental>()?;
    Some(new_no_stack(fundamental.message.clone()))
}

/// 构造显式空堆栈的 Fundamental（供 adaptor/SuspendStack 使用）。
pub(crate) fn new_no_stack(message: impl Into<String>) -> SharedError {
    SharedError::new(Fundamental {
        message: message.into(),
        stack: Stack::default(),
    })
}
