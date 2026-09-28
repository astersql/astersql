// Copyright 2026 AsterSQL.

// 规范化错误（terror）与日志脱敏。
//
// 对应 Go `Normalize`/`Error`：携带 MySQL/RFC 错误码、消息模板与参数，
// 支持生成时捕获堆栈或挂起堆栈，并按全局开关对敏感参数脱敏。

use std::error::Error as StdError;
use std::fmt;
use std::panic::Location;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::de::Deserializer;
use serde::ser::{SerializeStruct, Serializer};
use serde::{Deserialize, Serialize};

use super::{
    AddStack, Cause as RootCause, ErrorArg, SharedError, SuspendStack, Unwrap as NextCause,
};

#[cfg(test)]
#[path = "normalize_test.rs"]
mod normalize_test;

/// 日志脱敏关闭（对应 Go `"OFF"`）。
pub const RedactLogDisable: &str = "OFF";
/// 日志脱敏开启：敏感参数替换为 `"?"`。
pub const RedactLogEnable: &str = "ON";
/// 日志脱敏标记模式：敏感参数用 `‹›` 包裹而非抹除。
pub const RedactLogMarker: &str = "MARKER";

/// Thread-safe Rust mapping of Go's atomic string redaction switch.
/// 线程安全的脱敏开关，内部用 `AtomicU8` 编码 OFF/ON/MARKER。
pub struct AtomicRedactLogState(AtomicU8);

impl AtomicRedactLogState {
    const fn new() -> Self {
        Self(AtomicU8::new(0))
    }

    /// 按字符串写入开关状态；未知值视为 OFF。
    pub fn Store(&self, value: &str) {
        let state = match value {
            RedactLogEnable => 1,
            RedactLogMarker => 2,
            _ => 0,
        };
        self.0.store(state, Ordering::SeqCst);
    }

    /// 读出当前开关对应的静态字符串。
    pub fn Load(&self) -> &'static str {
        match self.0.load(Ordering::SeqCst) {
            1 => RedactLogEnable,
            2 => RedactLogMarker,
            _ => RedactLogDisable,
        }
    }
}

/// 进程级全局脱敏开关，供 [`RedactErrorArg`] 查询。
pub static RedactLogEnabled: AtomicRedactLogState = AtomicRedactLogState::new();

/// Supplies a stable snapshot for a string that may alias mutable storage.
/// 为可能别名可变存储的字符串提供稳定快照，避免延迟格式化读到脏数据。
pub trait HackedStr {
    fn FreezeStr(&self) -> String;
}

/// MySQL 风格数值错误码。
pub type ErrCode = i32;
/// RFC/文本形式的错误码字符串。
pub type ErrCodeText = String;
/// 错误身份标识（优先文本码，否则数值码字符串）。
pub type ErrorID = String;
/// RFC 错误码别名，语义同 [`ErrorID`]。
pub type RFCErrorCode = String;

/// [`Normalize`] 的可选配置项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NormalizeOption {
    /// 指定需要脱敏的参数下标列表。
    RedactArgs(Vec<usize>),
    /// 设置 RFC/文本错误码。
    RFCCodeText(ErrCodeText),
    /// 设置 MySQL 数值错误码。
    MySQLErrorCode(ErrCode),
}

/// 规范化错误：错误码 + 消息模板 + 参数 + 可选 cause 与源位置。
#[derive(Clone)]
pub struct Error {
    code: ErrCode,
    code_text: ErrCodeText,
    message: String,
    redact_args_pos: Vec<usize>,
    cause: Option<SharedError>,
    args: Vec<ErrorArg>,
    file: String,
    line: i32,
}

impl Error {
    /// 返回 MySQL 数值错误码。
    pub fn Code(&self) -> ErrCode {
        self.code
    }

    /// 返回 RFC 码（与 [`ID`] 相同）。
    pub fn RFCCode(&self) -> RFCErrorCode {
        self.ID()
    }

