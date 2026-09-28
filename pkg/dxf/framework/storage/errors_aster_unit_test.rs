// Copyright 2026 AsterSQL.

// 验证 storage 错误包装与 Go 错误语义的兼容性。
//
// 重点覆盖添加上下文后仍能按原始哨兵错误分类、保留 source 链，
// 同时将补充信息拼接到面向用户的错误文本中。

use std::error::Error as _;

use crate::{ErrTaskNotFound, errors};

#[test]
/// 验证 `Annotatef` 不会因追加上下文而丢失原始哨兵错误。
fn annotated_error_preserves_go_error_classification_and_source() {
    let error = errors::Annotatef(ErrTaskNotFound, "history lookup failed".to_owned());

    // 等值比较、source 与 Display 分别覆盖错误分类、根因链和完整上下文。
    assert_eq!(error, ErrTaskNotFound);
    assert_eq!(
        error.source().map(ToString::to_string),
        Some("task not found".to_owned())
    );
    assert_eq!(
        error.to_string(),
        "history lookup failed: task not found".to_owned()
    );
}
