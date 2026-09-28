// Copyright 2026 AsterSQL.

// 调用栈捕获与帧格式化。
//
// 对应 Go `pkg/errors` 的 Stack/Frame：捕获真实 backtrace，解析符号后提供
// 紧凑（`basename:line`）与扩展（`function\n\tpath:line`）两种展示；
// [`StackTracer`] / [`StackTraceCarrier`] 用于在错误链上定位堆栈。

use std::ffi::c_void;
use std::fmt;
use std::ops::{Deref, Index};
use std::path::Path;

use backtrace::Backtrace;

const UNKNOWN: &str = "unknown";

/// A resolved Rust stack frame.
///
/// Go keeps a program counter and resolves it when formatting. Rust's mapping
/// keeps that instruction pointer as well as the structured symbol data
/// resolved while the backtrace is captured.
/// 已解析的栈帧：保留指令指针及捕获时解析出的文件/行号/函数名。
#[derive(Clone, Eq, PartialEq)]
pub struct Frame {
    instruction_pointer: usize,
    file: String,
    line: u32,
    function: String,
}

impl Frame {
    /// Resolves one instruction pointer into a queryable frame.
    /// 将指令指针解析为可查询栈帧；指针为 0 时直接返回 unknown。
    pub fn from_instruction_pointer(instruction_pointer: usize) -> Self {
        let mut frame = Self::unknown(instruction_pointer);
        if instruction_pointer == 0 {
            return frame;
        }

        let mut found = false;
        // 只取第一个命中的符号，忽略内联展开的后续条目
        backtrace::resolve(instruction_pointer as *mut c_void, |symbol| {
            if found {
                return;
            }
            frame.file = symbol
                .filename()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|| UNKNOWN.to_owned());
            frame.line = symbol.lineno().unwrap_or(0);
            frame.function = symbol
                .name()
                .map(|name| strip_symbol_hash(name.to_string()))
                .unwrap_or_default();
            found = true;
        });
        frame
    }

    /// 构造符号不可用时的占位帧。
    fn unknown(instruction_pointer: usize) -> Self {
        Self {
            instruction_pointer,
            file: UNKNOWN.to_owned(),
            line: 0,
            function: String::new(),
        }
    }

    /// Returns the captured instruction pointer.
    /// 返回捕获时的指令指针。
    pub fn instruction_pointer(&self) -> usize {
        self.instruction_pointer
    }

    /// Returns the full source path, or `unknown` when symbols are unavailable.
    /// 返回完整源路径；符号不可用时为 `unknown`。
    pub fn file(&self) -> &str {
        &self.file
    }

    /// Returns the one-based source line, or zero when symbols are unavailable.
    /// 返回 1 起始的源行号；不可用时为 0。
    pub fn line(&self) -> u32 {
        self.line
    }

    /// Returns the resolved Rust function name, without its symbol hash.
    /// 返回去掉 Rust 符号哈希后缀的函数名。
    pub fn function(&self) -> &str {
        &self.function
    }

    /// Maps Go's `%v` frame format to `basename:line`.
    /// 对应 Go `%v`：仅文件名与行号。
    pub fn compact(&self) -> String {
        let file = Path::new(&self.file)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&self.file);
        format!("{file}:{}", self.line)
    }

    /// Maps Go's `%+v` frame format to `function\n\tpath:line`.
    /// 对应 Go `%+v`：函数名换行后缩进写全路径与行号。
    pub fn extended(&self) -> String {
        if self.function.is_empty() {
            return format!("{}:{}", self.file, self.line);
        }
        format!("{}\n\t{}:{}", self.function, self.file, self.line)
    }
}

impl fmt::Display for Frame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.compact())
    }
}

impl fmt::Debug for Frame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if formatter.alternate() {
            formatter.write_str(&self.extended())
        } else {
            formatter.write_str(&self.compact())
        }
    }
}

/// Stack frames ordered from innermost (newest) to outermost (oldest).
/// 栈帧序列：由内（最新）到外（最旧）。
#[derive(Clone, Default, Eq, PartialEq)]
pub struct StackTrace(Vec<Frame>);