    /// 错误身份：有文本码用文本，否则用数值码字符串。
    pub fn ID(&self) -> ErrorID {
        if self.code_text.is_empty() {
            self.code.to_string()
        } else {
            self.code_text.clone()
        }
    }

    /// 返回生成时记录的源文件与行号。
    pub fn Location(&self) -> (&str, i32) {
        (&self.file, self.line)
    }

    /// 返回未格式化的消息模板。
    pub fn MessageTemplate(&self) -> &str {
        &self.message
    }

    /// 返回当前绑定的格式化参数。
    pub fn Args(&self) -> &[ErrorArg] {
        &self.args
    }

    /// 用模板与参数生成展示消息；无参数时直接返回模板副本。
    pub fn GetMsg(&self) -> String {
        if self.args.is_empty() {
            self.message.clone()
        } else {
            super::core::format_message(&self.message, &self.args)
        }
    }

    /// 与 [`GetMsg`] 相同，保留 Go API 命名。
    pub fn GetSelfMsg(&self) -> String {
        self.GetMsg()
    }

    /// 比较根因是否为相同 ID 的规范化错误。
    pub fn Equal(&self, error: Option<&SharedError>) -> bool {
        let Some(origin) = RootCause(error) else {
            return false;
        };
        origin
            .downcast_ref::<Self>()
            .is_some_and(|other| self.ID() == other.ID())
    }

    /// [`Equal`] 的取反。
    pub fn NotEqual(&self, error: Option<&SharedError>) -> bool {
        !self.Equal(error)
    }

    /// 克隆自身并挂上 cause；cause 为 `None` 时返回 `None`。
    pub fn Wrap(&self, cause: Option<SharedError>) -> Option<Self> {
        cause.map(|cause| {
            let mut wrapped = self.clone();
            wrapped.cause = Some(cause);
            wrapped
        })
    }

    /// 返回直接挂载的 cause。
    pub fn Unwrap(&self) -> Option<SharedError> {
        self.cause.clone()
    }

    /// 按 ID 判断两个规范化错误是否同类。
    pub fn Is(&self, other: &Self) -> bool {
        self.ID() == other.ID()
    }

    /// 对直接 cause 剥一层包装；若无法下钻则返回直接 cause。
    pub fn Cause(&self) -> Option<SharedError> {
        let cause = self.cause.as_ref()?;
        NextCause(Some(cause)).or_else(|| Some(cause.clone()))
    }

    /// 用自定义格式串生成带堆栈的错误实例。
    #[track_caller]
    pub fn GenWithStack(&self, format: &str, args: &[ErrorArg]) -> SharedError {
        self.generate(Some(format), args, true, false, false)
    }

    /// 用原型消息模板按参数生成带堆栈实例，并按配置脱敏参数。
    #[track_caller]
    pub fn GenWithStackByArgs(&self, args: &[ErrorArg]) -> SharedError {
        self.generate(None, args, true, true, false)
    }

    /// 快速生成：不捕获堆栈（挂起栈），自定义格式串。
    pub fn FastGen(&self, format: &str, args: &[ErrorArg]) -> SharedError {
        self.generate(Some(format), args, false, false, false)
    }

    /// 快速生成：按原型模板填参数且脱敏，无堆栈。
    pub fn FastGenByArgs(&self, args: &[ErrorArg]) -> SharedError {
        self.generate(None, args, false, true, false)
    }

    /// 快速生成：消息取自已挂载 cause 的展示文本。
    pub fn FastGenWithCause(&self, args: &[ErrorArg]) -> SharedError {
        self.generate(None, args, false, false, true)
    }

    /// 同 [`FastGenWithCause`]，但捕获调用栈。
    #[track_caller]
    pub fn GenWithStackByCause(&self, args: &[ErrorArg]) -> SharedError {
        self.generate(None, args, true, false, true)
    }

