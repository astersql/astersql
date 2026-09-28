// Copyright 2026 AsterSQL.

// 调用栈（stack）捕获与 `StackTraceCarrier` 遍历测试。
//
// 校验 `NewStack`/`Frame` 的文件路径、行号、函数名与 compact/extended 格式，
// skip 帧语义、空栈 Display/Debug，以及 `GetStackTracer` 沿 cause 链定位首个 tracer。

use astersql_errors::{
    Frame, GetStackTracer, NewStack, StackTrace, StackTraceCarrier, StackTracer,
};

/// 捕获业务调用栈；`#[inline(never)]` 保证帧函数名可被解析到本函数。
#[inline(never)]
fn capture_business_stack(skip: usize) -> StackTrace {
    NewStack(skip).stack_trace()
}

/// 对照 Go：帧解析、skip、空栈格式，以及 `NewStack` 作为 StackTracer 的行为。
#[test]
fn frame_and_stack_trace_match_go_semantics() {
    let trace = capture_business_stack(0);
    assert!(!trace.is_empty());

    let first = &trace[0];
    assert!(first.file().ends_with("pkg/errors/tests/stack_test.rs"));
    assert!(first.line() > 0);
    assert!(first.function().contains("capture_business_stack"));
    assert!(first.compact().contains("stack_test.rs:"));
    assert!(first.extended().contains("capture_business_stack"));
    assert!(first.extended().contains("\n\t"));
    eprintln!("captured business frame: {}", first.extended());

    // 指令指针为 0：无法解析符号，回退为 unknown:0。
    let invalid = Frame::from_instruction_pointer(0);
    assert_eq!(invalid.instruction_pointer(), 0);
    assert_eq!(invalid.file(), "unknown");
    assert_eq!(invalid.line(), 0);
    assert_eq!(invalid.function(), "");
    assert_eq!(invalid.compact(), "unknown:0");
    assert_eq!(invalid.extended(), "unknown:0");

    // skip=1：跳过 capture_business_stack，顶帧应为本测试函数。
    let skipped = capture_business_stack(1);
    assert!(!skipped[0].function().contains("capture_business_stack"));
    assert!(
        skipped[0]
            .function()
            .contains("frame_and_stack_trace_match_go_semantics")
    );

    let empty = StackTrace::default();
    assert!(empty.is_empty());
    assert_eq!(empty.to_string(), "[]");
    assert_eq!(format!("{empty:#?}"), "");

    assert!(trace.to_string().starts_with("[stack_test.rs:"));
    let extended = format!("{trace:#?}");
    assert!(extended.starts_with('\n'));
    assert!(extended.contains("capture_business_stack"));

    let stack = NewStack(0);
    assert!(!stack.empty());
    let tracer = GetStackTracer(&stack).expect("NewStack returns a stack tracer");
    assert!(!tracer.empty());
    assert_eq!(tracer.stack_trace().len(), stack.stack_trace().len());
}

/// 测试用 cause 载体：可显式配置本层 tracer 与下一层 cause。
struct Carrier<'a> {
    tracer: Option<&'a dyn StackTracer>,
    cause: Option<&'a dyn StackTraceCarrier>,
}

impl StackTraceCarrier for Carrier<'_> {
    fn stack_tracer(&self) -> Option<&dyn StackTracer> {
        self.tracer
    }

    fn cause(&self) -> Option<&dyn StackTraceCarrier> {
        self.cause
    }
}

/// 校验 `GetStackTracer` 沿 cause 链向下，返回第一个非空 tracer。
#[test]
fn get_stack_tracer_walks_to_the_first_tracer_in_the_cause_chain() {
    let inner = NewStack(0);
    let inner_carrier = Carrier {
        tracer: Some(&inner),
        cause: None,
    };
    let outer = Carrier {
        tracer: None,
        cause: Some(&inner_carrier),
    };

    let found = GetStackTracer(&outer).expect("inner cause carries a stack");
    assert_eq!(found.stack_trace(), inner.stack_trace());
}
