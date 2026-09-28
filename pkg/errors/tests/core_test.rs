// Copyright 2026 AsterSQL.

// 错误核心 API 基础行为测试。
//
// 对应 Go `github.com/pingcap/errors` 中 `New` / `Errorf` 等基础构造路径：
// 校验字面量与格式化消息、指针相等（`ptr_eq`）、类型向下转换（downcast），
// 以及标准库 `source` 因果链与 Go 风格格式化标志（精度、错误占位）语义一致。

use std::error::Error as StdError;
use std::fmt;

use astersql_errors::{ErrorArg, Errorf, New, SharedError};

/// 外层包装错误：通过 `source` 暴露内层 `SharedError`，用于验证因果链。
#[derive(Debug)]
struct OuterError {
    source: SharedError,
}

/// 可 downcast 的标记错误，用于验证 `SharedError::downcast_ref`。
#[derive(Debug)]
struct MarkerError;

impl fmt::Display for MarkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("marker")
    }
}

impl StdError for MarkerError {}

impl fmt::Display for OuterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("outer")
    }
}

impl StdError for OuterError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(&self.source)
    }
}

/// 校验 `New`/`Errorf` 与 Go 基础语义：空串、字面量、`%` 转义、克隆指针相等与 source 链。
#[test]
fn new_and_errorf_match_go_basics() {
    // Go TestNew 的 4 个表项：空串、与 fmt.Errorf 同文本、与 New 同文本，
    // 以及含格式说明符的字面量（不应被二次解析）。
    for (message, expected) in [
        ("", ""),
        ("foo", "foo"),
        ("foo", "foo"),
        (
            "string with format specifiers: %v",
            "string with format specifiers: %v",
        ),
    ] {
        assert_eq!(New(message).to_string(), expected);
    }

    let literal = New("string with format specifiers: %v");
    assert_eq!(literal.to_string(), "string with format specifiers: %v");

    let clone = literal.clone();
    assert!(literal.ptr_eq(&clone));

    let marker = SharedError::new(MarkerError);
    assert!(marker.downcast_ref::<MarkerError>().is_some());

    // Go 风格：`%s`/`%d`/`%v`/`%%`；`ErrorArg::debug` 对应 `%v` 调试格式。
    let formatted = Errorf(
        "read %s: %d/%v %% %v %v",
        &[
            ErrorArg::from("row"),
            ErrorArg::from(7_i32),
            ErrorArg::from(true),
            ErrorArg::from(2.5_f64),
            ErrorArg::debug(vec![1, 2]),
        ],
    );
    assert_eq!(formatted.to_string(), "read row: 7/true % 2.5 [1, 2]");

    // Go TestErrorf 的两个表项保持为独立回归，避免复合用例掩盖基础路径。
    assert_eq!(
        Errorf("read error without format specifiers", &[]).to_string(),
        "read error without format specifiers"
    );
    assert_eq!(
        Errorf(
            "read error with %d format specifier",
            &[ErrorArg::from(1_i32)],
        )
        .to_string(),
        "read error with 1 format specifier"
    );

    let outer = OuterError {
        source: formatted.clone(),
    };
    assert_eq!(
        outer.source().map(ToString::to_string).as_deref(),
        Some("read row: 7/true % 2.5 [1, 2]")
    );
    // 用相同文本新建错误得到新实例，指针不应相等。
    assert!(!formatted.ptr_eq(&New(formatted.to_string())));
}

/// 校验 Go 字符串精度标志（`%-.4s`/`%.6s`）与类型不匹配时的 `%!d(...)` 错误串。
#[test]
fn errorf_parses_go_string_flags_and_precision() {
    let formatted = Errorf(
        "Duplicate entry '%-.4s' for key '%.6s'",
        &[ErrorArg::from("sensitive"), ErrorArg::from("public-key")],
    );
    assert_eq!(
        formatted.to_string(),
        "Duplicate entry 'sens' for key 'public'"
    );
    assert_eq!(
        Errorf("timestamp=%d", &[ErrorArg::from("not-a-number")]).to_string(),
        "timestamp=%!d(string=not-a-number)"
    );
}
