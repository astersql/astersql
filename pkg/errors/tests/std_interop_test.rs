// Copyright 2026 AsterSQL.

// 与标准库 `std::error::Error` 互操作测试。
//
// 验证 Normalize 包装后的错误仍可通过 `source` 链 downcast 到原型 `Error`
// 与具体根类型，以及 `ptr_eq` / `HasStack` 在 New 与 FastGen 路径上的表现。

use std::error::Error as StdError;
use std::fmt;

use astersql_errors::{Error, HasStack, New, Normalize, RFCCodeText, SharedError};

/// 带整数载荷的简单 std 错误，用于 downcast 身份校验。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FooError(i32);

impl fmt::Display for FooError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("foo")
    }
}

impl StdError for FooError {}

/// 沿标准库 `source` 链查找并 downcast 为类型 `T`。
fn find_in_std_chain<'a, T>(mut error: &'a (dyn StdError + 'static)) -> Option<&'a T>
where
    T: StdError + 'static,
{
    loop {
        if let Some(found) = error.downcast_ref::<T>() {
            return Some(found);
        }
        error = error.source()?;
    }
}

/// 校验 Normalize.Wrap 后 std 链上既能找到 `Error`，也能 downcast 到具体根。
#[test]
fn normalize_works_with_std_source_and_downcast() {
    let root = SharedError::new(FooError(100));
    let prototype = Normalize("e2", &[RFCCodeText("e2".to_owned())]);
    let wrapped = SharedError::new(prototype.Wrap(Some(root)).expect("cause is present"));

    let normalized =
        find_in_std_chain::<Error>(&wrapped).expect("Normalize Error is in source chain");
    assert_eq!(normalized.ID(), "e2");
    assert_eq!(
        find_in_std_chain::<FooError>(&wrapped),
        Some(&FooError(100)),
        "the concrete std root remains downcastable",
    );

    let unwrapped = normalized
        .Unwrap()
        .expect("Normalize Error unwraps one level");
    assert!(unwrapped.downcast_ref::<FooError>().is_some());
}

/// 校验指针相等、`HasStack` 标记与 FastGen 无栈路径仍可在 std 链中找到 `Error`。
#[test]
fn equality_and_stack_markers_remain_std_compatible() {
    let error = New("same message");
    let clone = error.clone();
    assert!(error.ptr_eq(&clone));
    assert!(!error.ptr_eq(&New("same message")));
    assert!(HasStack(&error));

    let prototype = Normalize("fast", &[]);
    let fast = prototype.FastGen("fast", &[]);
    assert!(!HasStack(&fast));
    assert!(find_in_std_chain::<Error>(&fast).is_some());
}