    /// 统一生成路径：可选覆盖消息、是否堆栈、是否脱敏、是否用 cause 消息。
    #[track_caller]
    fn generate(
        &self,
        format: Option<&str>,
        args: &[ErrorArg],
        stackful: bool,
        redact: bool,
        use_cause_message: bool,
    ) -> SharedError {
        let mut generated = self.clone();
        if let Some(format) = format {
            generated.message = format.to_owned();
        } else if use_cause_message && let Some(cause) = &self.cause {
            generated.message = cause.to_string();
        }
        generated.args = args.to_vec();
        if redact {
            RedactErrorArg(&mut generated.args, &self.redact_args_pos);
        }
        if stackful {
            // track_caller：记录真正对外调用点的文件与行号
            let caller = Location::caller();
            generated.file = caller.file().to_owned();
            generated.line = caller.line() as i32;
            AddStack(Some(SharedError::new(generated)))
                .expect("a generated error is always present")
        } else {
            SuspendStack(Some(SharedError::new(generated)))
                .expect("a generated error is always present")
        }
    }
}

impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        self.ID() == other.ID()
    }
}

impl Eq for Error {}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "[{}]{}", self.RFCCode(), self.GetMsg())?;
        if let Some(cause) = &self.cause {
            write!(formatter, ": {cause}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 交替格式：先展开 cause 堆栈，再附加本层 RFC 消息
        if formatter.alternate()
            && let Some(cause) = &self.cause
        {
            write!(
                formatter,
                "{cause:#?}\n[{}]{}",
                self.RFCCode(),
                self.GetMsg()
            )
        } else {
            fmt::Display::fmt(self, formatter)
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.cause
            .as_ref()
            .map(|cause| cause as &(dyn StdError + 'static))
    }
}

impl Serialize for Error {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // JSON 字段对齐 Go MarshalJSON：class/code/message/rfccode
        let component = self.code_text.split(':').next().unwrap_or_default();
        let mut state = serializer.serialize_struct("Error", 4)?;
        state.serialize_field("class", &class_for_component(component))?;
        state.serialize_field("code", &self.code)?;
        state.serialize_field("message", &self.GetMsg())?;
        state.serialize_field("rfccode", &self.code_text)?;
        state.end()
    }
}

/// 反序列化用的中间 JSON 结构。
#[derive(Deserialize)]
struct JsonError {
    #[serde(default)]
    class: i32,
    #[serde(default)]
    code: ErrCode,
    #[serde(default, rename = "message")]
    message: String,
    #[serde(default)]
    rfccode: String,
}

impl<'de> Deserialize<'de> for Error {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let json = JsonError::deserialize(deserializer)?;
        // 仅有 class 时反推 `component:code` 文本码
        let code_text = if json.rfccode.is_empty() && json.class > 0 {
            component_for_class(json.class)
                .map(|component| format!("{component}:{}", json.code))
                .unwrap_or_default()
        } else {
            json.rfccode
        };
        Ok(Self {
            code: json.code,
            code_text,
            message: json.message,
            redact_args_pos: Vec::new(),
            cause: None,
            args: Vec::new(),
            file: String::new(),
            line: 0,
        })
    }
}

/// 构造“按位置脱敏参数”的规范化选项。
pub fn RedactArgs(positions: &[usize]) -> NormalizeOption {
    NormalizeOption::RedactArgs(positions.to_vec())
}

/// Redacts selected arguments according to [`RedactLogEnabled`].
/// 按全局脱敏开关处理指定下标的参数（替换为 `?` 或标记包裹）。
pub fn RedactErrorArg(args: &mut [ErrorArg], positions: &[usize]) {
    match RedactLogEnabled.Load() {
        RedactLogEnable => {
            for &position in positions {
                if let Some(argument) = args.get_mut(position) {
                    *argument = ErrorArg::from("?");
                }
            }
        }
        RedactLogMarker => {
            for &position in positions {
                if let Some(argument) = args.get_mut(position) {
                    *argument = ErrorArg::Redacted(Box::new(argument.clone()));
                }
            }
        }
        _ => {}
    }
}

/// 构造 RFC 文本错误码选项。
pub fn RFCCodeText(code_text: impl Into<String>) -> NormalizeOption {
    NormalizeOption::RFCCodeText(code_text.into())
}

/// 构造 MySQL 数值错误码选项。
pub fn MySQLErrorCode(code: ErrCode) -> NormalizeOption {
    NormalizeOption::MySQLErrorCode(code)
}

/// 用消息模板与选项列表构造规范化错误原型（尚未绑定具体参数/堆栈）。
pub fn Normalize(message: impl Into<String>, options: &[NormalizeOption]) -> Error {
    let mut error = Error {
        code: 0,
        code_text: String::new(),
        message: message.into(),
        redact_args_pos: Vec::new(),
        cause: None,
        args: Vec::new(),
        file: String::new(),
        line: 0,
    };
    for option in options {
        match option {
            NormalizeOption::RedactArgs(positions) => error.redact_args_pos.clone_from(positions),
            NormalizeOption::RFCCodeText(code_text) => error.code_text.clone_from(code_text),
            NormalizeOption::MySQLErrorCode(code) => error.code = *code,
        }
    }
    error
}

/// 比较两个错误的根因是否等价（同指针、同规范化 ID，或同展示文本）。
pub fn ErrorEqual(left: Option<&SharedError>, right: Option<&SharedError>) -> bool {
    let left = RootCause(left);
    let right = RootCause(right);
    match (left, right) {
        (None, None) => true,
        (Some(_), None) | (None, Some(_)) => false,
        (Some(left), Some(right)) if left.ptr_eq(&right) => true,
        (Some(left), Some(right)) => {
            match (left.downcast_ref::<Error>(), right.downcast_ref::<Error>()) {
                (Some(left), Some(right)) => left.ID() == right.ID(),
                _ => left.to_string() == right.to_string(),
            }
        }
    }
}

/// [`ErrorEqual`] 的取反。
pub fn ErrorNotEqual(left: Option<&SharedError>, right: Option<&SharedError>) -> bool {
    !ErrorEqual(left, right)
}

/// 若为规范化错误，返回其直接 cause 引用。
pub(crate) fn error_cause(error: &SharedError) -> Option<&SharedError> {
    error.downcast_ref::<Error>()?.cause.as_ref()
}

/// 若为规范化错误，返回格式化后的消息。
pub(crate) fn error_message(error: &SharedError) -> Option<String> {
    error.downcast_ref::<Error>().map(Error::GetMsg)
}

/// 由 RFC 码的组件名前缀反查 class 编号；未知则 0。
fn class_for_component(component: &str) -> i32 {
    (1..=27)
        .find(|class| component_for_class(*class) == Some(component))
        .unwrap_or(0)
}

/// TiDB 错误 class 编号到组件名的固定映射表。
fn component_for_class(class: i32) -> Option<&'static str> {
    match class {
        1 => Some("autoid"),
        2 => Some("ddl"),
        3 => Some("domain"),
        4 => Some("evaluator"),
        5 => Some("executor"),
        6 => Some("expression"),
        7 => Some("admin"),
        8 => Some("kv"),
        9 => Some("meta"),
        10 => Some("planner"),
        11 => Some("parser"),
        12 => Some("perfschema"),
        13 => Some("privilege"),
        14 => Some("schema"),
        15 => Some("server"),
        16 => Some("struct"),
        17 => Some("variable"),
        18 => Some("xeval"),
        19 => Some("table"),
        20 => Some("types"),
        21 => Some("global"),
        22 => Some("mocktikv"),
        23 => Some("json"),
        24 => Some("tikv"),
        25 => Some("session"),
        26 => Some("plugin"),
        27 => Some("util"),
        _ => None,
    }
}
