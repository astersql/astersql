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

// 内部断言公共实现：参数类型、消息格式化、实际检查与 panic。
//
// 启用版（`assert`）与禁用版（`no_assert`）共用本模块的 `doAssert*`，
// 以对齐 Go `intest` 的断言消息格式（首参为格式串，支持 `%s`/`%d`/`%v`/`%+v`）。

use std::fmt::{self, Display};
use std::sync::atomic::AtomicBool;

// EnableInternalCheck is the general switch for internal assertions.
/// 内部检查总开关：测试、`intest` 或 `enableassert` feature 下默认开启。
pub static EnableInternalCheck: AtomicBool = AtomicBool::new(cfg!(any(
    test,
    feature = "intest",
    feature = "enableassert"
)));

// AssertArg preserves Go's variadic message values for assertion formatting.
/// 断言可变参数：对应 Go 变参中的字符串与数值，用于格式化失败消息。
#[derive(Clone, Debug, PartialEq)]
pub enum AssertArg {
    /// 字符串参数。
    String(String),
    /// 有符号整数参数。
    Int(i64),
    /// 无符号整数参数。
    Uint(u64),
    /// 浮点参数。
    Float(f64),
    /// 布尔参数。
    Bool(bool),
}

impl Display for AssertArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(value) => f.write_str(value),
            Self::Int(value) => value.fmt(f),
            Self::Uint(value) => value.fmt(f),
            Self::Float(value) => value.fmt(f),
            Self::Bool(value) => value.fmt(f),
        }
    }
}

impl AssertArg {
    fn go_type_name(&self) -> &'static str {
        match self {
            Self::String(_) => "string",
            Self::Int(_) => "int64",
            Self::Uint(_) => "uint64",
            Self::Float(_) => "float64",
            Self::Bool(_) => "bool",
        }
    }

    fn accepts_verb(&self, verb: char) -> bool {
        match verb {
            's' => matches!(self, Self::String(_)),
            'd' => matches!(self, Self::Int(_) | Self::Uint(_)),
            'v' => true,
            _ => false,
        }
    }
}

impl From<&str> for AssertArg {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<String> for AssertArg {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

/// 为多种数值类型批量实现到 `AssertArg` 的转换。
macro_rules! impl_assert_arg {
    ($variant:ident: $($ty:ty),+ $(,)?) => {
        $(impl From<$ty> for AssertArg {
            fn from(value: $ty) -> Self {
                Self::$variant(value as _)
            }
        })+
    };
}

impl_assert_arg!(Int: i8, i16, i32, i64, isize);
impl_assert_arg!(Uint: u8, u16, u32, u64, usize);
impl_assert_arg!(Float: f32, f64);

impl From<bool> for AssertArg {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

/// 条件断言：失败则带用户消息 panic。
pub(crate) fn doAssert(cond: bool, msg_and_args: &[AssertArg]) {
    if !cond {
        doPanic("", msg_and_args);
    }
}

/// 断言无错误：存在可显示错误时 panic。
pub(crate) fn doAssertNoError(err: Option<&dyn Display>, msg_and_args: &[AssertArg]) {
    if let Some(err) = err {
        doPanic(&format!("error is not nil: {err}"), msg_and_args);
    }
}

/// 断言非空：`None` 对应 Go 的 nil。
pub(crate) fn doAssertNotNil<T>(obj: Option<T>, msg_and_args: &[AssertArg]) {
    // Option represents nil interfaces and typed nil pointers in the Rust API.
    doAssert(obj.is_some(), msg_and_args);
}

/// 断言函数非空且返回真；先查 nil 再调用，消息与 Go 一致。
pub(crate) fn doAssertFunc(fn_check: Option<fn() -> bool>, msg_and_args: &[AssertArg]) {
    // Check for nil before invocation so the panic message matches Go.
    doAssert(fn_check.is_some(), msg_and_args);
    doAssert(fn_check.expect("assert function checked")(), msg_and_args);
}

/// 组装失败消息并 panic，永不返回。
fn doPanic(extra_msg: &str, user_msg_and_args: &[AssertArg]) -> ! {
    panic!("{}", assertionFailedMsg(extra_msg, user_msg_and_args));
}

/// 拼装断言失败消息：前缀 `assert failed`，再拼用户格式串与额外说明。
pub(crate) fn assertionFailedMsg(extra_msg: &str, user_msg_and_args: &[AssertArg]) -> String {
    // Go treats the first user argument as the format string and the rest as values.
    let mut msg = String::from("assert failed");
    if user_msg_and_args.is_empty() {
        if !extra_msg.is_empty() {
            msg.push_str(", ");
            msg.push_str(extra_msg);
        }
        return msg;
    }

    msg.push_str(", ");
    msg.push_str(&user_msg_and_args[0].to_string());
    if !extra_msg.is_empty() {
        msg.push_str(", ");
        msg.push_str(extra_msg);
    }
    sprintf(&msg, &user_msg_and_args[1..])
}

/// 简易 sprintf：支持 `%%`、`%s`/`%d`/`%v`、`%+v`；缺参时对齐 Go `fmt` 诊断。
fn sprintf(format: &str, args: &[AssertArg]) -> String {
    let mut result = String::with_capacity(format.len());
    let mut chars = format.chars().peekable();
    let mut args = args.iter();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            result.push(ch);
            continue;
        }
        match chars.peek().copied() {
            Some('%') => {
                chars.next();
                result.push('%');
            }
            Some('s' | 'd' | 'v') => {
                let verb = chars.next().expect("format verb exists");
                if let Some(arg) = args.next() {
                    if arg.accepts_verb(verb) {
                        result.push_str(&arg.to_string());
                    } else {
                        result.push_str("%!");
                        result.push(verb);
                        result.push('(');
                        result.push_str(arg.go_type_name());
                        result.push('=');
                        result.push_str(&arg.to_string());
                        result.push(')');
                    }
                } else {
                    result.push_str("%!");
                    result.push(verb);
                    result.push_str("(MISSING)");
                }
            }
            Some('+') => {
                chars.next();
                if chars.next_if_eq(&'v').is_some() {
                    if let Some(arg) = args.next() {
                        result.push_str(&arg.to_string());
                    } else {
                        result.push_str("%!v(MISSING)");
                    }
                } else {
                    result.push_str("%+");
                }
            }
            _ => result.push('%'),
        }
    }
    if args.len() > 0 {
        result.push_str("%!(EXTRA ");
        for (index, arg) in args.enumerate() {
            if index > 0 {
                result.push_str(", ");
            }
            result.push_str(arg.go_type_name());
            result.push('=');
            result.push_str(&arg.to_string());
        }
        result.push(')');
    }
    result
}