impl StackTrace {
    /// 是否无任何帧。
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// 帧数量。
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl From<Vec<Frame>> for StackTrace {
    fn from(frames: Vec<Frame>) -> Self {
        Self(frames)
    }
}

impl Deref for StackTrace {
    type Target = [Frame];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Index<usize> for StackTrace {
    type Output = Frame;

    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}

impl fmt::Display for StackTrace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[")?;
        for (index, frame) in self.0.iter().enumerate() {
            if index != 0 {
                formatter.write_str(" ")?;
            }
            fmt::Display::fmt(frame, formatter)?;
        }
        formatter.write_str("]")
    }
}

impl fmt::Debug for StackTrace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if formatter.alternate() {
            for frame in &self.0 {
                formatter.write_str("\n")?;
                formatter.write_str(&frame.extended())?;
            }
            return Ok(());
        }
        fmt::Display::fmt(self, formatter)
    }
}

/// Rust mapping of the Go `StackTracer` interface.
/// 对应 Go `StackTracer`：可导出堆栈并报告是否为空。
pub trait StackTracer {
    fn stack_trace(&self) -> StackTrace;
    fn empty(&self) -> bool;

    /// Go 风格大写别名，便于迁移代码对照。
    fn StackTrace(&self) -> StackTrace {
        self.stack_trace()
    }

    fn Empty(&self) -> bool {
        self.empty()
    }
}

/// A node in an error/cause chain that may expose a stack tracer.
///
/// This explicit carrier trait replaces Go's runtime interface assertion and
/// lets later wrapper tasks participate without hard-coding concrete types.
/// 错误/原因链节点：可暴露 [`StackTracer`]，替代 Go 运行时接口断言。
pub trait StackTraceCarrier {
    fn stack_tracer(&self) -> Option<&dyn StackTracer>;

    fn cause(&self) -> Option<&dyn StackTraceCarrier> {
        None
    }
}

/// Returns the first stack tracer in a carrier's cause chain.
/// 沿 carrier 原因链查找第一个可用的 [`StackTracer`]。
pub fn GetStackTracer(mut original: &dyn StackTraceCarrier) -> Option<&dyn StackTracer> {
    loop {
        if let Some(tracer) = original.stack_tracer() {
            return Some(tracer);
        }
        original = original.cause()?;
    }
}

/// Owned captured stack returned by [`NewStack`].
/// [`NewStack`] 返回的已拥有捕获栈。
#[derive(Clone, Default)]
pub struct Stack {
    trace: StackTrace,
}

impl StackTracer for Stack {
    fn stack_trace(&self) -> StackTrace {
        self.trace.clone()
    }

    fn empty(&self) -> bool {
        self.trace.is_empty()
    }
}

impl StackTraceCarrier for Stack {
    fn stack_tracer(&self) -> Option<&dyn StackTracer> {
        Some(self)
    }
}

/// Captures a real stack and excludes this function plus `skip` callers.
/// 捕获真实堆栈，并排除本函数以及再往上 `skip` 层调用者。
#[inline(never)]
pub fn NewStack(skip: usize) -> Stack {
    let frames = resolve_backtrace(Backtrace::new());
    // 定位 NewStack 自身帧，再加 skip 跳过调用方
    let first_caller = frames
        .iter()
        .position(|frame| is_new_stack(&frame.function))
        .map_or(0, |index| index + 1);
    let trace = frames.into_iter().skip(first_caller + skip).collect();
    Stack {
        trace: StackTrace(trace),
    }
}

/// 将 backtrace 物理帧解析为 [`Frame`] 列表；无符号时填 unknown。
fn resolve_backtrace(backtrace: Backtrace) -> Vec<Frame> {
    let mut resolved = Vec::new();
    for physical_frame in backtrace.frames() {
        if physical_frame.symbols().is_empty() {
            resolved.push(Frame::unknown(physical_frame.ip() as usize));
            continue;
        }

        for symbol in physical_frame.symbols() {
            resolved.push(Frame {
                instruction_pointer: physical_frame.ip() as usize,
                file: symbol
                    .filename()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|| UNKNOWN.to_owned()),
                line: symbol.lineno().unwrap_or(0),
                function: symbol
                    .name()
                    .map(|name| strip_symbol_hash(name.to_string()))
                    .unwrap_or_default(),
            });
        }
    }
    resolved
}

/// 判断函数名是否属于 `NewStack`（含闭包路径）。
fn is_new_stack(function: &str) -> bool {
    function.ends_with("::stack::NewStack") || function.contains("::stack::NewStack::")
}

/// 去掉 Rust 符号末尾 `::h` + 16 位十六进制哈希。
fn strip_symbol_hash(mut function: String) -> String {
    if let Some((prefix, suffix)) = function.rsplit_once("::") {
        if suffix.len() == 17
            && suffix.starts_with('h')
            && suffix[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            function.truncate(prefix.len());
        }
    }
    function
}
