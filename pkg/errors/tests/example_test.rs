// Copyright 2026 AsterSQL.

// 错误包公开示例用例测试（对应 Go `Example*`）。
//
// 覆盖 `New`/`WithMessage`/`WithStack`/`Wrap`/`Wrapf`/`Errorf`/`Cause` 等 API
// 的 Display 与扩展 Debug（`{:#?}`，映射 Go `%+v`）输出，以及调用栈帧文件路径结构。

use astersql_errors::{
    Cause, ErrorArg, Errorf, GetStackTracer, New, SharedError, WithMessage, WithStack, Wrap, Wrapf,
};

/// 断言扩展格式输出包含指定消息行，且至少含 `minimum_stack_count` 个栈帧缩进行。
fn assert_extended_stack(rendered: &str, messages: &[&str], minimum_stack_count: usize) {
    for message in messages {
        assert!(
            rendered.lines().any(|line| line == *message),
            "missing message {message:?} in {rendered:?}"
        );
    }
    assert!(
        rendered.matches("\n\t").count() >= minimum_stack_count,
        "expected at least {minimum_stack_count} stack frames in {rendered:?}"
    );
    assert!(rendered.contains("example_test.rs:"));
}

/// 构造 `outer → middle → inner → error` 四层 Wrap 链，供 Cause / 扩展格式用例复用。
#[inline(never)]
fn example_chain() -> SharedError {
    let inner = Wrap(Some(New("error")), "inner").unwrap();
    let middle = Wrap(Some(inner), "middle").unwrap();
    Wrap(Some(middle), "outer").unwrap()
}

// Go source: ExampleNew.
/// 校验 `New` 的 Display 文本与构造字符串一致。
#[test]
fn example_new() {
    assert_eq!(New("whoops").to_string(), "whoops");
}

// Go source: ExampleNew_printf. Rust alternate Debug maps Go's `%+v`.
/// 校验 `New` 在 `{:#?}` 下输出消息与至少一帧栈信息。
#[test]
fn example_new_printf() {
    let rendered = format!("{:#?}", New("whoops"));
    assert_extended_stack(&rendered, &["whoops"], 1);
}

// Go source: ExampleWithMessage.
/// 校验 `WithMessage` 将上下文拼到原因前：`msg: cause`。
#[test]
fn example_with_message() {
    let error = WithMessage(Some(New("whoops")), "oh noes").unwrap();
    assert_eq!(error.to_string(), "oh noes: whoops");
}

// Go source: ExampleWithStack.
/// 校验仅附加调用栈时 Display 仍为原始原因文本。
#[test]
fn example_with_stack() {
    let error = WithStack(Some(New("whoops"))).unwrap();
    assert_eq!(error.to_string(), "whoops");
}

// Go source: ExampleWithStack_printf. Both the cause and wrapper expose stacks.
/// 校验 `WithStack` 在扩展 Debug 下可暴露多层栈（原因与包装各一帧）。
#[test]
fn example_with_stack_printf() {
    let error = WithStack(Some(New("whoops"))).unwrap();
    let rendered = format!("{error:#?}");
    assert_extended_stack(&rendered, &["whoops"], 2);
}

// Go source: ExampleWrap.
/// 校验 `Wrap` 同时附加消息与栈后的 Display 拼接。
#[test]
fn example_wrap() {
    let error = Wrap(Some(New("whoops")), "oh noes").unwrap();
    assert_eq!(error.to_string(), "oh noes: whoops");
}

// Go source: ExampleCause.
/// 校验多层 Wrap 链的 Display 拼接，以及 `Cause` 取到最内层根错误。
#[test]
fn example_cause() {
    let error = example_chain();
    assert_eq!(error.to_string(), "outer: middle: inner: error");
    assert_eq!(Cause(Some(&error)).unwrap().to_string(), "error");
}

// Go source: ExampleWrap_extended.
/// 校验扩展 Debug 按从内到外列出各层消息，并含多层栈帧。
#[test]
fn example_wrap_extended() {
    let rendered = format!("{:#?}", example_chain());
    assert_extended_stack(&rendered, &["error", "inner", "middle", "outer"], 4);
}

// Go source: ExampleWrapf.
/// 校验 `Wrapf` 对上下文消息做 Go 风格格式化后再拼接原因。
#[test]
fn example_wrapf() {
    let error = Wrapf(Some(New("whoops")), "oh noes #%d", &[2.into()]).unwrap();
    assert_eq!(error.to_string(), "oh noes #2: whoops");
}

// Go source: ExampleErrorf_extended.
/// 校验 `Errorf` 在扩展 Debug 下带消息与栈。
#[test]
fn example_errorf_extended() {
    let error = Errorf("whoops: %s", &[ErrorArg::from("foo")]);
    let rendered = format!("{error:#?}");
    assert_extended_stack(&rendered, &["whoops: foo"], 1);
}

// Go source: Example_stackTrace. Runtime-specific paths and lines stay structural.
/// 校验根错误上的 `StackTracer`：帧文件落在本测试、行号与函数名非空。
#[test]
fn example_stack_trace() {
    let root = Cause(Some(&example_chain())).unwrap();
    let trace = GetStackTracer(&root)
        .expect("New errors expose a stack trace")
        .stack_trace();
    assert!(trace.len() >= 2);
    for frame in trace.iter().take(2) {
        assert!(frame.file().ends_with("pkg/errors/tests/example_test.rs"));
        assert!(frame.line() > 0);
        assert!(!frame.function().is_empty());
        assert!(frame.extended().contains("\n\t"));
    }
}

// Go source: ExampleCause_printf. Rust Display maps Go's `%v`.
/// 校验 Display（映射 Go `%v`）为 `failed: hello world` 形式。
#[test]
fn example_cause_printf() {
    let inner = Errorf("hello %s", &["world".into()]);
    let error = Wrap(Some(inner), "failed").unwrap();
    assert_eq!(format!("{error}"), "failed: hello world");
}
