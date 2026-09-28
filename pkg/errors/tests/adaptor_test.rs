// Copyright 2026 AsterSQL.

// Juju 适配层行为测试。
//
// 覆盖 `Trace`/`Annotate`/`SuspendStack` 的堆栈语义，以及
// `NotFoundf`/`AlreadyExistsf` 等分类后缀与判定函数。

use std::io;

use astersql_errors::{
    AddStack, AlreadyExistsf, Annotate, Annotatef, BadRequestf, Cause, ErrorStack, GetStackTracer,
    HasStack, IsAlreadyExists, IsNotFound, NewNoStackError, NewNoStackErrorf, NotFoundf,
    NotSupportedf, NotValidf, SuspendStack, Trace, WithMessage, WithStack,
};

/// 验证 adaptor API 与堆栈/分类语义对齐 Go/Juju 行为。
#[test]
fn juju_adaptors_match_stack_and_classification_semantics() {
    // None 输入应保持透传为 None / 空串
    assert!(Trace(None).is_none());
    assert!(Annotate(None, "ignored").is_none());
    assert!(Annotatef(None, "ignored %d", &[1_i32.into()]).is_none());
    assert!(SuspendStack(None).is_none());
    assert_eq!(ErrorStack(None), "");

    let root = astersql_errors::SharedError::new(io::Error::other("root failure"));
    // Trace 为无栈错误补栈；再次 Trace 应保持同一包装（幂等）
    let traced = Trace(Some(root.clone())).expect("Trace adds a stack");
    assert!(HasStack(&traced));
    let traced_again = Trace(Some(traced.clone())).expect("Trace preserves the error");
    assert!(traced_again.ptr_eq(&traced));
    let original_trace = GetStackTracer(&traced)
        .expect("traced error carries a stack")
        .stack_trace();

    // Annotate 附加上下文且复用原因上的既有堆栈
    let annotated = Annotate(Some(traced), "read row").expect("Annotate adds context");
    assert_eq!(annotated.to_string(), "read row: root failure");
    assert!(HasStack(&annotated));
    assert_eq!(
        GetStackTracer(&annotated)
            .expect("Annotate reuses the cause stack")
            .stack_trace(),
        original_trace
    );

    let formatted = Annotatef(Some(root.clone()), "row %d", &[7_i32.into()])
        .expect("Annotatef formats context");
    assert_eq!(formatted.to_string(), "row 7: root failure");
    assert!(ErrorStack(Some(&formatted)).contains("adaptor_test.rs"));

    // 显式无栈错误可被后续 Trace 补栈
    let no_stack = NewNoStackError("plain");
    assert!(!HasStack(&no_stack));
    let traced_no_stack = Trace(Some(no_stack)).expect("empty stack permits Trace");
    assert!(HasStack(&traced_no_stack));
    assert!(ErrorStack(Some(&traced_no_stack)).contains("adaptor_test.rs"));

    let no_stack_formatted = NewNoStackErrorf("plain %s", &["yes".into()]);
    assert_eq!(no_stack_formatted.to_string(), "plain yes");
    assert!(!HasStack(&no_stack_formatted));

    // SuspendStack 清除堆栈后仍可通过 AddStack 恢复
    let suspended = SuspendStack(WithStack(Some(root.clone())))
        .expect("SuspendStack preserves a present error");
    assert!(!HasStack(&suspended));
    let resumed = AddStack(Some(suspended)).expect("an upper layer can add a stack again");
    assert!(HasStack(&resumed));
    assert!(Cause(Some(&resumed)).is_some_and(|cause| cause.ptr_eq(&root)));

    // 嵌套包装：挂起后保留标记，再 Trace 不重复展开堆栈文本
    let nested =
        WithMessage(WithStack(WithStack(Some(root))), "outer").expect("message wrapper is created");
    let suspended_nested = SuspendStack(Some(nested)).expect("nested stacks are cleared");
    assert!(HasStack(&suspended_nested));
    let retraced_nested = Trace(Some(suspended_nested)).expect("cached marker is preserved");
    assert_eq!(ErrorStack(Some(&retraced_nested)), "root failure\nouter");

    // 分类后缀与 Is* 判定
    let not_found = NotFoundf("item %d", &[3_i32.into()]);
    assert_eq!(not_found.to_string(), "item 3 not found");
    assert!(IsNotFound(&not_found));
    assert!(!IsAlreadyExists(&not_found));

    let already_exists = AlreadyExistsf("item %d", &[3_i32.into()]);
    assert_eq!(already_exists.to_string(), "item 3 already exists");
    assert!(IsAlreadyExists(&already_exists));
    assert!(!IsNotFound(&already_exists));

    assert_eq!(
        BadRequestf("item %d", &[3_i32.into()]).to_string(),
        "item 3 bad request"
    );
    assert_eq!(
        NotSupportedf("item %d", &[3_i32.into()]).to_string(),
        "item 3 not supported"
    );
    assert_eq!(
        NotValidf("item %d", &[3_i32.into()]).to_string(),
        "item 3 not valid"
    );
}
