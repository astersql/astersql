// Copyright 2026 AsterSQL.

// 错误包装 API（Wrap / WithStack / WithMessage 等）行为测试。
//
// 覆盖空输入短路、原因保留、栈附加与去重（`AddStack` vs `WithStack`）、
// `Find`/`Cause`/`Unwrap` 链遍历、扩展 Debug 中消息与栈的相对顺序，以及 `Wrapf`。

use std::error::Error as StdError;
use std::io;

use astersql_errors::{
    AddStack, Cause, Find, GetErrStackMsg, GetStackTracer, HasStack, Join, Unwrap, WithMessage,
    WithStack, Wrap, Wrapf,
};

/// 校验包装器保留 cause、上下文与栈，并与 std `source` 遍历兼容。
#[test]
fn wrappers_preserve_cause_context_and_stack() {
    // 全部包装 API 对 None 返回 None；无错误时栈消息为空串。
    assert!(WithStack(None).is_none());
    assert!(AddStack(None).is_none());
    assert!(Wrap(None, "ignored").is_none());
    assert!(Wrapf(None, "ignored %d", &[1_i32.into()]).is_none());
    assert!(WithMessage(None, "ignored").is_none());
    assert!(Cause(None).is_none());
    assert!(Unwrap(None).is_none());
    assert_eq!(GetErrStackMsg(None), "");

    let root = astersql_errors::SharedError::new(io::Error::other("root failure"));
    assert!(!HasStack(&root));
    assert!(Cause(Some(&root)).is_some_and(|cause| cause.ptr_eq(&root)));
    assert!(Unwrap(Some(&root)).is_none());

    let wrapped = Wrap(Some(root.clone()), "read row").expect("non-nil input is wrapped");
    assert_eq!(wrapped.to_string(), "read row: root failure");
    assert!(HasStack(&wrapped));
    let trace = GetStackTracer(&wrapped)
        .expect("Wrap carries a stack")
        .stack_trace();
    assert!(
        trace[0]
            .function()
            .contains("wrappers_preserve_cause_context_and_stack")
    );

    // 扩展 Debug：原因文本 → 上下文消息 → 栈帧文件，顺序固定。
    let extended = format!("{wrapped:#?}");
    let root_position = extended.find("root failure").expect("cause is formatted");
    let message_position = extended.find("read row").expect("message is formatted");
    let stack_position = extended
        .find("wrap_test.rs")
        .expect("stack is formatted after context");
    assert!(root_position < message_position && message_position < stack_position);

    // Unwrap 剥掉 WithStack，得到 WithMessage；再 Unwrap 回到 root。
    let message = Unwrap(Some(&wrapped)).expect("WithStack unwraps to WithMessage");
    assert_eq!(message.to_string(), "read row: root failure");
    assert!(!message.ptr_eq(&root));
    assert!(Unwrap(Some(&message)).is_some_and(|cause| cause.ptr_eq(&root)));
    assert!(Cause(Some(&wrapped)).is_some_and(|cause| cause.ptr_eq(&root)));

    let found = Find(Some(&wrapped), |candidate| candidate.ptr_eq(&message));
    assert!(found.is_some_and(|candidate| candidate.ptr_eq(&message)));
    assert!(Find(Some(&wrapped), |_| false).is_none());

    // std::error::Error::source 链应能走到具体 io::Error 根。
    let source = wrapped.source().expect("wrapper implements std source");
    assert_eq!(source.to_string(), "read row: root failure");
    let mut current = Some(source);
    let mut found_root = false;
    while let Some(error) = current {
        if error.downcast_ref::<io::Error>().is_some() {
            found_root = true;
            break;
        }
        current = error.source();
    }
    assert!(found_root, "std source traversal exposes the concrete root");

    // AddStack 在已有栈时去重；WithStack 总是再包一层新栈。
    let already_stacked = WithStack(Some(root.clone())).expect("stack is added");
    let deduplicated = AddStack(Some(already_stacked.clone())).expect("error remains present");
    assert!(deduplicated.ptr_eq(&already_stacked));
    let stacked_again = WithStack(Some(already_stacked.clone())).expect("stack is always added");
    assert!(!stacked_again.ptr_eq(&already_stacked));

    let messaged = WithMessage(Some(already_stacked), "outer").expect("message is added");
    assert!(HasStack(&messaged));
    assert_eq!(GetErrStackMsg(Some(&messaged)), "outer: root failure");
    assert_eq!(GetErrStackMsg(Some(&wrapped)), "read row: root failure");

    let formatted = Wrapf(Some(root), "row %d", &[7_i32.into()]).expect("format wraps");
    assert_eq!(formatted.to_string(), "row 7: root failure");
}

/// Go `Find` delegates to `WalkDeep`, so it must search group children after the cause chain.
#[test]
fn find_searches_error_group_children() {
    let first = astersql_errors::New("first");
    let target = astersql_errors::New("target");
    let group = Join(&[Some(first), Some(target.clone())]).expect("non-empty group");
    let wrapped = WithMessage(Some(group), "outer").expect("message wrapper");

    assert!(
        Find(Some(&wrapped), |error| error.ptr_eq(&target))
            .is_some_and(|error| error.ptr_eq(&target))
    );
}
