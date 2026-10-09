// Copyright 2026 AsterSQL.

// 错误格式化输出等价性测试（对应 Go `format_test.go`）。
//
// 将 Go 表驱动用例映射为 Rust：校验 `New`/`Errorf`/`Annotate`/`Annotatef`/
// `WithStack`/`WithMessage` 在 Plain（`%s`/`%v`）、Quoted（`%q`）与 Extended
// （`%+v` → `{:#?}`）三种期望下的消息行与栈帧数量。

use std::error::Error as StdError;
use std::fmt;

use astersql_errors::{
    Annotate, Annotatef, Errorf, GetStackTracer, New, SharedError, WithMessage, WithStack,
};

/// 模拟标准库 EOF 哨兵错误，作为无栈原因参与包装用例。
#[derive(Debug)]
struct Eof;

impl fmt::Display for Eof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EOF")
    }
}

impl StdError for Eof {}

/// 构造包装了 `Eof` 的 `SharedError`。
fn eof() -> SharedError {
    SharedError::new(Eof)
}

/// 单条用例的格式化期望：纯文本、带栈扩展，或引号转义。
enum FormatExpectation {
    /// Display / `%s`/`%v` 纯文本。
    Plain(&'static str),
    /// `{:#?}` / `%+v`：消息行序列与期望栈帧数。
    Extended {
        messages: &'static [&'static str],
        stacks: usize,
    },
    /// Debug 包装字符串后的 `%q` 结果。
    Quoted(&'static str),
}

/// 一条来自 Go 表的格式化用例：来源标签、错误值与期望。
struct FormatCase {
    source: &'static str,
    error: SharedError,
    expectation: FormatExpectation,
}

/// 将错误 Display 再以 Debug 引号化，对应 Go `%q`。
fn quoted(error: &SharedError) -> String {
    format!("{:?}", error.to_string())
}

/// 从扩展 Debug 文本中剥离栈帧缩进行，只保留消息行。
fn message_lines(rendered: &str) -> Vec<&str> {
    let lines: Vec<_> = rendered.lines().collect();
    let mut messages = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        // 下一行以 tab 开头且含 `:` 时视为栈帧行，与消息成对跳过。
        if index + 1 < lines.len()
            && lines[index + 1].starts_with('\t')
            && lines[index + 1].contains(':')
        {
            index += 2;
        } else {
            messages.push(lines[index]);
            index += 1;
        }
    }
    messages
}

