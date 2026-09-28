// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! 对应 Go `br/pkg/errors/errors_test.go`（`package errors_test`）的等价测试。
//!
//! 覆盖 `IsContextCanceled` 对 None/普通错误/哨兵/Trace/类 url.Error 包装的识别，
//! 以及 `Error.Equal` 在 Annotate 后仍按 RFC ID 匹配的行为。

use std::error::Error as StdError;
use std::fmt;

use astersql_errors::{Annotate, New, SharedError, Trace};

use super::{Canceled, DeadlineExceeded, ErrPDBatchScanRegion, IsContextCanceled};

/// 模拟 Go `net/url.Error`：经 `source`/`Unwrap` 暴露内层 Err，验证间接取消识别。
#[derive(Debug)]
struct UrlError {
    err: SharedError,
}

impl fmt::Display for UrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Get \"\": {}", self.err)
    }
}

impl StdError for UrlError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(&self.err)
    }
}

/// 对应 Go `TestIsContextCanceled`：空值、无关错误为假；取消/超时及包装为真。
#[test]
fn test_is_context_canceled() {
    // None 与无 cancel 语义的普通错误不得误判。
    assert!(!IsContextCanceled(None));
    assert!(!IsContextCanceled(Some(&New("connection closed"))));

    let canceled = SharedError::new(Canceled);
    let deadline = SharedError::new(DeadlineExceeded);
    // 直接挂载哨兵应识别。
    assert!(IsContextCanceled(Some(&canceled)));
    assert!(IsContextCanceled(Some(&deadline)));

    // errors.Trace wrapping still recognized via Cause / source chain.
    // Trace 包装后仍应通过 Cause/source 链识别（对齐 Go Trace）。
    assert!(IsContextCanceled(Trace(Some(canceled.clone())).as_ref()));
    assert!(IsContextCanceled(Trace(Some(deadline.clone())).as_ref()));

    // url.Error{Err: ...} shape: indirect unwrap through StdError::source.
    // 类 url.Error：仅通过 StdError::source 间接暴露内层 cancel/deadline。
    let url_canceled = SharedError::new(UrlError {
        err: canceled.clone(),
    });
    let url_deadline = SharedError::new(UrlError { err: deadline });
    assert!(IsContextCanceled(Some(&url_canceled)));
    assert!(IsContextCanceled(Some(&url_deadline)));
}

/// 对应 Go `TestEqual`：Annotate 后仍按错误 ID 与 `ErrPDBatchScanRegion` 相等。
#[test]
fn test_equal() {
    // 故意保留 Go 测试拼写 “equla”，只验证 equal 语义不受 annotate 消息影响。
    let err = Annotate(
        Some(SharedError::new((*ErrPDBatchScanRegion).clone())),
        "test error equla",
    )
    .expect("annotate keeps error");
    let matched = ErrPDBatchScanRegion.Equal(Some(&err));
    assert!(matched);
}