/// 审计 Go format_test.go（commit 3697ad5）全部启用行在 Rust 侧的等价输出。
#[test]
fn all_source_format_cases_have_rust_equivalents() {
    // One entry for every enabled table row in format_test.go at source commit 3697ad5.
    let cases = vec![
        // TestFormatNew: 4 cases.
        FormatCase {
            source: "TestFormatNew/%s",
            error: New("error"),
            expectation: FormatExpectation::Plain("error"),
        },
        FormatCase {
            source: "TestFormatNew/%v",
            error: New("error"),
            expectation: FormatExpectation::Plain("error"),
        },
        FormatCase {
            source: "TestFormatNew/%+v",
            error: New("error"),
            expectation: FormatExpectation::Extended {
                messages: &["error"],
                stacks: 1,
            },
        },
        FormatCase {
            source: "TestFormatNew/%q",
            error: New("error"),
            expectation: FormatExpectation::Quoted("\"error\""),
        },
        // TestFormatErrorf: 3 cases.
        FormatCase {
            source: "TestFormatErrorf/%s",
            error: Errorf("%s", &["error".into()]),
            expectation: FormatExpectation::Plain("error"),
        },
        FormatCase {
            source: "TestFormatErrorf/%v",
            error: Errorf("%s", &["error".into()]),
            expectation: FormatExpectation::Plain("error"),
        },
        FormatCase {
            source: "TestFormatErrorf/%+v",
            error: Errorf("%s", &["error".into()]),
            expectation: FormatExpectation::Extended {
                messages: &["error"],
                stacks: 1,
            },
        },
        // TestFormatWrap: 8 cases.
        FormatCase {
            source: "TestFormatWrap/new/%s",
            error: Annotate(Some(New("error")), "error2").unwrap(),
            expectation: FormatExpectation::Plain("error2: error"),
        },
        FormatCase {
            source: "TestFormatWrap/new/%v",
            error: Annotate(Some(New("error")), "error2").unwrap(),
            expectation: FormatExpectation::Plain("error2: error"),
        },
        FormatCase {
            source: "TestFormatWrap/new/%+v",
            error: Annotate(Some(New("error")), "error2").unwrap(),
            expectation: FormatExpectation::Extended {
                messages: &["error", "error2"],
                stacks: 1,
            },
        },
        FormatCase {
            source: "TestFormatWrap/eof/%s",
            error: Annotate(Some(eof()), "error").unwrap(),
            expectation: FormatExpectation::Plain("error: EOF"),
        },
        FormatCase {
            source: "TestFormatWrap/eof/%v",
            error: Annotate(Some(eof()), "error").unwrap(),
            expectation: FormatExpectation::Plain("error: EOF"),
        },
        FormatCase {
            source: "TestFormatWrap/eof/%+v",
            error: Annotate(Some(eof()), "error").unwrap(),
            expectation: FormatExpectation::Extended {
                messages: &["EOF", "error"],
                stacks: 1,
            },
        },
        FormatCase {
            source: "TestFormatWrap/nested/%+v",
            error: Annotate(Some(Annotate(Some(eof()), "error1").unwrap()), "error2").unwrap(),
            expectation: FormatExpectation::Extended {
                messages: &["EOF", "error1", "error2"],
                stacks: 1,
            },
        },
        FormatCase {
            source: "TestFormatWrap/%q",
            error: Annotate(Some(New("error with space")), "context").unwrap(),
            expectation: FormatExpectation::Quoted("\"context: error with space\""),
        },
        // TestFormatWrapf: 6 cases.
        FormatCase {
            source: "TestFormatWrapf/eof/%s",
            error: Annotatef(Some(eof()), "error%d", &[2.into()]).unwrap(),
            expectation: FormatExpectation::Plain("error2: EOF"),
        },
        FormatCase {
            source: "TestFormatWrapf/eof/%v",
            error: Annotatef(Some(eof()), "error%d", &[2.into()]).unwrap(),
            expectation: FormatExpectation::Plain("error2: EOF"),
        },
        FormatCase {
            source: "TestFormatWrapf/eof/%+v",
            error: Annotatef(Some(eof()), "error%d", &[2.into()]).unwrap(),
            expectation: FormatExpectation::Extended {
                messages: &["EOF", "error2"],
                stacks: 1,
            },
        },
        FormatCase {
            source: "TestFormatWrapf/new/%s",
            error: Annotatef(Some(New("error")), "error%d", &[2.into()]).unwrap(),
            expectation: FormatExpectation::Plain("error2: error"),
        },
        FormatCase {
            source: "TestFormatWrapf/new/%v",
            error: Annotatef(Some(New("error")), "error%d", &[2.into()]).unwrap(),
            expectation: FormatExpectation::Plain("error2: error"),
        },
        FormatCase {
            source: "TestFormatWrapf/new/%+v",
            error: Annotatef(Some(New("error")), "error%d", &[2.into()]).unwrap(),
            expectation: FormatExpectation::Extended {
                messages: &["error", "error2"],
                stacks: 1,
            },
        },
        // TestFormatWithStack: 9 cases.
        FormatCase {
            source: "TestFormatWithStack/eof/%s",
            error: WithStack(Some(eof())).unwrap(),
            expectation: FormatExpectation::Plain("EOF"),
        },
        FormatCase {
            source: "TestFormatWithStack/eof/%v",
            error: WithStack(Some(eof())).unwrap(),
            expectation: FormatExpectation::Plain("EOF"),
        },
        FormatCase {
            source: "TestFormatWithStack/eof/%+v",
            error: WithStack(Some(eof())).unwrap(),
            expectation: FormatExpectation::Extended {
                messages: &["EOF"],
                stacks: 1,
            },
        },
        FormatCase {
            source: "TestFormatWithStack/new/%s",
            error: WithStack(Some(New("error"))).unwrap(),
            expectation: FormatExpectation::Plain("error"),
        },
        FormatCase {
            source: "TestFormatWithStack/new/%v",
            error: WithStack(Some(New("error"))).unwrap(),
            expectation: FormatExpectation::Plain("error"),
        },
        FormatCase {
            source: "TestFormatWithStack/new/%+v",
            error: WithStack(Some(New("error"))).unwrap(),
            expectation: FormatExpectation::Extended {
                messages: &["error"],
                stacks: 2,
            },
        },
        FormatCase {
            source: "TestFormatWithStack/nested-eof/%+v",
            error: WithStack(WithStack(Some(eof()))).unwrap(),
            expectation: FormatExpectation::Extended {
                messages: &["EOF"],
                stacks: 2,
            },
        },
        FormatCase {
            source: "TestFormatWithStack/nested-wrap/%+v",
            error: WithStack(WithStack(Annotatef(Some(eof()), "message", &[]))).unwrap(),
            expectation: FormatExpectation::Extended {
                messages: &["EOF", "message"],
                stacks: 3,
            },
        },
        FormatCase {
            source: "TestFormatWithStack/errorf/%+v",
            error: WithStack(Some(Errorf("error%d", &[1.into()]))).unwrap(),
            expectation: FormatExpectation::Extended {
                messages: &["error1"],
                stacks: 2,
            },
        },
        // TestFormatWithMessage: 12 cases.
        FormatCase {
            source: "TestFormatWithMessage/new/%s",
            error: WithMessage(Some(New("error")), "error2").unwrap(),
            expectation: FormatExpectation::Plain("error2: error"),
        },
        FormatCase {
            source: "TestFormatWithMessage/new/%v",
            error: WithMessage(Some(New("error")), "error2").unwrap(),
            expectation: FormatExpectation::Plain("error2: error"),
        },
        FormatCase {
            source: "TestFormatWithMessage/new/%+v",
            error: WithMessage(Some(New("error")), "error2").unwrap(),
            expectation: FormatExpectation::Extended {
                messages: &["error", "error2"],
                stacks: 1,
            },
        },
        FormatCase {
            source: "TestFormatWithMessage/eof/%s",
            error: WithMessage(Some(eof()), "addition1").unwrap(),
            expectation: FormatExpectation::Plain("addition1: EOF"),
        },
        FormatCase {
            source: "TestFormatWithMessage/eof/%v",
            error: WithMessage(Some(eof()), "addition1").unwrap(),
            expectation: FormatExpectation::Plain("addition1: EOF"),
        },
        FormatCase {
            source: "TestFormatWithMessage/eof/%+v",
            error: WithMessage(Some(eof()), "addition1").unwrap(),
            expectation: FormatExpectation::Extended {
                messages: &["EOF", "addition1"],
                stacks: 0,
            },
        },
        FormatCase {
            source: "TestFormatWithMessage/nested/%v",
            error: WithMessage(WithMessage(Some(eof()), "addition1"), "addition2").unwrap(),
            expectation: FormatExpectation::Plain("addition2: addition1: EOF"),
        },
        FormatCase {
            source: "TestFormatWithMessage/nested/%+v",
            error: WithMessage(WithMessage(Some(eof()), "addition1"), "addition2").unwrap(),
            expectation: FormatExpectation::Extended {
                messages: &["EOF", "addition1", "addition2"],
                stacks: 0,
            },
        },
        FormatCase {
            source: "TestFormatWithMessage/annotate/%+v",
            error: Annotate(WithMessage(Some(eof()), "error1"), "error2").unwrap(),
            expectation: FormatExpectation::Extended {
                messages: &["EOF", "error1", "error2"],
                stacks: 1,
            },
        },
        FormatCase {
            source: "TestFormatWithMessage/errorf/%+v",
            error: WithMessage(Some(Errorf("error%d", &[1.into()])), "error2").unwrap(),
            expectation: FormatExpectation::Extended {
                messages: &["error1", "error2"],
                stacks: 1,
            },
        },
        FormatCase {
            source: "TestFormatWithMessage/stack/%+v",
            error: WithMessage(WithStack(Some(eof())), "error").unwrap(),
            expectation: FormatExpectation::Extended {
                messages: &["EOF", "error"],
                stacks: 1,
            },
        },
        FormatCase {
            source: "TestFormatWithMessage/wrap-stack/%+v",
            error: WithMessage(
                Annotate(WithStack(Some(eof())), "inside-error"),
                "outside-error",
            )
            .unwrap(),
            expectation: FormatExpectation::Extended {
                messages: &["EOF", "inside-error", "outside-error"],
                stacks: 1,
            },
        },
    ];

    assert_eq!(cases.len(), 42, "source case audit count changed");
    for case in cases {
        match case.expectation {
            FormatExpectation::Plain(expected) => {
                assert_eq!(case.error.to_string(), expected, "{}", case.source);
            }
            FormatExpectation::Quoted(expected) => {
                assert_eq!(quoted(&case.error), expected, "{}", case.source);
            }
            FormatExpectation::Extended { messages, stacks } => {
                let rendered = format!("{:#?}", case.error);
                assert_eq!(
                    message_lines(&rendered),
                    messages,
                    "{}: {rendered}",
                    case.source
                );
                // 统计本测试函数名出现次数，作为捕获栈帧数的代理指标。
                // line-tables-only DWARF 在部分平台只提供函数短名，完整调试信息则提供模块限定名。
                let captured_stacks = rendered
                    .lines()
                    .filter(|line| {
                        line.rsplit("::").next()
                            == Some("all_source_format_cases_have_rust_equivalents")
                    })
                    .count();
                assert_eq!(captured_stacks, stacks, "{}: {rendered}", case.source);
                if stacks > 0 {
                    assert!(
                        rendered.contains("all_source_format_cases_have_rust_equivalents"),
                        "{}: {rendered}",
                        case.source
                    );
                    assert!(
                        rendered.contains("pkg/errors/tests/format_test.rs:"),
                        "{}: {rendered}",
                        case.source
                    );
                    let trace = GetStackTracer(&case.error)
                        .expect("extended stack case exposes a tracer")
                        .stack_trace();
                    assert!(
                        trace[0]
                            .function()
                            .contains("all_source_format_cases_have_rust_equivalents"),
                        "{}: {}",
                        case.source,
                        trace[0].extended()
                    );
                    assert!(
                        trace[0].file().ends_with("pkg/errors/tests/format_test.rs"),
                        "{}: {}",
                        case.source,
                        trace[0].extended()
                    );
                    assert!(
                        trace[0].line() > 0,
                        "{}: {}",
                        case.source,
                        trace[0].extended()
                    );
                }
            }
        }
    }
}
